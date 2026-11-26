//! Process-wide HTTP connection budget, shared backoff, and download throttle.

/// kernel32 的本地时钟。**只有这份 extern 声明**——此前它在 local_weekday_iso 与
/// chrono_minutes_now 里各写了一遍。
///
/// 故意留在 `#[cfg(windows)]` 里：它带 `#[link(name = "kernel32")]`，无条件编译会让
/// 非 Windows 构建链接 `-lkernel32` 而失败。能安全搬到共享模块的是结构体与派生逻辑
/// （见 `crate::local_time`，那里有对应单测），拿本地时间这一次 FFI 不行。
#[cfg(windows)]
mod sys_time {
    use crate::local_time::LocalTimeFields;
    #[link(name = "kernel32")]
    // SAFETY: `GetLocalTime` is the standard `kernel32` `stdcall` export taking a
    // writable `SYSTEMTIME` pointer, which matches `#[repr(C)] LocalTimeFields` (8 `u16`s).
    unsafe extern "system" {
        pub(crate) fn GetLocalTime(time: *mut LocalTimeFields);
    }
}

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const GLOBAL_CONNECTION_LIMIT: u32 = 128;
pub const PER_HOST_CONNECTION_LIMIT: u32 = 24;

const HINT_GLOBAL: u8 = 1 << 0;
const HINT_QUEUE: u8 = 1 << 1;
const HINT_TASK: u8 = 1 << 2;
const HINT_HOURLY: u8 = 1 << 3;

static ACTIVE_THROTTLE_HINTS: AtomicU8 = AtomicU8::new(0);

#[derive(Default)]
struct BudgetState {
    global: u32,
    hosts: HashMap<String, u32>,
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
    static NETWORK_POLICY_CONTEXT: RefCell<Option<NetworkPolicyContext>> =
        const { RefCell::new(None) };
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkPolicyContext {
    pub allowed_hosts: String,
    pub proxy_mode: String,
    pub proxy_url: String,
    pub proxy_bypass: String,
    pub spec_proxy: String,
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
    pub control: PathBuf,
}

fn budget() -> &'static Mutex<BudgetState> {
    BUDGET.get_or_init(|| Mutex::new(BudgetState::default()))
}

fn valid_url_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedUrlAuthority<'a> {
    pub scheme: String,
    pub userinfo: Option<&'a str>,
    pub raw_hostport: &'a str,
    pub host: String,
    pub ipv6: bool,
    pub port: Option<u16>,
}

impl ParsedUrlAuthority<'_> {
    pub(crate) fn bracketed_host(&self) -> String {
        if self.ipv6 {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    pub(crate) fn hostport(&self) -> String {
        match (self.ipv6, self.port) {
            (true, Some(port)) => format!("[{}]:{port}", self.host),
            (true, None) => format!("[{}]", self.host),
            (false, Some(port)) => format!("{}:{port}", self.host),
            (false, None) => self.host.clone(),
        }
    }
}

fn parse_nonzero_port(raw: &str) -> Option<u16> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u16>().ok().filter(|&port| port != 0)
}

fn trim_url_outer(url: &str) -> &str {
    url.trim().trim_start_matches('\u{feff}').trim()
}

pub(crate) fn parse_url_authority(url: &str) -> Option<ParsedUrlAuthority<'_>> {
    let trimmed = trim_url_outer(url);
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    let (raw_scheme, rest) = trimmed.split_once("://")?;
    if !valid_url_scheme(raw_scheme) {
        return None;
    }
    let scheme = raw_scheme.to_ascii_lowercase();
    let (userinfo, raw_hostport, hostport) = if matches!(scheme.as_str(), "ftp" | "ftps" | "sftp") {
        let authority = rest.split('/').next().unwrap_or("");
        let (userinfo, raw_hostport) = match authority.rfind('@') {
            Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
            None => (None, authority),
        };
        let stripped = raw_hostport.split(['?', '#']).next().unwrap_or("");
        (userinfo, raw_hostport, stripped)
    } else {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (userinfo, raw_hostport) = match authority.rfind('@') {
            Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
            None => (None, authority),
        };
        (userinfo, raw_hostport, raw_hostport)
    };
    if hostport.is_empty() || hostport.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(after_bracket) = hostport.strip_prefix('[') {
        let (ipv6_str, tail) = after_bracket.split_once(']')?;
        let ip = ipv6_str.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if tail.is_empty() {
            None
        } else {
            Some(parse_nonzero_port(tail.strip_prefix(':')?)?)
        };
        return Some(ParsedUrlAuthority {
            scheme,
            userinfo,
            raw_hostport,
            host: ip.to_string(),
            ipv6: true,
            port,
        });
    }
    let (raw_host, port) = if let Some((h, p)) = hostport.rsplit_once(':') {
        if h.contains(':') {
            return None;
        }
        (h, Some(parse_nonzero_port(p)?))
    } else {
        (hostport, None)
    };
    let host = raw_host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host
            .chars()
            .any(|ch| matches!(ch, '@' | '?' | '#' | '/' | '\\' | '[' | ']' | ':'))
    {
        return None;
    }
    Some(ParsedUrlAuthority {
        scheme,
        userinfo,
        raw_hostport,
        host,
        ipv6: false,
        port,
    })
}

