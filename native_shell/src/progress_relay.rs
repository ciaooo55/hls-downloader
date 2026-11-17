//! Coalescing relay for periodic worker progress.
//!
//! Download workers sample their transfer every 200 ms. They used to take the
//! global `Mutex<PersistentCore>` for every sample, so N active tasks meant 5N
//! blocking acquisitions per second competing with every IPC command.
//!
//! Workers now [`post`] the sample into a small per-Core mailbox (latest value
//! per task wins) and never block on the Core lock: a post that finds the
//! mailbox due ([`DRAIN_INTERVAL_MS`] since the last drain) makes one
//! `try_lock` attempt and, if it gets the lock, applies *every* pending sample
//! of that Core in one pass. If the lock is busy the samples stay queued and
//! the next post (from any worker) drains them. The Core therefore sees at
//! most one progress lock acquisition per interval regardless of how many
//! tasks are running, and a worker thread never waits behind an IPC command.
//!
//! Ordering: every drain runs while holding the Core lock, and every
//! synchronous progress path ([`flush_task_locked`] / [`drain_locked`] before
//! its own command) applies the queued sample first, also under the Core
//! lock. A queued "downloading" sample therefore can never land after a later
//! synchronous "checking"/"merging"/"paused" transition of the same task; the
//! event order is the same as when every sample was applied synchronously,
//! only intermediate samples within one interval are coalesced.
//!
//! IPC/event contract: drained samples go through the same
//! `CoreCommand::UpdateProgress` handler, so event names, sequence numbers and
//! the volatile/SQLite policy of `PersistentCore` are unchanged.

use crate::{CoreCommand, PersistentCore};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, TryLockError, Weak};
use std::time::Instant;

/// Minimum spacing between drains of one Core's mailbox.
pub(crate) const DRAIN_INTERVAL_MS: u64 = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgressSample {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: u64,
    pub stage: String,
    pub status: String,
}

struct CoreSlot {
    core: Weak<Mutex<PersistentCore>>,
    last_drain_ms: u64,
    pending: BTreeMap<String, ProgressSample>,
    #[cfg(test)]
    drains: u64,
}

fn slots() -> &'static Mutex<HashMap<usize, CoreSlot>> {
    static SLOTS: OnceLock<Mutex<HashMap<usize, CoreSlot>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64 + DRAIN_INTERVAL_MS + 1
}

fn key(core: &Arc<Mutex<PersistentCore>>) -> usize {
    Arc::as_ptr(core) as *const () as usize
}

fn slot_for<'a>(
    map: &'a mut HashMap<usize, CoreSlot>,
    core: &Arc<Mutex<PersistentCore>>,
) -> &'a mut CoreSlot {
    let id = key(core);
    let stale = map.get(&id).is_some_and(|slot| {
        !slot
            .core
            .upgrade()
            .is_some_and(|live| Arc::ptr_eq(&live, core))
    });
    if stale || !map.contains_key(&id) {
        // A dropped Core whose allocation was reused must not leak samples
        // into the new one; dead slots of other Cores are swept as well.
        map.retain(|_, slot| slot.core.strong_count() > 0);
        map.insert(
            id,
            CoreSlot {
                core: Arc::downgrade(core),
                last_drain_ms: 0,
                pending: BTreeMap::new(),
                #[cfg(test)]
                drains: 0,
            },
        );
    }
    map.get_mut(&id).expect("slot inserted above")
}

/// Queues a periodic progress sample and opportunistically drains the
/// mailbox without ever blocking on the Core lock.
pub(crate) fn post(core: &Arc<Mutex<PersistentCore>>, task_id: &str, sample: ProgressSample) {
    let due = {
        let mut map = slots().lock().unwrap_or_else(|error| error.into_inner());
        let slot = slot_for(&mut map, core);
        slot.pending.insert(task_id.to_string(), sample);
        now_ms().saturating_sub(slot.last_drain_ms) >= DRAIN_INTERVAL_MS
    };
    if !due {
        return;
    }
    match core.try_lock() {
        Ok(mut guard) => {
            drain_locked(core, &mut guard);
        }
        // Busy: the samples stay queued for the next post or synchronous path.
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Poisoned(_)) => {}
    }
}

