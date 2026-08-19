//! Small, deterministic Rust Core state machine used by the v6 UI and bridge.
//!
//! The protocol workers attach to this state machine instead of inventing
//! their own task/status model. Persistence and the HTTP runner can be added
//! behind the same commands without changing the UI or extension contract.

use crate::v6_contract::{
    CoreCommand, CoreEvent, ResourceKind, ResourceOffer, TaskSnapshot, TaskSpec,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub event: CoreEvent,
}

#[derive(Debug, Default, Clone)]
pub struct CoreRuntime {
    tasks: BTreeMap<String, TaskSnapshot>,
    specs: BTreeMap<String, TaskSpec>,
    pending_handoffs: BTreeMap<String, ResourceOffer>,
    events: VecDeque<EventEnvelope>,
    next_task: u64,
    next_handoff: u64,
    sequence: u64,
}

impl CoreRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshots(snapshots: impl IntoIterator<Item = TaskSnapshot>) -> Self {
        Self::from_state(snapshots, std::iter::empty(), 0)
    }

    pub fn from_state(
        snapshots: impl IntoIterator<Item = TaskSnapshot>,
        specs: impl IntoIterator<Item = (String, TaskSpec)>,
        sequence: u64,
    ) -> Self {
        let mut runtime = Self::new();
        runtime.sequence = sequence;
        for snapshot in snapshots {
            if let Some(number) = snapshot
                .task_id
                .strip_prefix("task-")
                .and_then(|value| value.parse::<u64>().ok())
            {
                runtime.next_task = runtime.next_task.max(number);
            }
            runtime.tasks.insert(snapshot.task_id.clone(), snapshot);
        }
        runtime.specs.extend(specs);
        runtime
    }

    pub fn handle(&mut self, command: CoreCommand) -> Vec<EventEnvelope> {
        let before = self.sequence;
        match command {
            CoreCommand::Ping => {
                self.publish(CoreEvent::Ready {
                    protocol: crate::V6_PROTOCOL_NAME.into(),
                    version: crate::V6_PROTOCOL_VERSION,
                });
            }
            CoreCommand::CreateTask { spec } => {
                let snapshot = self.create_task(spec);
                self.publish(CoreEvent::TaskCreated { snapshot });
            }
            CoreCommand::TaskAction { task_id, action } => self.action(&task_id, &action),
            CoreCommand::UpdateProgress {
                task_id,
                downloaded_bytes,
                total_bytes,
                speed_bytes_per_sec,
                stage,
                status,
            } => self.progress(
                &task_id,
                downloaded_bytes,
                total_bytes,
                speed_bytes_per_sec,
                &stage,
                &status,
            ),
            CoreCommand::OfferResource { mut offer } => {
                if offer.handoff_id.trim().is_empty() {
                    self.next_handoff += 1;
                    offer.handoff_id = format!("handoff-{}", self.next_handoff);
                }
                self.pending_handoffs
                    .insert(offer.handoff_id.clone(), offer.clone());
                self.publish(CoreEvent::HandoffOffered { offer });
            }
            CoreCommand::AcceptHandoff {
                handoff_id,
                filename,
                download_dir,
            } => self.accept_handoff(&handoff_id, &filename, &download_dir),
            CoreCommand::RejectHandoff { handoff_id } => self.reject_handoff(&handoff_id),
            CoreCommand::OpenMain => {
                self.publish(CoreEvent::UiShow {
                    surface: "main".into(),
                });
            }
            CoreCommand::HideMain => {
                self.publish(CoreEvent::UiShow {
                    surface: "hide".into(),
                });
            }
            CoreCommand::Shutdown => {}
            CoreCommand::ReorderQueue { task_id, delta } => self.reorder(&task_id, delta),
            CoreCommand::PlaceQueue { task_id, before_id } => self.place(&task_id, &before_id),
            CoreCommand::GetTaskLog { task_id } => self.emit_log(&task_id),
            CoreCommand::BrowserHello { version, browser } => {
                self.publish(CoreEvent::BrowserStatus {
                    connected: true,
                    version: version.clone(),
                    browser: browser.clone(),
                    message: if browser.is_empty() {
                        "浏览器插件已连接".into()
                    } else {
                        format!("插件已连接 · {browser}")
                    },
                });
            }
            CoreCommand::CheckUpdate => self.check_update(),
            CoreCommand::SetSetting { .. }
            | CoreCommand::PlayTask { .. }
            | CoreCommand::CastTask { .. }
            | CoreCommand::PlayerControl { .. }
            | CoreCommand::DownloadUpdate
            | CoreCommand::ProbeUrl { .. }
            | CoreCommand::DiscoverCastDevices
            | CoreCommand::CastToDevice { .. }
            | CoreCommand::OpenCompleted { .. }
            | CoreCommand::CancelPowerAction
            | CoreCommand::SaveSiteProfile { .. }
            | CoreCommand::ImportPaths { .. }
            | CoreCommand::HarvestPage { .. }
            | CoreCommand::ControlCast { .. }
            | CoreCommand::PresentHandoff { .. } => {}
            CoreCommand::ClearCompleted => self.clear_completed(),
        }
        self.events
            .iter()
            .filter(|item| item.sequence > before)
            .cloned()
            .collect()
    }

    pub fn snapshot(&self, task_id: &str) -> Option<&TaskSnapshot> {
        self.tasks.get(task_id)
    }

    pub fn task_spec(&self, task_id: &str) -> Option<&TaskSpec> {
        self.specs.get(task_id)
    }

    pub fn pending_handoff(&self, handoff_id: &str) -> Option<&ResourceOffer> {
        self.pending_handoffs.get(handoff_id)
    }

    pub fn take_pending_handoff(&mut self, handoff_id: &str) -> Option<ResourceOffer> {
        self.pending_handoffs.remove(handoff_id)
    }

    pub fn restore_pending_handoff(&mut self, offer: ResourceOffer) {
        if offer.handoff_id.trim().is_empty() {
            return;
        }
        self.pending_handoffs.insert(offer.handoff_id.clone(), offer);
    }

    pub fn list_tasks(&self) -> Vec<TaskSnapshot> {
        let mut tasks: Vec<_> = self.tasks.values().cloned().collect();
        tasks.sort_by_key(|item| (item.queue_index, item.task_id.clone()));
        tasks
    }

    pub fn events_after(&self, sequence: u64, limit: usize) -> Vec<EventEnvelope> {
        self.events
            .iter()
            .filter(|item| item.sequence > sequence)
            .take(limit.max(1))
            .cloned()
            .collect()
    }

    pub fn latest_sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn emit(&mut self, event: CoreEvent) -> Vec<EventEnvelope> {
        let before = self.sequence;
        self.publish(event);
        self.events
            .iter()
            .filter(|item| item.sequence > before)
            .cloned()
            .collect()
    }

    fn create_task(&mut self, spec: TaskSpec) -> TaskSnapshot {
        self.next_task += 1;
        let task_id = format!("task-{}", self.next_task);
        let title = if spec.title.is_empty() {
            spec.filename.clone()
        } else {
            spec.title.clone()
        };
        let snapshot = TaskSnapshot {
            task_id: task_id.clone(),
            resource_kind: spec.resource_kind,
            status: "queued".into(),
            stage: "waiting".into(),
            title,
            filename: spec.filename.clone(),
            downloaded_bytes: 0,
            total_bytes: spec.expected_size,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_workers: 0,
            completed_ranges: 0,
            total_ranges: spec
                .expected_size
                .map(|bytes| bytes.div_ceil(8 * 1024 * 1024))
                .unwrap_or(0),
            playback_ready: false,
            is_live: matches!(spec.resource_kind, ResourceKind::Live),
            available_actions: vec!["start".into(), "delete".into()],
            url: spec.url.clone(),
            error_code: None,
            error_message: None,
            queue_index: self.next_task as i64,
            output_missing: false,
            connection_hint: String::new(),
            connection_parts: Vec::new(),
            log_tail: Vec::new(),
        };
        self.tasks.insert(task_id, snapshot.clone());
        self.specs.insert(snapshot.task_id.clone(), spec);
        snapshot
    }

    fn accept_handoff(&mut self, handoff_id: &str, filename: &str, download_dir: &str) {
        let Some(offer) = self.pending_handoffs.remove(handoff_id) else {
            self.publish(CoreEvent::Error {
                code: "handoff_not_found".into(),
                message: "接管请求不存在或已过期".into(),
            });
            return;
        };
        let filename = if filename.trim().is_empty() {
            if offer.filename.trim().is_empty() {
                offer
                    .url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(&offer.url)
                    .rsplit('/')
                    .find(|part| !part.is_empty())
                    .unwrap_or("download")
                    .to_string()
            } else {
                offer.filename.clone()
            }
        } else {
            filename.to_string()
        };
        let spec = TaskSpec {
            url: offer.url,
            resource_kind: offer.resource_kind,
            title: if offer.title.trim().is_empty() {
                filename.clone()
            } else {
                offer.title
            },
            filename,
            download_dir: download_dir.to_string(),
            request_method: offer.request_method,
            credential_ref: offer.credential_ref,
            replay_context_ref: offer.replay_context_ref,
            expected_size: (offer.size > 0).then_some(offer.size),
            ..Default::default()
        };
        let snapshot = self.create_task(spec);
        let task_id = snapshot.task_id.clone();
        self.publish(CoreEvent::TaskCreated { snapshot });
        self.publish(CoreEvent::HandoffResolved {
            handoff_id: handoff_id.to_string(),
            task_id: Some(task_id),
        });
    }

    fn reject_handoff(&mut self, handoff_id: &str) {
        if self.pending_handoffs.remove(handoff_id).is_none() {
            self.publish(CoreEvent::Error {
                code: "handoff_not_found".into(),
                message: "接管请求不存在或已过期".into(),
            });
            return;
        }
        self.publish(CoreEvent::HandoffResolved {
            handoff_id: handoff_id.to_string(),
            task_id: None,
        });
    }

    fn action(&mut self, task_id: &str, action: &str) {
        if action == "delete" {
            if self.tasks.remove(task_id).is_some() {
                self.specs.remove(task_id);
                self.publish(CoreEvent::TaskDeleted {
                    task_id: task_id.to_string(),
                });
            }
            return;
        }
        let updated = {
            let Some(snapshot) = self.tasks.get_mut(task_id) else {
                self.publish(CoreEvent::Error {
                    code: "task_not_found".into(),
                    message: format!("unknown task {task_id}"),
                });
                return;
            };
            match action {
                "start" | "resume" | "retry" => {
                    snapshot.status = "downloading".into();
                    snapshot.stage = "transfer".into();
                    snapshot.available_actions = vec!["pause".into(), "cancel".into()];
                    Some(snapshot.clone())
                }
                "pause" => {
                    snapshot.status = "paused".into();
                    snapshot.stage = "waiting".into();
                    snapshot.available_actions = vec!["resume".into(), "cancel".into()];
                    Some(snapshot.clone())
                }
                "cancel" => {
                    snapshot.status = "canceled".into();
                    snapshot.stage = "finished".into();
                    snapshot.available_actions = vec!["delete".into()];
                    Some(snapshot.clone())
                }
                _ => None,
            }
        };
        if let Some(snapshot) = updated {
            self.publish(CoreEvent::TaskUpdated { snapshot });
        } else {
            self.publish(CoreEvent::Error {
                code: "unknown_action".into(),
                message: format!("unknown task action {action}"),
            });
        }
    }

    fn place(&mut self, task_id: &str, before_id: &str) {
        let mut ordered: Vec<String> = {
            let mut tasks: Vec<_> = self.tasks.values().cloned().collect();
            tasks.sort_by_key(|item| (item.queue_index, item.task_id.clone()));
            tasks.into_iter().map(|item| item.task_id).collect()
        };
        let Some(index) = ordered.iter().position(|id| id == task_id) else {
            return;
        };
        ordered.remove(index);
        let dest = if before_id == "^" {
            0
        } else if before_id.trim().is_empty() {
            ordered.len()
        } else {
            ordered
                .iter()
                .position(|id| id == before_id)
                .unwrap_or(ordered.len())
        };
        ordered.insert(dest.min(ordered.len()), task_id.to_string());
        for (position, id) in ordered.iter().enumerate() {
            if let Some(snapshot) = self.tasks.get_mut(id) {
                snapshot.queue_index = (position as i64) + 1;
                let updated = snapshot.clone();
                self.publish(CoreEvent::TaskUpdated { snapshot: updated });
            }
        }
    }

    fn emit_log(&mut self, task_id: &str) {
        let lines = self
            .tasks
            .get(task_id)
            .map(|snapshot| snapshot.log_tail.clone())
            .unwrap_or_default();
        self.publish(CoreEvent::TaskLog {
            task_id: task_id.to_string(),
            lines,
        });
    }

    fn reorder(&mut self, task_id: &str, delta: i32) {
        if delta == 0 {
            return;
        }
        let mut ordered: Vec<String> = {
            let mut tasks: Vec<_> = self.tasks.values().cloned().collect();
            tasks.sort_by_key(|item| (item.queue_index, item.task_id.clone()));
            tasks.into_iter().map(|item| item.task_id).collect()
        };
        let Some(index) = ordered.iter().position(|id| id == task_id) else {
            return;
        };
        let item = ordered.remove(index);
        let dest = (index as i32 + delta).clamp(0, ordered.len() as i32) as usize;
        ordered.insert(dest, item);
        for (position, id) in ordered.iter().enumerate() {
            if let Some(snapshot) = self.tasks.get_mut(id) {
                snapshot.queue_index = (position as i64) + 1;
                let updated = snapshot.clone();
                self.publish(CoreEvent::TaskUpdated { snapshot: updated });
            }
        }
    }

    fn check_update(&mut self) {
        match crate::updater::check_for_update(crate::updater::CURRENT_VERSION) {
            Ok(info) if info.newer => self.publish(CoreEvent::Error {
                code: "update_available".into(),
                message: if info.installer_url.is_empty() {
                    format!("发现新版本 {}（当前 {}）\n{}", info.latest, info.current, info.html_url)
                } else {
                    format!(
                        "发现新版本 {}（当前 {}）。可在设置里下载安装包。\n{}",
                        info.latest, info.current, info.html_url
                    )
                },
            }),
            Ok(info) => self.publish(CoreEvent::Error {
                code: "update_current".into(),
                message: format!("已是最新版本 {}", info.current),
            }),
            Err(error) => self.publish(CoreEvent::Error {
                code: "update_failed".into(),
                message: error,
            }),
        }
    }

    fn progress(
        &mut self,
        task_id: &str,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        speed_bytes_per_sec: u64,
        stage: &str,
        status: &str,
    ) {
        let spec = self.specs.get(task_id).cloned();
        let updated = {
            let Some(snapshot) = self.tasks.get_mut(task_id) else {
                self.publish(CoreEvent::Error {
                    code: "task_not_found".into(),
                    message: format!("unknown task {task_id}"),
                });
                return;
            };
            snapshot.downloaded_bytes = downloaded_bytes;
            snapshot.total_bytes = total_bytes.or(snapshot.total_bytes);
            snapshot.speed_bytes_per_sec = speed_bytes_per_sec;
            snapshot.stage = stage.to_string();
            snapshot.status = status.to_string();
            snapshot.playback_ready = downloaded_bytes > 0
                || matches!(status, "completed" | "downloading" | "merging");
            snapshot.completed_ranges =
                if snapshot.total_ranges > 0 && snapshot.total_bytes.unwrap_or(0) > 0 {
                    snapshot
                        .downloaded_bytes
                        .saturating_mul(snapshot.total_ranges)
                        .checked_div(snapshot.total_bytes.unwrap_or(1))
                        .unwrap_or(0)
                        .min(snapshot.total_ranges)
                } else {
                    0
                };
            let spec = spec.as_ref();
            if let Some(spec) = spec {
                let root = if spec.download_dir.trim().is_empty() {
                    std::path::PathBuf::from("downloads")
                } else {
                    std::path::PathBuf::from(&spec.download_dir)
                };
                let progress = root
                    .join(".v6-tasks")
                    .join(task_id)
                    .join("progress.json");
                let parts = crate::paint_from_progress(
                    &progress,
                    downloaded_bytes,
                    snapshot.total_bytes.unwrap_or(0),
                    matches!(status, "downloading" | "recording"),
                );
                if !parts.is_empty() {
                    let (workers, completed, total, hint) = crate::summarize_parts(&parts);
                    snapshot.active_workers = workers;
                    snapshot.completed_ranges = completed;
                    snapshot.total_ranges = total.max(1);
                    snapshot.connection_hint = hint;
                    snapshot.connection_parts = parts;
                }
            }
            if snapshot.connection_hint.is_empty() {
                snapshot.connection_hint = if snapshot.total_ranges > 0 {
                    format!(
                        "{} 连接 · {}/{} 分段",
                        snapshot.active_workers, snapshot.completed_ranges, snapshot.total_ranges
                    )
                } else if snapshot.active_workers > 0 {
                    format!("{} 连接", snapshot.active_workers)
                } else {
                    String::new()
                };
            }
            let line = format!(
                "{} {} {}/{}",
                status,
                stage,
                downloaded_bytes,
                snapshot.total_bytes.unwrap_or(0)
            );
            snapshot.log_tail.push(line);
            if snapshot.log_tail.len() > 16 {
                let extra = snapshot.log_tail.len() - 16;
                snapshot.log_tail.drain(0..extra);
            }
            snapshot.available_actions = match status {
                "completed" | "done" => {
                    vec![
                        "open".into(),
                        "launch".into(),
                        "play".into(),
                        "cast".into(),
                        "retry".into(),
                        "delete".into(),
                    ]
                }
                "failed" | "canceled" => vec!["retry".into(), "delete".into()],
                "paused" => vec!["resume".into(), "cancel".into(), "delete".into(), "queue_up".into(), "queue_down".into()],
                "queued" => vec![
                    "start".into(),
                    "delete".into(),
                    "queue_up".into(),
                    "queue_top".into(),
                    "queue_down".into(),
                    "queue_bottom".into(),
                ],
                _ => vec!["pause".into(), "cancel".into()],
            };
            snapshot.eta_seconds = match (snapshot.total_bytes, speed_bytes_per_sec) {
                (Some(total), speed) if speed > 0 && total > downloaded_bytes => {
                    Some((total - downloaded_bytes) / speed)
                }
                _ => None,
            };
            if status == "failed" && snapshot.error_message.is_none() {
                snapshot.error_message = Some(stage.to_string());
            }
            snapshot.clone()
        };
        self.publish(CoreEvent::TaskProgress { snapshot: updated });
    }

    fn clear_completed(&mut self) {
        let ids: Vec<String> = self
            .tasks
            .values()
            .filter(|task| matches!(task.status.as_str(), "completed" | "done"))
            .map(|task| task.task_id.clone())
            .collect();
        for task_id in ids {
            self.action(&task_id, "delete");
        }
    }

    pub fn mark_output_missing(&mut self, task_id: &str, missing: bool) {
        if let Some(snapshot) = self.tasks.get_mut(task_id) {
            if snapshot.output_missing != missing {
                snapshot.output_missing = missing;
                if missing && matches!(snapshot.status.as_str(), "completed" | "done") {
                    snapshot.available_actions =
                        vec!["retry".into(), "open".into(), "delete".into()];
                }
                let updated = snapshot.clone();
                self.publish(CoreEvent::TaskUpdated { snapshot: updated });
            }
        }
    }

    fn publish(&mut self, event: CoreEvent) {
        self.sequence += 1;
        self.events.push_back(EventEnvelope {
            sequence: self.sequence,
            event,
        });
        while self.events.len() > 4096 {
            self.events.pop_front();
        }
    }
}