fn host_key(url: &str) -> String {
    if let Some(auth) = parse_url_authority(url) {
        return auth.hostport();
    }
    let trimmed = trim_url_outer(url);
    if trimmed.contains(':') || trimmed.chars().any(char::is_control) {
        // Non-authority URI schemes such as `magnet:?...` have no host authority.
        return String::new();
    }
    trimmed
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

pub(crate) fn queue_profile_allowed(profile: &crate::QueueProfile) -> bool {
    profile.enabled
        && weekday_allowed(&profile.active_days)
        && (!profile.schedule_enabled
            || schedule_window_active(&profile.start_time, &profile.stop_time))
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
        .split(|ch: char| matches!(ch, ',' | ';') || ch.is_whitespace())
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
                if item
                    .parse::<IpAddr>()
                    .is_ok_and(|pattern_ip| pattern_ip == address)
                    || address_in_cidr(address, &item)
                {
                    return true;
                }
            }
            let suffix = item.trim_start_matches("*.").trim_start_matches('.');
            (item.starts_with("*.") || item.starts_with('.'))
                && (host == suffix || host.ends_with(&format!(".{suffix}")))
                || wildcard_match(&item, &host)
        })
}

#[cfg(test)]
pub(crate) fn url_host(url: &str) -> String {
    url_host_port(url).0
}

pub(crate) fn url_host_port(url: &str) -> (String, Option<u16>) {
    if let Some(auth) = parse_url_authority(url) {
        return (auth.host, auth.port);
    }
    let trimmed = trim_url_outer(url);
    if trimmed.contains(':') || trimmed.chars().any(char::is_control) {
        return (String::new(), None);
    }
    let host = trimmed
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host
        .chars()
        .any(|ch| matches!(ch, '@' | '?' | '#' | '[' | ']'))
    {
        return (String::new(), None);
    }
    (host, None)
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
                return (host.trim_end_matches('.').to_string(), Some(port));
            }
        }
    }
    (raw.trim_end_matches('.').to_string(), None)
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

pub fn acquire<E: From<String>>(
    url: &str,
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<ConnectionGuard, E> {
    let host = host_key(url);
    loop {
        // 控制检查不能持有全局预算锁，避免慢文件 I/O 阻止其他任务释放名额。
        check()?;
        let mut state = budget()
            .lock()
            .map_err(|_| E::from("connection budget poisoned".to_string()))?;
        let host_used = state.hosts.get(&host).copied().unwrap_or(0);
        if state.global < GLOBAL_CONNECTION_LIMIT && host_used < PER_HOST_CONNECTION_LIMIT {
            state.global += 1;
            state.hosts.insert(host.clone(), host_used + 1);
            return Ok(ConnectionGuard { host });
        }
        drop(
            BUDGET_PULSE
                .wait_timeout(state, Duration::from_millis(50))
                .map_err(|_| E::from("connection budget wait".to_string()))?,
        );
    }
}

fn release(host: &str) {
    if let Ok(mut state) = budget().lock() {
        state.global = state.global.saturating_sub(1);
        let remove = if let Some(used) = state.hosts.get_mut(host) {
            *used = used.saturating_sub(1);
            *used == 0
        } else {
            false
        };
        if remove {
            state.hosts.remove(host);
        }
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
            scoped: HashMap::new(),
            hourly: RollingQuota::new(),
            global_schedule: GlobalSchedule::default(),
        })
    })
}

fn update_throttle_hints(state: &ThrottleState) {
    let mut hints = 0u8;
    if state.global_schedule.global_limit_kib > 0
        || state.global_schedule.enabled
        || state
            .scoped
            .get("global")
            .is_some_and(|bucket| bucket.limit_bps > 0.0)
    {
        hints |= HINT_GLOBAL;
    }
    if state.hourly.limit_bytes > 0 {
        hints |= HINT_HOURLY;
    }
    for (scope, bucket) in &state.scoped {
        if bucket.limit_bps <= 0.0 {
            continue;
        }
        if scope.starts_with("queue:") {
            hints |= HINT_QUEUE;
        } else if scope.starts_with("task:") {
            hints |= HINT_TASK;
        }
    }
    ACTIVE_THROTTLE_HINTS.store(hints, Ordering::Release);
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
        update_throttle_hints(&state);
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
        update_throttle_hints(&state);
    }
}

pub fn configure_hourly_quota_mib(limit_mib: u64) {
    if let Ok(mut state) = throttle().lock() {
        state.hourly.set_limit_mib(limit_mib);
        update_throttle_hints(&state);
    }
}