fn take(core: &Arc<Mutex<PersistentCore>>, only: Option<&str>) -> Vec<(String, ProgressSample)> {
    let mut map = slots().lock().unwrap_or_else(|error| error.into_inner());
    let Some(slot) = map.get_mut(&key(core)) else {
        return Vec::new();
    };
    if !slot
        .core
        .upgrade()
        .is_some_and(|live| Arc::ptr_eq(&live, core))
    {
        return Vec::new();
    }
    match only {
        Some(task_id) => slot
            .pending
            .remove(task_id)
            .map(|sample| vec![(task_id.to_string(), sample)])
            .unwrap_or_default(),
        None => {
            slot.last_drain_ms = now_ms();
            #[cfg(test)]
            {
                slot.drains += 1;
            }
            std::mem::take(&mut slot.pending).into_iter().collect()
        }
    }
}

fn apply(guard: &mut PersistentCore, samples: Vec<(String, ProgressSample)>) -> usize {
    let count = samples.len();
    for (task_id, sample) in samples {
        // Same handler and error policy as the former per-sample call sites,
        // which ignored the result of periodic progress updates.
        let _ = guard.handle(CoreCommand::UpdateProgress {
            task_id,
            downloaded_bytes: sample.downloaded,
            total_bytes: sample.total,
            speed_bytes_per_sec: sample.speed,
            stage: sample.stage,
            status: sample.status,
        });
    }
    count
}

/// Applies every queued sample of `core`. The caller holds the Core lock
/// (`guard` is the locked `core`).
pub(crate) fn drain_locked(core: &Arc<Mutex<PersistentCore>>, guard: &mut PersistentCore) -> usize {
    apply(guard, take(core, None))
}

/// Applies the queued sample of one task, if any, before a synchronous
/// progress or state change of that task. The caller holds the Core lock.
pub(crate) fn flush_task_locked(
    core: &Arc<Mutex<PersistentCore>>,
    guard: &mut PersistentCore,
    task_id: &str,
) -> usize {
    apply(guard, take(core, Some(task_id)))
}

#[cfg(test)]
pub(crate) fn pending_len(core: &Arc<Mutex<PersistentCore>>) -> usize {
    let map = slots().lock().unwrap_or_else(|error| error.into_inner());
    map.get(&key(core)).map_or(0, |slot| slot.pending.len())
}

