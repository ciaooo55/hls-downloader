//! Process-wide HTTP connection budget, shared backoff, and download throttle.

use std::collections::HashMap;
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

struct ThrottleState {
    limit_bps: f64,
    tokens: f64,
    updated: Instant,
}

fn throttle() -> &'static Mutex<ThrottleState> {
    THROTTLE.get_or_init(|| {
        Mutex::new(ThrottleState {
            limit_bps: 0.0,
            tokens: 0.0,
            updated: Instant::now(),
        })
    })
}

pub fn configure_limit_kib(limit_kib: u64) {
    if let Ok(mut state) = throttle().lock() {
        let next = limit_kib as f64 * 1024.0;
        if (state.limit_bps - next).abs() > f64::EPSILON {
            state.limit_bps = next;
            state.tokens = 0.0;
            state.updated = Instant::now();
        }
    }
}

pub fn consume(nbytes: usize) {
    let mut remaining = nbytes as f64;
    if remaining <= 0.0 {
        return;
    }
    loop {
        let Ok(mut state) = throttle().lock() else {
            return;
        };
        if state.limit_bps <= 0.0 {
            return;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(state.updated).as_secs_f64();
        state.updated = now;
        state.tokens = (state.tokens + elapsed * state.limit_bps).min(state.limit_bps);
        let take = remaining.min(state.tokens);
        if take > 0.0 {
            state.tokens -= take;
            remaining -= take;
        }
        if remaining <= 0.0 {
            return;
        }
        let wait = Duration::from_secs_f64((remaining.min(state.limit_bps) / state.limit_bps).max(0.001));
        drop(state);
        std::thread::sleep(wait);
    }
}

pub fn schedule_window_active(start: &str, end: &str) -> bool {
    in_window(start, end)
}

pub fn effective_limit_kib(global_kib: u64, schedule_enabled: bool, schedule_start: &str, schedule_end: &str, schedule_kib: u64) -> u64 {
    if !schedule_enabled {
        return global_kib;
    }
    if in_window(schedule_start, schedule_end) {
        schedule_kib
    } else {
        global_kib
    }
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
        assert_eq!(effective_limit_kib(2048, false, "00:00", "23:59", 128), 2048);
    }

    #[test]
    fn local_clock_stays_in_day() {
        assert!(chrono_minutes_now() < 24 * 60);
        let stamp = local_hhmm();
        assert_eq!(stamp.len(), 5);
        assert_eq!(stamp.chars().nth(2), Some(':'));
    }

    #[test]
    fn throttle_unlimited_does_not_sleep() {
        configure_limit_kib(0);
        let started = Instant::now();
        consume(1024 * 1024);
        assert!(started.elapsed() < Duration::from_millis(50));
    }
}
