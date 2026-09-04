//! Process-wide HTTP connection budget, shared backoff, and download throttle.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const GLOBAL_CONNECTION_LIMIT: u32 = 128;
pub const PER_HOST_CONNECTION_LIMIT: u32 = 24;

#[derive(Default)]
struct BudgetState {
    global: u32,
    hosts: HashMap<String, u32>,
    retry_until: HashMap<String, Instant>,
}

pub struct ConnectionGuard {
    host: String,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        release(&self.host);
    }
}

static BUDGET: OnceLock<Mutex<BudgetState>> = OnceLock::new();
static BUDGET_PULSE: Condvar = Condvar::new();
static THROTTLE: OnceLock<Mutex<ThrottleState>> = OnceLock::new();

thread_local! {
    static THROTTLE_CONTEXT: RefCell<Option<ThrottleContext>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThrottleContext {
    pub global_limit_kib: u64,
    pub schedule_enabled: bool,
    pub schedule_start: String,
    pub schedule_end: String,
    pub schedule_limit_kib: u64,
    pub hourly_quota_mib: u64,
    pub queue_id: String,
    pub queue_limit_kib: u64,
    pub task_id: String,
    pub task_limit_kib: u64,
}

fn budget() -> &'static Mutex<BudgetState> {
    BUDGET.get_or_init(|| Mutex::new(BudgetState::default()))
}

fn host_key(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase()
}

pub fn url_allowed(url: &str, patterns: &str) -> bool {
    if patterns.trim().is_empty() {
        return true;
    }
    let (host, port) = url_host_port(url);
    if host.is_empty() {
        return false;
    }
    patterns
        .split([',', ';', '\n', '\t', ' '])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|item| {
            let (item, required_port) = pattern_host_port(item);
            if item.is_empty() || required_port.is_some() && required_port != port {
                return false;
            }
            if item == "*" || item == host {
                return true;
            }
            if let Ok(address) = host.parse::<IpAddr>() {
                if address_in_cidr(address, &item) {
                    return true;
                }
            }
            let suffix = item.trim_start_matches("*.").trim_start_matches('.');
            (item.starts_with("*.") || item.starts_with('.'))
                && (host == suffix || host.ends_with(&format!(".{suffix}")))
                || wildcard_match(&item, &host)
        })
}

fn url_host_port(url: &str) -> (String, Option<u16>) {
    let authority = host_key(url);
    let authority = authority.rsplit('@').next().unwrap_or(&authority);
    if let Some(rest) = authority.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            return (
                host.trim_end_matches('.').to_ascii_lowercase(),
                tail.strip_prefix(':').and_then(|value| value.parse().ok()),
            );
        }
    }
    if authority.matches(':').count() == 1 {
        if let Some((host, port)) = authority.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.trim_end_matches('.').to_ascii_lowercase(), Some(port));
            }
        }
    }
    (authority.trim_end_matches('.').to_ascii_lowercase(), None)
}

fn pattern_host_port(raw: &str) -> (String, Option<u16>) {
    let raw = raw.trim().to_ascii_lowercase();
    if raw.contains('/') {
        return (raw, None);
    }
    if let Some(rest) = raw.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            return (
                host.to_string(),
                tail.strip_prefix(':').and_then(|value| value.parse().ok()),
            );
        }
    }
    if raw.matches(':').count() == 1 {
        if let Some((host, port)) = raw.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.to_string(), Some(port));
            }
        }
    }
    (raw, None)
}

