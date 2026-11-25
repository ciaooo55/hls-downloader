//! Durable command boundary shared by the UI and protocol front-ends.

use crate::url_credentials::redact_urls;
use crate::{
    AvScanStatus, CoreCommand, CoreRuntime, CoreStore, EventEnvelope, MirrorStatus, TaskFailure,
    TaskSnapshot, TaskSpec,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenterWakeMode {
    BackgroundToast,
    Workbench,
}

#[derive(Default)]
struct PendingPresenterQueue {
    ui_shows: VecDeque<(String, Instant)>,
    clipboard_offers: VecDeque<(Vec<String>, Instant)>,
    undelivered_power_action: bool,
}

pub struct PresenterBridge {
    subscribers: AtomicUsize,
    pending: Mutex<PendingPresenterQueue>,
    #[allow(clippy::type_complexity)]
    waker: Mutex<Option<Arc<dyn Fn(PresenterWakeMode) + Send + Sync>>>,
}

impl PresenterBridge {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            subscribers: AtomicUsize::new(0),
            pending: Mutex::new(PendingPresenterQueue::default()),
            waker: Mutex::new(None),
        })
    }

    pub(crate) fn enter_subscriber(self: &Arc<Self>) -> PresenterSubscriberGuard {
        self.subscribers.fetch_add(1, Ordering::AcqRel);
        PresenterSubscriberGuard(Arc::clone(self))
    }

    pub(crate) fn has_subscribers(&self) -> bool {
        self.subscribers.load(Ordering::Acquire) > 0
    }

    pub(crate) fn set_waker(&self, waker: Option<Arc<dyn Fn(PresenterWakeMode) + Send + Sync>>) {
        if let Ok(mut guard) = self.waker.lock() {
            *guard = waker;
        }
    }

    pub(crate) fn wake(&self, mode: PresenterWakeMode) {
        let waker = self.waker.lock().ok().and_then(|guard| guard.clone());
        if let Some(waker) = waker {
            waker(mode);
        }
    }

    pub(crate) fn queue_ui_show(&self, surface: &str, ttl: Duration) {
        let now = Instant::now();
        if let Ok(mut guard) = self.pending.lock() {
            guard.ui_shows.retain(|(_, expires)| *expires > now);
            if guard
                .ui_shows
                .back()
                .is_none_or(|(queued, _)| queued != surface)
            {
                guard.ui_shows.push_back((surface.to_string(), now + ttl));
            }
        }
    }

    pub(crate) fn queue_clipboard_offer(&self, urls: Vec<String>, ttl: Duration) {
        if urls.is_empty() {
            return;
        }
        let now = Instant::now();
        if let Ok(mut guard) = self.pending.lock() {
            guard.clipboard_offers.retain(|(_, expires)| *expires > now);
            if guard
                .clipboard_offers
                .back()
                .is_none_or(|(queued, _)| *queued != urls)
            {
                guard.clipboard_offers.push_back((urls, now + ttl));
            }
        }
    }

    pub(crate) fn mark_power_action_pending(&self) {
        if let Ok(mut guard) = self.pending.lock() {
            guard.undelivered_power_action = true;
        }
    }

    pub(crate) fn take_ready_events(&self) -> Vec<crate::CoreEvent> {
        let now = Instant::now();
        let Ok(mut guard) = self.pending.lock() else {
            return Vec::new();
        };
        let mut ready = Vec::new();
        if guard.undelivered_power_action {
            guard.undelivered_power_action = false;
            if let Some((action, title, delay_seconds)) = crate::power_action::pending_details() {
                ready.push(crate::CoreEvent::PowerActionPending {
                    action,
                    title,
                    delay_seconds,
                });
            }
        }
        while let Some((urls, expires)) = guard.clipboard_offers.pop_front() {
            if expires > now {
                ready.push(crate::CoreEvent::ClipboardOffer { urls });
            }
        }
        while let Some((surface, expires)) = guard.ui_shows.pop_front() {
            if expires > now {
                ready.push(crate::CoreEvent::UiShow { surface });
            }
        }
        ready
    }
}

pub(crate) struct PresenterSubscriberGuard(Arc<PresenterBridge>);

impl Drop for PresenterSubscriberGuard {
    fn drop(&mut self) {
        self.0.subscribers.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Active-progress snapshots of one task reach SQLite at most this often; a
/// status or stage change is always written at once.
pub(crate) const PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_secs(5);
/// Event sequences handed out without a SQLite write are covered by a
/// reserved checkpoint, so a restart after a crash never reuses a sequence a
/// subscriber may already have seen.
const VOLATILE_SEQUENCE_RESERVE: u64 = 1024;

#[derive(Debug, Clone)]
struct PersistedProgress {
    status: String,
    stage: String,
    at: Instant,
}

pub struct PersistentCore {
    runtime: CoreRuntime,
    store: CoreStore,
    event_notify: Arc<(Mutex<u64>, Condvar)>,
    /// Last snapshot state written to SQLite per task.
    persisted_progress: HashMap<String, PersistedProgress>,
    /// Tasks whose in-memory snapshot is newer than their SQLite row.
    volatile_tasks: BTreeSet<String>,
    /// Highest sequence covered by the persisted event checkpoint.
    reserved_sequence: u64,
    /// Off-lock writer for periodic same-state progress rows (file-backed
    /// stores only; see `progress_writer`).
    progress_writer: Option<crate::progress_writer::ProgressWriter>,
    /// Per-queue task IDs that failed or were canceled during the current active
    /// batch (reset when the queue starts a fresh run from idle). Historical
    /// failed/canceled tasks loaded from disk are never placed here.
    batch_disrupted: BTreeMap<String, BTreeSet<String>>,
    presenter_bridge: Arc<PresenterBridge>,
}

impl PersistentCore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::from_store(CoreStore::open(path)?)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::from_store(CoreStore::in_memory()?)
    }

    pub(crate) fn presenter_bridge(&self) -> Arc<PresenterBridge> {
        Arc::clone(&self.presenter_bridge)
    }

    pub(crate) fn offer_eligible_for_quick_accept(offer: &crate::ResourceOffer) -> bool {
        if offer.resource_kind == crate::ResourceKind::Torrent {
            return false;
        }
        let url = offer.url.trim();
        if url.is_empty() {
            return false;
        }
        if let Some(local) = offer.local_source_path.as_deref() {
            offer.resource_kind == crate::ResourceKind::File
                && !local.trim().is_empty()
                && hls_client::request_validation::validate_local_source_path(
                    url,
                    local,
                    (offer.size > 0).then_some(offer.size),
                )
                .is_ok()
        } else {
            crate::http_engine::remote_resource_url_allowed(url)
                && (offer.audio_url.trim().is_empty()
                    || (crate::http_engine::remote_resource_url_allowed(offer.audio_url.trim())
                        && crate::http_engine::http_fetch_url_allowed(offer.audio_url.trim())))
        }
    }