#[allow(dead_code)]
fn _keep_contract_types_visible(_: ResourceOffer) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> TaskSpec {
        TaskSpec {
            url: "https://example.test/file.bin".into(),
            resource_kind: ResourceKind::File,
            title: "Demo".into(),
            filename: "demo.bin".into(),
            download_dir: "".into(),
            request_method: "GET".into(),
            credential_ref: None,
            replay_context_ref: None,
            concurrency: 8,
            checksum: None,
            expected_size: None,
            etag: String::new(),
            last_modified: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn task_lifecycle_is_shared_with_ui() {
        let mut runtime = CoreRuntime::new();
        let created = runtime.handle(CoreCommand::CreateTask { spec: task() });
        let task_id = match &created[0].event {
            CoreEvent::TaskCreated { snapshot } => snapshot.task_id.clone(),
            other => panic!("unexpected event: {other:?}"),
        };
        runtime.handle(CoreCommand::TaskAction {
            task_id: task_id.clone(),
            action: "start".into(),
        });
        assert_eq!(runtime.snapshot(&task_id).unwrap().status, "downloading");
        runtime.handle(CoreCommand::TaskAction {
            task_id: task_id.clone(),
            action: "pause".into(),
        });
        assert_eq!(runtime.snapshot(&task_id).unwrap().status, "paused");
    }

    #[test]
    fn reorder_swaps_adjacent_queue_index() {
        let mut runtime = CoreRuntime::new();
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        let tasks = runtime.list_tasks();
        assert_eq!(tasks[0].task_id, "task-1");
        runtime.handle(CoreCommand::ReorderQueue {
            task_id: "task-1".into(),
            delta: 1,
        });
        let tasks = runtime.list_tasks();
        assert_eq!(tasks[0].task_id, "task-2");
        assert_eq!(tasks[1].task_id, "task-1");
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::ReorderQueue {
            task_id: "task-2".into(),
            delta: 2,
        });
        let tasks = runtime.list_tasks();
        assert_eq!(
            tasks
                .iter()
                .map(|item| item.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["task-1", "task-3", "task-2"]
        );
    }

    #[test]
    fn place_queue_moves_to_top_and_bottom() {
        let mut runtime = CoreRuntime::new();
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::PlaceQueue {
            task_id: "task-1".into(),
            before_id: "^".into(),
        });
        let tasks = runtime.list_tasks();
        assert_eq!(tasks[0].task_id, "task-1");
        runtime.handle(CoreCommand::PlaceQueue {
            task_id: "task-1".into(),
            before_id: String::new(),
        });
        let tasks = runtime.list_tasks();
        assert_eq!(tasks.last().unwrap().task_id, "task-1");
    }

    #[test]
    fn accept_handoff_creates_task_and_reject_drops_offer() {
        let mut runtime = CoreRuntime::new();
        let events = runtime.handle(CoreCommand::OfferResource {
            offer: ResourceOffer {
                url: "https://cdn.test/setup.exe".into(),
                resource_kind: ResourceKind::File,
                owner: "tab:1".into(),
                evidence: vec!["download_item".into()],
                confidence: 0.9,
                source_page_url: "https://site.test/".into(),
                credential_ref: None,
                replay_context_ref: None,
                request_method: "GET".into(),
                handoff_id: "handoff-ui".into(),
                filename: "setup.exe".into(),
                title: "Setup".into(),
                size: 1024,
            },
        });
        assert!(matches!(
            events[0].event,
            CoreEvent::HandoffOffered { .. }
        ));
        let accepted = runtime.handle(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-ui".into(),
            filename: "installer.exe".into(),
            download_dir: String::new(),
        });
        assert!(matches!(
            accepted[0].event,
            CoreEvent::TaskCreated { .. }
        ));
        match &accepted[1].event {
            CoreEvent::HandoffResolved {
                handoff_id,
                task_id,
            } => {
                assert_eq!(handoff_id, "handoff-ui");
                assert_eq!(task_id.as_deref(), Some("task-1"));
            }
            other => panic!("expected HandoffResolved, got {other:?}"),
        }
        assert_eq!(runtime.list_tasks()[0].filename, "installer.exe");
        runtime.handle(CoreCommand::OfferResource {
            offer: ResourceOffer {
                url: "https://cdn.test/other.bin".into(),
                handoff_id: "handoff-reject".into(),
                filename: "other.bin".into(),
                ..Default::default()
            },
        });
        let rejected = runtime.handle(CoreCommand::RejectHandoff {
            handoff_id: "handoff-reject".into(),
        });
        match &rejected[0].event {
            CoreEvent::HandoffResolved {
                handoff_id,
                task_id,
            } => {
                assert_eq!(handoff_id, "handoff-reject");
                assert!(task_id.is_none());
            }
            other => panic!("expected HandoffResolved, got {other:?}"),
        }
        assert!(runtime.pending_handoff("handoff-ui").is_none());
        assert!(runtime.pending_handoff("handoff-reject").is_none());
    }

    #[test]
    fn event_cursor_is_monotonic_and_bounded() {
        let mut runtime = CoreRuntime::new();
        for _ in 0..5000 {
            runtime.handle(CoreCommand::Ping);
        }
        assert!(runtime.events_after(0, 10).len() <= 10);
        assert!(runtime.latest_sequence() >= 5000);
        assert!(runtime.events.len() <= 4096);
    }
}