#[cfg(test)]
pub(crate) fn drains(core: &Arc<Mutex<PersistentCore>>) -> u64 {
    let map = slots().lock().unwrap_or_else(|error| error.into_inner());
    map.get(&key(core)).map_or(0, |slot| slot.drains)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_with_tasks(count: usize) -> (Arc<Mutex<PersistentCore>>, Vec<String>) {
        let mut core = PersistentCore::in_memory().unwrap();
        let mut ids = Vec::new();
        for index in 0..count {
            let events = core
                .handle(CoreCommand::CreateTask {
                    spec: crate::TaskSpec {
                        url: format!("https://example.test/relay-{index}.bin"),
                        resource_kind: crate::ResourceKind::File,
                        filename: format!("relay-{index}.bin"),
                        request_method: "GET".into(),
                        concurrency: 4,
                        ..Default::default()
                    },
                })
                .unwrap();
            ids.extend(events.iter().filter_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            }));
        }
        assert_eq!(ids.len(), count);
        (Arc::new(Mutex::new(core)), ids)
    }

    fn sample(bytes: u64, status: &str) -> ProgressSample {
        ProgressSample {
            downloaded: bytes,
            total: Some(1000),
            speed: 10,
            stage: "transfer".into(),
            status: status.into(),
        }
    }

    fn bytes(core: &Arc<Mutex<PersistentCore>>, id: &str) -> u64 {
        core.lock().unwrap().snapshot(id).unwrap().downloaded_bytes
    }

    #[test]
    fn a_busy_core_lock_never_blocks_the_worker_and_the_sample_lands_later() {
        let (core, ids) = core_with_tasks(1);
        let id = ids[0].as_str();
        let held = core.lock().unwrap();
        let started = Instant::now();
        post(&core, id, sample(100, "downloading"));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert_eq!(pending_len(&core), 1);
        drop(held);
        std::thread::sleep(std::time::Duration::from_millis(DRAIN_INTERVAL_MS + 20));
        post(&core, id, sample(200, "downloading"));
        assert_eq!(pending_len(&core), 0);
        assert_eq!(bytes(&core, id), 200);
    }

    #[test]
    fn many_tasks_share_one_drain_per_interval() {
        let (core, ids) = core_with_tasks(8);
        // Force the first post to drain, then post the rest within the window.
        std::thread::sleep(std::time::Duration::from_millis(DRAIN_INTERVAL_MS + 5));
        for (index, id) in ids.iter().enumerate() {
            post(&core, id, sample(10 + index as u64, "downloading"));
        }
        // One drain applied the first; the rest wait for the next window.
        assert_eq!(drains(&core), 1);
        assert_eq!(pending_len(&core), ids.len() - 1);
        std::thread::sleep(std::time::Duration::from_millis(DRAIN_INTERVAL_MS + 5));
        post(&core, &ids[0], sample(500, "downloading"));
        assert_eq!(drains(&core), 2);
        assert_eq!(pending_len(&core), 0);
        assert_eq!(bytes(&core, &ids[0]), 500);
        for (index, id) in ids.iter().enumerate().skip(1) {
            assert_eq!(bytes(&core, id), 10 + index as u64);
        }
    }

    #[test]
    fn synchronous_transition_applies_the_queued_sample_first() {
        let (core, ids) = core_with_tasks(1);
        let id = ids[0].as_str();
        {
            let _held = core.lock().unwrap();
            post(&core, id, sample(300, "downloading"));
        }
        assert_eq!(pending_len(&core), 1);
        let mut guard = core.lock().unwrap();
        let after = guard.latest_sequence();
        assert_eq!(flush_task_locked(&core, &mut guard, id), 1);
        guard
            .handle(CoreCommand::UpdateProgress {
                task_id: id.into(),
                downloaded_bytes: 400,
                total_bytes: Some(1000),
                speed_bytes_per_sec: 0,
                stage: "checking".into(),
                status: "checking".into(),
            })
            .unwrap();
        let events = guard.events_after(after, 16);
        let statuses: Vec<(u64, String)> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskProgress { snapshot } => {
                    Some((snapshot.downloaded_bytes, snapshot.status.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            statuses,
            vec![(300, "downloading".into()), (400, "checking".into())]
        );
        drop(guard);
        assert_eq!(pending_len(&core), 0);
        // Nothing queued remains to overwrite the transition later.
        std::thread::sleep(std::time::Duration::from_millis(DRAIN_INTERVAL_MS + 5));
        let mut guard = core.lock().unwrap();
        assert_eq!(drain_locked(&core, &mut guard), 0);
        assert_eq!(guard.snapshot(id).unwrap().status, "checking");
    }

    #[test]
    fn a_reused_allocation_does_not_inherit_samples_of_a_dropped_core() {
        let (first, ids) = core_with_tasks(1);
        {
            let _held = first.lock().unwrap();
            post(&first, &ids[0], sample(1, "downloading"));
        }
        let weak = Arc::downgrade(&first);
        drop(first);
        assert!(weak.upgrade().is_none());
        let (second, _) = core_with_tasks(1);
        let mut guard = second.lock().unwrap();
        assert_eq!(drain_locked(&second, &mut guard), 0);
    }
}
