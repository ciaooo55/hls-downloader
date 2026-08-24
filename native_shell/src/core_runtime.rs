//! Small, deterministic state machine shared by v7 Core clients.
//!
//! The protocol workers attach to this state machine instead of inventing
//! their own task/status model. Persistence and the HTTP runner can be added
//! behind the same commands without changing the UI or extension contract.

use crate::contract::{
    CoreCommand, CoreEvent, MediaPushRequest, ResourceKind, ResourceOffer, TaskSnapshot, TaskSpec,
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
    media_push_requests: BTreeMap<String, MediaPushRequest>,
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
        for (task_id, mut spec) in specs {
            if let Some(snapshot) = runtime.tasks.get(&task_id) {
                spec.queue_id = snapshot.queue_id.clone();
            }
            runtime.specs.insert(task_id, spec);
        }
        runtime
    }

    pub fn handle(&mut self, command: CoreCommand) -> Vec<EventEnvelope> {
        let before = self.sequence;
        match command {
            CoreCommand::Ping => {
                self.publish(CoreEvent::Ready {
                    protocol: crate::V7_PROTOCOL_NAME.into(),
                    version: crate::V7_PROTOCOL_VERSION,
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
                trusted_ui: _,
            } => self.accept_handoff(&handoff_id, &filename, &download_dir),
            CoreCommand::RejectHandoff {
                handoff_id,
                suppress_site_kind: _,
            } => self.reject_handoff(&handoff_id),
            CoreCommand::RequestMediaPush { request } => {
                self.media_push_requests
                    .insert(request.id.clone(), request.clone());
                self.publish(CoreEvent::MediaPushRequested { request });
            }
            CoreCommand::ResolveMediaPush {
                request_id,
                status,
                message,
                location,
            } => {
                if let Some(request) = self.media_push_requests.get_mut(&request_id) {
                    request.status = status;
                    request.message = message;
                    request.location = location;
                    let resolved = request.clone();
                    self.publish(CoreEvent::MediaPushResolved { request: resolved });
                } else {
                    self.publish(CoreEvent::Error {
                        code: "media_push_not_found".into(),
                        message: "媒体推送请求不存在或已过期".into(),
                    });
                }
            }
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
            CoreCommand::AssignQueue { task_ids, queue_id } => {
                self.assign_queue(&task_ids, &queue_id)
            }
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
            CoreCommand::CheckUpdate { silent } => self.check_update(silent),
            CoreCommand::SetSetting { .. }
            | CoreCommand::PlayTask { .. }
            | CoreCommand::CastTask { .. }
            | CoreCommand::PlayerControl { .. }
            | CoreCommand::DownloadUpdate
            | CoreCommand::InstallUpdate { .. }
            | CoreCommand::ProbeUrl { .. }
            | CoreCommand::ProbeTorrent { .. }
            | CoreCommand::SelectTorrentFiles { .. }
            | CoreCommand::DiscoverCastDevices { .. }
            | CoreCommand::CastToDevice { .. }
            | CoreCommand::ShareMedia { .. }
            | CoreCommand::OpenCompleted { .. }
            | CoreCommand::ConfirmPowerAction
            | CoreCommand::CancelPowerAction
            | CoreCommand::SaveSiteProfile { .. }
            | CoreCommand::ImportPaths { .. }
            | CoreCommand::ExportTasks { .. }
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

    pub fn replace_spec(&mut self, task_id: &str, spec: TaskSpec) {
        if let Some(snapshot) = self.tasks.get_mut(task_id) {
            snapshot.url = spec.url.clone();
            snapshot.mirror_status = if spec.mirrors.is_empty() {
                String::new()
            } else {
                format!("{} 镜像", spec.mirrors.len())
            };
        }
        self.specs.insert(task_id.to_string(), spec);
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
        self.pending_handoffs
            .insert(offer.handoff_id.clone(), offer);
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
            queue_id: spec.queue_id.clone(),
            output_missing: false,
            output_path: std::path::Path::new(&spec.download_dir)
                .join(&spec.filename)
                .to_string_lossy()
                .into_owned(),
            connection_hint: String::new(),
            connection_parts: Vec::new(),
            log_tail: Vec::new(),
            speed_history: Vec::new(),
            mirror_status: if spec.mirrors.is_empty() {
                String::new()
            } else {
                format!("{} 镜像", spec.mirrors.len())
            },
            request_method: spec.request_method.clone(),
            download_dir: spec.download_dir.clone(),
            speed_limit_kib: spec.speed_limit_kib,
            expected_checksum: spec.checksum.clone().unwrap_or_default(),
            max_workers: spec.concurrency,
            mirrors: spec.mirrors.clone(),
            scheduled_start_at: spec.scheduled_start_at.clone(),
            scheduled_stop_at: spec.scheduled_stop_at.clone(),
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
        let Some(queue_id) = self.tasks.get(task_id).map(|task| task.queue_id.clone()) else {
            return;
        };
        let mut ordered: Vec<String> = {
            let mut tasks: Vec<_> = self
                .tasks
                .values()
                .filter(|task| task.queue_id == queue_id)
                .cloned()
                .collect();
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
        let Some(queue_id) = self.tasks.get(task_id).map(|task| task.queue_id.clone()) else {
            return;
        };
        let mut ordered: Vec<String> = {
            let mut tasks: Vec<_> = self
                .tasks
                .values()
                .filter(|task| task.queue_id == queue_id)
                .cloned()
                .collect();
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

    fn assign_queue(&mut self, task_ids: &[String], queue_id: &str) {
        let queue_id = queue_id.trim();
        if queue_id.is_empty() {
            self.publish(CoreEvent::Error {
                code: "queue_id_missing".into(),
                message: "队列编号不能为空".into(),
            });
            return;
        }
        let mut next_index = self
            .tasks
            .values()
            .filter(|task| task.queue_id == queue_id)
            .map(|task| task.queue_index)
            .max()
            .unwrap_or(0);
        let mut updates = Vec::new();
        for task_id in task_ids {
            let Some(snapshot) = self.tasks.get_mut(task_id) else {
                continue;
            };
            next_index += 1;
            snapshot.queue_id = queue_id.to_string();
            snapshot.queue_index = next_index;
            if let Some(spec) = self.specs.get_mut(task_id) {
                spec.queue_id = queue_id.to_string();
            }
            updates.push(snapshot.clone());
        }
        for snapshot in updates {
            self.publish(CoreEvent::TaskUpdated { snapshot });
        }
    }

    fn check_update(&mut self, silent: bool) {
        match crate::updater::check_for_update(crate::updater::CURRENT_VERSION) {
            Ok(info) if info.newer => self.publish(CoreEvent::UpdateAvailable {
                current: info.current,
                latest: info.latest,
                notes: info.notes,
                release_url: info.html_url,
                installer_name: info.installer_name,
                installer_size: info.installer_size,
                sha256_verified: !info.expected_sha256.is_empty(),
            }),
            Ok(info) if !silent => self.publish(CoreEvent::UpdateCurrent {
                current: info.current,
            }),
            Ok(_) => {}
            Err(error) if !silent => self.publish(CoreEvent::Error {
                code: "update_failed".into(),
                message: error,
            }),
            Err(error) => eprintln!("silent update check failed: {error}"),
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
            let stale_active_progress =
                matches!(
                    snapshot.status.as_str(),
                    "paused" | "canceled" | "failed" | "completed" | "done"
                ) && matches!(status, "downloading" | "recording" | "merging" | "checking");
            let effective_status = if stale_active_progress {
                snapshot.status.clone()
            } else {
                status.to_string()
            };
            let effective_stage = if stale_active_progress {
                snapshot.stage.clone()
            } else {
                stage.to_string()
            };
            let effective_speed = if stale_active_progress {
                0
            } else {
                speed_bytes_per_sec
            };
            snapshot.downloaded_bytes = if stale_active_progress {
                snapshot.downloaded_bytes.max(downloaded_bytes)
            } else {
                downloaded_bytes
            };
            snapshot.total_bytes = total_bytes.or(snapshot.total_bytes);
            snapshot.speed_bytes_per_sec = effective_speed;
            if effective_speed > 0 {
                snapshot.speed_history.push(effective_speed);
                if snapshot.speed_history.len() > 180 {
                    snapshot.speed_history.remove(0);
                }
            }
            snapshot.stage = effective_stage.clone();
            snapshot.status = effective_status.clone();
            snapshot.playback_ready = snapshot.downloaded_bytes > 0
                || matches!(
                    effective_status.as_str(),
                    "completed" | "downloading" | "merging"
                );
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
                let progress = crate::TaskPaths::for_task(task_id, spec)
                    .map(|paths| paths.progress)
                    .unwrap_or_else(|_| {
                        root.join(".hls-tasks").join(task_id).join("progress.json")
                    });
                let parts = crate::paint_from_progress(
                    &progress,
                    snapshot.downloaded_bytes,
                    snapshot.total_bytes.unwrap_or(0),
                    matches!(effective_status.as_str(), "downloading" | "recording"),
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
                effective_status,
                effective_stage,
                snapshot.downloaded_bytes,
                snapshot.total_bytes.unwrap_or(0)
            );
            snapshot.log_tail.push(line);
            if snapshot.log_tail.len() > 16 {
                let extra = snapshot.log_tail.len() - 16;
                snapshot.log_tail.drain(0..extra);
            }
            snapshot.available_actions = match effective_status.as_str() {
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
                "paused" => vec![
                    "resume".into(),
                    "cancel".into(),
                    "delete".into(),
                    "queue_up".into(),
                    "queue_down".into(),
                ],
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
            snapshot.eta_seconds = match (snapshot.total_bytes, effective_speed) {
                (Some(total), speed) if speed > 0 && total > snapshot.downloaded_bytes => {
                    Some((total - snapshot.downloaded_bytes) / speed)
                }
                _ => None,
            };
            if effective_status == "failed" && snapshot.error_message.is_none() {
                snapshot.error_message = Some(effective_stage);
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

    pub fn set_output_path(&mut self, task_id: &str, path: String) {
        if let Some(snapshot) = self.tasks.get_mut(task_id) {
            if snapshot.output_path != path {
                snapshot.output_path = path;
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
    fn late_active_progress_cannot_overwrite_a_paused_task() {
        let mut runtime = CoreRuntime::new();
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        });
        runtime.handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 100,
            total_bytes: Some(1_000),
            speed_bytes_per_sec: 50,
            stage: "transfer".into(),
            status: "downloading".into(),
        });
        runtime.handle(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "pause".into(),
        });

        runtime.handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 120,
            total_bytes: Some(1_000),
            speed_bytes_per_sec: 50,
            stage: "transfer".into(),
            status: "downloading".into(),
        });

        let paused = runtime.snapshot("task-1").unwrap();
        assert_eq!(paused.status, "paused");
        assert_eq!(paused.stage, "waiting");
        assert_eq!(paused.downloaded_bytes, 120);
        assert_eq!(paused.speed_bytes_per_sec, 0);
        assert!(paused
            .available_actions
            .iter()
            .any(|action| action == "resume"));
        assert!(!paused
            .available_actions
            .iter()
            .any(|action| action == "pause"));

        runtime.handle(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "resume".into(),
        });
        runtime.handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 140,
            total_bytes: Some(1_000),
            speed_bytes_per_sec: 50,
            stage: "transfer".into(),
            status: "downloading".into(),
        });
        assert_eq!(runtime.snapshot("task-1").unwrap().status, "downloading");
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
    fn assign_queue_updates_snapshot_and_spec_then_reorders_inside_destination() {
        let mut runtime = CoreRuntime::new();
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        runtime.handle(CoreCommand::CreateTask { spec: task() });

        let events = runtime.handle(CoreCommand::AssignQueue {
            task_ids: vec!["task-1".into(), "task-3".into()],
            queue_id: "night-media".into(),
        });
        assert_eq!(runtime.snapshot("task-1").unwrap().queue_id, "night-media");
        assert_eq!(runtime.task_spec("task-3").unwrap().queue_id, "night-media");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, CoreEvent::TaskUpdated { .. }))
                .count(),
            2
        );

        runtime.handle(CoreCommand::ReorderQueue {
            task_id: "task-3".into(),
            delta: -1,
        });
        assert!(
            runtime.snapshot("task-3").unwrap().queue_index
                < runtime.snapshot("task-1").unwrap().queue_index
        );
        assert_eq!(
            runtime.snapshot("task-2").unwrap().queue_id,
            crate::DEFAULT_QUEUE_ID
        );
    }

    #[test]
    fn blank_queue_assignment_is_rejected_without_mutating_tasks() {
        let mut runtime = CoreRuntime::new();
        runtime.handle(CoreCommand::CreateTask { spec: task() });
        let events = runtime.handle(CoreCommand::AssignQueue {
            task_ids: vec!["task-1".into()],
            queue_id: "  ".into(),
        });
        assert_eq!(
            runtime.snapshot("task-1").unwrap().queue_id,
            crate::DEFAULT_QUEUE_ID
        );
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::Error { code, .. } if code == "queue_id_missing"
        )));
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
                mime_type: "application/vnd.microsoft.portable-executable".into(),
                size: 1024,
            },
        });
        assert!(matches!(events[0].event, CoreEvent::HandoffOffered { .. }));
        let accepted = runtime.handle(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-ui".into(),
            filename: "installer.exe".into(),
            download_dir: String::new(),
            trusted_ui: false,
        });
        assert!(matches!(accepted[0].event, CoreEvent::TaskCreated { .. }));
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
            suppress_site_kind: false,
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