pub fn clear_scoped_limit(scope: &str) {
    if let Ok(mut state) = throttle().lock() {
        state.scoped.remove(scope);
        update_throttle_hints(&state);
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
        update_throttle_hints(&state);
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

fn consume_bucket<E>(
    scope: &str,
    nbytes: usize,
    check: &mut impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let mut remaining = nbytes as f64;
    if remaining <= 0.0 {
        return Ok(());
    }
    loop {
        let Ok(mut state) = throttle().lock() else {
            return Ok(());
        };
        if scope == "global" {
            refresh_global_limit_at(&mut state, chrono_minutes_now());
        }
        let Some(bucket) = state.scoped.get_mut(scope) else {
            return Ok(());
        };
        if bucket.limit_bps <= 0.0 {
            return Ok(());
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
            return Ok(());
        }
        let wait = Duration::from_secs_f64(
            (remaining.min(bucket.limit_bps) / bucket.limit_bps).max(0.001),
        );
        drop(state);
        check()?;
        std::thread::sleep(wait.min(Duration::from_millis(50)));
    }
}

fn consume_hourly_quota<E>(
    nbytes: usize,
    check: &mut impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let mut remaining = nbytes as u64;
    while remaining > 0 {
        let Ok(mut state) = throttle().lock() else {
            return Ok(());
        };
        if state.hourly.limit_bytes == 0 {
            return Ok(());
        }
        let (reserved, wait) = state
            .hourly
            .reserve(Instant::now(), remaining, HOURLY_QUOTA_WINDOW);
        remaining = remaining.saturating_sub(reserved);
        if remaining == 0 {
            return Ok(());
        }
        drop(state);
        check()?;
        std::thread::sleep(wait.clamp(Duration::from_millis(1), Duration::from_millis(50)));
    }
    Ok(())
}

// 仅在限速/配额实际等待时轮询控制；无上限的热路径不取全局锁、不分配字符串、不查本地时钟。
pub fn consume<E>(nbytes: usize, mut check: impl FnMut() -> Result<(), E>) -> Result<(), E> {
    if nbytes == 0 {
        return Ok(());
    }
    let hints = ACTIVE_THROTTLE_HINTS.load(Ordering::Acquire);
    if hints == 0 {
        let unlimited = THROTTLE_CONTEXT.with(|slot| {
            let borrow = slot.borrow();
            match borrow.as_ref() {
                None => true,
                Some(context) => {
                    context.global_limit_kib == 0
                        && !context.schedule_enabled
                        && context.hourly_quota_mib == 0
                        && context.queue_limit_kib == 0
                        && context.task_limit_kib == 0
                }
            }
        });
        if unlimited {
            return Ok(());
        }
    }
    let (has_ctx, check_global, check_queue, check_task, check_hourly, queue_id, task_id) =
        THROTTLE_CONTEXT.with(|slot| {
            let borrow = slot.borrow();
            let Some(context) = borrow.as_ref() else {
                return (
                    false,
                    false,
                    false,
                    false,
                    false,
                    String::new(),
                    String::new(),
                );
            };
            let check_global = (hints & HINT_GLOBAL) != 0
                || context.global_limit_kib > 0
                || context.schedule_enabled;
            let check_queue = (hints & HINT_QUEUE) != 0 || context.queue_limit_kib > 0;
            let check_task = (hints & HINT_TASK) != 0 || context.task_limit_kib > 0;
            let check_hourly = (hints & HINT_HOURLY) != 0 || context.hourly_quota_mib > 0;
            (
                true,
                check_global,
                check_queue,
                check_task,
                check_hourly,
                if check_queue {
                    context.queue_id.clone()
                } else {
                    String::new()
                },
                if check_task {
                    context.task_id.clone()
                } else {
                    String::new()
                },
            )
        });
    if !has_ctx {
        return Ok(());
    }
    if check_global {
        consume_bucket("global", nbytes, &mut check)?;
    }
    if check_queue {
        consume_bucket(&format!("queue:{queue_id}"), nbytes, &mut check)?;
    }
    if check_task {
        consume_bucket(&format!("task:{task_id}"), nbytes, &mut check)?;
    }
    if check_hourly {
        consume_hourly_quota(nbytes, &mut check)?;
    }
    Ok(())
}

pub fn effective_global_speed_limit_kib(limit_kib: u64, enabled: bool) -> u64 {
    if enabled {
        limit_kib
    } else {
        0
    }
}

/// Returns true when any speed limit (global, scheduled, queue, task) or hourly
/// quota is currently active for this transfer context. Used by curl-impersonate
/// to avoid self-aborting on `--speed-limit` when the user intentionally throttles.
pub fn throttle_or_quota_active() -> bool {
    let hints = ACTIVE_THROTTLE_HINTS.load(Ordering::Acquire);
    let ctx = current_throttle_context();
    if let Some(ref context) = ctx {
        if context.global_limit_kib > 0
            || context.schedule_enabled
            || context.hourly_quota_mib > 0
            || context.queue_limit_kib > 0
            || context.task_limit_kib > 0
        {
            return true;
        }
    }
    if hints == 0 {
        return false;
    }
    let Ok(mut state) = throttle().lock() else {
        return false;
    };
    if state.hourly.limit_bytes > 0 {
        return true;
    }
    if state.global_schedule.enabled {
        refresh_global_limit_at(&mut state, chrono_minutes_now());
    }
    if state.global_schedule.global_limit_kib > 0
        || state
            .scoped
            .get("global")
            .is_some_and(|bucket| bucket.limit_bps > 0.0)
    {
        return true;
    }
    if let Some(context) = ctx {
        if !context.queue_id.is_empty()
            && state
                .scoped
                .get(&format!("queue:{}", context.queue_id))
                .is_some_and(|bucket| bucket.limit_bps > 0.0)
        {
            return true;
        }
        if !context.task_id.is_empty()
            && state
                .scoped
                .get(&format!("task:{}", context.task_id))
                .is_some_and(|bucket| bucket.limit_bps > 0.0)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
pub(crate) fn active_global_limit_bps() -> f64 {
    let Ok(mut state) = throttle().lock() else {
        return 0.0;
    };
    refresh_global_limit_at(&mut state, chrono_minutes_now());
    state
        .scoped
        .get("global")
        .map_or(0.0, |bucket| bucket.limit_bps)
}

pub fn schedule_window_active(start: &str, end: &str) -> bool {
    in_window(start, end)
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

pub fn current_network_policy() -> Option<NetworkPolicyContext> {
    NETWORK_POLICY_CONTEXT.with(|slot| slot.borrow().clone())
}

pub fn with_network_policy<T>(policy: Option<NetworkPolicyContext>, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<NetworkPolicyContext>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            NETWORK_POLICY_CONTEXT.with(|slot| *slot.borrow_mut() = previous);
        }
    }

    let previous =
        NETWORK_POLICY_CONTEXT.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), policy));
    let _restore = Restore(previous);
    work()
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn with_allowed_hosts<T>(allowed_hosts: &str, work: impl FnOnce() -> T) -> T {
    let mut next = current_network_policy().unwrap_or_default();
    next.allowed_hosts = allowed_hosts.to_string();
    with_network_policy(Some(next), work)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn with_proxy_policy<T>(
    mode: &str,
    configured: &str,
    bypass: &str,
    spec_proxy: &str,
    work: impl FnOnce() -> T,
) -> T {
    let mut next = current_network_policy().unwrap_or_default();
    next.proxy_mode = mode.to_string();
    next.proxy_url = configured.to_string();
    next.proxy_bypass = bypass.to_string();
    next.spec_proxy = spec_proxy.to_string();
    with_network_policy(Some(next), work)
}

pub fn active_allowed_hosts() -> String {
    NETWORK_POLICY_CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|policy| policy.allowed_hosts.clone())
            .unwrap_or_default()
    })
}