    fn default_quick_spec_from_store(
        &self,
        offer: &crate::ResourceOffer,
    ) -> Result<Option<TaskSpec>, String> {
        if !self
            .store
            .setting_bool("browser_quick_download_enabled", false)?
        {
            return Ok(None);
        }
        let legal_ok = self.store.setting_bool("legal_terms_accepted", true)?;
        let legal_version = self
            .store
            .setting_string("legal_terms_version", crate::LEGAL_TERMS_VERSION)?;
        if !legal_ok
            || (!legal_version.is_empty() && legal_version != crate::LEGAL_TERMS_VERSION)
            || !Self::offer_eligible_for_quick_accept(offer)
        {
            return Ok(None);
        }
        let allowed_hosts = self.store.setting_string("allowed_hosts", "")?;
        if offer.local_source_path.is_none()
            && (!crate::net_policy::url_allowed(offer.url.trim(), &allowed_hosts)
                || (!offer.audio_url.trim().is_empty()
                    && !crate::net_policy::url_allowed(offer.audio_url.trim(), &allowed_hosts)))
        {
            return Ok(None);
        }
        let filename = if !offer.filename.trim().is_empty() {
            offer.filename.trim().to_string()
        } else {
            offer
                .url
                .split(['?', '#'])
                .next()
                .unwrap_or(&offer.url)
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or("download")
                .to_string()
        };
        let filename = if offer.resource_kind == crate::ResourceKind::File
            && !offer.audio_url.trim().is_empty()
        {
            crate::download_worker::normalize_dual_track_mp4_filename(&filename)
        } else {
            filename
        };
        if crate::download_worker::validate_filename(&filename).is_err() {
            return Ok(None);
        }
        let configured_dir = self.store.setting_string("download_dir", "")?;
        let base_dir = if !configured_dir.trim().is_empty() {
            configured_dir
        } else {
            std::env::var("HLS_V7_DOWNLOAD_DIR")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| {
                    crate::default_v7_download_dir()
                        .to_string_lossy()
                        .into_owned()
                })
        };
        let auto_cat = self.store.setting_bool("auto_category_dirs", false)?;
        let cat_dirs = crate::category::parse_category_dirs(
            &self.store.setting_string("browser_category_dirs", "")?,
        );
        let download_dir = if offer.local_source_path.is_some() {
            base_dir
        } else {
            crate::category::resolve_category_dir(
                &base_dir,
                &filename,
                &offer.url,
                offer.resource_kind,
                auto_cat,
                &cat_dirs,
            )
        };
        let temp_dir = self.store.setting_string("temp_dir", "")?;
        let work_dir = if temp_dir.trim().is_empty() {
            download_dir.clone()
        } else {
            temp_dir
        };
        let concurrency = self
            .store
            .setting_u64("default_concurrency", 12)?
            .clamp(1, 64) as u32;
        let allow_duplicate = self.store.setting_bool("allow_duplicate", false)?;
        let local_source_path = match offer.local_source_path.as_deref() {
            Some(local) => Some(
                hls_client::request_validation::validate_local_source_path(
                    offer.url.trim(),
                    local,
                    (offer.size > 0).then_some(offer.size),
                )?
                .to_string_lossy()
                .into_owned(),
            ),
            None => None,
        };
        Ok(Some(TaskSpec {
            url: offer.url.trim().to_string(),
            resource_kind: offer.resource_kind,
            local_source_path,
            title: if offer.title.trim().is_empty() {
                filename.clone()
            } else {
                offer.title.clone()
            },
            filename,
            download_dir,
            work_dir,
            queue_id: crate::DEFAULT_QUEUE_ID.into(),
            allow_duplicate,
            request_method: crate::http_engine::sanitize_http_method(&offer.request_method),
            mime_type: offer.mime_type.clone(),
            concurrency,
            credential_ref: offer.credential_ref.clone(),
            replay_context_ref: offer.replay_context_ref.clone(),
            expected_size: (offer.audio_url.trim().is_empty() && offer.size > 0)
                .then_some(offer.size),
            preferred_bandwidth: offer.preferred_bandwidth,
            preferred_height: offer.preferred_height,
            preferred_variant_id: offer.preferred_variant_id.clone(),
            source_page_url: offer.source_page_url.clone(),
            audio_url: offer.audio_url.trim().to_string(),
            resource_key: if offer.local_source_path.is_some() {
                String::new()
            } else if offer.resource_key.trim().is_empty() {
                crate::http_engine::canonical_url_key(&offer.url)
            } else {
                offer.resource_key.clone()
            },
            ..Default::default()
        }))
    }

    pub(crate) fn has_quick_handoff_duplicate_conflict(
        &self,
        offer: &crate::ResourceOffer,
        spec: &TaskSpec,
    ) -> bool {
        if spec.allow_duplicate {
            return false;
        }
        let without_password = |url: &str| {
            crate::duplicate::canonicalize_url(&hls_client::url_credentials::redact_ftp_passwords(
                url,
            ))
        };
        let want_url = without_password(&offer.url);
        let is_adaptive = matches!(
            offer.resource_kind,
            crate::ResourceKind::Hls | crate::ResourceKind::Dash | crate::ResourceKind::Live
        );
        let offer_variant = (
            offer.preferred_variant_id.trim(),
            offer.preferred_bandwidth,
            offer.preferred_height,
        );
        let offer_name_lower = crate::download_worker::provisional_task_name(spec)
            .trim()
            .to_lowercase();
        let offer_key = if offer.local_source_path.is_some() {
            String::new()
        } else if !offer.resource_key.trim().is_empty() {
            offer.resource_key.trim().to_string()
        } else {
            crate::http_engine::canonical_url_key(&offer.url)
        };
        let offer_page = crate::duplicate::canonicalize_url(&offer.source_page_url);
        let want_target = (!spec.download_dir.trim().is_empty() && !offer_name_lower.is_empty())
            .then(|| {
                Path::new(spec.download_dir.trim())
                    .join(&offer_name_lower)
                    .to_string_lossy()
                    .to_lowercase()
            });

        for task in self.tasks() {
            if matches!(task.status.as_str(), "canceled" | "cancelled") {
                continue;
            }
            let stored_spec = self.task_spec(&task.task_id);
            if !want_url.is_empty() && without_password(&task.url) == want_url {
                if is_adaptive {
                    let same_variant = match stored_spec {
                        Some(stored) => {
                            (
                                stored.preferred_variant_id.trim(),
                                stored.preferred_bandwidth,
                                stored.preferred_height,
                            ) == offer_variant
                        }
                        None => offer_variant == ("", 0, 0),
                    };
                    if same_variant {
                        return true;
                    }
                } else {
                    return true;
                }
            }
            if is_adaptive
                || matches!(
                    task.resource_kind,
                    crate::ResourceKind::Hls
                        | crate::ResourceKind::Dash
                        | crate::ResourceKind::Live
                )
            {
                continue;
            }
            let task_name_lower = if !task.filename.trim().is_empty() {
                task.filename.trim().to_lowercase()
            } else if let Some(stored) = stored_spec {
                crate::download_worker::provisional_task_name(stored)
                    .trim()
                    .to_lowercase()
            } else {
                task.title.trim().to_lowercase()
            };
            let task_key = stored_spec
                .map(|stored| {
                    if stored.resource_key.trim().is_empty() {
                        crate::http_engine::canonical_url_key(&stored.url)
                    } else {
                        stored.resource_key.trim().to_string()
                    }
                })
                .unwrap_or_else(|| crate::http_engine::canonical_url_key(&task.url));
            if !offer_key.is_empty()
                && !offer_name_lower.is_empty()
                && task_key == offer_key
                && task_name_lower == offer_name_lower
            {
                return true;
            }
            if !offer_page.is_empty()
                && !offer_name_lower.is_empty()
                && crate::duplicate::canonicalize_url(&task.source_page_url) == offer_page
                && task_name_lower == offer_name_lower
            {
                return true;
            }
            if let Some(ref want_path) = want_target {
                let task_target = if !task.output_path.trim().is_empty() {
                    Some(task.output_path.trim().to_lowercase())
                } else {
                    stored_spec
                        .filter(|s| {
                            !s.download_dir.trim().is_empty() && !task_name_lower.is_empty()
                        })
                        .map(|s| {
                            Path::new(s.download_dir.trim())
                                .join(&task_name_lower)
                                .to_string_lossy()
                                .to_lowercase()
                        })
                };
                if task_target.as_deref() == Some(want_path.as_str()) {
                    return true;
                }
            }
        }
        false
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn admit_handoff(&mut self, encoded: &str) -> Result<String, String> {
        self.admit_handoff_with_quick_spec(encoded, None)
            .map(|(admitted, _)| admitted)
    }

    pub(crate) fn admit_handoff_with_quick_spec(
        &mut self,
        encoded: &str,
        quick_spec: Option<TaskSpec>,
    ) -> Result<(String, Option<String>), String> {
        let original: crate::contract::BrowserHandoff = serde_json::from_str(encoded)
            .map_err(|error| format!("decode browser handoff: {error}"))?;
        let handoff = redact_urls(original.clone())?;
        if handoff.id.trim().is_empty()
            || handoff.offer.handoff_id != handoff.id
            || handoff.status != "pending"
            || handoff.presentation != "queued"
            || handoff.task_id.is_some()
        {
            return Err("浏览器交接初始状态无效".into());
        }
        // The coordinator holds the single Core lock across lookup, runtime
        // admission and commit. Retried Hosts cannot both admit the same offer.
        if !handoff.request_id.is_empty() {
            if let Some(existing) = self
                .store
                .query_handoffs(None, Some(&handoff.request_id), false)?
                .pop()
            {
                return Ok((existing, None));
            }
        }
        if !self
            .store
            .query_handoffs(Some(&handoff.id), None, false)?
            .is_empty()
        {
            return Err("交接编号已属于另一请求".into());
        }
        let resolved_quick_spec = match quick_spec {
            Some(spec) => Some(spec),
            None => self.default_quick_spec_from_store(&original.offer)?,
        };
        if let Some(spec) = resolved_quick_spec {
            if crate::download_worker::validate_filename(&spec.filename).is_ok()
                && !self.has_quick_handoff_duplicate_conflict(&original.offer, &spec)
            {
                let redacted_spec = redact_urls(spec.clone())?;
                let before = self.runtime.clone();
                let mut events = self.runtime.handle(CoreCommand::CreateTask {
                    spec: redacted_spec,
                });
                let created_task_id = events.iter().find_map(|envelope| match &envelope.event {
                    crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                    _ => None,
                });
                if let Some(task_id) = created_task_id {
                    let mut accepted_original = original;
                    accepted_original.status = "accepted".into();
                    accepted_original.presentation = "quick_accepted".into();
                    accepted_original.task_id = Some(task_id.clone());
                    let mut accepted_handoff = handoff;
                    accepted_handoff.status = "accepted".into();
                    accepted_handoff.presentation = "quick_accepted".into();
                    accepted_handoff.task_id = Some(task_id.clone());
                    events.extend(self.runtime.emit(crate::CoreEvent::HandoffResolved {
                        handoff_id: accepted_handoff.id.clone(),
                        task_id: Some(task_id.clone()),
                    }));
                    let encoded_accepted = serde_json::to_string(&accepted_handoff)
                        .map_err(|error| format!("encode browser handoff: {error}"))?;
                    if let Err(error) = self.store.apply_events_spec_and_handoff(
                        &events,
                        Some(&spec),
                        Some(&accepted_original),
                    ) {
                        self.runtime = before;
                        return Err(error);
                    }
                    self.update_batch_disrupted_against(&before, &events);
                    self.committed(&events);
                    return Ok((encoded_accepted, Some(task_id)));
                }
                self.runtime = before;
            }
        }
        let encoded = serde_json::to_string(&handoff)
            .map_err(|error| format!("encode browser handoff: {error}"))?;
        let before = self.runtime.clone();
        let events = self.runtime.handle(CoreCommand::OfferResource {
            offer: handoff.offer.clone(),
        });
        if let Err(error) = self.store.apply_events_and_handoff(&events, &original) {
            self.runtime = before;
            return Err(error);
        }
        self.committed(&events);
        Ok((encoded, None))
    }

    pub fn handle(&mut self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
        match &command {
            CoreCommand::CreateTask { spec } => {
                crate::download_worker::validate_filename(&spec.filename)?;
            }
            CoreCommand::AcceptHandoff { filename, .. } => {
                crate::download_worker::validate_filename(filename)?;
            }
            _ => {}
        }
        // Runtime state, errors and idempotent replies only see redacted values.
        // The store takes the original accepted payload in the event transaction.
        let original = command;
        let command = redact_urls(original.clone())?;
        if let CoreCommand::GetTaskLog { task_id } = &command {
            let mut lines = self.store.load_task_log(task_id, 500)?;
            if lines.is_empty() {
                lines = self
                    .runtime
                    .snapshot(task_id)
                    .map(|snapshot| snapshot.log_tail.clone())
                    .unwrap_or_default();
            }
            return self.emit(crate::CoreEvent::TaskLog {
                task_id: task_id.clone(),
                lines,
            });
        }
        if let CoreCommand::UpdateProgress {
            task_id,
            downloaded_bytes,
            total_bytes,
            speed_bytes_per_sec,
            stage,
            status,
        } = &command
        {
            let before_snapshot = self.runtime.snapshot(task_id).cloned();
            let state_changed = before_snapshot
                .as_ref()
                .is_some_and(|previous| previous.stage != *stage || previous.status != *status);
            let queue_was_idle = before_snapshot
                .as_ref()
                .is_some_and(|previous| !self.runtime.queue_has_live_work(&previous.queue_id));
            let event = self.runtime.progress_event(
                task_id,
                *downloaded_bytes,
                *total_bytes,
                *speed_bytes_per_sec,
                stage,
                status,
            );
            let events = vec![EventEnvelope {
                sequence: self.runtime.latest_sequence() + 1,
                event,
            }];
            let volatile = self.progress_is_volatile(&events[0].event);
            // 同一活动状态下到期的周期写库交给写线程（文件库）；状态/阶段变化仍在锁内同步提交。
            let deferred = !volatile
                && self.progress_writer.is_some()
                && self.progress_is_periodic(&events[0].event);
            if volatile || deferred {
                // 活动进度只进内存快照与事件环（同名事件、同一序号规则），SQLite 每任务
                // 至多每 5 s 写一次；序号由预留检查点覆盖，崩溃重启后不会重用。
                let sequence = events[0].sequence;
                if sequence > self.reserved_sequence {
                    let reserved = sequence.saturating_add(VOLATILE_SEQUENCE_RESERVE);
                    self.store.reserve_event_sequence(reserved)?;
                    self.reserved_sequence = reserved;
                }
                self.runtime.commit_progress(events[0].event.clone());
                self.update_batch_disrupted_for_events(
                    &events,
                    before_snapshot.as_ref(),
                    queue_was_idle,
                );
                self.volatile_tasks.insert(task_id.clone());
                if deferred {
                    if let (Some(writer), crate::CoreEvent::TaskProgress { snapshot }) =
                        (self.progress_writer.as_ref(), &events[0].event)
                    {
                        // 写线程只做带序号保护的 UPDATE：更新的同步写或删除总是优先。
                        writer.submit(crate::store::ProgressRow {
                            snapshot: snapshot.clone(),
                            sequence,
                        });
                        self.persisted_progress.insert(
                            snapshot.task_id.clone(),
                            PersistedProgress {
                                status: snapshot.status.clone(),
                                stage: snapshot.stage.clone(),
                                at: Instant::now(),
                            },
                        );
                    }
                }
                self.notify_committed(&events);
                return Ok(events);
            }
            // 失败时运行态和事件环尚未改变；成功后才发布同一序号并唤醒订阅者。
            self.store.apply_events(&events)?;
            self.runtime.commit_progress(events[0].event.clone());
            self.update_batch_disrupted_for_events(
                &events,
                before_snapshot.as_ref(),
                queue_was_idle,
            );
            self.committed(&events);
            if state_changed {
                crate::record_runtime_log(
                    "task_state_changed",
                    serde_json::json!({
                        "task_id": task_id, "stage": stage, "status": status,
                        "downloaded_bytes": downloaded_bytes, "total_bytes": total_bytes,
                    }),
                );
            }
            return Ok(events);
        }
        if let CoreCommand::RequestMediaPush { request } = &command {
            let active = self
                .store
                .query_handoffs(None, None, true)?
                .into_iter()
                .filter_map(|encoded| {
                    serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok()
                })
                .find(|item| item.status == "pending");
            if let Some(active) = active {
                if active.id == request.id {
                    return Ok(Vec::new());
                }
                let mut rejected = request.clone();
                rejected.status = "failed".into();
                rejected.message = "已有投送请求正在等待设备选择".into();
                rejected.location.clear();
                return self.emit(crate::CoreEvent::MediaPushResolved { request: rejected });
            }
        }
        if let CoreCommand::ResolveMediaPush {
            request_id, status, ..
        } = &command
        {
            if matches!(status.as_str(), "done" | "failed" | "canceled") {
                let persisted = self
                    .store
                    .query_handoffs(Some(request_id), None, false)?
                    .into_iter()
                    .filter_map(|encoded| {
                        serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok()
                    })
                    .find(|item| item.id == *request_id);
                match persisted {
                    Some(item) if item.status != "pending" => return Ok(Vec::new()),
                    Some(item) => self.runtime.restore_pending_media_push(item),
                    None => {}
                }
            }
        }
        let before = self.runtime.clone();
        let events = self.runtime.handle(command);
        let spec = events.iter().find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => self.runtime.task_spec(&snapshot.task_id),
            _ => None,
        });
        if let Some(spec) = spec {
            if let Err(error) = crate::download_worker::validate_filename(&spec.filename) {
                self.runtime = before;
                return Err(error);
            }
        }
        let stored = match (original, events.as_slice()) {
            (
                CoreCommand::CreateTask { spec },
                [EventEnvelope {
                    event: crate::CoreEvent::TaskCreated { .. },
                    ..
                }],
            ) => self.store.apply_events_and_spec(&events, Some(&spec)),
            (
                CoreCommand::OfferResource { mut offer },
                [EventEnvelope {
                    sequence,
                    event: crate::CoreEvent::HandoffOffered { offer: admitted },
                }],
            ) => {
                offer.handoff_id.clone_from(&admitted.handoff_id);
                self.store.apply_events(&[EventEnvelope {
                    sequence: *sequence,
                    event: crate::CoreEvent::HandoffOffered { offer },
                }])
            }
            (
                CoreCommand::RequestMediaPush { request },
                [EventEnvelope {
                    sequence,
                    event: crate::CoreEvent::MediaPushRequested { .. },
                }],
            ) => self.store.apply_events(&[EventEnvelope {
                sequence: *sequence,
                event: crate::CoreEvent::MediaPushRequested { request },
            }]),
            _ => self.store.apply_events_and_spec(&events, spec),
        };
        if let Err(error) = stored {
            self.runtime = before;
            return Err(error);
        }
        self.update_batch_disrupted_against(&before, &events);
        self.committed(&events);
        Ok(events)
    }

    pub fn assign_queue_and_set_settings(
        &mut self,
        task_ids: Vec<String>,
        queue_id: String,
        values: &BTreeMap<String, serde_json::Value>,
    ) -> Result<Vec<EventEnvelope>, String> {
        if queue_id.trim().is_empty() {
            return Err("队列编号不能为空".into());
        }
        let changed = crate::CoreEvent::SettingsChanged {
            keys: values.keys().cloned().collect(),
        };
        if task_ids.is_empty() {
            let envelope = EventEnvelope {
                sequence: self.runtime.latest_sequence() + 1,
                event: changed,
            };
            self.store
                .apply_events_and_settings(std::slice::from_ref(&envelope), values)?;
            let events = self.runtime.emit(envelope.event);
            self.committed(&events);
            return Ok(events);
        }
        let before = self.runtime.clone();
        let mut events = self
            .runtime
            .handle(CoreCommand::AssignQueue { task_ids, queue_id });
        events.extend(self.runtime.emit(changed));
        if let Err(error) = self.store.apply_events_and_settings(&events, values) {
            self.runtime = before;
            return Err(error);
        }
        self.update_batch_disrupted_against(&before, &events);
        self.committed(&events);
        Ok(events)
    }

    pub fn emit(&mut self, event: crate::CoreEvent) -> Result<Vec<EventEnvelope>, String> {
        let clean = redact_urls(event.clone())?;
        let envelope = EventEnvelope {
            sequence: self.runtime.latest_sequence() + 1,
            event: if matches!(event, crate::CoreEvent::Error { .. }) {
                clean.clone()
            } else {
                event
            },
        };
        // 单事件发布只改变事件环和序号；先提交数据库，无需复制整个运行态来回滚。
        self.store.apply_events(std::slice::from_ref(&envelope))?;
        let events = self.runtime.emit(clean);
        self.committed(&events);
        Ok(events)
    }

    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.runtime.list_tasks()
    }

    pub(crate) fn scheduler_task_flags(&self) -> (bool, bool) {
        self.runtime.scheduler_task_flags()
    }

    pub(crate) fn transfer_summary(&self) -> (usize, u64) {
        self.runtime.transfer_summary()
    }

    pub(crate) fn batch_disrupted_for_queue(&self, queue_id: &str) -> Option<&BTreeSet<String>> {
        self.batch_disrupted.get(queue_id)
    }

    fn is_live_task_status(status: &str) -> bool {
        matches!(
            status,
            "queued"
                | "downloading"
                | "recording"
                | "merging"
                | "checking"
                | "parsing"
                | "processing"
        )
    }

    fn clear_task_from_disrupted(&mut self, task_id: &str) {
        self.batch_disrupted.retain(|_, tasks| {
            tasks.remove(task_id);
            !tasks.is_empty()
        });
    }

    fn update_batch_disrupted_for_events(
        &mut self,
        events: &[EventEnvelope],
        before_snapshot: Option<&TaskSnapshot>,
        queue_was_idle: bool,
    ) {
        for envelope in events {
            match &envelope.event {
                crate::CoreEvent::TaskProgress { snapshot }
                | crate::CoreEvent::TaskUpdated { snapshot } => {
                    if matches!(
                        snapshot.status.as_str(),
                        "failed" | "canceled" | "cancelled"
                    ) {
                        self.batch_disrupted
                            .entry(snapshot.queue_id.clone())
                            .or_default()
                            .insert(snapshot.task_id.clone());
                    } else if Self::is_live_task_status(&snapshot.status) {
                        let was_live = before_snapshot
                            .is_some_and(|prev| Self::is_live_task_status(&prev.status));
                        self.clear_task_from_disrupted(&snapshot.task_id);
                        if queue_was_idle && !was_live {
                            self.batch_disrupted.remove(&snapshot.queue_id);
                        }
                    }
                }
                crate::CoreEvent::TaskDeleted { task_id } => {
                    self.clear_task_from_disrupted(task_id);
                }
                _ => {}
            }
        }
    }

    fn update_batch_disrupted_against(&mut self, before: &CoreRuntime, events: &[EventEnvelope]) {
        for envelope in events {
            match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => {
                    if Self::is_live_task_status(&snapshot.status)
                        && !before.queue_has_live_work(&snapshot.queue_id)
                    {
                        self.batch_disrupted.remove(&snapshot.queue_id);
                    }
                }
                crate::CoreEvent::TaskUpdated { snapshot }
                | crate::CoreEvent::TaskProgress { snapshot } => {
                    let prev = before.snapshot(&snapshot.task_id);
                    if prev.is_some_and(|p| p.queue_id != snapshot.queue_id) {
                        self.clear_task_from_disrupted(&snapshot.task_id);
                    }
                    if matches!(
                        snapshot.status.as_str(),
                        "failed" | "canceled" | "cancelled"
                    ) {
                        self.batch_disrupted
                            .entry(snapshot.queue_id.clone())
                            .or_default()
                            .insert(snapshot.task_id.clone());
                    } else if Self::is_live_task_status(&snapshot.status) {
                        let was_live = prev.is_some_and(|p| Self::is_live_task_status(&p.status));
                        self.clear_task_from_disrupted(&snapshot.task_id);
                        if !was_live && !before.queue_has_live_work(&snapshot.queue_id) {
                            self.batch_disrupted.remove(&snapshot.queue_id);
                        }
                    }
                }
                crate::CoreEvent::TaskDeleted { task_id } => {
                    self.clear_task_from_disrupted(task_id);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn task_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<hls_protocol::TaskPage, String> {
        self.runtime.task_page(after, limit)
    }

    pub(crate) fn event_batch_after(
        &self,
        sequence: u64,
    ) -> Result<hls_protocol::EventBatch, String> {
        self.runtime.event_batch_after(sequence)
    }

    pub(crate) fn snapshot(&self, task_id: &str) -> Option<&TaskSnapshot> {
        self.runtime.snapshot(task_id)
    }

    pub fn mark_output_missing(
        &mut self,
        task_id: &str,
        missing: bool,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.mark_output_missing(task_id, missing);
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.update_batch_disrupted_against(&before, &events);
        self.committed(&events);
        Ok(events)
    }

    pub fn report_failure(
        &mut self,
        task_id: &str,
        failure: TaskFailure,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.report_failure(task_id, redact_urls(failure)?);
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.update_batch_disrupted_against(&before, &events);
        self.committed(&events);
        Ok(events)
    }

    pub fn set_task_filename(
        &mut self,
        task_id: &str,
        filename: String,
    ) -> Result<Vec<EventEnvelope>, String> {
        crate::download_worker::validate_filename(&filename)?;
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_task_filename(task_id, filename);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_output_path(
        &mut self,
        task_id: &str,
        path: String,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_output_path(task_id, path);
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.update_batch_disrupted_against(&before, &events);
        self.committed(&events);
        Ok(events)
    }

    pub fn set_checksum_result(
        &mut self,
        task_id: &str,
        algorithm: String,
        actual: String,
        verified: bool,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime
            .set_checksum_result(task_id, algorithm, actual, verified);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_av_scan_result(
        &mut self,
        task_id: &str,
        result: AvScanStatus,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_av_scan_result(task_id, result);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_mirror_result(
        &mut self,
        task_id: &str,
        statuses: Vec<MirrorStatus>,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_mirror_result(task_id, statuses);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_torrent_telemetry(
        &mut self,
        task_id: &str,
        peer_count: u32,
        seed_count: u32,
        uploaded_bytes: u64,
        upload_speed_bytes_per_sec: u64,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_torrent_telemetry(
            task_id,
            peer_count,
            seed_count,
            uploaded_bytes,
            upload_speed_bytes_per_sec,
        );
        self.persist_runtime_events(before, sequence)
    }

    pub fn task_spec(&self, task_id: &str) -> Option<&TaskSpec> {
        self.runtime.task_spec(task_id)
    }

    pub fn replace_spec(&mut self, task_id: &str, spec: TaskSpec) -> Result<(), String> {
        crate::download_worker::validate_filename(&spec.filename)?;
        if self.runtime.snapshot(task_id).is_none() {
            return Err("任务不存在".into());
        }
        let clean = redact_urls(spec.clone())?;
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.replace_spec(task_id, clean);
        let events = self.runtime.events_after(sequence, 16);
        if let Err(error) = self.store.apply_events_and_spec(&events, Some(&spec)) {
            self.runtime = before;
            return Err(error);
        }
        self.committed(&events);
        Ok(())
    }

    pub(crate) fn apply_armed_refresh(
        &mut self,
        task_id: &str,
        spec: TaskSpec,
        handoff_id: String,
        handoff: Option<crate::contract::BrowserHandoff>,
    ) -> Result<Vec<EventEnvelope>, String> {
        crate::download_worker::validate_filename(&spec.filename)?;
        if self.runtime.snapshot(task_id).is_none() {
            return Err(format!("unknown task {task_id}"));
        }
        let clean = redact_urls(spec.clone())?;
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        let resolved_handoff_id = if handoff_id.trim().is_empty() {
            self.runtime.allocate_handoff_id()
        } else {
            handoff_id
        };
        self.runtime.replace_spec_clearing_error(task_id, clean);
        let _ = self.runtime.emit(crate::CoreEvent::HandoffResolved {
            handoff_id: resolved_handoff_id,
            task_id: Some(task_id.to_string()),
        });
        let _ = self.runtime.emit(crate::CoreEvent::Toast {
            level: "info".into(),
            message: "下载地址与凭据已刷新，正在断点续传".into(),
        });
        let events = self.runtime.events_after(sequence, 16);
        if let Err(error) =
            self.store
                .apply_events_spec_and_handoff(&events, Some(&spec), handoff.as_ref())
        {
            self.runtime = before;
            return Err(error);
        }
        self.committed(&events);
        Ok(events)
    }

    pub fn pending_handoff(&self, handoff_id: &str) -> Option<crate::ResourceOffer> {
        self.runtime.pending_handoff(handoff_id).cloned()
    }

    pub fn take_pending_handoff(&mut self, handoff_id: &str) -> Option<crate::ResourceOffer> {
        self.runtime.take_pending_handoff(handoff_id)
    }

    pub fn latest_sequence(&self) -> u64 {
        self.runtime.latest_sequence()
    }

    pub fn browser_connected(&self) -> bool {
        self.runtime.browser_connected()
    }

    pub fn events_after(&self, sequence: u64, limit: usize) -> Vec<EventEnvelope> {
        self.runtime.events_after(sequence, limit)
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    pub(crate) fn event_notify(&self) -> Arc<(Mutex<u64>, Condvar)> {
        Arc::clone(&self.event_notify)
    }

    /// Whether an UpdateProgress event may skip SQLite: the task is still in
    /// the same active status/stage that was last written, and that write is
    /// younger than [`PROGRESS_PERSIST_INTERVAL`].
    fn progress_is_volatile(&self, event: &crate::CoreEvent) -> bool {
        let crate::CoreEvent::TaskProgress { snapshot } = event else {
            return false;
        };
        if !matches!(
            snapshot.status.as_str(),
            "downloading" | "processing" | "merging" | "recording" | "checking"
        ) {
            return false;
        }
        self.persisted_progress
            .get(&snapshot.task_id)
            .is_some_and(|persisted| {
                persisted.status == snapshot.status
                    && persisted.stage == snapshot.stage
                    && persisted.at.elapsed() < PROGRESS_PERSIST_INTERVAL
            })
    }

    /// Whether a non-volatile UpdateProgress is only the periodic refresh of
    /// an unchanged active state (status and stage equal to the last write,
    /// interval elapsed) rather than a state change.
    fn progress_is_periodic(&self, event: &crate::CoreEvent) -> bool {
        let crate::CoreEvent::TaskProgress { snapshot } = event else {
            return false;
        };
        matches!(
            snapshot.status.as_str(),
            "downloading" | "processing" | "merging" | "recording" | "checking"
        ) && self
            .persisted_progress
            .get(&snapshot.task_id)
            .is_some_and(|persisted| {
                persisted.status == snapshot.status && persisted.stage == snapshot.stage
            })
    }

    /// Waits until the progress writer committed every queued row. Used by an
    /// orderly shutdown and by tests; the hot path never waits.
    pub fn wait_progress_writer_idle(&self) {
        if let Some(writer) = self.progress_writer.as_ref() {
            writer.wait_idle();
        }
    }

    /// Bookkeeping after events reached SQLite, then subscriber notification.
    fn committed(&mut self, events: &[EventEnvelope]) {
        let now = Instant::now();
        for envelope in events {
            match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot }
                | crate::CoreEvent::TaskUpdated { snapshot }
                | crate::CoreEvent::TaskProgress { snapshot } => {
                    self.persisted_progress.insert(
                        snapshot.task_id.clone(),
                        PersistedProgress {
                            status: snapshot.status.clone(),
                            stage: snapshot.stage.clone(),
                            at: now,
                        },
                    );
                    self.volatile_tasks.remove(&snapshot.task_id);
                    // 同步写入的状态更新；写线程中排队的旧行已无意义。
                    if let Some(writer) = self.progress_writer.as_ref() {
                        writer.discard(&snapshot.task_id);
                    }
                }
                crate::CoreEvent::TaskDeleted { task_id } => {
                    self.persisted_progress.remove(task_id);
                    self.volatile_tasks.remove(task_id);
                    if let Some(writer) = self.progress_writer.as_ref() {
                        writer.discard(task_id);
                    }
                }
                _ => {}
            }
        }
        if let Some(last) = events.last() {
            // The store checkpoint is strictly monotonic; never regress reserved_sequence.
            self.reserved_sequence = self.reserved_sequence.max(last.sequence);
        }
        self.notify_committed(events);
    }

    /// Writes every in-memory progress snapshot that is newer than its SQLite
    /// row. Called on drop and available for an orderly shutdown.
    pub fn flush_volatile_progress(&mut self) -> Result<(), String> {
        if self.volatile_tasks.is_empty() {
            return Ok(());
        }
        let snapshots: Vec<TaskSnapshot> = self
            .volatile_tasks
            .iter()
            .filter_map(|task_id| self.runtime.snapshot(task_id).cloned())
            .collect();
        // Reserved sequences protect restart, but are not revisions of task rows.
        // A future row revision would reject subsequent periodic writer updates.
        let sequence = self.runtime.latest_sequence();
        self.store.persist_task_snapshots(&snapshots, sequence)?;
        let now = Instant::now();
        for snapshot in &snapshots {
            self.persisted_progress.insert(
                snapshot.task_id.clone(),
                PersistedProgress {
                    status: snapshot.status.clone(),
                    stage: snapshot.stage.clone(),
                    at: now,
                },
            );
        }
        self.volatile_tasks.clear();
        self.reserved_sequence = self.reserved_sequence.max(sequence);
        Ok(())
    }

    fn notify_committed(&self, events: &[EventEnvelope]) {
        let Some(last) = events.last() else {
            return;
        };
        if let Ok(mut sequence) = self.event_notify.0.lock() {
            *sequence = last.sequence;
            self.event_notify.1.notify_all();
        }
    }

    fn persist_runtime_events(
        &mut self,
        before: CoreRuntime,
        sequence: u64,
    ) -> Result<Vec<EventEnvelope>, String> {
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.update_batch_disrupted_against(&before, &events);
        self.committed(&events);
        Ok(events)
    }

    pub(crate) fn from_store(store: CoreStore) -> Result<Self, String> {
        let snapshots = store.load_tasks()?;
        let specs = store.load_task_specs()?;
        let sequence = store.latest_sequence()?;
        let mut runtime = CoreRuntime::from_state(snapshots, specs, sequence);
        for encoded in store.query_handoffs(None, None, true)? {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&encoded) {
                if let Ok(request) =
                    serde_json::from_value::<crate::MediaPushRequest>(value.clone())
                {
                    if request.status == "pending" && !request.id.trim().is_empty() {
                        runtime.restore_pending_media_push(request);
                    }
                    continue;
                }
                let status = value
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if status != "pending" {
                    continue;
                }
                if let Some(offer_value) = value.get("offer") {
                    if let Ok(offer) =
                        serde_json::from_value::<crate::ResourceOffer>(offer_value.clone())
                    {
                        runtime.restore_pending_handoff(offer);
                    }
                }
            }
        }
        let progress_writer = store
            .open_progress_writer_connection()
            .and_then(crate::progress_writer::ProgressWriter::start);
        Ok(Self {
            event_notify: Arc::new((Mutex::new(runtime.latest_sequence()), Condvar::new())),
            reserved_sequence: runtime.latest_sequence(),
            progress_writer,
            runtime,
            store,
            persisted_progress: HashMap::new(),
            volatile_tasks: BTreeSet::new(),
            batch_disrupted: BTreeMap::new(),
            presenter_bridge: PresenterBridge::new(),
        })
    }
    pub fn flush_for_shutdown(&mut self) -> Result<(), String> {
        if let Some(mut writer) = self.progress_writer.take() {
            writer.shutdown();
        }
        self.flush_volatile_progress()?;
        let latest = self.runtime.latest_sequence();
        if latest > self.reserved_sequence {
            self.store.reserve_event_sequence(latest)?;
            self.reserved_sequence = latest;
        }
        Ok(())
    }
}

impl Drop for PersistentCore {
    fn drop(&mut self) {
        if let Err(error) = self.flush_for_shutdown() {
            crate::record_runtime_log("progress_flush_failed", serde_json::json!({"error": error}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, ResourceKind, TaskSpec};

    fn browser_admission(id: &str, request_id: &str) -> String {
        serde_json::json!({
            "id": id, "request_id": request_id,
            "offer": crate::ResourceOffer {
                handoff_id: id.into(), url: "https://cdn.test/setup.exe".into(),
                filename: "setup.exe".into(), ..Default::default()
            },
            "filename": "setup.exe", "title": "", "mime_type": "", "size": 0,
            "status": "pending", "presentation": "queued", "task_id": null,
            "created_at_ms": 1
        })
        .to_string()
    }

    #[test]
    fn rejected_commands_and_failed_commits_preserve_the_account_password() {
        let mut core = PersistentCore::in_memory().unwrap();
        let accepted = "sftp://alice:accepted@host/file";
        let rejected = "sftp://alice:rejected@host/file";
        let clean = core.store().sanitize_urls(accepted.to_owned()).unwrap();
        let events = core
            .handle(CoreCommand::GetTask {
                task_id: rejected.into(),
            })
            .unwrap();
        assert!(
            matches!(&events[0].event, crate::CoreEvent::Error { message, .. } if message.contains(&clean) && !message.contains("rejected"))
        );
        let spec = TaskSpec {
            url: rejected.into(),
            resource_kind: ResourceKind::Sftp,
            filename: "file".into(),
            ..Default::default()
        };
        assert!(core.replace_spec("missing", spec.clone()).is_err());
        assert_eq!(core.store().hydrate_url(&clean).unwrap(), accepted);
        core.store().connection.execute_batch(
            "CREATE TRIGGER reject_spec BEFORE INSERT ON task_specs BEGIN SELECT RAISE(ABORT, 'reject'); END;"
        ).unwrap();
        assert!(core.handle(CoreCommand::CreateTask { spec }).is_err());
        assert!(core.tasks().is_empty());
        assert_eq!(core.store().hydrate_url(&clean).unwrap(), accepted);
    }

    #[test]
    fn handoff_replay_cannot_replace_the_admitted_password() {
        let mut core = PersistentCore::in_memory().unwrap();
        let encoded = browser_admission("first", "same-request");
        let record = |password: &str| {
            encoded.replace(
                "https://cdn.test/setup.exe",
                &format!("ftp://user:{password}@host/setup.exe"),
            )
        };
        let first = core.admit_handoff(&record("first-secret")).unwrap();
        assert!(!first.contains("first-secret"));
        assert_eq!(core.admit_handoff(&record("second-secret")).unwrap(), first);
        assert_eq!(
            core.store()
                .hydrate_url("ftp://user@host/setup.exe")
                .unwrap(),
            "ftp://user:first-secret@host/setup.exe"
        );
    }

    #[test]
    fn handoff_admission_retries_return_one_offer_without_reopening_rejection() {
        let mut core = PersistentCore::in_memory().unwrap();
        let first = core
            .admit_handoff(&browser_admission("first", "same-request"))
            .unwrap();
        let sequence = core.latest_sequence();
        assert_eq!(
            core.admit_handoff(&browser_admission("second", "same-request"))
                .unwrap(),
            first
        );
        assert_eq!(core.latest_sequence(), sequence);
        assert_eq!(core.store().load_handoffs().unwrap().len(), 1);
        assert!(core
            .admit_handoff(&browser_admission("first", "different-request"))
            .is_err());
        assert_eq!(core.store().load_handoffs().unwrap(), vec![first]);
        core.handle(CoreCommand::RejectHandoff {
            handoff_id: "first".into(),
            suppress_site_kind: false,
        })
        .unwrap();
        let sequence = core.latest_sequence();
        let retry: serde_json::Value = serde_json::from_str(
            &core
                .admit_handoff(&browser_admission("third", "same-request"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(retry["id"], "first");
        assert_eq!(retry["status"], "rejected");
        assert_eq!(core.latest_sequence(), sequence);
    }

    #[test]
    fn handoff_admission_write_failure_rolls_back_row_event_and_runtime() {
        let root = std::env::temp_dir().join(format!(
            "hls-admission-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("core.db");
        let mut core = PersistentCore::open(&path).unwrap();
        let sequence = core.latest_sequence();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON event_checkpoints BEGIN SELECT RAISE(ABORT, 'injected admission failure'); END;").unwrap();
        assert!(core
            .admit_handoff(&browser_admission("first", "same-request"))
            .is_err());
        assert!(core.store().load_handoffs().unwrap().is_empty());
        assert_eq!(core.latest_sequence(), sequence);
        assert_eq!(*core.event_notify.0.lock().unwrap(), sequence);
        connection
            .execute_batch("DROP TRIGGER reject_checkpoint")
            .unwrap();
        let rejected = core
            .handle(CoreCommand::RejectHandoff {
                handoff_id: "first".into(),
                suppress_site_kind: false,
            })
            .unwrap();
        assert!(rejected.iter().any(|event| matches!(&event.event, crate::CoreEvent::Error { code, .. } if code == "handoff_not_found")));
        core.admit_handoff(&browser_admission("second", "same-request"))
            .unwrap();
        core.handle(CoreCommand::RejectHandoff {
            handoff_id: "second".into(),
            suppress_site_kind: false,
        })
        .unwrap();
        assert_eq!(core.store().load_handoffs().unwrap().len(), 1);
        drop(connection);
        drop(core);
        // Pending admission restores from the same transaction after restart.
        let mut core = PersistentCore::open(&path).unwrap();
        core.admit_handoff(&browser_admission("pending", "restart-request"))
            .unwrap();
        drop(core);
        let mut core = PersistentCore::open(&path).unwrap();
        let recovered: serde_json::Value = serde_json::from_str(
            &core
                .admit_handoff(&browser_admission("duplicate", "restart-request"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(recovered["id"], "pending");
        core.handle(CoreCommand::RejectHandoff {
            handoff_id: "pending".into(),
            suppress_site_kind: false,
        })
        .unwrap();
        let row = core
            .store()
            .query_handoffs(Some("pending"), None, false)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&row).unwrap()["status"],
            "rejected"
        );
        drop(core);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn handoff_admission_concurrent_hosts_share_one_committed_offer() {
        let coordinator = crate::CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let workers: Vec<_> = ["host-a", "host-b"]
            .into_iter()
            .map(|id| {
                let coordinator = coordinator.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let encoded = browser_admission(id, "same-request");
                    barrier.wait();
                    coordinator.admit_handoff(&encoded).unwrap()
                })
            })
            .collect();
        barrier.wait();
        let receipts: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(receipts[0], receipts[1]);
        let core = coordinator.lock().unwrap();
        assert_eq!(core.store().load_handoffs().unwrap().len(), 1);
        assert_eq!(
            core.events_after(0, 100)
                .iter()
                .filter(|event| matches!(event.event, crate::CoreEvent::HandoffOffered { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn failed_commit_restores_snapshot_spec_sequence_and_notification() {
        let root = std::env::temp_dir().join(format!(
            "hls-core-rollback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("core.db");
        let mut core = PersistentCore::open(&path).unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        // 填满历史环；失败的进度不能挤掉最早事件或推进订阅游标。
        for index in 0..4096 {
            core.runtime.emit(crate::CoreEvent::Toast {
                level: "info".into(),
                message: index.to_string(),
            });
        }
        core.emit(crate::CoreEvent::TaskExport {
            format: "json".into(),
            data: "{}".into(),
            task_count: 0,
        })
        .unwrap();
        let tasks = core.tasks();
        let spec = core.task_spec("task-1").unwrap().clone();
        let sequence = core.latest_sequence();
        let events = core.events_after(0, 4096);
        let progress = CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 1024,
            total_bytes: Some(4096),
            speed_bytes_per_sec: 512,
            stage: "transfer".into(),
            status: "downloading".into(),
        };
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_task_write BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();

        assert!(core.handle(progress.clone()).is_err());
        assert!(core
            .set_output_path("task-1", "changed.bin".into())
            .is_err());
        assert!(core
            .set_task_filename("task-1", "changed.bin".into())
            .is_err());
        let mut changed = spec.clone();
        changed.url = "https://example.test/changed.bin".into();
        assert!(core.replace_spec("task-1", changed).is_err());
        assert_eq!(core.tasks(), tasks);
        assert_eq!(core.store().load_tasks().unwrap(), tasks);
        assert_eq!(core.task_spec("task-1"), Some(&spec));
        assert_eq!(core.store().load_task_specs().unwrap()[0].1, spec);
        assert_eq!(core.latest_sequence(), sequence);
        assert_eq!(core.events_after(0, 4096), events);
        assert_eq!(*core.event_notify.0.lock().unwrap(), sequence);

        connection
            .execute_batch("DROP TRIGGER reject_task_write")
            .unwrap();
        // 在快照和日志已写入事务后再失败，二者仍必须与事件检查点一起回滚。
        connection.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON event_checkpoints BEGIN SELECT RAISE(ABORT, 'injected checkpoint failure'); END;").unwrap();
        assert!(core.handle(progress.clone()).is_err());
        assert!(core
            .emit(crate::CoreEvent::Error {
                code: "inject".into(),
                message: "checkpoint blocked".into(),
            })
            .is_err());
        assert!(core
            .emit(crate::CoreEvent::MediaPushRequested {
                request: crate::MediaPushRequest {
                    id: "must-not-persist".into(),
                    push_kind: "tvbox".into(),
                    url: "https://example.test/video.mp4".into(),
                    title: "Video".into(),
                    status: "pending".into(),
                    message: String::new(),
                    location: String::new(),
                    credential_ref: None,
                    mime_type: String::new(),
                    created_at_ms: 1,
                },
            })
            .is_err());
        assert!(core.store().load_handoffs().unwrap().is_empty());
        assert_eq!(core.tasks(), tasks);
        assert_eq!(core.store().load_tasks().unwrap(), tasks);
        assert!(core
            .store()
            .load_task_log("task-1", 500)
            .unwrap()
            .is_empty());
        assert_eq!(core.store().latest_sequence().unwrap(), sequence);
        assert_eq!(core.latest_sequence(), sequence);
        assert_eq!(core.events_after(0, 4096), events);
        assert_eq!(*core.event_notify.0.lock().unwrap(), sequence);
        connection
            .execute_batch("DROP TRIGGER reject_checkpoint")
            .unwrap();
        let committed = core.handle(progress).unwrap();
        assert_eq!(committed[0].sequence, sequence + 1);
        assert_eq!(core.events_after(sequence, 1), committed);
        assert_eq!(core.snapshot("task-1").unwrap().downloaded_bytes, 1024);
        assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
        core.set_output_path("task-1", "committed.bin".into())
            .unwrap();
        assert_eq!(*core.event_notify.0.lock().unwrap(), core.latest_sequence());
        assert_eq!(core.events_after(sequence, 1)[0].sequence, sequence + 1);
        drop(connection);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_progress_preserves_runtime_lifecycle_and_other_tasks() {
        let mut core = PersistentCore::in_memory().unwrap();
        let mut runtime = CoreRuntime::new();
        let progress =
            |task_id: &str, bytes, status: &str, stage: &str| CoreCommand::UpdateProgress {
                task_id: task_id.into(),
                downloaded_bytes: bytes,
                total_bytes: Some(4096),
                speed_bytes_per_sec: 512,
                stage: stage.into(),
                status: status.into(),
            };
        for command in [
            CoreCommand::CreateTask { spec: test_spec() },
            CoreCommand::CreateTask { spec: test_spec() },
            progress("task-1", 512, "downloading", "transfer"),
            CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "pause".into(),
            },
            progress("task-1", 768, "downloading", "transfer"),
            CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "resume".into(),
            },
            progress("task-1", 2048, "downloading", "transfer"),
            progress("task-1", 4096, "completed", "completed"),
            progress("task-1", 3072, "downloading", "transfer"),
            progress("missing-task", 512, "downloading", "transfer"),
        ] {
            let expected = runtime.handle(command.clone());
            let actual = core.handle(command).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(core.tasks(), runtime.list_tasks());
            // P0-1: steady active progress may stay in memory for up to
            // PROGRESS_PERSIST_INTERVAL; the stored row then still has the same
            // status and stage, and a flush makes the two identical.
            let stored = core.store().load_tasks().unwrap();
            if core.volatile_tasks.is_empty() {
                assert_eq!(stored, core.tasks());
            } else {
                for (row, live) in stored.iter().zip(core.tasks()) {
                    assert_eq!(
                        (&row.task_id, &row.status, &row.stage),
                        (&live.task_id, &live.status, &live.stage)
                    );
                }
                core.flush_volatile_progress().unwrap();
                assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
            }
            assert_eq!(core.latest_sequence(), runtime.latest_sequence());
            assert_eq!(*core.event_notify.0.lock().unwrap(), core.latest_sequence());
        }
    }

    #[test]
    fn worker_commit_wakes_subscriber_without_an_ipc_command() {
        let core = Arc::new(Mutex::new(PersistentCore::in_memory().unwrap()));
        let notify = core.lock().unwrap().event_notify();
        let guard = notify.0.lock().unwrap();
        let worker_core = Arc::clone(&core);
        let worker = std::thread::spawn(move || {
            worker_core
                .lock()
                .unwrap()
                .emit(crate::CoreEvent::Toast {
                    level: "info".into(),
                    message: "worker completed".into(),
                })
                .unwrap();
        });
        let (sequence, timeout) = notify
            .1
            .wait_timeout_while(guard, std::time::Duration::from_secs(5), |sequence| {
                *sequence == 0
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(*sequence, 1);
        drop(sequence);
        worker.join().unwrap();
        assert_eq!(core.lock().unwrap().events_after(0, 256).len(), 1);
    }

    #[test]
    fn service_persists_commands_through_the_shared_boundary() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://example.test/file.bin".into(),
                resource_kind: ResourceKind::File,
                title: "File".into(),
                filename: "file.bin".into(),
                download_dir: String::new(),
                request_method: "GET".into(),
                credential_ref: None,
                replay_context_ref: None,
                concurrency: 8,
                checksum: None,
                expected_size: None,
                etag: String::new(),
                last_modified: String::new(),
                ..Default::default()
            },
        })
        .unwrap();
        assert_eq!(core.tasks().len(), 1);
        assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
        assert_eq!(
            core.store().load_task_specs().unwrap()[0].1.url,
            "https://example.test/file.bin"
        );
    }

    #[test]
    fn spec_updates_refresh_snapshot_and_task_log() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        let mut spec = core.task_spec("task-1").unwrap().clone();
        spec.speed_limit_kib = 256;
        core.replace_spec("task-1", spec).unwrap();
        assert_eq!(core.tasks()[0].speed_limit_kib, 256);
        assert_eq!(core.store().load_tasks().unwrap()[0].speed_limit_kib, 256);

        core.handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 1024,
            total_bytes: Some(4096),
            speed_bytes_per_sec: 512,
            stage: "transfer".into(),
            status: "downloading".into(),
        })
        .unwrap();
        let events = core
            .handle(CoreCommand::GetTaskLog {
                task_id: "task-1".into(),
            })
            .unwrap();
        assert!(matches!(
            &events[0].event,
            crate::CoreEvent::TaskLog { lines, .. }
                if lines.last().is_some_and(|line| line.contains("1024/4096"))
        ));
    }

    #[test]
    fn file_store_restores_tasks_after_core_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v6-core-restart-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::CreateTask { spec: test_spec() })
                .unwrap();
            core.handle(CoreCommand::AssignQueue {
                task_ids: vec!["task-1".into()],
                queue_id: "night-media".into(),
            })
            .unwrap();
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(reopened.tasks().len(), 1);
        assert_eq!(reopened.tasks()[0].task_id, "task-1");
        assert_eq!(reopened.tasks()[0].queue_id, "night-media");
        assert_eq!(
            reopened.runtime.task_spec("task-1").unwrap().url,
            "https://example.test/restart.bin"
        );
        assert_eq!(
            reopened.runtime.task_spec("task-1").unwrap().queue_id,
            "night-media"
        );
        assert_eq!(reopened.store().latest_sequence().unwrap(), 2);
        let events = reopened.handle(CoreCommand::Ping).unwrap();
        assert_eq!(events[0].sequence, 3);
        // Windows 上无法删除仍被 SQLite 持有的文件：先释放 owner 再清理，
        // 否则 `remove_file` 静默失败并在 %TEMP% 堆积测试数据库。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn file_store_new_task_after_restart_never_reuses_a_restored_task_id() {
        // 重启后 next_task 必须从已恢复的 task-N 最大值继续（core_runtime.rs:54 的 max 折叠）。
        // 这条不变式没有任何测试钉住：一旦 counter 归零，新任务会拿到 task-1，
        // 与恢复出来的 task-1 撞号——同 id 两条不同任务，静默污染 store。
        let path = std::env::temp_dir().join(format!(
            "hls-v7-core-restart-idcollision-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::CreateTask { spec: test_spec() })
                .unwrap();
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(reopened.tasks().len(), 1);
        assert_eq!(reopened.tasks()[0].task_id, "task-1");
        // 重启后再建一条：必须拿到 task-2，绝不能回到 task-1。
        reopened
            .handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        let ids: Vec<String> = reopened
            .tasks()
            .iter()
            .map(|task| task.task_id.clone())
            .collect();
        assert_eq!(ids, vec!["task-1".to_string(), "task-2".to_string()]);
        // 再建一条，确认 counter 不是只跳过一次。
        reopened
            .handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        assert_eq!(reopened.tasks().len(), 3);
        assert_eq!(reopened.tasks()[2].task_id, "task-3");
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn queue_profile_removal_migrates_tasks_and_settings_together() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(CoreCommand::AssignQueue {
            task_ids: vec!["task-1".into()],
            queue_id: "removed".into(),
        })
        .unwrap();
        let profiles = serde_json::json!([{
            "id": "default",
            "name": "默认队列"
        }]);
        let values = BTreeMap::from([("queue_profiles".into(), profiles.clone())]);

        core.assign_queue_and_set_settings(
            vec!["task-1".into()],
            crate::DEFAULT_QUEUE_ID.into(),
            &values,
        )
        .unwrap();

        assert_eq!(core.tasks()[0].queue_id, crate::DEFAULT_QUEUE_ID);
        assert_eq!(
            core.store().setting_string("queue_profiles", "").unwrap(),
            profiles.to_string()
        );
    }

    #[test]
    fn concurrent_media_push_is_failed_without_replacing_active_request() {
        let mut core = PersistentCore::in_memory().unwrap();
        let first = crate::MediaPushRequest {
            id: "push-first".into(),
            push_kind: "cast".into(),
            url: "https://example.test/first.mp4".into(),
            title: "First".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            credential_ref: None,
            mime_type: String::new(),
            created_at_ms: 1,
        };
        let second = crate::MediaPushRequest {
            id: "push-second".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/second.mp4".into(),
            title: "Second".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            credential_ref: None,
            mime_type: String::new(),
            created_at_ms: 2,
        };

        let admitted = core
            .handle(CoreCommand::RequestMediaPush {
                request: first.clone(),
            })
            .unwrap();
        assert!(admitted.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushRequested { request } if request.id == first.id
        )));

        let duplicate = core
            .handle(CoreCommand::RequestMediaPush {
                request: first.clone(),
            })
            .unwrap();
        assert!(duplicate.is_empty());

        let rejected = core
            .handle(CoreCommand::RequestMediaPush {
                request: second.clone(),
            })
            .unwrap();
        assert!(rejected.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushResolved { request }
                if request.id == second.id
                    && request.status == "failed"
                    && request.message.contains("已有投送请求")
        )));
        assert!(!rejected.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushRequested { request } if request.id == second.id
        )));

        let rows: Vec<crate::MediaPushRequest> = core
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|row| serde_json::from_str(&row).ok())
            .collect();
        assert!(rows
            .iter()
            .any(|request| request.id == first.id && request.status == "pending"));
        assert!(rows
            .iter()
            .any(|request| request.id == second.id && request.status == "failed"));

        core.handle(CoreCommand::ResolveMediaPush {
            request_id: first.id.clone(),
            status: "done".into(),
            message: "ok".into(),
            location: String::new(),
        })
        .unwrap();
        let third = crate::MediaPushRequest {
            id: "push-third".into(),
            created_at_ms: 3,
            ..second
        };
        let next = core
            .handle(CoreCommand::RequestMediaPush { request: third })
            .unwrap();
        assert!(next
            .iter()
            .any(|event| matches!(event.event, crate::CoreEvent::MediaPushRequested { .. })));
    }

    #[test]
    fn media_push_terminal_resolution_rehydrates_missing_runtime_request() {
        let mut core = PersistentCore::in_memory().unwrap();
        let request = crate::MediaPushRequest {
            id: "push-runtime-rehydrate".into(),
            push_kind: "cast".into(),
            url: "https://example.test/runtime-rehydrate.mp4".into(),
            title: "Runtime rehydrate".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            credential_ref: None,
            mime_type: String::new(),
            created_at_ms: 8,
        };
        core.handle(CoreCommand::RequestMediaPush {
            request: request.clone(),
        })
        .unwrap();
        core.runtime = CoreRuntime::new();

        let events = core
            .handle(CoreCommand::ResolveMediaPush {
                request_id: request.id.clone(),
                status: "done".into(),
                message: "sent".into(),
                location: "http://192.168.1.9/media/runtime-rehydrate".into(),
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushResolved { request }
                if request.id == "push-runtime-rehydrate" && request.status == "done"
        )));
        let persisted = core
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == request.id)
            .unwrap();
        assert_eq!(persisted.status, "done");
        assert_eq!(persisted.message, "sent");
        assert_eq!(
            persisted.location,
            "http://192.168.1.9/media/runtime-rehydrate"
        );
    }

    #[test]
    fn media_push_terminal_resolution_retry_is_idempotent_after_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-media-push-terminal-retry-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let request = crate::MediaPushRequest {
            id: "push-terminal-retry".into(),
            push_kind: "cast".into(),
            url: "https://example.test/retry.mp4".into(),
            title: "Retry media".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            credential_ref: None,
            mime_type: String::new(),
            created_at_ms: 7,
        };
        let terminal = CoreCommand::ResolveMediaPush {
            request_id: request.id.clone(),
            status: "done".into(),
            message: "sent".into(),
            location: "http://192.168.1.8/media/retry".into(),
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::RequestMediaPush {
                request: request.clone(),
            })
            .unwrap();
            let first = core.handle(terminal.clone()).unwrap();
            assert!(first.iter().any(|event| matches!(
                &event.event,
                crate::CoreEvent::MediaPushResolved { request }
                    if request.id == "push-terminal-retry" && request.status == "done"
            )));
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert!(reopened.handle(terminal).unwrap().is_empty());
        let conflicting_retry = CoreCommand::ResolveMediaPush {
            request_id: request.id.clone(),
            status: "failed".into(),
            message: "late failure".into(),
            location: String::new(),
        };
        assert!(reopened.handle(conflicting_retry).unwrap().is_empty());
        let persisted = reopened
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == request.id)
            .unwrap();
        assert_eq!(persisted.status, "done");
        assert_eq!(persisted.message, "sent");
        assert_eq!(persisted.location, "http://192.168.1.8/media/retry");
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn file_store_restores_pending_media_push_after_core_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-media-push-restart-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let request = crate::MediaPushRequest {
            id: "push-restart".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/media.mp4".into(),
            title: "Restart media".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            credential_ref: None,
            mime_type: String::new(),
            created_at_ms: 1,
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::RequestMediaPush {
                request: request.clone(),
            })
            .unwrap();
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        let events = reopened
            .handle(CoreCommand::ResolveMediaPush {
                request_id: request.id.clone(),
                status: "done".into(),
                message: "ok".into(),
                location: "http://127.0.0.1:9978".into(),
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.event,
            crate::CoreEvent::MediaPushResolved { ref request } if request.id == "push-restart" && request.status == "done"
        )));
        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn side_rows_and_checkpoint_commit_and_restore_together() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-side-row-transaction-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let handoff_id = "handoff-transaction";
        let push = crate::MediaPushRequest {
            id: "push-transaction".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/media.mp4".into(),
            title: "Media".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            credential_ref: None,
            mime_type: String::new(),
            created_at_ms: 2,
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.store_mut()
                .save_handoff(
                    handoff_id,
                    &serde_json::json!({
                        "id": handoff_id,
                        "status": "pending",
                        "created_at_ms": 1
                    })
                    .to_string(),
                    "pending",
                    None,
                    1,
                )
                .unwrap();
            core.emit(crate::CoreEvent::HandoffResolved {
                handoff_id: handoff_id.into(),
                task_id: None,
            })
            .unwrap();
            core.emit(crate::CoreEvent::MediaPushRequested {
                request: push.clone(),
            })
            .unwrap();
            assert_eq!(core.store().latest_sequence().unwrap(), 2);
        }

        let reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(reopened.latest_sequence(), 2);
        assert!(reopened.pending_handoff(handoff_id).is_none());
        let rows: Vec<serde_json::Value> = reopened
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .map(|row| serde_json::from_str(&row).unwrap())
            .collect();
        assert!(rows.iter().any(|row| {
            row.get("id").and_then(serde_json::Value::as_str) == Some(handoff_id)
                && row.get("status").and_then(serde_json::Value::as_str) == Some("rejected")
        }));
        assert!(rows.iter().any(|row| {
            row.get("id").and_then(serde_json::Value::as_str) == Some(push.id.as_str())
                && row.get("status").and_then(serde_json::Value::as_str) == Some("pending")
        }));
        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn verification_and_mirror_results_survive_core_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-verification-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut core = PersistentCore::open(&path).unwrap();
            let mut spec = test_spec();
            spec.checksum = Some("sha256:abc".into());
            spec.mirrors = vec!["https://mirror.example.test/restart.bin".into()];
            core.handle(CoreCommand::CreateTask { spec }).unwrap();
            core.set_checksum_result("task-1", "SHA-256".into(), "def".into(), false)
                .unwrap();
            core.set_av_scan_result(
                "task-1",
                AvScanStatus {
                    state: "clean".into(),
                    engine: "Windows Defender".into(),
                    detail: "scan completed".into(),
                },
            )
            .unwrap();
            core.set_mirror_result(
                "task-1",
                vec![MirrorStatus {
                    url: "https://mirror.example.test/restart.bin".into(),
                    final_url: "https://cdn.example.test/restart.bin".into(),
                    state: "active".into(),
                    detail: "下载已使用此地址".into(),
                    ranges: true,
                }],
            )
            .unwrap();
            core.set_torrent_telemetry("task-1", 18, 4, 0, 0).unwrap();
        }
        let reopened = PersistentCore::open(&path).unwrap();
        let task = &reopened.tasks()[0];
        assert_eq!(task.checksum_algorithm, "SHA-256");
        assert_eq!(task.checksum_actual, "def");
        assert_eq!(task.checksum_verified, Some(false));
        assert_eq!(task.av_scan.as_ref().unwrap().state, "clean");
        assert_eq!(task.mirror_status[0].state, "active");
        assert!(task.mirror_status[0].ranges);
        assert_eq!((task.peer_count, task.seed_count), (18, 4));
        assert_eq!(
            (task.uploaded_bytes, task.upload_speed_bytes_per_sec),
            (0, 0)
        );
        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    fn temp_db(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hls-core-{name}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn remove_db(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    fn active_progress(bytes: u64) -> CoreCommand {
        CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: bytes,
            total_bytes: Some(1 << 20),
            speed_bytes_per_sec: 1024,
            stage: "transfer".into(),
            status: "downloading".into(),
        }
    }

    fn stored_bytes(core: &PersistentCore) -> u64 {
        core.store().load_tasks().unwrap()[0].downloaded_bytes
    }

    #[test]
    fn steady_progress_is_published_in_memory_and_persisted_on_state_change() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        // The first active update changes status (queued -> downloading) and is written.
        core.handle(active_progress(100)).unwrap();
        assert_eq!(stored_bytes(&core), 100);
        let before = core.latest_sequence();
        let events = core.handle(active_progress(200)).unwrap();
        // Same event name and sequence contract for subscribers ...
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, before + 1);
        assert!(matches!(
            &events[0].event,
            crate::CoreEvent::TaskProgress { snapshot } if snapshot.downloaded_bytes == 200
        ));
        assert_eq!(core.events_after(before, 16), events);
        assert_eq!(*core.event_notify.0.lock().unwrap(), before + 1);
        assert_eq!(core.snapshot("task-1").unwrap().downloaded_bytes, 200);
        // ... but SQLite keeps the last written row, with the checkpoint reserved ahead.
        assert_eq!(stored_bytes(&core), 100);
        assert!(core.store().latest_sequence().unwrap() > before);
        for bytes in 300..320 {
            core.handle(active_progress(bytes)).unwrap();
        }
        assert_eq!(stored_bytes(&core), 100);
        // A state change writes the newest in-memory bytes at once.
        core.handle(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "pause".into(),
        })
        .unwrap();
        assert_eq!(stored_bytes(&core), 319);
        assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
        assert!(core.volatile_tasks.is_empty());
    }

    #[test]
    fn steady_progress_reaches_sqlite_after_the_persist_interval() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(active_progress(100)).unwrap();
        core.handle(active_progress(200)).unwrap();
        assert_eq!(stored_bytes(&core), 100);
        let aged = Instant::now()
            .checked_sub(PROGRESS_PERSIST_INTERVAL + Duration::from_millis(10))
            .unwrap();
        core.persisted_progress.get_mut("task-1").unwrap().at = aged;
        core.handle(active_progress(300)).unwrap();
        assert_eq!(stored_bytes(&core), 300);
        // The event checkpoint is monotonic so persisting progress never
        // regresses a previously reserved volatile sequence window.
        assert!(core.store().latest_sequence().unwrap() >= core.latest_sequence());
        // A stage change inside the interval is written too.
        let merging = CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 400,
            total_bytes: Some(1 << 20),
            speed_bytes_per_sec: 0,
            stage: "merging".into(),
            status: "merging".into(),
        };
        core.handle(merging).unwrap();
        assert_eq!(stored_bytes(&core), 400);
    }

    #[test]
    fn crash_after_volatile_progress_never_reuses_a_published_sequence() {
        let path = temp_db("volatile-crash");
        let published = {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::CreateTask { spec: test_spec() })
                .unwrap();
            core.handle(active_progress(100)).unwrap();
            let mut last = 0;
            for bytes in 200..210 {
                last = core.handle(active_progress(bytes)).unwrap()[0].sequence;
            }
            // Suppress only the orderly snapshot flush. Close the writer and DB
            // normally so this crash-state fixture leaks neither handles nor threads.
            core.volatile_tasks.clear();
            drop(core);
            last
        };
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert!(reopened.latest_sequence() >= published);
        assert_eq!(stored_bytes(&reopened), 100);
        let next = reopened.handle(CoreCommand::Ping).unwrap();
        assert!(next[0].sequence > published);
        drop(reopened);
        remove_db(&path);
    }

    #[test]
    fn orderly_drop_flushes_volatile_progress() {
        let path = temp_db("volatile-flush");
        let published = {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::CreateTask { spec: test_spec() })
                .unwrap();
            core.handle(active_progress(100)).unwrap();
            core.handle(active_progress(250)).unwrap()[0].sequence
        };
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(stored_bytes(&reopened), 250);
        assert_eq!(reopened.tasks()[0].downloaded_bytes, 250);
        let next = reopened.handle(CoreCommand::Ping).unwrap();
        assert!(next[0].sequence > published);
        drop(reopened);
        remove_db(&path);
    }

    fn age_persisted(core: &mut PersistentCore) {
        let aged = Instant::now()
            .checked_sub(PROGRESS_PERSIST_INTERVAL + Duration::from_millis(10))
            .unwrap();
        core.persisted_progress.get_mut("task-1").unwrap().at = aged;
    }

    fn row_sequence(path: &std::path::Path) -> (u64, u64) {
        let connection = rusqlite::Connection::open(path).unwrap();
        let (json, sequence): (String, i64) = connection
            .query_row(
                "SELECT snapshot_json, event_sequence FROM tasks WHERE task_id = 'task-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let snapshot: TaskSnapshot = serde_json::from_str(&json).unwrap();
        (snapshot.downloaded_bytes, sequence as u64)
    }

    #[test]
    fn flush_does_not_block_later_periodic_progress() {
        let path = temp_db("flush-then-progress");
        let mut core = PersistentCore::open(&path).unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(active_progress(100)).unwrap();
        core.handle(active_progress(200)).unwrap();
        core.flush_volatile_progress().unwrap();
        age_persisted(&mut core);
        let sequence = core.handle(active_progress(300)).unwrap()[0].sequence;
        core.wait_progress_writer_idle();
        let row = row_sequence(&path);
        drop(core);
        remove_db(&path);
        assert_eq!(row, (300, sequence));
    }

    #[test]
    fn periodic_progress_write_runs_off_the_core_lock_on_the_writer_thread() {
        let path = temp_db("progress-writer");
        let mut core = PersistentCore::open(&path).unwrap();
        assert!(
            core.progress_writer.is_some(),
            "file-backed Core starts the writer"
        );
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(active_progress(100)).unwrap();
        core.handle(active_progress(200)).unwrap(); // volatile, reserves sequences
        age_persisted(&mut core);
        // Another connection holds the SQLite write lock: a synchronous write
        // would now wait for the busy timeout. The periodic write must not.
        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = Instant::now();
        let events = core.handle(active_progress(300)).unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "periodic progress waited on SQLite under the Core lock"
        );
        let sequence = events[0].sequence;
        assert_eq!(core.snapshot("task-1").unwrap().downloaded_bytes, 300);
        assert!(
            core.store().latest_sequence().unwrap() >= sequence,
            "sequence reserved before publish"
        );
        blocker.execute_batch("COMMIT").unwrap();
        drop(blocker);
        core.wait_progress_writer_idle();
        assert_eq!(row_sequence(&path), (300, sequence));
        assert_eq!(core.progress_writer.as_ref().unwrap().written(), 1);
        // The persist interval restarted with the queued write.
        let events = core.handle(active_progress(400)).unwrap();
        core.wait_progress_writer_idle();
        assert_eq!(row_sequence(&path).0, 300);
        assert!(events[0].sequence > sequence);
        drop(core);
        // Orderly drop still flushes the newest in-memory bytes.
        let reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(stored_bytes(&reopened), 400);
        drop(reopened);
        remove_db(&path);
    }

    #[test]
    fn queued_periodic_row_never_overwrites_newer_state_or_resurrects_a_task() {
        let path = temp_db("progress-writer-order");
        let mut core = PersistentCore::open(&path).unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(active_progress(100)).unwrap();
        let stale = core.snapshot("task-1").unwrap().clone();
        let stale_sequence = core.latest_sequence();
        core.handle(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "pause".into(),
        })
        .unwrap();
        let paused = row_sequence(&path);
        let logs_before = core.store().load_task_log("task-1", 500).unwrap();
        // A row queued before the pause but written after it is ignored.
        let mut writer = core.store().open_progress_writer_connection().unwrap();
        let mut downloading = stale.clone();
        downloading.downloaded_bytes = 150;
        downloading
            .log_tail
            .push("downloading transfer 150/1048576".into());
        let applied = crate::store::write_progress_rows(
            &mut writer,
            &[crate::store::ProgressRow {
                snapshot: downloading.clone(),
                sequence: stale_sequence,
            }],
        )
        .unwrap();
        assert_eq!(applied, 0);
        assert_eq!(row_sequence(&path), paused);
        assert_eq!(core.store().load_tasks().unwrap()[0].status, "paused");
        assert_eq!(
            core.store().load_task_log("task-1", 500).unwrap(),
            logs_before
        );
        // ... and a deleted task is not re-created by a late row.
        core.handle(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "delete".into(),
        })
        .unwrap();
        assert!(core.store().load_tasks().unwrap().is_empty());
        let applied = crate::store::write_progress_rows(
            &mut writer,
            &[crate::store::ProgressRow {
                snapshot: downloading,
                sequence: u64::from(u32::MAX),
            }],
        )
        .unwrap();
        assert_eq!(applied, 0);
        assert!(core.store().load_tasks().unwrap().is_empty());
        drop(writer);
        drop(core);
        remove_db(&path);
    }

    #[test]
    fn writer_connection_and_core_connection_write_concurrently_without_busy_errors() {
        let path = temp_db("progress-writer-busy");
        let mut core = PersistentCore::open(&path).unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(active_progress(100)).unwrap();
        let snapshot = core.snapshot("task-1").unwrap().clone();
        let base = core.latest_sequence();
        let mut writer = core.store().open_progress_writer_connection().unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut writes = 0u64;
            let mut sequence = base;
            while !thread_stop.load(std::sync::atomic::Ordering::SeqCst) {
                sequence += 1;
                let mut snapshot = snapshot.clone();
                snapshot.downloaded_bytes = sequence;
                crate::store::write_progress_rows(
                    &mut writer,
                    &[crate::store::ProgressRow { snapshot, sequence }],
                )
                .unwrap();
                writes += 1;
                // The real writer commits batches between queue waits; a
                // zero-gap loop only measures SQLite busy-handler fairness.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            writes
        });
        // Read-then-write transactions on the Core connection (settings,
        // events) must wait for the writer instead of failing.
        for index in 0..40 {
            core.store_mut()
                .set_setting("download_speed_limit_kib", index)
                .unwrap();
            core.handle(CoreCommand::Ping).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(handle.join().unwrap() > 0);
        drop(core);
        remove_db(&path);
    }

    fn test_spec() -> TaskSpec {
        TaskSpec {
            url: "https://example.test/restart.bin".into(),
            resource_kind: ResourceKind::File,
            title: "Restart".into(),
            filename: "restart.bin".into(),
            download_dir: String::new(),
            request_method: "GET".into(),
            credential_ref: None,
            replay_context_ref: None,
            concurrency: 4,
            checksum: None,
            expected_size: None,
            etag: String::new(),
            last_modified: String::new(),
            ..Default::default()
        }
    }
}
