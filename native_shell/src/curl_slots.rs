//! Concurrency cap for curl-impersonate subprocesses.
//!
//! The browser-identity transport runs one curl-impersonate process (plus a
//! reader thread) per HTTP response. A multi-connection task therefore used to
//! spawn as many processes as it had range workers, and several such tasks
//! multiplied that again. Reusing one process for several ranges is not safe
//! with the curl CLI: ranges are claimed and split dynamically, each response
//! must pass its own identity check before it may be written or split, and the
//! CLI cannot accept new requests after start. So the processes are capped
//! instead:
//!
//! * at most [`DEFAULT_PER_TASK`] live processes per task (keyed by the task's
//!   control file; `HLS_CURL_MAX_PER_TASK` overrides, clamped to 1..=16), and
//! * at most [`GLOBAL_LIMIT`] in the whole Core.
//!
//! A worker over the cap waits (observing pause/cancel every 50 ms) until a
//! response of the same task is finished. A thread that already holds a
//! permit is never made to wait for a second one (a redirect or retry that
//! overlaps its own previous response), so the cap cannot self-deadlock.
//! Requests without a task (probes, previews) only count toward the global
//! cap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;

pub(crate) const DEFAULT_PER_TASK: usize = 4;
pub(crate) const GLOBAL_LIMIT: usize = 16;
const WAIT_SLICE: Duration = Duration::from_millis(50);

#[derive(Default)]
struct Slots {
    per_task: HashMap<PathBuf, usize>,
    per_thread: HashMap<ThreadId, usize>,
    total: usize,
}

fn state() -> &'static (Mutex<Slots>, Condvar) {
    static STATE: OnceLock<(Mutex<Slots>, Condvar)> = OnceLock::new();
    STATE.get_or_init(|| (Mutex::new(Slots::default()), Condvar::new()))
}

/// Per-task cap from `HLS_CURL_MAX_PER_TASK`, else [`DEFAULT_PER_TASK`].
pub(crate) fn per_task_limit() -> usize {
    std::env::var("HLS_CURL_MAX_PER_TASK")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 16))
        .unwrap_or(DEFAULT_PER_TASK)
}

/// Held for the lifetime of one curl process.
#[derive(Debug)]
pub(crate) struct CurlPermit {
    task: Option<PathBuf>,
    thread: ThreadId,
}

impl Drop for CurlPermit {
    fn drop(&mut self) {
        let (lock, wake) = state();
        let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
        slots.total = slots.total.saturating_sub(1);
        if let Some(task) = &self.task {
            if let Some(count) = slots.per_task.get_mut(task) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    slots.per_task.remove(task);
                }
            }
        }
        if let Some(count) = slots.per_thread.get_mut(&self.thread) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                slots.per_thread.remove(&self.thread);
            }
        }
        drop(slots);
        wake.notify_all();
    }
}

/// Waits for a curl slot of `task` (empty path: global cap only). `stopped`
/// is polled while waiting; an `Err` from it aborts the wait.
pub(crate) fn acquire<E>(
    task: &Path,
    per_task: usize,
    global: usize,
    mut stopped: impl FnMut() -> Result<(), E>,
) -> Result<CurlPermit, E> {
    let task = (!task.as_os_str().is_empty()).then(|| task.to_path_buf());
    let thread = std::thread::current().id();
    let (lock, wake) = state();
    let mut slots = lock.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        let reentrant = slots.per_thread.get(&thread).copied().unwrap_or(0) > 0;
        let task_count = task
            .as_ref()
            .map_or(0, |task| slots.per_task.get(task).copied().unwrap_or(0));
        if reentrant || (task_count < per_task.max(1) && slots.total < global.max(1)) {
            slots.total += 1;
            if let Some(task) = &task {
                *slots.per_task.entry(task.clone()).or_default() += 1;
            }
            *slots.per_thread.entry(thread).or_default() += 1;
            return Ok(CurlPermit { task, thread });
        }
        drop(slots);
        stopped()?;
        slots = lock.lock().unwrap_or_else(|error| error.into_inner());
        let (guard, _) = wake
            .wait_timeout(slots, WAIT_SLICE)
            .unwrap_or_else(|error| error.into_inner());
        slots = guard;
    }
}

#[cfg(test)]
pub(crate) fn live_for(task: &Path) -> usize {
    let (lock, _) = state();
    let slots = lock.lock().unwrap_or_else(|error| error.into_inner());
    slots.per_task.get(task).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    fn task(name: &str) -> PathBuf {
        PathBuf::from(format!(
            "/curl-slots-test/{name}-{}/control",
            std::process::id()
        ))
    }

    #[test]
    fn per_task_cap_holds_and_a_release_admits_the_next_worker() {
        let key = task("cap");
        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let key = key.clone();
                let peak = Arc::clone(&peak);
                let live = Arc::clone(&live);
                std::thread::spawn(move || {
                    let permit = acquire::<()>(&key, 2, 1000, || Ok(())).unwrap();
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    live.fetch_sub(1, Ordering::SeqCst);
                    drop(permit);
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 2);
        assert_eq!(live_for(&key), 0);
    }

    #[test]
    fn other_tasks_are_not_blocked_by_a_full_task() {
        let full = task("full");
        let other = task("other");
        let _a = acquire::<()>(&full, 1, 1000, || Ok(())).unwrap();
        let waiter = {
            let full = full.clone();
            std::thread::spawn(move || {
                let mut polls = 0;
                acquire::<&'static str>(&full, 1, 1000, || {
                    polls += 1;
                    if polls > 3 {
                        Err("stopped")
                    } else {
                        Ok(())
                    }
                })
                .map(|_| ())
            })
        };
        // A different task gets its slot at once.
        let started = std::time::Instant::now();
        let _b = acquire::<()>(&other, 1, 1000, || Ok(())).unwrap();
        assert!(started.elapsed() < Duration::from_millis(40));
        // The waiter on the full task observes its stop condition.
        assert_eq!(waiter.join().unwrap().unwrap_err(), "stopped");
    }

    #[test]
    fn a_thread_holding_a_permit_is_never_blocked_by_its_own_task() {
        let key = task("reentrant");
        let first = acquire::<()>(&key, 1, 1000, || Ok(())).unwrap();
        let second = acquire::<&str>(&key, 1, 1000, || Err("would deadlock")).unwrap();
        assert_eq!(live_for(&key), 2);
        drop(first);
        drop(second);
        assert_eq!(live_for(&key), 0);
    }

    #[test]
    fn pause_releases_a_waiting_worker() {
        let key = task("pause");
        let _held = acquire::<()>(&key, 1, 1000, || Ok(())).unwrap();
        let paused = Arc::new(AtomicBool::new(false));
        let waiter = {
            let key = key.clone();
            let paused = Arc::clone(&paused);
            std::thread::spawn(move || {
                acquire(&key, 1, 1000, || {
                    if paused.load(Ordering::SeqCst) {
                        Err("paused")
                    } else {
                        Ok(())
                    }
                })
                .map(|_| ())
            })
        };
        std::thread::sleep(Duration::from_millis(80));
        paused.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        assert_eq!(waiter.join().unwrap().unwrap_err(), "paused");
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