pub fn resolve_context_proxy(url: &str, fallback_proxy: &str) -> String {
    let Some(policy) = current_network_policy() else {
        return fallback_proxy.to_string();
    };
    let has_proxy_policy = !policy.proxy_mode.trim().is_empty()
        || !policy.proxy_url.trim().is_empty()
        || !policy.proxy_bypass.trim().is_empty()
        || !policy.spec_proxy.trim().is_empty();
    if !has_proxy_policy {
        return fallback_proxy.to_string();
    }
    if !policy.proxy_mode.trim().is_empty() {
        return effective_proxy(
            &policy.proxy_mode,
            &policy.proxy_url,
            &policy.proxy_bypass,
            url,
            &policy.spec_proxy,
        );
    }
    let base = if !policy.spec_proxy.trim().is_empty() {
        policy.spec_proxy.trim()
    } else {
        fallback_proxy.trim()
    };
    if base == INVALID_MANUAL_PROXY {
        return INVALID_MANUAL_PROXY.to_string();
    }
    if base == DIRECT_PROXY_SENTINEL {
        return DIRECT_PROXY_SENTINEL.into();
    }
    if url_bypassed(url, &policy.proxy_bypass) {
        return DIRECT_PROXY_SENTINEL.into();
    }
    base.to_string()
}

fn default_scheme_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        "ftps" => Some(990),
        "sftp" => Some(22),
        _ => None,
    }
}

fn parse_bypass_target(raw: &str) -> Option<(String, Option<u16>)> {
    let raw = trim_url_outer(raw);
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let (ipv6_raw, tail) = rest.split_once(']')?;
        let ip = ipv6_raw.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if tail.is_empty() {
            None
        } else {
            Some(parse_nonzero_port(tail.strip_prefix(':')?)?)
        };
        return Some((ip.to_string(), port));
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Some((ip.to_string(), None));
    }
    if raw.matches(':').count() == 1 {
        let (h, p_str) = raw.rsplit_once(':')?;
        let port = parse_nonzero_port(p_str)?;
        let host = h.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() || host.chars().any(char::is_whitespace) {
            return None;
        }
        return Some((host, Some(port)));
    }
    if raw.contains(':') || raw.chars().any(char::is_whitespace) {
        return None;
    }
    let host = raw.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some((host, None))
}

fn host_port_bypassed(host: &str, port: Option<u16>, bypass: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || bypass.trim().is_empty() {
        return false;
    }
    let host_ip = host.parse::<IpAddr>().ok();
    bypass
        .split(|ch: char| matches!(ch, ';' | ',') || ch.is_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|raw_pattern| {
            if let Some((net_raw, prefix_raw)) = raw_pattern.split_once('/') {
                let net_clean = net_raw
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim_end_matches('.');
                if !prefix_raw.is_empty() && prefix_raw.bytes().all(|b| b.is_ascii_digit()) {
                    if let Some(h_ip) = host_ip {
                        return address_in_cidr(h_ip, &format!("{net_clean}/{prefix_raw}"));
                    }
                }
                return false;
            }
            let (pat_host, required_port) = if let Some(rest) = raw_pattern.strip_prefix('[') {
                let Some((inner, tail)) = rest.split_once(']') else {
                    return false;
                };
                let req_port = if tail.is_empty() {
                    None
                } else {
                    let Some(p) = tail.strip_prefix(':').and_then(parse_nonzero_port) else {
                        return false;
                    };
                    Some(p)
                };
                (inner.to_ascii_lowercase(), req_port)
            } else if let Ok(ip) = raw_pattern.parse::<IpAddr>() {
                (ip.to_string(), None)
            } else if raw_pattern.matches(':').count() == 1 {
                let Some((h, p_str)) = raw_pattern.rsplit_once(':') else {
                    return false;
                };
                let Some(p) = parse_nonzero_port(p_str) else {
                    return false;
                };
                (h.to_ascii_lowercase(), Some(p))
            } else if raw_pattern.contains(':') {
                return false;
            } else {
                (raw_pattern.to_ascii_lowercase(), None)
            };
            if let Some(req_port) = required_port {
                if port != Some(req_port) {
                    return false;
                }
            }
            let pat_host = pat_host.trim_end_matches('.');
            if pat_host.is_empty() {
                return false;
            }
            if pat_host == "*" {
                return true;
            }
            if pat_host == "<local>" {
                return !host.contains('.') && !host.contains(':') && host_ip.is_none();
            }
            if let (Some(h_ip), Ok(p_ip)) = (host_ip, pat_host.parse::<IpAddr>()) {
                return h_ip == p_ip;
            }
            if pat_host.contains('*') || pat_host.contains('?') {
                return wildcard_match(pat_host, &host);
            }
            if host_ip.is_some() {
                return false;
            }
            let suffix = pat_host.trim_start_matches('.');
            !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")))
        })
}