fn address_in_cidr(address: IpAddr, pattern: &str) -> bool {
    let Some((network, prefix)) = pattern.split_once('/') else {
        return false;
    };
    let Ok(network) = network.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u32>() else {
        return false;
    };
    match (address, network) {
        (IpAddr::V4(address), IpAddr::V4(network)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            u32::from(address) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(address), IpAddr::V6(network)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            u128::from(address) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let (mut pattern_index, mut value_index, mut star, mut checkpoint) = (0, 0, None, 0);
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            checkpoint = value_index;
        } else if let Some(index) = star {
            pattern_index = index + 1;
            checkpoint += 1;
            value_index = checkpoint;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

pub fn acquire(url: &str) -> Result<ConnectionGuard, String> {
    let host = host_key(url);
    let mut state = budget().lock().map_err(|_| "connection budget poisoned")?;
    loop {
        if let Some(until) = state.retry_until.get(&host).copied() {
            let now = Instant::now();
            if until > now {
                let wait = until - now;
                state = BUDGET_PULSE
                    .wait_timeout(state, wait)
                    .map_err(|_| "connection budget wait")?
                    .0;
                continue;
            }
        }
        let host_used = state.hosts.get(&host).copied().unwrap_or(0);
        if state.global < GLOBAL_CONNECTION_LIMIT && host_used < PER_HOST_CONNECTION_LIMIT {
            state.global += 1;
            state.hosts.insert(host.clone(), host_used + 1);
            return Ok(ConnectionGuard { host });
        }
        state = BUDGET_PULSE
            .wait_timeout(state, Duration::from_millis(50))
            .map_err(|_| "connection budget wait")?
            .0;
    }
}

fn release(host: &str) {
    if let Ok(mut state) = budget().lock() {
        state.global = state.global.saturating_sub(1);
        if let Some(used) = state.hosts.get_mut(host) {
            *used = used.saturating_sub(1);
        }
        BUDGET_PULSE.notify_all();
    }
}

pub fn note_retry_after(url: &str, seconds: u64) {
    let host = host_key(url);
    if let Ok(mut state) = budget().lock() {
        state.retry_until.insert(
            host,
            Instant::now() + Duration::from_secs(seconds.max(1).min(60)),
        );
        BUDGET_PULSE.notify_all();
    }
}

struct TokenBucket {
    limit_bps: f64,
    tokens: f64,
    updated: Instant,
}

impl TokenBucket {
    fn new() -> Self {
        Self {
            limit_bps: 0.0,
            tokens: 0.0,
            updated: Instant::now(),
        }
    }

    fn set_limit_kib(&mut self, limit_kib: u64) {
        let next = limit_kib as f64 * 1024.0;
        if (self.limit_bps - next).abs() > f64::EPSILON {
            self.limit_bps = next;
            self.tokens = 0.0;
            self.updated = Instant::now();
        }
    }
}

struct ThrottleState {
    legacy: TokenBucket,
    scoped: HashMap<String, TokenBucket>,
    hourly: RollingQuota,
    global_schedule: GlobalSchedule,
}

#[derive(Default)]
struct GlobalSchedule {
    global_limit_kib: u64,
    enabled: bool,
    start: String,
    end: String,
    schedule_limit_kib: u64,
}

const HOURLY_QUOTA_WINDOW: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
struct RollingQuota {
    limit_bytes: u64,
    used_bytes: u64,
    samples: VecDeque<(Instant, u64)>,
}

impl RollingQuota {
    fn new() -> Self {
        Self {
            limit_bytes: 0,
            used_bytes: 0,
            samples: VecDeque::new(),
        }
    }

    fn set_limit_mib(&mut self, limit_mib: u64) {
        let next = limit_mib.saturating_mul(1024 * 1024);
        if self.limit_bytes != next {
            self.limit_bytes = next;
            self.used_bytes = 0;
            self.samples.clear();
        }
    }

    fn prune(&mut self, now: Instant, window: Duration) {
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= window)
        {
            if let Some((_, bytes)) = self.samples.pop_front() {
                self.used_bytes = self.used_bytes.saturating_sub(bytes);
            }
        }
    }

    fn reserve(&mut self, now: Instant, requested: u64, window: Duration) -> (u64, Duration) {
        self.prune(now, window);
        if self.limit_bytes == 0 || requested == 0 {
            return (requested, Duration::ZERO);
        }
        let reserved = requested.min(self.limit_bytes.saturating_sub(self.used_bytes));
        if reserved > 0 {
            if let Some((at, bytes)) = self.samples.back_mut() {
                if now.saturating_duration_since(*at) < Duration::from_secs(1) {
                    *bytes = bytes.saturating_add(reserved);
                } else {
                    self.samples.push_back((now, reserved));
                }
            } else {
                self.samples.push_back((now, reserved));
            }
            self.used_bytes = self.used_bytes.saturating_add(reserved);
        }
        let wait = if reserved < requested {
            self.samples
                .front()
                .map(|(at, _)| window.saturating_sub(now.saturating_duration_since(*at)))
                .unwrap_or(Duration::from_millis(1))
        } else {
            Duration::ZERO
        };
        (reserved, wait)
    }
}

fn throttle() -> &'static Mutex<ThrottleState> {
    THROTTLE.get_or_init(|| {
        Mutex::new(ThrottleState {
            legacy: TokenBucket::new(),
            scoped: HashMap::new(),
            hourly: RollingQuota::new(),
            global_schedule: GlobalSchedule::default(),
        })
    })
}

#[cfg(test)]
pub fn configure_limit_kib(limit_kib: u64) {
    if let Ok(mut state) = throttle().lock() {
        state.legacy.set_limit_kib(limit_kib);
    }
}

pub fn configure_scoped_limit(scope: &str, limit_kib: u64) {
    if scope.trim().is_empty() {
        return;
    }
    if let Ok(mut state) = throttle().lock() {
        state
            .scoped
            .entry(scope.to_string())
            .or_insert_with(TokenBucket::new)
            .set_limit_kib(limit_kib);
    }
}

pub fn configure_global_schedule(
    global_limit_kib: u64,
    enabled: bool,
    start: &str,
    end: &str,
    schedule_limit_kib: u64,
) {
    if let Ok(mut state) = throttle().lock() {
        state.global_schedule = GlobalSchedule {
            global_limit_kib,
            enabled,
            start: start.to_string(),
            end: end.to_string(),
            schedule_limit_kib,
        };
        refresh_global_limit_at(&mut state, chrono_minutes_now());
    }
}

pub fn configure_hourly_quota_mib(limit_mib: u64) {
    if let Ok(mut state) = throttle().lock() {
        state.hourly.set_limit_mib(limit_mib);
    }
}

pub fn clear_scoped_limit(scope: &str) {
    if let Ok(mut state) = throttle().lock() {
        state.scoped.remove(scope);
    }
}

pub fn sync_queue_limits<'a>(limits: impl IntoIterator<Item = (&'a str, u64)>) {
    let limits: HashMap<String, u64> = limits
        .into_iter()
        .map(|(id, limit)| (format!("queue:{id}"), limit))
        .collect();
    if let Ok(mut state) = throttle().lock() {
        state
            .scoped
            .retain(|scope, _| !scope.starts_with("queue:") || limits.contains_key(scope));
        for (scope, limit) in limits {
            state
                .scoped
                .entry(scope)
                .or_insert_with(TokenBucket::new)
                .set_limit_kib(limit);
        }
    }
}

