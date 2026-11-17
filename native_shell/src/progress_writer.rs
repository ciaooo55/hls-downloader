//! Single off-lock writer for periodic active-progress rows.
//!
//! P0 limited steady progress to one SQLite write per task every
//! [`crate::core_service::PROGRESS_PERSIST_INTERVAL`], but that write still ran
//! inside the global `Mutex<PersistentCore>`, so every IPC command and every
//! progress drain could wait behind a SQLite transaction.
//!
//! A file-backed Core now hands those periodic rows to one writer thread with
//! its own connection to the same WAL database. The Core publishes the event
//! exactly like a volatile progress event (same name, same sequence rules,
//! sequence covered by the reserved checkpoint *before* publication), then
//! queues the snapshot here. The writer coalesces per task (latest row wins)
//! and writes the batch in one transaction with
//! [`crate::store::write_progress_rows`], whose guarded `UPDATE` never
//! overwrites a newer synchronous row and never re-creates a deleted task.
//!
//! Everything that is not a periodic same-state progress sample (state or
//! stage changes, failures, completion, settings, handoffs, deletions) keeps
//! the synchronous commit-before-publish path under the Core lock.
//!
//! In-memory stores (tests, diagnostics) have no shareable file, so they keep
//! the previous synchronous write.

use crate::store::{write_progress_rows, ProgressRow};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

#[derive(Default)]
struct Queue {
    pending: BTreeMap<String, ProgressRow>,
    /// Rows taken by the writer and not yet committed (or failed).
    in_flight: usize,
    shutdown: bool,
    written: u64,
}

pub(crate) struct ProgressWriter {
    state: Arc<(Mutex<Queue>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl ProgressWriter {
    /// Starts the writer on `connection` (a second connection to the Core's
    /// database file).
    pub(crate) fn start(connection: Connection) -> Option<Self> {
        let state = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker_state = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("hls-core-progress-writer".into())
            .spawn(move || run(connection, worker_state))
            .ok()?;
        Some(Self {
            state,
            thread: Some(thread),
        })
    }

    /// Queues the latest row of a task. Never blocks on SQLite.
    pub(crate) fn submit(&self, row: ProgressRow) {
        let (lock, wake) = &*self.state;
        let mut queue = lock.lock().unwrap_or_else(|error| error.into_inner());
        queue.pending.insert(row.snapshot.task_id.clone(), row);
        wake.notify_all();
    }

    /// Drops a task's queued row (the task was deleted or written
    /// synchronously with newer state). A row already being written is
    /// harmless: the guarded update cannot overwrite the newer row.
    pub(crate) fn discard(&self, task_id: &str) {
        let (lock, _) = &*self.state;
        let mut queue = lock.lock().unwrap_or_else(|error| error.into_inner());
        queue.pending.remove(task_id);
    }

    /// Waits until every queued row is committed (or failed).
    pub(crate) fn wait_idle(&self) {
        let (lock, wake) = &*self.state;
        let mut queue = lock.lock().unwrap_or_else(|error| error.into_inner());
        while !queue.pending.is_empty() || queue.in_flight > 0 {
            if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
                return;
            }
            queue = wake
                .wait_timeout(queue, std::time::Duration::from_millis(50))
                .map(|(guard, _)| guard)
                .unwrap_or_else(|error| error.into_inner().0);
        }
    }

    #[cfg(test)]
    pub(crate) fn written(&self) -> u64 {
        let (lock, _) = &*self.state;
        lock.lock()
            .unwrap_or_else(|error| error.into_inner())
            .written
    }

    /// Writes everything still queued, then stops the thread.
    pub(crate) fn shutdown(&mut self) {
        {
            let (lock, wake) = &*self.state;
            let mut queue = lock.lock().unwrap_or_else(|error| error.into_inner());
            queue.shutdown = true;
            wake.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ProgressWriter {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(mut connection: Connection, state: Arc<(Mutex<Queue>, Condvar)>) {
    let (lock, wake) = &*state;
    loop {
        let rows: Vec<ProgressRow> = {
            let mut queue = lock.lock().unwrap_or_else(|error| error.into_inner());
            while queue.pending.is_empty() && !queue.shutdown {
                queue = wake.wait(queue).unwrap_or_else(|error| error.into_inner());
            }
            if queue.pending.is_empty() {
                return;
            }
            let rows: Vec<ProgressRow> = std::mem::take(&mut queue.pending).into_values().collect();
            queue.in_flight = rows.len();
            rows
        };
        let result = write_progress_rows(&mut connection, &rows);
        let mut queue = lock.lock().unwrap_or_else(|error| error.into_inner());
        queue.in_flight = 0;
        match result {
            Ok(_) => queue.written += rows.len() as u64,
            // The Core keeps these tasks marked volatile, so the orderly flush
            // on drop (or the next synchronous state write) still persists them.
            Err(error) => crate::record_runtime_log(
                "progress_writer_failed",
                serde_json::json!({"error": error, "rows": rows.len()}),
            ),
        }
        wake.notify_all();
    }
}