pub fn host_bypassed(host: &str, bypass: &str) -> bool {
    let Some((parsed_host, parsed_port)) = parse_bypass_target(host) else {
        return false;
    };
    host_port_bypassed(&parsed_host, parsed_port, bypass)
}

pub fn url_bypassed(url: &str, bypass: &str) -> bool {
    if bypass.trim().is_empty() {
        return false;
    }
    if let Some(auth) = parse_url_authority(url) {
        let port = auth.port.or_else(|| default_scheme_port(&auth.scheme));
        return host_port_bypassed(&auth.host, port, bypass);
    }
    let trimmed = trim_url_outer(url);
    if trimmed.contains("://") || trimmed.chars().any(char::is_control) {
        return false;
    }
    let authority = trimmed
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    host_bypassed(authority, bypass)
}

pub const DIRECT_PROXY_SENTINEL: &str = "hls-downloader://direct-proxy";
pub const SYSTEM_PROXY_SENTINEL: &str = "hls-downloader://system-proxy";
pub const INVALID_MANUAL_PROXY: &str = "__invalid_manual_proxy__";

pub fn proxy_route_sentinel(proxy: &str) -> bool {
    matches!(
        proxy.trim(),
        DIRECT_PROXY_SENTINEL | SYSTEM_PROXY_SENTINEL | INVALID_MANUAL_PROXY
    )
}

pub fn effective_proxy(
    mode: &str,
    configured: &str,
    bypass: &str,
    url: &str,
    spec_proxy: &str,
) -> String {
    let spec_proxy = spec_proxy.trim();
    if spec_proxy == INVALID_MANUAL_PROXY {
        return INVALID_MANUAL_PROXY.to_string();
    }
    if mode.trim().eq_ignore_ascii_case("manual")
        && spec_proxy.is_empty()
        && configured.trim().is_empty()
    {
        return INVALID_MANUAL_PROXY.to_string();
    }
    if spec_proxy == DIRECT_PROXY_SENTINEL {
        return DIRECT_PROXY_SENTINEL.into();
    }
    if url_bypassed(url, bypass) {
        return DIRECT_PROXY_SENTINEL.into();
    }
    if proxy_route_sentinel(spec_proxy) || !spec_proxy.is_empty() {
        return spec_proxy.to_string();
    }
    match mode.trim().to_ascii_lowercase().as_str() {
        "direct" => DIRECT_PROXY_SENTINEL.into(),
        "manual" => configured.trim().to_string(),
        _ => SYSTEM_PROXY_SENTINEL.into(),
    }
}

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

pub(crate) fn parse_rfc3339_epoch(value: &str) -> Option<i64> {
    let (date, time_zone) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year_str = date_parts.next()?;
    if year_str.len() != 4 || !year_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year = year_str.parse::<i64>().ok()?;
    let month_str = date_parts.next()?;
    let day_str = date_parts.next()?;
    if date_parts.next().is_some()
        || month_str.len() != 2
        || !month_str.bytes().all(|b| b.is_ascii_digit())
        || day_str.len() != 2
        || !day_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let month = month_str.parse::<u32>().ok()?;
    let day = day_str.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
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
        let hours_str = parts.next()?;
        let minutes_str = parts.next()?;
        if parts.next().is_some()
            || hours_str.len() != 2
            || !hours_str.bytes().all(|b| b.is_ascii_digit())
            || minutes_str.len() != 2
            || !minutes_str.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let hours = hours_str.parse::<u32>().ok()?;
        let minutes = minutes_str.parse::<u32>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        (
            clock,
            sign * (i64::from(hours) * 3600 + i64::from(minutes) * 60),
        )
    };
    let clock = if let Some((clock, fraction)) = clock.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        clock
    } else {
        clock
    };
    let mut clock_parts = clock.split(':');
    let hour_str = clock_parts.next()?;
    let minute_str = clock_parts.next()?;
    let second_str = clock_parts.next()?;
    if clock_parts.next().is_some()
        || hour_str.len() != 2
        || !hour_str.bytes().all(|b| b.is_ascii_digit())
        || minute_str.len() != 2
        || !minute_str.bytes().all(|b| b.is_ascii_digit())
        || second_str.len() != 2
        || !second_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hour = hour_str.parse::<u32>().ok()?;
    let minute = minute_str.parse::<u32>().ok()?;
    let second = second_str.parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
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
    days.checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))?
        .checked_sub(offset_seconds)
}

/// 本地 ISO 星期（1=周一…7=周日）。
///
/// 字段结构体与派生逻辑见 `crate::local_time`：那里有对应单测，包括"周日必须落到 7"
/// 与"两个平台对同一天必须给同一答案"——星期映射算错不会报错，只会让计划窗口
/// 整体偏移一天，所以这两条是核心。
fn local_weekday_iso() -> u8 {
    #[cfg(windows)]
    {
        let mut now = crate::local_time::LocalTimeFields::default();
        // SAFETY: `now` is a valid initialized `#[repr(C)]` `SYSTEMTIME`-compatible stack slot.
        unsafe { sys_time::GetLocalTime(&mut now) };
        now.iso_weekday()
    }
    #[cfg(not(windows))]
    {
        crate::local_time::unix_seconds_to_iso_weekday(crate::local_time::unix_now_seconds())
    }
}