pub fn configure_throttle_context(context: &ThrottleContext) {
    configure_global_schedule(
        context.global_limit_kib,
        context.schedule_enabled,
        &context.schedule_start,
        &context.schedule_end,
        context.schedule_limit_kib,
    );
    configure_hourly_quota_mib(context.hourly_quota_mib);
    configure_scoped_limit(
        &format!("queue:{}", context.queue_id),
        context.queue_limit_kib,
    );
    configure_scoped_limit(&format!("task:{}", context.task_id), context.task_limit_kib);
}

pub fn current_throttle_context() -> Option<ThrottleContext> {
    THROTTLE_CONTEXT.with(|slot| slot.borrow().clone())
}

pub fn with_throttle_context<T>(context: Option<ThrottleContext>, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<ThrottleContext>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            THROTTLE_CONTEXT.with(|slot| *slot.borrow_mut() = previous);
        }
    }

    let previous =
        THROTTLE_CONTEXT.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), context));
    let _restore = Restore(previous);
    work()
}

fn consume_bucket(scope: Option<&str>, nbytes: usize) {
    let mut remaining = nbytes as f64;
    if remaining <= 0.0 {
        return;
    }
    loop {
        let Ok(mut state) = throttle().lock() else {
            return;
        };
        if scope == Some("global") {
            refresh_global_limit_at(&mut state, chrono_minutes_now());
        }
        let bucket = match scope {
            Some(scope) => state
                .scoped
                .entry(scope.to_string())
                .or_insert_with(TokenBucket::new),
            None => &mut state.legacy,
        };
        if bucket.limit_bps <= 0.0 {
            return;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.updated = now;
        bucket.tokens = (bucket.tokens + elapsed * bucket.limit_bps).min(bucket.limit_bps);
        let take = remaining.min(bucket.tokens);
        if take > 0.0 {
            bucket.tokens -= take;
            remaining -= take;
        }
        if remaining <= 0.0 {
            return;
        }
        let wait = Duration::from_secs_f64(
            (remaining.min(bucket.limit_bps) / bucket.limit_bps).max(0.001),
        );
        drop(state);
        std::thread::sleep(wait);
    }
}

fn consume_hourly_quota(nbytes: usize) {
    let mut remaining = nbytes as u64;
    while remaining > 0 {
        let Ok(mut state) = throttle().lock() else {
            return;
        };
        if state.hourly.limit_bytes == 0 {
            return;
        }
        let (reserved, wait) = state
            .hourly
            .reserve(Instant::now(), remaining, HOURLY_QUOTA_WINDOW);
        remaining = remaining.saturating_sub(reserved);
        if remaining == 0 {
            return;
        }
        drop(state);
        std::thread::sleep(wait.max(Duration::from_millis(1)));
    }
}

pub fn consume(nbytes: usize) {
    let Some(context) = current_throttle_context() else {
        consume_bucket(None, nbytes);
        return;
    };
    consume_bucket(Some("global"), nbytes);
    consume_bucket(Some(&format!("queue:{}", context.queue_id)), nbytes);
    consume_bucket(Some(&format!("task:{}", context.task_id)), nbytes);
    consume_hourly_quota(nbytes);
}

pub fn schedule_window_active(start: &str, end: &str) -> bool {
    in_window(start, end)
}

pub fn effective_limit_kib(
    global_kib: u64,
    schedule_enabled: bool,
    schedule_start: &str,
    schedule_end: &str,
    schedule_kib: u64,
) -> u64 {
    effective_limit_kib_at(
        global_kib,
        schedule_enabled,
        schedule_start,
        schedule_end,
        schedule_kib,
        chrono_minutes_now(),
    )
}

fn effective_limit_kib_at(
    global_kib: u64,
    schedule_enabled: bool,
    schedule_start: &str,
    schedule_end: &str,
    schedule_kib: u64,
    now: u32,
) -> u64 {
    if !schedule_enabled {
        return global_kib;
    }
    if in_window_at(schedule_start, schedule_end, now) {
        schedule_kib
    } else {
        global_kib
    }
}

fn refresh_global_limit_at(state: &mut ThrottleState, now: u32) {
    let schedule = &state.global_schedule;
    let limit = effective_limit_kib_at(
        schedule.global_limit_kib,
        schedule.enabled,
        &schedule.start,
        &schedule.end,
        schedule.schedule_limit_kib,
        now,
    );
    state
        .scoped
        .entry("global".into())
        .or_insert_with(TokenBucket::new)
        .set_limit_kib(limit);
}

fn in_window(start: &str, end: &str) -> bool {
    in_window_at(start, end, chrono_minutes_now())
}

fn in_window_at(start: &str, end: &str, now: u32) -> bool {
    let Some(start) = parse_hhmm(start) else {
        return false;
    };
    let Some(end) = parse_hhmm(end) else {
        return false;
    };
    if start == end {
        return false;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

pub fn host_bypassed(host: &str, bypass: &str) -> bool {
    let host = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host.is_empty() || bypass.trim().is_empty() {
        return false;
    }
    bypass
        .split([';', ',', '\n', '\t', ' '])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|pattern| {
            let pattern = pattern.trim_start_matches('.').to_ascii_lowercase();
            if pattern == "<local>" {
                return !host.contains('.');
            }
            host == pattern || host.ends_with(&format!(".{pattern}"))
        })
}

pub fn effective_proxy(
    mode: &str,
    configured: &str,
    bypass: &str,
    url: &str,
    spec_proxy: &str,
) -> String {
    if spec_proxy == DIRECT_PROXY_SENTINEL {
        return String::new();
    }
    let host = host_key(url);
    if host_bypassed(&host, bypass) {
        return String::new();
    }
    match mode.trim().to_ascii_lowercase().as_str() {
        "direct" => String::new(),
        _ => {
            if !spec_proxy.trim().is_empty() {
                spec_proxy.trim().to_string()
            } else {
                configured.trim().to_string()
            }
        }
    }
}

pub const DIRECT_PROXY_SENTINEL: &str = "hls-downloader://direct-proxy";

pub fn weekday_allowed(days: &str) -> bool {
    weekday_allowed_at(days, local_weekday_iso())
}

pub fn weekday_allowed_at(days: &str, today: u8) -> bool {
    let spec = days.trim();
    if spec.is_empty() || spec == "1,2,3,4,5,6,7" {
        return true;
    }
    spec.split(',')
        .filter_map(|item| item.trim().parse::<u8>().ok())
        .any(|day| day == today)
}

pub fn scheduled_start_reached(value: &str) -> bool {
    let stamp = value.trim();
    if stamp.is_empty() {
        return true;
    }
    if let Some(start) = parse_hhmm(stamp) {
        return chrono_minutes_now() >= start;
    }
    parse_rfc3339_epoch(stamp)
        .map(|start| unix_now() >= start)
        .unwrap_or(false)
}

pub fn scheduled_stop_hit(value: &str) -> bool {
    let stamp = value.trim();
    if stamp.is_empty() {
        return false;
    }
    if let Some(stop) = parse_hhmm(stamp) {
        return chrono_minutes_now() >= stop;
    }
    parse_rfc3339_epoch(stamp)
        .map(|stop| unix_now() >= stop)
        .unwrap_or(true)
}

pub fn schedule_value_valid(value: &str) -> bool {
    let stamp = value.trim();
    stamp.is_empty() || parse_hhmm(stamp).is_some() || parse_rfc3339_epoch(stamp).is_some()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_rfc3339_epoch(value: &str) -> Option<i64> {
    let (date, time_zone) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day == 0 || day > max_day {
        return None;
    }
    let (clock, offset_seconds) = if let Some(clock) = time_zone.strip_suffix('Z') {
        (clock, 0i64)
    } else {
        let index = time_zone.rfind(['+', '-'])?;
        let (clock, offset) = time_zone.split_at(index);
        let sign = if offset.starts_with('-') { -1i64 } else { 1i64 };
        let mut parts = offset[1..].split(':');
        let hours = parts.next()?.parse::<i64>().ok()?;
        let minutes = parts.next()?.parse::<i64>().ok()?;
        if parts.next().is_some() || hours > 23 || minutes > 59 {
            return None;
        }
        (clock, sign * (hours * 3600 + minutes * 60))
    };
    let clock = clock.split('.').next()?;
    let mut clock_parts = clock.split(':');
    let hour = clock_parts.next()?.parse::<i64>().ok()?;
    let minute = clock_parts.next()?.parse::<i64>().ok()?;
    let second = clock_parts.next()?.parse::<i64>().ok()?;
    if clock_parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds)
}

fn local_weekday_iso() -> u8 {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }
        let mut now = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        unsafe { GetLocalTime(&mut now) };
        match now.day_of_week {
            0 => 7,
            other => other as u8,
        }
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        (((secs / 86400) + 3) % 7 + 1) as u8
    }
}