fn parse_hhmm(value: &str) -> Option<u32> {
    let mut parts = value.trim().split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

#[cfg(test)]
fn local_hhmm() -> String {
    local_day_stamp_and_hhmm().1
}

pub fn local_day_stamp_and_hhmm() -> (u32, String) {
    #[cfg(windows)]
    {
        let mut now = crate::local_time::LocalTimeFields::default();
        // SAFETY: `now` is a valid initialized `#[repr(C)]` `SYSTEMTIME`-compatible stack slot.
        unsafe { sys_time::GetLocalTime(&mut now) };
        let minutes = now.minutes_since_midnight();
        (
            now.day_stamp(),
            format!("{:02}:{:02}", minutes / 60, minutes % 60),
        )
    }
    #[cfg(not(windows))]
    {
        let secs = crate::local_time::unix_now_seconds();
        let minutes = ((secs / 60) % (24 * 60)) as u32;
        (
            (secs / 86_400) as u32,
            format!("{:02}:{:02}", minutes / 60, minutes % 60),
        )
    }
}

fn chrono_minutes_now() -> u32 {
    #[cfg(windows)]
    {
        let mut now = crate::local_time::LocalTimeFields::default();
        // SAFETY: `now` is a valid initialized `#[repr(C)]` `SYSTEMTIME`-compatible stack slot.
        unsafe { sys_time::GetLocalTime(&mut now) };
        now.minutes_since_midnight()
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
    fn ipv6_policy_matches_canonical_address_without_treating_it_as_local_name() {
        assert!(url_allowed(
            "https://[2001:db8::1]/x",
            "[2001:0db8:0:0:0:0:0:1]"
        ));
        assert!(!host_bypassed("2001:db8::1", "<local>"));
        assert!(!url_allowed("magnet:?xt=urn:test", "*"));
    }

    #[test]
    fn saturated_connection_budget_can_stop_without_leaking_a_slot() {
        use std::sync::mpsc;
        let url = "http://budget-stop.invalid/fixture";
        let mut guards: Vec<_> = (0..PER_HOST_CONNECTION_LIMIT)
            .map(|_| acquire(url, || Ok::<(), String>(())).unwrap())
            .collect();
        for action in ["paused", "canceled"] {
            let (waiting_tx, waiting_rx) = mpsc::channel();
            let (stop_tx, stop_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let mut checks = 0;
                acquire(url, || {
                    checks += 1;
                    if checks > 1 {
                        // 第二次检查说明确实等过满额预算；检查期间预算锁须已释放。
                        waiting_tx.send(()).unwrap();
                        return Err(stop_rx.recv().unwrap());
                    }
                    Ok::<(), String>(())
                })
            });
            waiting_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(
                budget().lock().unwrap().hosts[&host_key(url)],
                PER_HOST_CONNECTION_LIMIT
            );
            stop_tx.send(action.to_string()).unwrap();
            assert!(matches!(worker.join().unwrap(), Err(message) if message == action));
            assert_eq!(
                budget().lock().unwrap().hosts[&host_key(url)],
                PER_HOST_CONNECTION_LIMIT
            );
        }
        guards.pop();
        guards.push(acquire(url, || Ok::<(), String>(())).unwrap());
        drop(guards);
        assert!(!budget().lock().unwrap().hosts.contains_key(&host_key(url)));
    }

    #[test]
    fn overnight_window_wraps_midnight() {
        assert!(in_window_at("22:00", "06:00", 23 * 60));
        assert!(in_window_at("22:00", "06:00", 60));
        assert!(!in_window_at("22:00", "06:00", 12 * 60));
        assert!(!in_window_at("10:00", "10:00", 10 * 60));
        assert_eq!(parse_hhmm("08:30"), Some(8 * 60 + 30));
    }

    #[test]
    fn running_workers_switch_the_shared_global_bucket_at_schedule_boundaries() {
        let mut state = ThrottleState {
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
        let started = Instant::now();
        consume(1024 * 1024, || -> Result<(), ()> {
            panic!("unlimited transfer must not poll control")
        })
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn throttle_wait_can_stop_and_recheck_live_configuration() {
        let scope = "fixture:interruptible-wait";
        configure_scoped_limit(scope, 1);
        assert_eq!(
            consume_bucket(scope, 64 * 1024, &mut || Err("paused")),
            Err("paused")
        );
        consume_bucket(scope, 64 * 1024, &mut || {
            // 同步回调可取得限速锁，且解除限速后不再等待剩余字节。
            configure_scoped_limit(scope, 0);
            Ok::<(), ()>(())
        })
        .unwrap();
        clear_scoped_limit(scope);
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
            control: PathBuf::new(),
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
                    consume_bucket(scope, 32 * 1024, &mut || Ok::<(), ()>(())).unwrap();
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
            DIRECT_PROXY_SENTINEL
        );
        for url in [
            "https://cdn.test/a",
            "https://cdn.test:8443/a",
            "https://user:secret@cdn.test/a",
        ] {
            assert_eq!(
                effective_proxy("manual", "http://127.0.0.1:9", "cdn.test", url, ""),
                DIRECT_PROXY_SENTINEL
            );
        }
        assert_eq!(
            effective_proxy("manual", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            "http://127.0.0.1:9"
        );
        assert_eq!(
            effective_proxy("system", "http://127.0.0.1:9", "", "https://cdn.test/a", ""),
            SYSTEM_PROXY_SENTINEL
        );
        assert_eq!(
            effective_proxy(
                "direct",
                "http://127.0.0.1:9",
                "",
                "https://cdn.test/a",
                SYSTEM_PROXY_SENTINEL,
            ),
            SYSTEM_PROXY_SENTINEL
        );
        assert_eq!(
            effective_proxy(
                "direct",
                "http://127.0.0.1:9",
                "",
                "https://cdn.test/a",
                "http://127.0.0.1:7777",
            ),
            "http://127.0.0.1:7777"
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

    #[test]
    fn throttle_fast_path_skips_mutex_when_unlimited_and_transitions_when_limited() {
        let unlimited = ThrottleContext {
            queue_id: "default".into(),
            task_id: "fast-path-task".into(),
            ..ThrottleContext::default()
        };
        // Hold the global THROTTLE mutex while resetting hints to 0: if `consume`
        // tried to acquire the mutex on the unlimited fast path, it would deadlock.
        {
            let mut state = throttle().lock().unwrap_or_else(|e| e.into_inner());
            let saved_schedule = std::mem::take(&mut state.global_schedule);
            let saved_hourly = state.hourly.limit_bytes;
            let saved_limits: Vec<(String, f64)> = state
                .scoped
                .iter()
                .map(|(k, b)| (k.clone(), b.limit_bps))
                .collect();
            state.hourly.limit_bytes = 0;
            for bucket in state.scoped.values_mut() {
                bucket.limit_bps = 0.0;
            }
            update_throttle_hints(&state);
            assert_eq!(ACTIVE_THROTTLE_HINTS.load(Ordering::Acquire), 0);

            with_throttle_context(Some(unlimited.clone()), || {
                consume(256 * 1024, || -> Result<(), &'static str> {
                    panic!("fast path must not invoke control check")
                })
                .unwrap();
            });

            // Restore prior global state for any concurrent tests.
            state.global_schedule = saved_schedule;
            state.hourly.limit_bytes = saved_hourly;
            for (scope, limit_bps) in saved_limits {
                if let Some(bucket) = state.scoped.get_mut(&scope) {
                    bucket.limit_bps = limit_bps;
                }
            }
            update_throttle_hints(&state);
        }

        // Transition into throttled mode for a private task scope and verify it enforces the limit.
        let scope = "task:fast-path-transition";
        let throttled_ctx = ThrottleContext {
            queue_id: "default".into(),
            task_id: "fast-path-transition".into(),
            ..ThrottleContext::default()
        };
        configure_scoped_limit(scope, 1);
        assert_ne!(ACTIVE_THROTTLE_HINTS.load(Ordering::Acquire) & HINT_TASK, 0);
        with_throttle_context(Some(throttled_ctx), || {
            assert_eq!(consume(64 * 1024, || Err("throttled")), Err("throttled"));
        });
        clear_scoped_limit(scope);
    }

    #[test]
    fn effective_global_speed_limit_respects_enabled_toggle() {
        assert_eq!(effective_global_speed_limit_kib(2048, true), 2048);
        assert_eq!(effective_global_speed_limit_kib(2048, false), 0);
        assert_eq!(effective_global_speed_limit_kib(0, true), 0);
        assert_eq!(effective_global_speed_limit_kib(0, false), 0);
    }

    #[test]
    fn throttle_or_quota_active_reflects_context_and_scoped_limits() {
        let task_id = "curl-low-speed-check-task";
        let scope = format!("task:{task_id}");
        let ctx_with_task_limit = ThrottleContext {
            task_id: task_id.into(),
            task_limit_kib: 64,
            ..ThrottleContext::default()
        };
        with_throttle_context(Some(ctx_with_task_limit), || {
            assert!(throttle_or_quota_active());
        });

        let ctx_with_hourly = ThrottleContext {
            task_id: task_id.into(),
            hourly_quota_mib: 100,
            ..ThrottleContext::default()
        };
        with_throttle_context(Some(ctx_with_hourly), || {
            assert!(throttle_or_quota_active());
        });

        let ctx_dynamic = ThrottleContext {
            task_id: task_id.into(),
            ..ThrottleContext::default()
        };
        configure_scoped_limit(&scope, 32);
        with_throttle_context(Some(ctx_dynamic), || {
            assert!(throttle_or_quota_active());
        });
        clear_scoped_limit(&scope);
    }

    #[test]
    fn parse_url_authority_handles_scheme_specific_delimiters_and_ipv6() {
        let ftp = parse_url_authority("ftp://alice:p?ss#1@Files.Example.Test.:2121/pub/a.bin")
            .expect("ftp authority");
        assert_eq!(ftp.scheme, "ftp");
        assert_eq!(ftp.userinfo, Some("alice:p?ss#1"));
        assert_eq!(ftp.host, "files.example.test");
        assert_eq!(ftp.port, Some(2121));
        assert!(url_allowed(
            "ftp://alice:p?ss#1@files.example.test:2121/pub/a.bin",
            "files.example.test:2121"
        ));
        assert!(!url_allowed(
            "ftp://alice:p?ss#1@files.example.test:2121/pub/a.bin",
            "alice"
        ));

        let sftp_v6 = parse_url_authority("sftp://bob:sec#ret?x@[2001:DB8::1]:2222?ignored=1#frag")
            .expect("sftp ipv6 authority");
        assert_eq!(sftp_v6.host, "2001:db8::1");
        assert!(sftp_v6.ipv6);
        assert_eq!(sftp_v6.port, Some(2222));
        assert_eq!(sftp_v6.bracketed_host(), "[2001:db8::1]");
        assert_eq!(sftp_v6.hostport(), "[2001:db8::1]:2222");

        // HTTP/HTTPS/WS/WSS split authority at ['/', '?', '#'] BEFORE '@', so '@evil' in query cannot spoof host.
        let http =
            parse_url_authority("https://real.example.test/path?redirect=https://@spoof.test")
                .expect("http authority");
        assert_eq!(http.host, "real.example.test");
        let http_q = parse_url_authority("https://real.example.test?@spoof.test")
            .expect("http query authority");
        assert_eq!(http_q.host, "real.example.test");

        for invalid in [
            "",
            "magnet:?xt=urn:btih:123",
            "https:///missing-host",
            "ftp://user@/missing-host",
            "http://[bad-ipv6]/a",
            "http://host:0/a",
            "http://host:99999/a",
            "https://evil\0host.test/a",
        ] {
            assert!(parse_url_authority(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn manual_proxy_without_url_returns_fail_closed_sentinel_even_when_bypassed() {
        assert!(proxy_route_sentinel(INVALID_MANUAL_PROXY));
        assert_eq!(
            effective_proxy("manual", "", "", "https://cdn.test/a", ""),
            INVALID_MANUAL_PROXY
        );
        assert_eq!(
            effective_proxy("MANUAL", "   ", "cdn.test,*", "https://cdn.test/a", ""),
            INVALID_MANUAL_PROXY
        );
        // Explicit per-task proxy is still honored in manual mode (and subject to bypass).
        assert_eq!(
            effective_proxy(
                "manual",
                "",
                "",
                "https://cdn.test/a",
                "http://127.0.0.1:7890"
            ),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            effective_proxy(
                "manual",
                "",
                "cdn.test",
                "https://cdn.test/a",
                "http://127.0.0.1:7890"
            ),
            DIRECT_PROXY_SENTINEL
        );
    }

    #[test]
    fn parse_url_authority_trims_unicode_whitespace_cr_lf_and_bom() {
        for wrapped in [
            "\u{00a0}https://Media.Example.Test:8443/path?x=1\u{00a0}",
            "\u{3000}https://Media.Example.Test:8443/path?x=1\u{3000}",
            "\r\n\t https://Media.Example.Test:8443/path?x=1 \r\n",
            "\u{feff}https://Media.Example.Test:8443/path?x=1",
            " \u{feff}\u{3000}https://Media.Example.Test:8443/path?x=1 ",
        ] {
            let auth = parse_url_authority(wrapped).expect(wrapped);
            assert_eq!(auth.scheme, "https");
            assert_eq!(auth.host, "media.example.test");
            assert_eq!(auth.port, Some(8443));
            assert_eq!(url_host(wrapped), "media.example.test");
            assert_eq!(host_key(wrapped), "media.example.test:8443");
            assert!(!url_allowed(wrapped, "other.example.test"));
            assert!(url_allowed(wrapped, "media.example.test:8443"));
            assert_eq!(
                effective_proxy(
                    "manual",
                    "http://127.0.0.1:7890",
                    "*.example.test",
                    wrapped,
                    "",
                ),
                DIRECT_PROXY_SENTINEL
            );
        }

        // Interior control characters must still fail closed.
        assert!(parse_url_authority("https://media.example.test/pa\nth").is_none());
        assert!(parse_url_authority("https://media.example\r.test/path").is_none());
    }

    #[test]
    fn host_and_url_bypass_support_wildcards_cidrs_ports_and_trailing_dots() {
        // `*` wildcard and `*.domain` / trailing-dot normalization
        assert!(host_bypassed("anything.example.com", "*"));
        assert!(host_bypassed("api.example.com", "*.example.com"));
        assert!(host_bypassed("api.example.com.", "*.example.com."));
        assert!(host_bypassed("example.com.", ".example.com."));
        assert!(!host_bypassed("example.com", "*.example.com"));

        // IPv4 and IPv6 CIDR blocks + non-matching numeric domain suffixes
        assert!(host_bypassed("10.24.0.9", "10.0.0.0/8, 192.168.0.0/16"));
        assert!(host_bypassed("192.168.12.34", "10.0.0.0/8;192.168.0.0/16"));
        assert!(!host_bypassed("172.16.0.1", "10.0.0.0/8, 192.168.0.0/16"));
        assert!(host_bypassed("fd12:3456::1", "fd00::/8"));
        assert!(host_bypassed("[fd12:3456::1]", "fd00::/8"));
        assert!(!host_bypassed("2001:db8::1", "fd00::/8"));
        assert!(!host_bypassed("127.0.0.1", "0.1"));

        // Port-qualified IPv4, IPv6, and domain bypass entries + default scheme ports
        assert!(host_bypassed("[::1]:8080", "[::1]:8080"));
        assert!(!host_bypassed("[::1]:8081", "[::1]:8080"));
        assert!(!host_bypassed("[::1]", "[::1]:8080"));
        assert!(host_bypassed("127.0.0.1:8080", "127.0.0.1:8080"));
        assert!(!host_bypassed("127.0.0.1:9090", "127.0.0.1:8080"));

        assert_eq!(
            effective_proxy(
                "manual",
                "http://127.0.0.1:7890",
                "[::1]:8080, 127.0.0.1:8080, internal.example.com.:443",
                "http://[0:0:0:0:0:0:0:1]:8080/video.mp4",
                "",
            ),
            DIRECT_PROXY_SENTINEL
        );
        assert_eq!(
            effective_proxy(
                "manual",
                "http://127.0.0.1:7890",
                "[::1]:8080",
                "http://[::1]:8081/video.mp4",
                "",
            ),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            effective_proxy(
                "manual",
                "http://127.0.0.1:7890",
                "internal.example.com.:443",
                "https://internal.example.com/secure.bin",
                "",
            ),
            DIRECT_PROXY_SENTINEL
        );
        assert_eq!(
            effective_proxy(
                "manual",
                "http://127.0.0.1:7890",
                "internal.example.com:443",
                "http://internal.example.com/plain.bin",
                "",
            ),
            "http://127.0.0.1:7890"
        );
    }
}