fn parse_hhmm(value: &str) -> Option<u32> {
    let mut parts = value.trim().split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

pub fn local_hhmm() -> String {
    let minutes = chrono_minutes_now();
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn chrono_minutes_now() -> u32 {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }
        let mut now = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        unsafe { GetLocalTime(&mut now) };
        u32::from(now.hour) * 60 + u32::from(now.minute)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        ((secs / 60) % (24 * 60)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overnight_window_wraps_midnight() {
        assert!(in_window_at("22:00", "06:00", 23 * 60));
        assert!(in_window_at("22:00", "06:00", 60));
        assert!(!in_window_at("22:00", "06:00", 12 * 60));
        assert!(!in_window_at("10:00", "10:00", 10 * 60));
        assert_eq!(parse_hhmm("08:30"), Some(8 * 60 + 30));
        assert_eq!(
            effective_limit_kib(2048, false, "00:00", "23:59", 128),
            2048
        );
    }

    #[test]
    fn running_workers_switch_the_shared_global_bucket_at_schedule_boundaries() {
        let mut state = ThrottleState {
            legacy: TokenBucket::new(),
            scoped: HashMap::new(),
            hourly: RollingQuota::new(),
            global_schedule: GlobalSchedule {
                global_limit_kib: 2048,
                enabled: true,
                start: "22:00".into(),
                end: "06:00".into(),
                schedule_limit_kib: 128,
            },
        };

        refresh_global_limit_at(&mut state, 21 * 60 + 59);
        assert_eq!(state.scoped["global"].limit_bps, 2048.0 * 1024.0);
        refresh_global_limit_at(&mut state, 22 * 60);
        assert_eq!(state.scoped["global"].limit_bps, 128.0 * 1024.0);
        refresh_global_limit_at(&mut state, 6 * 60);
        assert_eq!(state.scoped["global"].limit_bps, 2048.0 * 1024.0);
        assert_eq!(state.scoped.len(), 1);
    }

    #[test]
    fn local_clock_stays_in_day() {
        assert!(chrono_minutes_now() < 24 * 60);
        let stamp = local_hhmm();
        assert_eq!(stamp.len(), 5);
        assert_eq!(stamp.chars().nth(2), Some(':'));
    }

    #[test]
    fn task_schedule_accepts_daily_and_absolute_times_without_fail_open() {
        assert!(schedule_value_valid("23:59"));
        assert!(schedule_value_valid("2999-01-01T00:00:00Z"));
        assert!(!schedule_value_valid("tomorrow morning"));
        assert!(!scheduled_start_reached("2999-01-01T00:00:00Z"));
        assert!(scheduled_start_reached("2000-01-01T00:00:00Z"));
        assert!(!scheduled_start_reached("invalid"));
        assert!(scheduled_stop_hit("invalid"));
        assert_eq!(
            parse_rfc3339_epoch("2026-08-23T08:00:00+08:00"),
            parse_rfc3339_epoch("2026-08-23T00:00:00Z")
        );
        assert!(parse_rfc3339_epoch("2026-02-29T00:00:00Z").is_none());
        assert!(parse_rfc3339_epoch("2024-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn throttle_unlimited_does_not_sleep() {
        configure_limit_kib(0);
        let started = Instant::now();
        consume(1024 * 1024);
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn scoped_throttle_shares_queue_bucket_and_separates_tasks() {
        let first = ThrottleContext {
            global_limit_kib: 4096,
            schedule_enabled: false,
            schedule_start: String::new(),
            schedule_end: String::new(),
            schedule_limit_kib: 0,
            hourly_quota_mib: 0,
            queue_id: "media".into(),
            queue_limit_kib: 1024,
            task_id: "one".into(),
            task_limit_kib: 512,
        };
        let second = ThrottleContext {
            task_id: "two".into(),
            task_limit_kib: 256,
            ..first.clone()
        };
        configure_throttle_context(&first);
        configure_throttle_context(&second);
        let state = throttle().lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(state.scoped["global"].limit_bps, 4096.0 * 1024.0);
        assert_eq!(state.scoped["queue:media"].limit_bps, 1024.0 * 1024.0);
        assert_eq!(state.scoped["task:one"].limit_bps, 512.0 * 1024.0);
        assert_eq!(state.scoped["task:two"].limit_bps, 256.0 * 1024.0);
        drop(state);
        with_throttle_context(Some(first.clone()), || {
            assert_eq!(current_throttle_context(), Some(first));
        });
        assert!(current_throttle_context().is_none());
        clear_scoped_limit("task:one");
        clear_scoped_limit("task:two");
    }

    #[test]
    fn scoped_queue_bucket_enforces_aggregate_bytes_across_workers() {
        // Queue configuration tests intentionally prune `queue:*` scopes and
        // run in parallel with this timing test. Use a private fixture scope so
        // that cleanup cannot turn this bucket into an unlimited one midway.
        let scope = "fixture:aggregate-workers";
        configure_scoped_limit(scope, 512);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let started = Instant::now();
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    consume_bucket(Some(scope), 32 * 1024);
                })
            })
            .collect();
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(90),
            "64 KiB escaped the shared 512 KiB/s queue bucket in {elapsed:?}"
        );
        clear_scoped_limit(scope);
    }

    #[test]
    fn queue_limit_sync_removes_deleted_profiles() {
        sync_queue_limits([("default", 0), ("temporary", 64)]);
        sync_queue_limits([("default", 128)]);
        let state = throttle().lock().unwrap_or_else(|error| error.into_inner());
        assert!(state.scoped.contains_key("queue:default"));
        assert!(!state.scoped.contains_key("queue:temporary"));
        assert_eq!(state.scoped["queue:default"].limit_bps, 128.0 * 1024.0);
    }

    #[test]
    fn rolling_hourly_quota_releases_only_expired_bytes() {
        let start = Instant::now();
        let window = Duration::from_secs(60);
        let mut quota = RollingQuota::new();
        quota.limit_bytes = 100;

        assert_eq!(quota.reserve(start, 70, window).0, 70);
        assert_eq!(
            quota.reserve(start + Duration::from_secs(30), 50, window).0,
            30
        );
        let (reserved, wait) = quota.reserve(start + Duration::from_secs(59), 1, window);
        assert_eq!(reserved, 0);
        assert!(wait <= Duration::from_secs(1));

        assert_eq!(
            quota.reserve(start + Duration::from_secs(60), 70, window).0,
            70
        );
        assert_eq!(quota.used_bytes, 100);
        assert!(quota.samples.len() <= 2);
    }

    #[test]
    fn weekday_and_proxy_bypass() {
        assert!(weekday_allowed_at("", 3));
        assert!(weekday_allowed_at("1,2,3,4,5,6,7", 7));
        assert!(!weekday_allowed_at("1,2,3,4,5", 6));
        assert!(weekday_allowed_at("6,7", 7));
        assert!(host_bypassed("intranet", "<local>"));
        assert!(host_bypassed("cdn.example.test", "example.test"));
        assert!(!host_bypassed("cdn.example.test", "other.test"));
        assert_eq!(
            effective_proxy("direct", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            ""
        );
        assert_eq!(
            effective_proxy(
                "manual",
                "http://127.0.0.1:9",
                "cdn.test",
                "https://cdn.test/a",
                ""
            ),
            ""
        );
        assert_eq!(
            effective_proxy("manual", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn allowed_hosts_support_exact_and_subdomain_patterns() {
        assert!(url_allowed("https://media.example.test/a.mp4", ""));
        assert!(url_allowed(
            "https://media.example.test/a.mp4",
            "media.example.test"
        ));
        assert!(url_allowed(
            "https://cdn.example.test/a.mp4",
            "*.example.test"
        ));
        assert!(!url_allowed(
            "https://example.invalid/a.mp4",
            "*.example.test,media.test"
        ));
        assert!(url_allowed("http://127.0.0.1:8765/a", "127.0.0.1"));
        assert!(url_allowed("http://10.12.4.8/a", "10.0.0.0/8"));
        assert!(!url_allowed("http://11.12.4.8/a", "10.0.0.0/8"));
        assert!(url_allowed("https://[2001:db8::3]/a", "2001:db8::/32"));
        assert!(url_allowed(
            "https://media12.example.test/a",
            "media*.example.test"
        ));
        assert!(url_allowed(
            "https://cdn.example.test:8443/a",
            "cdn.example.test:8443"
        ));
        assert!(!url_allowed(
            "https://cdn.example.test/a",
            "cdn.example.test:8443"
        ));
    }
}
