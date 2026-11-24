//! Python-free Native Messaging front-end for the v7 Rust Core.
//!
//! Chrome and Firefox keep this process alive through a Native Messaging port.
//! One session therefore owns one durable Core connection plus its short-lived
//! handoff offers. No HTTP request or Python process is required on this path.

use crate::contract::BrowserHandoff as Handoff;
use crate::{
    CoreCommand, CoreEvent, CoreIpcClient, CredentialVault, EventEnvelope, MediaPushRequest,
    ResourceKind, ResourceOffer, TaskSnapshot, TaskSpec, V7_PROTOCOL_NAME, V7_PROTOCOL_VERSION,
};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    io::{self, Read, Write},
    thread,
    time::Duration,
};

const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
/// Chrome/Firefox 对 Host → 浏览器的单条消息限制为 1 MiB，超限会直接断开端口。
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const RECOMMENDED_EXTENSION_VERSION: &str = env!("CARGO_PKG_VERSION");
const MINIMUM_EXTENSION_VERSION: &str = "2.0.11";
const EXTENSION_RELEASE_URL: &str = "https://github.com/ciaooo55/hls-downloader/releases";
static NEXT_HANDOFF: AtomicU64 = AtomicU64::new(1);

static NEXT_WORKBENCH: AtomicU64 = AtomicU64::new(1);

/// What a workbench readiness poll observed after `since_sequence`.
#[derive(Debug, PartialEq, Eq)]
enum WorkbenchObservation {
    Ready,
    Failed,
    Waiting,
}

fn observe_workbench(
    events: &[EventEnvelope],
    since_sequence: u64,
    launch_id: &str,
) -> WorkbenchObservation {
    let mut observation = WorkbenchObservation::Waiting;
    for envelope in events
        .iter()
        .filter(|envelope| envelope.sequence > since_sequence)
    {
        match &envelope.event {
            // 一次成功即视为可用；之后的失败可能来自另一个重复窗口，不覆盖已就绪。
            CoreEvent::UiReady {
                launch_id: id,
                surface,
            } if id == launch_id && surface == "main" => return WorkbenchObservation::Ready,
            CoreEvent::UiReady {
                launch_id: id,
                surface,
            } if id == launch_id && surface == "main_failed" => {
                observation = WorkbenchObservation::Failed
            }
            _ => {}
        }
    }
    observation
}

struct NativeHostSession {
    core: HostCore,
    media_push_ui_request_id: Option<String>,
}

#[allow(clippy::large_enum_variant)]
enum HostCore {
    #[cfg(test)]
    Local(crate::PersistentCore),
    Remote(CoreIpcClient),
}

impl HostCore {
    fn open_workbench(&mut self) -> Result<(), String> {
        self.handle(CoreCommand::OpenMain { launch_id: None })
            .map_err(|error| format!("core:{error}"))?;
        // In-process protocol fixtures must not launch a cached real desktop
        // binary. Packaged integration tests exercise the remote Core path.
        #[cfg(test)]
        if matches!(self, Self::Local(_)) {
            return Ok(());
        }
        if let Some(root) = crate::install_root() {
            if !crate::spawn_desktop_ui(&root, None) {
                return Err("spawn:无法启动桌面工作台".into());
            }
        }
        Ok(())
    }

    fn request_workbench(&mut self) -> Result<Option<(String, u64)>, String> {
        let launch_id = format!(
            "{}-{}-{}",
            std::process::id(),
            unix_time_ms(),
            NEXT_WORKBENCH.fetch_add(1, Ordering::Relaxed),
        );
        let sequence = self
            .handle(CoreCommand::Ping)
            .map_err(|error| format!("core:{error}"))?
            .last()
            .map_or(0, |envelope| envelope.sequence);
        if !self.supports_command("workbench_launch_receipt") {
            return self.open_workbench().map(|()| None);
        }
        #[cfg(test)]
        if matches!(self, Self::Local(_)) {
            return Ok(Some((launch_id, sequence)));
        }
        let root = crate::install_root().ok_or("spawn:找不到桌面工作台安装目录")?;
        if !crate::spawn_desktop_ui(&root, Some(&launch_id)) {
            return Err("spawn:找不到或无法启动桌面工作台程序".into());
        }
        Ok(Some((launch_id, sequence)))
    }

    fn supports_command(&mut self, name: &str) -> bool {
        match self {
            #[cfg(test)]
            Self::Local(_) => true,
            Self::Remote(client) => matches!(
                client.request(&crate::CorePipeRequest::Capabilities { request_id: 1 }),
                Ok(crate::CorePipeResponse::Capabilities { commands, .. })
                    if commands.iter().any(|command| command == name)
            ),
        }
    }

    fn events_after(&mut self, after: u64, wait_ms: u64) -> Result<hls_client::EventBatch, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => {
                let _ = wait_ms;
                core.event_batch_after(after)
            }
            Self::Remote(client) => client.wait_event_batch(after, wait_ms),
        }
    }

    fn admit_handoff(&mut self, handoff_json: &str) -> Result<crate::CorePipeResponse, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => Ok(match core.admit_handoff(handoff_json) {
                Ok(encoded) => crate::CorePipeResponse::Handoffs {
                    request_id: 1,
                    items: vec![encoded],
                    next: None,
                },
                Err(message) => crate::CorePipeResponse::Error {
                    request_id: Some(1),
                    code: "handoff_failed".into(),
                    message,
                },
            }),
            Self::Remote(client) => client.request(&crate::CorePipeRequest::AdmitHandoff {
                request_id: 1,
                handoff_json: handoff_json.into(),
            }),
        }
    }

    fn handle(&mut self, command: CoreCommand) -> Result<Vec<crate::EventEnvelope>, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.handle(command),
            Self::Remote(client) => client.command(command),
        }
    }

    fn snapshot_tasks(&mut self) -> Result<Vec<crate::TaskSnapshot>, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => Ok(core.tasks()),
            Self::Remote(client) => client.snapshot(),
        }
    }

    fn takeover_settings(&mut self) -> Result<(bool, u64), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => Ok((
                core.store()
                    .setting_bool("browser_takeover_enabled", true)?,
                core.store()
                    .setting_u64("browser_takeover_minimum_bytes", 0)?,
            )),
            Self::Remote(client) => match client.load_settings()? {
                crate::CorePipeResponse::Settings {
                    takeover_enabled,
                    takeover_minimum_bytes,
                    ..
                } => Ok((takeover_enabled, takeover_minimum_bytes)),
                other => Err(format!("unexpected takeover settings response: {other:?}")),
            },
        }
    }

    fn set_setting_bool(&mut self, key: &str, value: bool) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store_mut().set_setting(key, value),
            Self::Remote(client) => client.store_setting(key, json!(value)),
        }
    }

    fn set_setting_u64(&mut self, key: &str, value: u64) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store_mut().set_setting(key, value),
            Self::Remote(client) => client.store_setting(key, json!(value)),
        }
    }

    fn store_credential(
        &mut self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => {
                core.store_mut()
                    .store_credential(credential_ref, protected_blob, kind)
            }
            Self::Remote(client) => client.store_credential(credential_ref, protected_blob, kind),
        }
    }

    fn delete_credential(&mut self, credential_ref: &str) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store_mut().delete_credential(credential_ref),
            Self::Remote(client) => client.delete_credential(credential_ref),
        }
    }

    #[cfg(test)]
    fn save_handoff(
        &mut self,
        handoff_id: &str,
        json: &str,
        status: &str,
        task_id: Option<&str>,
        created_at_ms: u64,
    ) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => {
                core.store_mut()
                    .save_handoff(handoff_id, json, status, task_id, created_at_ms)
            }
            Self::Remote(client) => {
                client.save_handoff(handoff_id, json, status, task_id, created_at_ms)
            }
        }
    }

    fn query_handoffs(
        &mut self,
        handoff_id: Option<&str>,
        client_request_id: Option<&str>,
        pending_only: bool,
    ) -> Result<Vec<String>, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => {
                core.store()
                    .query_handoffs(handoff_id, client_request_id, pending_only)
            }
            Self::Remote(client) => {
                client.query_handoffs(handoff_id, client_request_id, pending_only)
            }
        }
    }

    #[cfg(test)]
    fn local(&self) -> &crate::PersistentCore {
        match self {
            Self::Local(core) => core,
            Self::Remote(_) => panic!("test expected local core"),
        }
    }
}

impl NativeHostSession {
    fn open_existing() -> Result<Self, String> {
        Ok(Self::from_backend(HostCore::Remote(connect_existing_core(
            Duration::from_millis(25),
        )?)))
    }

    fn open_or_start() -> Result<Self, String> {
        Ok(Self::from_backend(HostCore::Remote(
            connect_or_start_core()?
        )))
    }

    fn open_for_policy(policy: CoreConnectPolicy) -> Result<Self, String> {
        match policy {
            CoreConnectPolicy::ExistingOnly => Self::open_existing(),
            CoreConnectPolicy::AllowAutostart => Self::open_or_start(),
        }
    }

    #[cfg(test)]
    fn in_memory() -> Result<Self, String> {
        Ok(Self::from_backend(HostCore::Local(
            crate::PersistentCore::in_memory()?,
        )))
    }

    fn from_backend(core: HostCore) -> Self {
        Self {
            core,
            media_push_ui_request_id: None,
        }
    }

    fn dispatch(&mut self, message: &Value) -> Result<Value, String> {
        let operation = message
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "Native Messaging 缺少 op".to_string())?;
        match operation {
            "subtitle" => {
                let mut request = message.get("request").cloned().unwrap_or(json!({}));
                if !request.is_object() {
                    return Err("字幕请求必须是对象".into());
                }
                if request.get("action").and_then(Value::as_str) == Some("configure") {
                    return Err("请在主程序“设置 → 字幕翻译”中修改字幕配置".into());
                }
                request["owner"] = json!(message
                    .get("client_id")
                    .and_then(Value::as_str)
                    .unwrap_or(""));
                match &mut self.core {
                    HostCore::Remote(client) => {
                        match client.request(&crate::CorePipeRequest::Subtitle {
                            request_id: 1,
                            request,
                        })? {
                            crate::CorePipeResponse::Subtitle { result, .. } => Ok(result),
                            crate::CorePipeResponse::Error { message, .. } => Err(message),
                            _ => Err("桌面端不支持在线字幕，请更新 HLS Downloader".into()),
                        }
                    }
                    #[cfg(test)]
                    HostCore::Local(_) => Err("字幕服务需要常驻 Core".into()),
                }
            }
            "ping" => self.ping(),
            "activate" => self.activate(),
            "workbench_status" => self.workbench_status(message),
            "offer" => self.offer(message, false),
            "recover_offer" => {
                if field(resource_payload(message)?, "client_request_id")
                    .trim()
                    .is_empty()
                {
                    return Err("恢复交接缺少原始请求编号".into());
                }
                self.offer(message, false)
            }
            "offer_local_file" => self.offer(message, true),
            "download" => self.download(message),
            "handoff_status" => self.handoff_status(message),
            "accept_handoff" => self.accept_handoff(message),
            "reject_handoff" => self.reject_handoff(message),
            "set_takeover_settings" => self.set_takeover_settings(message),
            "media_push" => self.media_push(message),
            "media_push_status" => self.media_push_status(message),
            _ => Err(format!("不支持的 Native Messaging 操作: {operation}")),
        }
    }

    /// Staged desktop activation. The response never claims the workbench is
    /// open: callers poll `workbench_status` until Core relays `UiReady`.
    fn activate(&mut self) -> Result<Value, String> {
        match self.core.request_workbench() {
            Ok(Some((launch_id, since_sequence))) => Ok(json!({
                "ok": true,
                "activated": true,
                "stage": "launching",
                "launch_id": launch_id,
                "since_sequence": since_sequence,
                "verifiable": true,
            })),
            // 旧版 Core 不认识 UiReady：只能说明已请求打开，不能冒充“已打开”。
            Ok(None) => Ok(json!({
                "ok": true,
                "activated": true,
                "stage": "requested",
                "verifiable": false,
            })),
            Err(error) => {
                if let Some(("core", detail)) = error.split_once(':') {
                    if is_pipe_transport_error(detail) {
                        return Err(detail.to_string());
                    }
                }
                let (code, detail) = match error.split_once(':') {
                    Some(("core", detail)) => ("core_unreachable", detail.to_string()),
                    Some(("spawn", detail)) => ("workbench_spawn_failed", detail.to_string()),
                    _ => ("workbench_spawn_failed", error.clone()),
                };
                Ok(json!({
                    "ok": false,
                    "code": code,
                    "stage": if code == "core_unreachable" { "core" } else { "workbench" },
                    "error": detail,
                }))
            }
        }
    }

    fn workbench_status(&mut self, message: &Value) -> Result<Value, String> {
        let launch_id = message
            .get("launch_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 80)
            .ok_or("缺少工作台启动编号")?;
        let since_sequence = message
            .get("since_sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "缺少工作台启动序号".to_string())?;
        // 每次最多占用宿主端口 1.5 秒，避免阻塞同一端口上的下载交接。
        let wait_ms = message
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(1_200)
            .min(1_500);
        let batch = match self.core.events_after(since_sequence, wait_ms) {
            Ok(result) => result,
            Err(error) => {
                return Ok(json!({
                    "ok": false,
                    "code": "core_unreachable",
                    "stage": "core",
                    "error": error,
                }))
            }
        };
        if batch.resync_required {
            return Ok(json!({ "ok": false, "code": "launch_status_lost" }));
        }
        let observation = observe_workbench(&batch.events, since_sequence, launch_id);
        Ok(json!({
            "ok": true,
            "ready": observation == WorkbenchObservation::Ready,
            "failed": observation == WorkbenchObservation::Failed,
            // 只前进到已检查过的事件，未出现的就绪回执仍可在下一次轮询读到。
            "since_sequence": if observation == WorkbenchObservation::Waiting {
                batch.events.last().map_or(since_sequence, |envelope| envelope.sequence)
            } else {
                since_sequence
            },
            "latest_sequence": batch.latest_sequence,
        }))
    }

    fn ping(&mut self) -> Result<Value, String> {
        self.core.handle(CoreCommand::Ping)?;
        let _ = self.core.handle(CoreCommand::BrowserHello {
            version: env!("CARGO_PKG_VERSION").into(),
            browser: "extension".into(),
        });
        let (takeover_enabled, minimum_bytes) = self.core.takeover_settings()?;
        Ok(json!({
            "ok": true,
            "protocol": V7_PROTOCOL_NAME,
            "protocol_version": V7_PROTOCOL_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "takeover_enabled": takeover_enabled,
            "takeover_minimum_bytes": minimum_bytes,
            "recommended_extension_version": RECOMMENDED_EXTENSION_VERSION,
            "minimum_extension_version": MINIMUM_EXTENSION_VERSION,
            "extension_release_url": EXTENSION_RELEASE_URL,
        }))
    }

    fn offer(&mut self, message: &Value, allow_local_file: bool) -> Result<Value, String> {
        let payload = resource_payload(message)?;
        let mut offer = parse_offer(payload, allow_local_file)?;
        let request_id = payload
            .get("client_request_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !request_id.is_empty() {
            if let Some(existing) = self
                .core
                .query_handoffs(None, Some(request_id), false)?
                .into_iter()
                .filter_map(|encoded| serde_json::from_str::<Handoff>(&encoded).ok())
                .next_back()
            {
                return Ok(json!({"ok": true, "handoff": public_handoff(&existing, None)}));
            }
        } else if matches!(&self.core, HostCore::Remote(_)) {
            // Confirm the Core pipe is still live before launching Presenter,
            // so a stale session after tray Shutdown cannot spawn an orphan UI.
            self.core.handle(CoreCommand::Ping)?;
        }
        if matches!(&self.core, HostCore::Remote(_)) {
            let root = crate::install_root().ok_or_else(|| "找不到主程序安装目录".to_string())?;
            crate::core_spawn::ensure_presenter(&root)?;
        }

        let owned_credential_ref = self.persist_browser_context(payload)?;
        offer.credential_ref = owned_credential_ref.clone();
        let id = next_handoff_id();
        offer.handoff_id = id.clone();
        offer.filename = field(payload, "filename");
        offer.title = field(payload, "title");
        let handoff = Handoff {
            id: id.clone(),
            offer: offer.clone(),
            filename: filename(payload, &offer.url),
            title: field(payload, "title"),
            mime_type: field(payload, "mime_type"),
            size: offer.size,
            status: "pending".into(),
            presentation: "queued".into(),
            task_id: None,
            created_at_ms: unix_time_ms(),
            request_id: request_id.to_string(),
            suppression: None,
        };
        let encoded = serde_json::to_string(&handoff)
            .map_err(|error| format!("encode browser handoff: {error}"))?;
        // A transport failure may follow a successful commit. Keep its vaulted
        // context until receipt recovery; deleting it would break the winner.
        let admitted = match self.core.admit_handoff(&encoded) {
            Ok(crate::CorePipeResponse::Handoffs {
                items, next: None, ..
            }) if items.len() == 1 => match serde_json::from_str::<Handoff>(&items[0]) {
                Ok(admitted) => admitted,
                Err(_) => {
                    return Ok(
                        json!({"ok": false, "transport_error": true, "error": "Core 交接接收回执无法解析"}),
                    )
                }
            },
            Ok(crate::CorePipeResponse::Error { code, message, .. })
                if code == "handoff_failed" =>
            {
                return self.rollback_browser_credential_on_error(
                    Err(message),
                    owned_credential_ref.as_deref(),
                );
            }
            Err(error) => return Ok(json!({"ok": false, "transport_error": true, "error": error})),
            _ => {
                return Ok(
                    json!({"ok": false, "transport_error": true, "error": "Core 返回了无效的交接接收回执"}),
                )
            }
        };
        if admitted.offer.credential_ref != owned_credential_ref {
            if let Some(credential_ref) = owned_credential_ref.as_deref() {
                if let Err(error) = self.core.delete_credential(credential_ref) {
                    crate::record_runtime_log(
                        "handoff_unused_context_cleanup_failed",
                        json!({"handoff_id": admitted.id, "error": error}),
                    );
                }
            }
        }
        let response = public_handoff(&admitted, None);
        Ok(json!({"ok": true, "handoff": response}))
    }

    fn download(&mut self, message: &Value) -> Result<Value, String> {
        let payload = resource_payload(message)?;
        let offer = parse_offer(payload, false)?;
        let owned_credential_ref = self.persist_browser_context(payload)?;
        let filename = filename(payload, &offer.url);
        let snapshot = self.create_download_task(
            TaskSpec {
                url: offer.url,
                resource_kind: offer.resource_kind,
                title: field(payload, "title"),
                filename,
                download_dir: String::new(),
                request_method: offer.request_method,
                credential_ref: owned_credential_ref.clone().or(offer.credential_ref),
                replay_context_ref: offer.replay_context_ref,
                concurrency: payload
                    .get("concurrency")
                    .and_then(Value::as_u64)
                    .and_then(|workers| u32::try_from(workers).ok())
                    .unwrap_or(0),
                speed_limit_kib: payload
                    .get("speed_limit_kib")
                    .and_then(Value::as_u64)
                    .and_then(|limit| u32::try_from(limit).ok())
                    .unwrap_or(0),
                checksum: None,
                expected_size: (offer.size > 0).then_some(offer.size),
                etag: field(payload, "etag"),
                last_modified: field(payload, "last_modified"),
                preferred_variant_id: offer.preferred_variant_id,
                preferred_bandwidth: payload
                    .get("preferred_bandwidth")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                preferred_height: payload
                    .get("preferred_height")
                    .and_then(Value::as_u64)
                    .and_then(|height| u32::try_from(height).ok())
                    .unwrap_or(0),
                ..Default::default()
            },
            owned_credential_ref,
        )?;
        if snapshot
            .available_actions
            .iter()
            .any(|action| action == "start")
        {
            let events = self.core.handle(CoreCommand::TaskAction {
                task_id: snapshot.task_id.clone(),
                action: "start".into(),
            })?;
            ensure_task_started(&events, &snapshot.task_id)?;
        }
        Ok(json!({"ok": true, "task": snapshot, "activated": true}))
    }

    fn create_download_task(
        &mut self,
        spec: TaskSpec,
        owned_credential_ref: Option<String>,
    ) -> Result<TaskSnapshot, String> {
        let result = self
            .core
            .handle(CoreCommand::CreateTask { spec })
            .and_then(|events| {
                let mut reused = None;
                let mut duplicate_error = None;
                for event in events {
                    match event.event {
                        CoreEvent::TaskCreated { snapshot } => return Ok((snapshot, true)),
                        CoreEvent::TaskUpdated { snapshot } if reused.is_none() => {
                            reused = Some(snapshot);
                        }
                        CoreEvent::DuplicateOffered { message, .. }
                            if duplicate_error.is_none() =>
                        {
                            duplicate_error = Some(message);
                        }
                        _ => {}
                    }
                }
                if let Some(snapshot) = reused {
                    return Ok((snapshot, false));
                }
                Err(duplicate_error.unwrap_or_else(|| "Rust Core 未返回新建任务快照".to_string()))
            });
        match result {
            Ok((snapshot, created)) => {
                if !created {
                    if let Some(reference) = owned_credential_ref {
                        self.core.delete_credential(&reference)?;
                    }
                }
                Ok(snapshot)
            }
            Err(error) => self
                .rollback_browser_credential_on_error(Err(error), owned_credential_ref.as_deref()),
        }
    }

    fn rollback_browser_credential_on_error<T>(
        &mut self,
        result: Result<T, String>,
        owned_credential_ref: Option<&str>,
    ) -> Result<T, String> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                if let Some(credential_ref) = owned_credential_ref {
                    if let Err(rollback_error) = self.core.delete_credential(credential_ref) {
                        return Err(format!(
                            "{error}; browser replay credential rollback failed: {rollback_error}"
                        ));
                    }
                }
                Err(error)
            }
        }
    }

    fn handoff_status(&mut self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("handoff_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let handoff = if id.is_empty() {
            let request_id = message
                .get("client_request_id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "缺少交接ID或客户端请求ID".to_string())?;
            let found = self
                .core
                .query_handoffs(None, Some(request_id), false)?
                .into_iter()
                .filter_map(|row| serde_json::from_str::<Handoff>(&row).ok())
                .next_back();
            match found {
                Some(handoff) => handoff,
                // Absence is an observation, not proof that an earlier timed-out
                // request cannot still commit. Callers must retain uncertainty.
                None => return Ok(json!({"ok": true, "handoff": null})),
            }
        } else {
            self.handoff(id)?
        };
        let task = match handoff.task_id.as_deref() {
            Some(task_id) => self
                .core
                .snapshot_tasks()?
                .into_iter()
                .find(|item| item.task_id == task_id),
            None => None,
        };
        Ok(json!({"ok": true, "handoff": public_handoff(&handoff, task.as_ref())}))
    }

    fn accept_handoff(&mut self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("handoff_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let filename = message
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let download_dir = message
            .get("download_dir")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.core.handle(CoreCommand::AcceptHandoff {
            handoff_id: id.clone(),
            filename,
            download_dir,
            allow_duplicate: false,
            trusted_ui: false,
            start_paused: message
                .get("start_paused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            queue_id: message
                .get("queue_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })?;
        self.handoff_status(&json!({"handoff_id": id}))
    }

    fn reject_handoff(&mut self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("handoff_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.core.handle(CoreCommand::RejectHandoff {
            handoff_id: id.clone(),
            suppress_site_kind: message
                .get("suppress_site_kind")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })?;
        self.handoff_status(&json!({"handoff_id": id}))
    }

    #[cfg(test)]
    fn persist_handoff(&mut self, handoff: &Handoff) -> Result<(), String> {
        let encoded = serde_json::to_string(handoff)
            .map_err(|error| format!("encode v6 handoff {}: {error}", handoff.id))?;
        self.core.save_handoff(
            &handoff.id,
            &encoded,
            &handoff.status,
            handoff.task_id.as_deref(),
            handoff.created_at_ms,
        )
    }

    fn set_takeover_settings(&mut self, message: &Value) -> Result<Value, String> {
        if let Some(enabled) = message.get("enabled").and_then(Value::as_bool) {
            self.core
                .set_setting_bool("browser_takeover_enabled", enabled)?;
        }
        if let Some(bytes) = message.get("minimum_bytes").and_then(Value::as_u64) {
            self.core
                .set_setting_u64("browser_takeover_minimum_bytes", bytes)?;
        }
        let (enabled, minimum_bytes) = self.core.takeover_settings()?;
        Ok(json!({
            "ok": true,
            "takeover_enabled": enabled,
            "takeover_minimum_bytes": minimum_bytes,
        }))
    }

    fn media_push(&mut self, message: &Value) -> Result<Value, String> {
        let kind = match message.get("kind").and_then(Value::as_str) {
            Some(kind @ ("tvbox" | "cast")) => kind,
            _ => return Err("浏览器投送类型必须为 tvbox 或 cast".into()),
        };
        let payload = resource_payload(message)?;
        let url = field(payload, "url");
        let title = field(payload, "title");
        let title = if title.is_empty() {
            filename(payload, &url)
        } else {
            title
        };
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://"))
            || url.chars().any(char::is_control)
        {
            return Err("浏览器投送地址无效".into());
        }
        let method = field(payload, "request_method");
        if !method.is_empty() && !method.eq_ignore_ascii_case("GET") {
            return Err("此媒体使用非 GET 请求，请先下载后投屏".into());
        }
        let owned_credential_ref = self.persist_browser_context(payload)?;
        let mime_type = match field(payload, "mime_type") {
            mime if !mime.is_empty() => mime,
            _ => match field(payload, "resource_kind").as_str() {
                "hls" | "live" => "application/vnd.apple.mpegurl".into(),
                "dash" => "application/dash+xml".into(),
                _ => String::new(),
            },
        };
        let request = MediaPushRequest {
            id: format!(
                "media-push-{:x}-{}",
                unix_time_ms(),
                NEXT_HANDOFF.fetch_add(1, Ordering::Relaxed)
            ),
            push_kind: kind.into(),
            url,
            title,
            mime_type,
            status: "pending".into(),
            message: "等待在桌面下载器中选择接收设备".into(),
            location: String::new(),
            credential_ref: owned_credential_ref.clone(),
            created_at_ms: unix_time_ms(),
        };
        let result = self.core.handle(CoreCommand::RequestMediaPush {
            request: request.clone(),
        });
        let events =
            self.rollback_browser_credential_on_error(result, owned_credential_ref.as_deref())?;
        let outcome = events
            .into_iter()
            .find_map(|event| match event.event {
                CoreEvent::MediaPushRequested { request: outcome }
                | CoreEvent::MediaPushResolved { request: outcome }
                    if outcome.id == request.id =>
                {
                    Some(outcome)
                }
                _ => None,
            })
            .unwrap_or(request);
        let pending_request_id = if outcome.status == "pending" {
            Some(outcome.id.clone())
        } else if outcome.status == "failed" {
            self.pending_media_push_id().ok().flatten()
        } else {
            None
        };
        if let Some(request_id) = pending_request_id {
            self.prompt_media_push_ui(&request_id);
        }
        Ok(json!({
            "ok": true,
            "id": outcome.id,
            "kind": outcome.push_kind,
            "status": outcome.status,
            "message": outcome.message,
            "location": outcome.location,
        }))
    }

    fn media_push_status(&mut self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(push) = self
            .core
            .query_handoffs(Some(id), None, false)?
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == id)
        else {
            return Ok(json!({
                "ok": false,
                "error": "投送请求不存在或已过期"
            }));
        };
        Ok(json!({
            "ok": true,
            "id": push.id,
            "kind": push.push_kind,
            "status": push.status,
            "message": push.message,
            "location": push.location,
        }))
    }

    fn pending_media_push_id(&mut self) -> Result<Option<String>, String> {
        Ok(self
            .core
            .query_handoffs(None, None, true)?
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
            .find(|request| request.status == "pending")
            .map(|request| request.id))
    }

    fn prompt_media_push_ui(&mut self, request_id: &str) {
        if self.media_push_ui_request_id.as_deref() == Some(request_id) {
            return;
        }
        self.media_push_ui_request_id = Some(request_id.to_string());
        let _ = self.core.open_workbench();
    }

    fn persist_browser_context(
        &mut self,
        payload: &Map<String, Value>,
    ) -> Result<Option<String>, String> {
        if !contains_browser_context(payload) {
            return Ok(None);
        }
        let mut context = Map::new();
        for key in [
            "cookie",
            "request_headers",
            "request_contexts",
            "request_body",
            "request_method",
            "referer",
            "origin",
            "user_agent",
        ] {
            if let Some(value) = payload.get(key) {
                context.insert(key.to_string(), value.clone());
            }
        }
        let serialized = serde_json::to_string(&Value::Object(context))
            .map_err(|error| format!("encode browser replay context: {error}"))?;
        let protected = CredentialVault.protect(&serialized)?;
        let credential_ref = next_credential_ref();
        self.core
            .store_credential(&credential_ref, &protected, "browser_replay")?;
        Ok(Some(credential_ref))
    }

    fn handoff(&mut self, id: &str) -> Result<Handoff, String> {
        self.core
            .query_handoffs(Some(id), None, false)?
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<Handoff>(&encoded).ok())
            .next()
            .ok_or_else(|| "接管请求不存在或已过期".into())
    }
}

fn connect_existing_core(timeout: Duration) -> Result<CoreIpcClient, String> {
    CoreIpcClient::connect_existing(timeout)
}

/// Start the packaged Core only for explicit user-initiated operations after
/// probing an already-running Core fails.
fn connect_or_start_core() -> Result<CoreIpcClient, String> {
    // 未运行时尽快启动；首次启动协调持续到 IPC 就绪，启动后的完整重试仍保留。
    match connect_existing_core(Duration::from_millis(10)) {
        Ok(client) => Ok(client),
        Err(first_error) => {
            #[cfg(windows)]
            let _launch = crate::instance::lock_core_launch()?;
            // 等待启动锁时，另一个 Host 可能已经使 Core 就绪。
            if let Ok(client) = connect_existing_core(Duration::from_millis(10)) {
                return Ok(client);
            }
            let root = crate::install_root()
                .ok_or_else(|| format!("v7 Core unavailable: {first_error}"))?;
            // Reaching here means the current request is explicitly allowed to
            // start Core (e.g. activate/offer/download/media_push, or a cold
            // ping when not tray-quit). Clear any stale user-shutdown marker
            // before spawning so the new resident Core session starts clean.
            hls_client::clear_user_shutdown();
            crate::spawn_core(&root).map_err(|error| {
                format!("v7 Core unavailable: {first_error}; start failed: {error}")
            })?;

            // On a first launch the Core may still be opening its database when
            // the browser host reconnects. The engine's single-instance lock
            // makes this retry safe when several browser processes start at once.
            let mut last_error = None;
            for _ in 0..3 {
                match CoreIpcClient::connect() {
                    Ok(client) => return Ok(client),
                    Err(error) => last_error = Some(error),
                }
                thread::sleep(Duration::from_millis(150));
            }

            Err(format!(
                "v7 Core did not become ready: {}",
                last_error.unwrap_or_else(|| "no connection attempt was made".to_string())
            ))
        }
    }
}

fn public_handoff(handoff: &Handoff, task: Option<&TaskSnapshot>) -> Value {
    let mut value = json!({
        "id": handoff.id,
        "url": handoff.offer.url,
        "filename": handoff.filename,
        "title": handoff.title,
        "mime_type": handoff.mime_type,
        "source_page_url": handoff.offer.source_page_url,
        "resource_kind": resource_kind_name(handoff.offer.resource_kind),
        "owner": handoff.offer.owner,
        "evidence": handoff.offer.evidence,
        "confidence": handoff.offer.confidence,
        "request_method": handoff.offer.request_method,
        "size": handoff.size,
        "status": handoff.status,
        "task_id": handoff.task_id,
        "created_at_ms": handoff.created_at_ms,
        "presented": handoff.presentation == "presented",
        "presentation": handoff.presentation,
        "presentation_mode": if handoff.presentation == "fallback" { "ui-fallback" } else { "native-rust" },
        "presentation_ok": handoff.presentation != "failed" && handoff.status != "failed",
        "presentation_queued": matches!(handoff.presentation.as_str(), "queued" | "presenting" | "fallback"),
        "presentable": true,
    });
    if !handoff.request_id.is_empty() {
        value["client_request_id"] = json!(handoff.request_id);
    }
    if let Some(suppression) = &handoff.suppression {
        value["suppression"] = suppression.clone();
    }
    if let Some(task) = task {
        value["task_id"] = json!(task.task_id);
        value["task_status"] = json!(task.status);
        value["task_stage"] = json!(task.stage);
        value["task_downloaded_bytes"] = json!(task.downloaded_bytes);
        value["task_total_bytes"] = json!(task.total_bytes);
        value["task_error_code"] = json!(task.error_code);
    }
    value
}

pub fn run() -> i32 {
    let started = std::time::Instant::now();
    let profile = std::env::var_os("HLS_V7_STARTUP_PROFILE").is_some();
    if profile {
        eprintln!("native_host_startup stage=entry elapsed_us=0");
    }
    // 延迟到首条消息再按策略连接 Core：
    // 1. 宿主进程刚拉起尚未收到消息时，不额外阻塞探测或拉起引擎；
    // 2. 用户点击托盘“退出下载引擎”写下 user-shutdown.marker 后，浏览器心跳 `ping` 与
    //    各类后台状态查询仅走 ExistingOnly，离线时返回 core_unreachable，绝不复活 Core；
    // 3. 首次冷启动或非用户主动退出状态下，`ping` 仍允许冷启动 Core（保持扩展与验收契约）；
    // 4. 显式操作（activate / offer / download / media_push）始终允许冷启动并清除退出标记。
    let mut session: Option<NativeHostSession> = None;
    if profile {
        eprintln!(
            "native_host_startup stage=session_ready elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let message = match read_message(&mut input) {
            Ok(Some(message)) => message,
            Ok(None) => return 0,
            Err(error) => {
                let _ = write_response(&mut output, &json!({"ok": false, "error": error}));
                return 1;
            }
        };
        let request_id = message
            .get("__request_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let response = dispatch_session_message(
            &mut session,
            &message,
            NativeHostSession::open_for_policy,
            |active, msg| active.dispatch(msg),
        );
        let response = attach_request_id(response, request_id);
        if let Err(error) = write_response(&mut output, &response) {
            eprintln!("native messaging write failed: {error}");
            return 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreConnectPolicy {
    ExistingOnly,
    AllowAutostart,
}

fn core_connect_policy(message: &Value) -> CoreConnectPolicy {
    core_connect_policy_with(message, hls_client::is_user_shutdown_active())
}

fn core_connect_policy_with(message: &Value, user_shutdown_active: bool) -> CoreConnectPolicy {
    // Explicit user-initiated launch/action operations may always cold-start
    // HLSDownloaderEngine (clearing any prior tray-quit marker on spawn).
    // `ping` may cold-start Core only when the user has NOT explicitly quit
    // the engine from the system tray (`user-shutdown.marker` absent),
    // preserving fresh-profile cold-start contracts while ensuring Tray
    // "退出下载引擎" is never undone by Chrome's 30s heartbeat or MV3 wake-up
    // pings. All other passive/polling/background ops connect only to an
    // already-running Core.
    match message.get("op").and_then(Value::as_str).unwrap_or("") {
        "activate" | "offer" | "download" | "media_push" => CoreConnectPolicy::AllowAutostart,
        "ping" if !user_shutdown_active => CoreConnectPolicy::AllowAutostart,
        _ => CoreConnectPolicy::ExistingOnly,
    }
}

fn is_pipe_transport_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    error.starts_with("Core pipe ")
        || error.starts_with("Core TCP ")
        || error.starts_with("Core IPC ")
        || error.starts_with("Core stream ")
        || lower.contains("broken pipe")
        || lower.contains("pipe has been ended")
        || lower.contains("os error 109")
        || lower.contains("os error 232")
        || lower.contains("os error 233")
        || lower.contains("unexpected eof")
        || lower.contains("failed to fill whole buffer")
}

fn is_safe_to_retry_after_transport_error(message: &Value) -> bool {
    // Only strictly read-only / idempotent operations (and `activate`, which
    // only propagates a pipe transport error from its pre-spawn `Ping` probe)
    // may auto-retry after a transport drop. Multi-RPC mutating ops (`download`,
    // `media_push`, `accept_handoff`, `reject_handoff`, etc.) can fail on a
    // follow-up write after step 1 already committed, so they must fail closed
    // and drop the dead session instead of replaying.
    matches!(
        message.get("op").and_then(Value::as_str).unwrap_or(""),
        "ping"
            | "activate"
            | "version"
            | "status"
            | "list_tasks"
            | "handoff_status"
            | "list_handoffs"
            | "get_takeover_settings"
            | "media_push_status"
            | "workbench_status"
    )
}

fn dispatch_session_message<S>(
    session: &mut Option<S>,
    message: &Value,
    open: impl FnMut(CoreConnectPolicy) -> Result<S, String>,
    dispatch: impl FnMut(&mut S, &Value) -> Result<Value, String>,
) -> Value {
    dispatch_session_message_with_policy(session, message, core_connect_policy, open, dispatch)
}

fn dispatch_session_message_with_policy<S>(
    session: &mut Option<S>,
    message: &Value,
    mut policy_for: impl FnMut(&Value) -> CoreConnectPolicy,
    mut open: impl FnMut(CoreConnectPolicy) -> Result<S, String>,
    mut dispatch: impl FnMut(&mut S, &Value) -> Result<Value, String>,
) -> Value {
    for attempt in 0..2 {
        if session.is_none() {
            // Evaluate policy right before reconnecting so a tray quit written
            // while an earlier in-flight request was hitting the closing pipe
            // still blocks autostart on retry.
            let policy = policy_for(message);
            match open(policy) {
                Ok(connected) => *session = Some(connected),
                Err(error) => {
                    eprintln!("v7 Core still unavailable: {error}");
                    return core_unreachable_response();
                }
            }
        }
        let active = session.as_mut().unwrap();
        match dispatch(active, message) {
            Ok(value) => {
                let transport_failed = value
                    .get("transport_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || value.get("code").and_then(Value::as_str) == Some("core_unreachable");
                if transport_failed {
                    *session = None;
                }
                return value;
            }
            Err(error) if is_pipe_transport_error(&error) => {
                *session = None;
                if attempt == 0 && is_safe_to_retry_after_transport_error(message) {
                    continue;
                }
                return core_unreachable_response();
            }
            Err(error) => return json!({"ok": false, "error": error}),
        }
    }
    core_unreachable_response()
}

fn core_unreachable_response() -> Value {
    json!({
        "ok": false,
        "code": "core_unreachable",
        "stage": "core",
        "error": "下载引擎未启动或无响应，请重试；仍失败时从开始菜单启动 HLS Downloader",
    })
}

fn attach_request_id(response: Value, request_id: String) -> Value {
    if request_id.is_empty() {
        return response;
    }
    let mut object = response.as_object().cloned().unwrap_or_default();
    object.insert("__request_id".into(), Value::String(request_id));
    Value::Object(object)
}

fn ensure_task_started(events: &[EventEnvelope], task_id: &str) -> Result<(), String> {
    for envelope in events {
        match &envelope.event {
            CoreEvent::Error { code, message } => {
                return Err(format!("启动下载失败 [{code}]: {message}"));
            }
            CoreEvent::TaskUpdated { snapshot } if snapshot.task_id == task_id => return Ok(()),
            _ => {}
        }
    }
    Err("Rust Core 未确认下载已启动".into())
}

fn resource_payload(message: &Value) -> Result<&Map<String, Value>, String> {
    message
        .get("resource")
        .and_then(Value::as_object)
        .ok_or_else(|| "Native Messaging 缺少 resource".to_string())
}

fn parse_offer(
    payload: &Map<String, Value>,
    allow_local_file: bool,
) -> Result<ResourceOffer, String> {
    let url = field(payload, "url");
    if url.is_empty() {
        return Err("资源 URL 为空".into());
    }
    let local_source_path = if allow_local_file {
        if !matches_local_payload_url(&url) {
            return Err("本地文件接管只接受 blob: 或 data: 资源".into());
        }
        let filename = field(payload, "filename");
        if filename.is_empty() {
            return Err("本地文件接管缺少已下载文件名".into());
        }
        let size = payload
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| "本地文件接管缺少有效文件大小".to_string())?;
        let source_path = field(payload, "local_source_path");
        let canonical = hls_client::request_validation::validate_local_source_path(
            &url,
            &source_path,
            Some(size),
        )?;
        Some(canonical.to_string_lossy().into_owned())
    } else {
        if payload.get("local_source_path").is_some() {
            return Err("本地文件路径只允许用于本地文件接管".into());
        }
        if !browser_resource_url_allowed(&url) {
            return Err("资源 URL 不受支持".into());
        }
        None
    };
    let resource_kind = if allow_local_file {
        ResourceKind::File
    } else {
        parse_resource_kind(
            payload
                .get("resource_kind")
                .and_then(Value::as_str)
                .unwrap_or("file"),
            &url,
        )
    };
    let evidence = payload
        .get("evidence")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.chars().take(64).collect())
                .take(16)
                .collect()
        })
        .unwrap_or_default();
    let confidence = payload
        .get("confidence")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0) as f32;
    Ok(ResourceOffer {
        url,
        resource_kind,
        local_source_path,
        owner: field(payload, "owner").chars().take(160).collect(),
        evidence,
        confidence,
        source_page_url: field(payload, "source_page_url"),
        credential_ref: None,
        replay_context_ref: None,
        request_method: hls_client::request_validation::sanitize_http_method(&field(
            payload,
            "request_method",
        )),
        handoff_id: String::new(),
        filename: field(payload, "filename"),
        title: field(payload, "title"),
        mime_type: field(payload, "mime_type"),
        preferred_variant_id: payload
            .get("preferred_variant_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.chars().any(char::is_control) && id.chars().nth(2048).is_none())
            .unwrap_or_default()
            .to_owned(),
        preferred_bandwidth: payload
            .get("preferred_bandwidth")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        preferred_height: payload
            .get("preferred_height")
            .and_then(Value::as_u64)
            .and_then(|height| u32::try_from(height).ok())
            .unwrap_or(0),
        // 浏览器捕获的是清单响应长度，不能用它校验下载并合流后的媒体文件。
        size: if matches!(
            resource_kind,
            ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live
        ) {
            0
        } else {
            payload.get("size").and_then(Value::as_u64).unwrap_or(0)
        },
    })
}

fn contains_browser_context(payload: &Map<String, Value>) -> bool {
    if payload
        .get("cookie")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("request_body")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("referer")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("origin")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        || payload
            .get("user_agent")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    let secret_key = |key: &str| {
        let key = key.to_ascii_lowercase();
        key.contains("cookie")
            || key.contains("authorization")
            || key.contains("token")
            || key.contains("password")
            || key.contains("secret")
    };
    for key in ["request_headers", "request_contexts", "replay_context"] {
        if let Some(value) = payload.get(key) {
            if let Some(object) = value.as_object() {
                if (key == "request_headers" && !object.is_empty())
                    || (key == "request_contexts"
                        && object.values().any(|context| {
                            context
                                .as_object()
                                .is_some_and(|context| !context.is_empty())
                        }))
                {
                    return true;
                }
                if object.keys().any(|name| secret_key(name)) {
                    return true;
                }
                if key == "request_contexts"
                    && object.values().any(|context| {
                        context
                            .as_object()
                            .is_some_and(|context| context.keys().any(|name| secret_key(name)))
                    })
                {
                    return true;
                }
            }
        }
    }
    false
}

fn parse_resource_kind(value: &str, url: &str) -> ResourceKind {
    match value.to_ascii_lowercase().as_str() {
        "hls" => ResourceKind::Hls,
        "dash" => ResourceKind::Dash,
        "live" => ResourceKind::Live,
        "ftp" | "ftps" => ResourceKind::Ftp,
        "sftp" => ResourceKind::Sftp,
        "magnet" | "torrent" => ResourceKind::Torrent,
        _ if url.to_ascii_lowercase().starts_with("magnet:") => ResourceKind::Torrent,
        _ => ResourceKind::File,
    }
}

fn resource_kind_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::File => "file",
        ResourceKind::Hls => "hls",
        ResourceKind::Dash => "dash",
        ResourceKind::Live => "live",
        ResourceKind::Ftp => "ftp",
        ResourceKind::Sftp => "sftp",
        ResourceKind::Torrent => "torrent",
    }
}

fn filename(payload: &Map<String, Value>, url: &str) -> String {
    let explicit = field(payload, "filename");
    if !explicit.is_empty() {
        return explicit;
    }
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("download")
        .to_string()
}

fn field(payload: &Map<String, Value>, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn browser_resource_url_allowed(url: &str) -> bool {
    hls_client::request_validation::remote_resource_url_allowed(url)
}

fn matches_local_payload_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("blob:") || lower.starts_with("data:")) && !url.chars().any(char::is_control)
}

fn next_handoff_id() -> String {
    let sequence = NEXT_HANDOFF.fetch_add(1, Ordering::Relaxed);
    format!(
        "handoff-{:x}-{:x}-{sequence:x}",
        std::process::id(),
        unix_time_ms()
    )
}

fn next_credential_ref() -> String {
    let sequence = NEXT_HANDOFF.fetch_add(1, Ordering::Relaxed);
    format!(
        "cred-{:x}-{:x}-{sequence:x}",
        std::process::id(),
        unix_time_ms()
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn read_message(input: &mut impl Read) -> Result<Option<Value>, String> {
    let mut header = [0u8; 4];
    let read = input.read(&mut header).map_err(|error| error.to_string())?;
    if read == 0 {
        return Ok(None);
    }
    if read != header.len() {
        input
            .read_exact(&mut header[read..])
            .map_err(|error| error.to_string())?;
    }
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_MESSAGE_BYTES {
        return Err("Native Messaging 消息过大".into());
    }
    let mut payload = vec![0u8; length];
    input
        .read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&payload).map_err(|error| format!("Native Messaging JSON 无效: {error}"))
}

fn write_response(output: &mut impl Write, response: &Value) -> Result<(), String> {
    let mut payload = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if payload.len() > MAX_RESPONSE_BYTES {
        // 超限响应会让浏览器断开端口；改为回一条可关联的错误，保持会话存活。
        let mut fallback = json!({"ok": false, "error": "Native Messaging 响应过大"});
        if let Some(request_id) = response.get("__request_id") {
            fallback["__request_id"] = request_id.clone();
        }
        payload = serde_json::to_vec(&fallback).map_err(|error| error.to_string())?;
        if payload.len() > MAX_RESPONSE_BYTES {
            return Err("Native Messaging 响应过大".into());
        }
    }
    output
        .write_all(&(payload.len() as u32).to_le_bytes())
        .and_then(|_| output.write_all(&payload))
        .and_then(|_| output.flush())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_unknown_operations_before_dispatch() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let error = session
            .dispatch(&json!({"op": "delete_everything"}))
            .unwrap_err();
        assert!(error.contains("不支持"));
    }

    #[test]
    fn media_push_rejects_invalid_kind_without_creating_a_request() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let before = session.core.local().store().load_handoffs().unwrap();
        for mut message in [json!({}), json!({"kind": "unsupported"})] {
            message["op"] = json!("media_push");
            message["resource"] = json!({"url": "https://cdn.test/video.mp4"});
            assert!(session.dispatch(&message).unwrap_err().contains("投送类型"));
        }
        assert_eq!(
            session.core.local().store().load_handoffs().unwrap(),
            before
        );
        assert!(session.media_push_ui_request_id.is_none());
    }

    #[test]
    fn browser_cannot_write_subtitle_configuration() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let error = session
            .dispatch(&json!({"op": "subtitle", "request": {
                "action": "configure", "provider": "qwencloud", "clear_key": true
            }}))
            .unwrap_err();
        assert!(error.contains("主程序"));
    }

    #[test]
    fn native_message_frame_roundtrip_is_little_endian() {
        let mut output = Vec::new();
        write_response(&mut output, &json!({"ok": true})).unwrap();
        let mut input = Cursor::new(output);
        let decoded = read_message(&mut input).unwrap().unwrap();
        assert_eq!(decoded["ok"], true);
    }

    #[test]
    fn native_host_connection_and_handoff_io_ignore_unrelated_history() {
        let server = crate::CoreServer::in_memory().unwrap();
        let mut row = Handoff {
            id: String::new(),
            offer: crate::ResourceOffer {
                url: "https://cdn.test/history.bin".into(),
                ..Default::default()
            },
            filename: "history.bin".into(),
            title: "x".repeat(384 * 1024),
            mime_type: String::new(),
            size: 1,
            status: "accepted".into(),
            presentation: "presented".into(),
            task_id: None,
            created_at_ms: 1,
            request_id: String::new(),
            suppression: None,
        };
        for index in 0..12 {
            row.id = format!("history-{index}");
            row.request_id = format!("browser-{index}");
            server
                .coordinator()
                .save_handoff(
                    &row.id,
                    &serde_json::to_string(&row).unwrap(),
                    &row.status,
                    None,
                    row.created_at_ms,
                )
                .unwrap();
        }
        let history = server.coordinator().load_handoffs().unwrap();
        assert!(
            crate::core_ipc::encode_message(&crate::CorePipeResponse::Handoffs {
                request_id: 1,
                items: history,
                next: None,
            })
            .is_err(),
            "The fixture must exceed the real IPC frame limit"
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let client = CoreIpcClient::connect_addr(addr).unwrap();
        let mut session = NativeHostSession::from_backend(HostCore::Remote(client));
        assert_eq!(session.dispatch(&json!({"op":"ping"})).unwrap()["ok"], true);
        assert_eq!(
            session.handoff("history-7").unwrap().request_id,
            "browser-7"
        );
        assert!(session
            .core
            .query_handoffs(None, None, true)
            .unwrap()
            .is_empty());
        let matched = session
            .core
            .query_handoffs(None, Some("browser-7"), false)
            .unwrap();
        assert_eq!(matched.len(), 1);
        assert_eq!(
            serde_json::from_str::<Handoff>(&matched[0]).unwrap().id,
            "history-7"
        );
        row.id = "new-pending".into();
        row.title.clear();
        row.request_id = "browser-new".into();
        row.status = "pending".into();
        session.persist_handoff(&row).unwrap();
        assert_eq!(session.handoff("new-pending").unwrap().status, "pending");
        assert_eq!(
            session.core.query_handoffs(None, None, true).unwrap().len(),
            1
        );
        for index in 0..12 {
            row.id = format!("history-{index}");
            row.title = "x".repeat(384 * 1024);
            session.persist_handoff(&row).unwrap();
        }
        // Pending requests can also exceed one frame; the IPC client must read
        // every page instead of dropping requests or disconnecting the pipe.
        assert_eq!(
            session.core.query_handoffs(None, None, true).unwrap().len(),
            13
        );
        server.shutdown();
    }

    #[test]
    fn download_credential_rollback_is_transactional() {
        let server = crate::CoreServer::in_memory().unwrap();
        server
            .coordinator()
            .set_setting("legal_terms_accepted", json!(false))
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let client = CoreIpcClient::connect_addr(addr).unwrap();
        let mut session = NativeHostSession::from_backend(HostCore::Remote(client));

        session
            .core
            .store_credential("owned-failure", "protected", "browser_replay")
            .unwrap();
        let error = session
            .create_download_task(
                TaskSpec {
                    url: "https://cdn.test/fail.bin".into(),
                    filename: "fail.bin".into(),
                    credential_ref: Some("owned-failure".into()),
                    ..Default::default()
                },
                Some("owned-failure".into()),
            )
            .unwrap_err();
        assert!(error.contains("legal terms"), "{error}");
        assert_eq!(
            server
                .coordinator()
                .load_credential("owned-failure")
                .unwrap(),
            None
        );

        server
            .coordinator()
            .set_setting("legal_terms_accepted", json!(true))
            .unwrap();
        session
            .core
            .store_credential("owned-success", "protected", "browser_replay")
            .unwrap();
        let snapshot = session
            .create_download_task(
                TaskSpec {
                    url: "https://cdn.test/success.bin".into(),
                    filename: "success.bin".into(),
                    credential_ref: Some("owned-success".into()),
                    ..Default::default()
                },
                Some("owned-success".into()),
            )
            .unwrap();
        assert_eq!(snapshot.filename, "success.bin");
        assert_eq!(
            server
                .coordinator()
                .load_credential("owned-success")
                .unwrap()
                .as_deref(),
            Some("protected")
        );
        server.shutdown();
    }

    #[test]
    fn offer_handoff_persistence_failure_rolls_back_browser_credential() {
        let mut session = NativeHostSession::in_memory().unwrap();
        session
            .core
            .store_credential("offer-owned-failure", "protected", "browser_replay")
            .unwrap();
        let error = session
            .rollback_browser_credential_on_error::<()>(
                Err("handoff persistence failed".into()),
                Some("offer-owned-failure"),
            )
            .unwrap_err();
        assert_eq!(error, "handoff persistence failed");
        assert_eq!(
            session
                .core
                .local()
                .store()
                .load_credential("offer-owned-failure")
                .unwrap(),
            None
        );
    }

    #[test]
    fn browser_download_creates_a_durable_rust_task_without_http() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let response = session
            .dispatch(&json!({
                "op": "download",
                "resource": {
                    "url": "https://cdn.test/movie.m3u8",
                    "filename": "movie.mp4",
                    "title": "Movie",
                    "resource_kind": "hls",
                    "preferred_bandwidth": 2_500_000,
                    "preferred_height": 720,
                    "evidence": ["manifest_mime"],
                    "confidence": 0.99
                }
            }))
            .unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["task"]["resource_kind"], "hls");
        assert_eq!(session.core.local().tasks().len(), 1);
        assert_eq!(session.core.local().store().load_tasks().unwrap().len(), 1);
        let specs = session.core.local().store().load_task_specs().unwrap();
        assert_eq!(specs[0].1.url, "https://cdn.test/movie.m3u8");
        assert_eq!(specs[0].1.preferred_bandwidth, 2_500_000);
        assert_eq!(specs[0].1.preferred_height, 720);
    }

    #[test]
    fn invalid_or_oversized_variant_hints_fall_back_to_automatic_selection() {
        for (hint, expected) in [
            (json!("a\u{0}b"), String::new()),
            (json!("界".repeat(2049)), String::new()),
            (json!("界".repeat(2048)), "界".repeat(2048)),
            (json!(42), String::new()),
        ] {
            let resource = json!({"url":"https://cdn.test/manifest", "preferred_variant_id": hint});
            let offer = parse_offer(resource.as_object().unwrap(), false).unwrap();
            assert_eq!(offer.preferred_variant_id, expected);
        }
    }

    #[test]
    fn browser_download_limits_are_unset_unless_explicitly_supplied() {
        for (limits, expected_workers, expected_speed) in [
            (json!({}), 0, 0),
            (json!({"concurrency": 3, "speed_limit_kib": 64}), 3, 64),
            (json!({"concurrency": 3}), 3, 0),
            (json!({"speed_limit_kib": 64}), 0, 64),
            (json!({"concurrency": 0, "speed_limit_kib": 0}), 0, 0),
            (json!({"concurrency": null, "speed_limit_kib": "64"}), 0, 0),
            (
                json!({"concurrency": -1, "speed_limit_kib": 4_294_967_296_u64}),
                0,
                0,
            ),
        ] {
            let mut session = NativeHostSession::in_memory().unwrap();
            let mut resource = limits;
            resource["url"] = json!("https://cdn.test/file.bin");
            session
                .dispatch(&json!({"op": "download", "resource": resource}))
                .unwrap();
            let specs = session.core.local().store().load_task_specs().unwrap();
            assert_eq!(specs[0].1.concurrency, expected_workers);
            assert_eq!(specs[0].1.speed_limit_kib, expected_speed);
        }
    }

    #[test]
    fn manifest_response_length_is_not_the_downloaded_media_length() {
        for (kind, expected_size) in [
            ("hls", None),
            ("dash", None),
            ("live", None),
            ("file", Some(336)),
        ] {
            for operation in ["download", "offer"] {
                let mut session = NativeHostSession::in_memory().unwrap();
                let response = session.dispatch(&json!({
                    "op": operation,
                    "resource": {"url": "https://cdn.test/media", "filename": "media.mp4", "resource_kind": kind, "size": 336, "preferred_bandwidth": 2_500_000, "preferred_height": 720, "preferred_variant_id": "  v2  "}
                })).unwrap();
                if operation == "offer" {
                    assert!(session.core.local().tasks().is_empty());
                    session.dispatch(&json!({
                        "op": "accept_handoff", "handoff_id": response["handoff"]["id"], "filename": "media.mp4"
                    })).unwrap();
                }
                let specs = session.core.local().store().load_task_specs().unwrap();
                assert_eq!(specs.len(), 1);
                assert_eq!(specs[0].1.preferred_bandwidth, 2_500_000);
                assert_eq!(specs[0].1.preferred_height, 720);
                assert_eq!(specs[0].1.preferred_variant_id, "v2");
                assert_eq!(
                    specs[0].1.expected_size, expected_size,
                    "{kind} {operation}"
                );
            }
        }
    }

    #[test]
    fn browser_download_reports_core_start_errors_instead_of_false_success() {
        let error = ensure_task_started(
            &[EventEnvelope {
                sequence: 1,
                event: CoreEvent::Error {
                    code: "illegal_task_action".into(),
                    message: "任务当前状态不允许操作: start".into(),
                },
            }],
            "task-1",
        )
        .unwrap_err();
        assert!(error.contains("illegal_task_action"), "{error}");
        assert!(error.contains("不允许操作"), "{error}");

        let snapshot = TaskSnapshot {
            task_id: "task-1".into(),
            ..Default::default()
        };
        assert!(ensure_task_started(
            &[EventEnvelope {
                sequence: 2,
                event: CoreEvent::TaskUpdated { snapshot },
            }],
            "task-1",
        )
        .is_ok());
    }

    #[test]
    fn offer_is_idempotent_for_the_extension_request_id() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let request = json!({
            "op": "offer",
            "resource": {
                "url": "https://cdn.test/setup.exe",
                "filename": "setup.exe",
                "resource_kind": "file",
                "client_request_id": "resource:1:abc"
            }
        });
        let first = session.dispatch(&request).unwrap();
        let persisted_after_first = session.core.query_handoffs(None, None, false).unwrap();
        let retry = json!({
            "op": "offer",
            "resource": {
                "url": "https://cdn.test/setup.exe",
                "filename": "setup.exe",
                "resource_kind": "file",
                "client_request_id": "resource:1:abc",
                "cookie": "session=retry-must-not-be-persisted"
            }
        });
        let second = session.dispatch(&retry).unwrap();
        assert_eq!(first["handoff"]["id"], second["handoff"]["id"]);
        let recovered = session
            .dispatch(&json!({
                "op": "handoff_status", "client_request_id": "resource:1:abc"
            }))
            .unwrap();
        assert_eq!(first["handoff"]["id"], recovered["handoff"]["id"]);
        assert_eq!(recovered["handoff"]["status"], "pending");
        let absent = session
            .dispatch(&json!({
                "op": "handoff_status", "client_request_id": "not-admitted"
            }))
            .unwrap();
        assert_eq!(absent["ok"], true);
        assert!(absent["handoff"].is_null());
        assert_eq!(persisted_after_first.len(), 1);
        assert_eq!(
            session.core.query_handoffs(None, None, false).unwrap(),
            persisted_after_first
        );
    }

    #[test]
    fn credential_refs_advance_without_handoff_progress() {
        let first = next_credential_ref();
        let second = next_credential_ref();
        assert_ne!(first, second);
    }

    #[test]
    fn handoff_admission_recovery_uses_original_id_and_requires_it() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let offer = json!({"op": "recover_offer", "resource": {
            "url": "https://cdn.test/setup.exe", "filename": "setup.exe", "client_request_id": "journal-before-send"
        }});
        let first = session.dispatch(&offer).unwrap();
        let mut delayed = offer.clone();
        delayed["op"] = json!("offer");
        let second = session.dispatch(&delayed).unwrap();
        assert_eq!(first["handoff"]["id"], second["handoff"]["id"]);
        assert_eq!(
            session
                .core
                .query_handoffs(None, None, false)
                .unwrap()
                .len(),
            1
        );
        assert!(session.core.local().tasks().is_empty());
        assert!(session
            .dispatch(
                &json!({"op": "recover_offer", "resource": {"url": "https://cdn.test/setup.exe"}})
            )
            .is_err());
    }

    #[test]
    fn browser_credentials_are_vaulted_or_rejected_without_echoing_them() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let result = session.dispatch(&json!({
            "op": "offer",
            "resource": {
                "url": "https://cdn.test/file.bin",
                "filename": "file.bin",
                "cookie": "session=secret",
                "request_headers": {"authorization": "Bearer secret"}
            }
        }));
        let encoded = match result {
            Ok(response) => response.to_string().to_ascii_lowercase(),
            Err(error) => error.to_ascii_lowercase(),
        };
        assert!(!encoded.contains("session=secret"));
        assert!(!encoded.contains("bearer secret"));
        assert!(!encoded.contains("authorization"));
        assert!(!encoded.contains("cookie"));
        #[cfg(not(windows))]
        assert!(encoded.contains("dpapi"));
    }

    #[test]
    fn ordinary_browser_headers_are_replay_context() {
        let payload = json!({
            "request_headers": { "X-Playback-Session": "session-1" }
        });
        assert!(contains_browser_context(payload.as_object().unwrap()));
    }

    #[test]
    fn v7_ping_does_not_advertise_fastapi_loopback() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let response = session.dispatch(&json!({"op": "ping"})).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["protocol"], "hls-downloader-v7-core");
        assert!(response.get("bridge_base").is_none());
        assert!(response.get("bridge_token").is_none());
        let encoded = response.to_string();
        assert!(!encoded.contains("bridge_base"));
        assert!(!encoded.contains("bridge_token"));
        assert!(!encoded.contains("8765"));
    }

    #[test]
    fn offer_coerces_non_http_methods() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let response = session
            .dispatch(&json!({
                "op": "offer",
                "resource": {
                    "url": "https://cdn.test/a.bin",
                    "filename": "a.bin",
                    "request_method": "CONNECT\r\nHost: evil"
                }
            }))
            .unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["handoff"]["request_method"], "GET");
        let post = session
            .dispatch(&json!({
                "op": "offer",
                "resource": {
                    "url": "https://cdn.test/post.bin",
                    "filename": "post.bin",
                    "request_method": "post"
                }
            }))
            .unwrap();
        assert_eq!(post["handoff"]["request_method"], "POST");
    }

    #[test]
    fn media_push_returns_a_request_id() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let response = session
            .dispatch(&json!({
                "op": "media_push",
                "kind": "tvbox",
                "resource": {
                    "url": "https://cdn.test/live.m3u8",
                    "title": "Demo"
                }
            }))
            .unwrap();
        assert_eq!(response["ok"], true);
        assert!(!response["id"].as_str().unwrap_or("").is_empty());
        let status = session
            .dispatch(&json!({
                "op": "media_push_status",
                "request_id": response["id"]
            }))
            .unwrap();
        assert_eq!(status["ok"], true);
        assert_eq!(status["status"], "pending");
    }

    #[cfg(not(windows))]
    #[test]
    fn media_push_with_secrets_fails_closed_without_dpapi_and_rejects_post() {
        // 非 Windows 平台没有 DPAPI：带凭据的投送必须拒绝，不能把 Cookie/请求头以明文落库。
        let mut session = NativeHostSession::in_memory().unwrap();
        let error = session
            .dispatch(&json!({
                "op":"media_push", "kind":"cast", "resource":{
                    "url":"https://cdn.test/media", "request_method":"GET", "resource_kind":"dash",
                    "cookie":"sid=private", "request_headers":{"Authorization":"Bearer private"},
                    "request_contexts":{"https://cdn.test":{"cookie":"sid=scoped"}}
                }
            }))
            .unwrap_err();
        assert!(error.contains("DPAPI"), "{error}");
        let rows = session.core.query_handoffs(None, None, false).unwrap();
        assert!(rows
            .iter()
            .all(|row| !row.contains("private") && !row.contains("scoped")));
        assert!(session.dispatch(&json!({"op":"media_push","kind":"cast","resource":{
            "url":"https://cdn.test/media.m3u8", "request_method":"POST", "request_body":"once=1"
        }})).unwrap_err().contains("非 GET"));
    }

    #[cfg(windows)]
    #[test]
    fn media_push_vaults_context_without_exposing_secrets_and_rejects_post() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let response = session
            .dispatch(&json!({
                "op":"media_push", "kind":"cast", "resource":{
                    "url":"https://cdn.test/media", "request_method":"GET", "resource_kind":"dash",
                    "cookie":"sid=private", "request_headers":{"Authorization":"Bearer private"},
                    "request_contexts":{"https://cdn.test":{"cookie":"sid=scoped"}}
                }
            }))
            .unwrap();
        let id = response["id"].as_str().unwrap();
        let rows = session.core.query_handoffs(Some(id), None, false).unwrap();
        let stored: MediaPushRequest = serde_json::from_str(&rows[0]).unwrap();
        assert_eq!(stored.mime_type, "application/dash+xml");
        assert!(!rows[0].contains("private") && !rows[0].contains("scoped"));
        let HostCore::Local(core) = &mut session.core else {
            unreachable!()
        };
        let reference = stored.credential_ref.as_ref().unwrap();
        let blob = core.store().load_credential(reference).unwrap().unwrap();
        let plain = CredentialVault.unprotect(&blob).unwrap();
        assert!(
            plain.contains("sid=private")
                && plain.contains("Bearer private")
                && plain.contains("sid=scoped")
        );
        core.handle(CoreCommand::ResolveMediaPush {
            request_id: id.into(),
            status: "canceled".into(),
            message: String::new(),
            location: String::new(),
        })
        .unwrap();
        assert!(core.store().load_credential(reference).unwrap().is_none());
        assert!(session.dispatch(&json!({"op":"media_push","kind":"cast","resource":{
            "url":"https://cdn.test/media.m3u8", "request_method":"POST", "request_body":"once=1"
        }})).unwrap_err().contains("非 GET"));
    }

    #[test]
    fn workbench_receipts_are_scoped_to_the_launch() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let first = session.dispatch(&json!({"op": "activate"})).unwrap();
        let second = session.dispatch(&json!({"op": "activate"})).unwrap();
        assert_eq!(first["stage"], "launching");
        assert_ne!(first["launch_id"], second["launch_id"]);
        let query = |launch: &Value| {
            json!({
                "op": "workbench_status",
                "launch_id": launch["launch_id"],
                "since_sequence": launch["since_sequence"],
                "wait_ms": 0,
            })
        };
        let send = |session: &mut NativeHostSession, launch: &Value, surface: &str| {
            session
                .core
                .handle(CoreCommand::UiReady {
                    launch_id: launch["launch_id"].as_str().unwrap().into(),
                    surface: surface.into(),
                })
                .unwrap();
        };
        send(&mut session, &second, "main_failed");
        assert_eq!(session.dispatch(&query(&first)).unwrap()["failed"], false);
        assert_eq!(session.dispatch(&query(&second)).unwrap()["failed"], true);
        send(&mut session, &first, "main");
        assert_eq!(session.dispatch(&query(&first)).unwrap()["ready"], true);
        assert_eq!(session.dispatch(&query(&second)).unwrap()["ready"], false);
    }

    #[test]
    fn workbench_status_requires_launch_identity_and_cursor() {
        let mut session = NativeHostSession::in_memory().unwrap();
        assert!(session
            .dispatch(&json!({"op": "workbench_status"}))
            .unwrap_err()
            .contains("启动编号"));
        assert!(session
            .dispatch(&json!({"op": "workbench_status", "launch_id": "current"}))
            .unwrap_err()
            .contains("启动序号"));
    }

    #[test]
    fn media_push_returns_core_overlap_rejection_without_reopening_ui() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let first = session
            .dispatch(&json!({
                "op": "media_push",
                "kind": "tvbox",
                "resource": {
                    "url": "https://cdn.test/first.m3u8",
                    "title": "First"
                }
            }))
            .unwrap();
        assert_eq!(first["status"], "pending");

        let sequence_before_second = session.core.local().latest_sequence();
        let second = session
            .dispatch(&json!({
                "op": "media_push",
                "kind": "cast",
                "resource": {
                    "url": "https://cdn.test/second.m3u8",
                    "title": "Second"
                }
            }))
            .unwrap();
        let second_id = second["id"].as_str().unwrap().to_string();
        assert_eq!(second["ok"], true);
        assert_eq!(second["status"], "failed");
        assert!(second["message"]
            .as_str()
            .is_some_and(|message| message.contains("已有投送请求")));

        let events = session.core.local().events_after(sequence_before_second, 8);
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::MediaPushResolved { request }
                if request.id == second_id && request.status == "failed"
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(&event.event, CoreEvent::UiShow { .. })));

        let persisted = session
            .dispatch(&json!({
                "op": "media_push_status",
                "request_id": second_id
            }))
            .unwrap();
        assert_eq!(persisted["ok"], true);
        assert_eq!(persisted["status"], "failed");
    }

    #[test]
    fn media_push_overlap_after_host_restart_reopens_restored_picker() {
        let mut core = crate::PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::RequestMediaPush {
            request: MediaPushRequest {
                id: "media-push-restored".into(),
                push_kind: "tvbox".into(),
                url: "https://cdn.test/restored.m3u8".into(),
                title: "Restored".into(),
                status: "pending".into(),
                message: "等待在桌面下载器中选择接收设备".into(),
                location: String::new(),
                credential_ref: None,
                mime_type: String::new(),
                created_at_ms: unix_time_ms(),
            },
        })
        .unwrap();
        let sequence_before_retry = core.latest_sequence();
        let mut session = NativeHostSession::from_backend(HostCore::Local(core));

        let retry = session
            .dispatch(&json!({
                "op": "media_push",
                "kind": "cast",
                "resource": {
                    "url": "https://cdn.test/retry.m3u8",
                    "title": "Retry"
                }
            }))
            .unwrap();
        let retry_id = retry["id"].as_str().unwrap().to_string();
        assert_eq!(retry["status"], "failed");
        assert!(retry["message"]
            .as_str()
            .is_some_and(|message| message.contains("已有投送请求")));

        let events = session.core.local().events_after(sequence_before_retry, 8);
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::MediaPushResolved { request }
                if request.id == retry_id && request.status == "failed"
        )));
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::UiShow { surface } if surface == "main"
        )));
        assert_eq!(
            session.media_push_ui_request_id.as_deref(),
            Some("media-push-restored")
        );
    }

    #[test]
    fn media_push_new_request_reopens_ui_after_previous_request_resolves() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let first = session
            .dispatch(&json!({
                "op": "media_push",
                "kind": "tvbox",
                "resource": {
                    "url": "https://cdn.test/first.m3u8",
                    "title": "First"
                }
            }))
            .unwrap();
        let first_id = first["id"].as_str().unwrap().to_string();
        session
            .core
            .handle(CoreCommand::ResolveMediaPush {
                request_id: first_id,
                status: "canceled".into(),
                message: "用户取消".into(),
                location: String::new(),
            })
            .unwrap();

        let sequence_before_second = session.core.local().latest_sequence();
        let second = session
            .dispatch(&json!({
                "op": "media_push",
                "kind": "cast",
                "resource": {
                    "url": "https://cdn.test/second.m3u8",
                    "title": "Second"
                }
            }))
            .unwrap();
        let second_id = second["id"].as_str().unwrap().to_string();
        assert_eq!(second["status"], "pending");
        assert_ne!(second_id, first["id"]);

        let events = session.core.local().events_after(sequence_before_second, 8);
        assert!(events.iter().any(|event| matches!(
            &event.event,
            CoreEvent::UiShow { surface } if surface == "main"
        )));
        assert_eq!(
            session.media_push_ui_request_id.as_deref(),
            Some(second_id.as_str())
        );
    }

    #[test]
    fn ui_accept_updates_handoff_status_with_task_fields() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let offered = session
            .dispatch(&json!({
                "op": "offer",
                "resource": {
                    "url": "https://cdn.test/setup.exe",
                    "filename": "setup.exe",
                    "title": "Setup",
                    "size": 2048,
                    "resource_kind": "file"
                }
            }))
            .unwrap();
        let id = offered["handoff"]["id"].as_str().unwrap().to_string();
        session
            .dispatch(&json!({
                "op": "accept_handoff",
                "handoff_id": id,
                "filename": "setup.exe"
            }))
            .unwrap();
        let status = session
            .dispatch(&json!({
                "op": "handoff_status",
                "handoff_id": id
            }))
            .unwrap();
        assert_eq!(status["ok"], true);
        assert_eq!(status["handoff"]["status"], "accepted");
        assert_eq!(status["handoff"]["filename"], "setup.exe");
        assert_eq!(status["handoff"]["task_status"], "queued");
        assert!(status["handoff"]["task_id"]
            .as_str()
            .unwrap()
            .starts_with("task-"));
    }

    #[test]
    fn browser_offer_rejects_javascript_and_file_urls() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let sequence_before_invalid = session.core.local().latest_sequence();
        let javascript = session
            .dispatch(&json!({
                "op": "offer",
                "resource": {
                    "url": "javascript:alert(1)",
                    "cookie": "session=must-not-be-vaulted"
                }
            }))
            .unwrap_err();
        assert!(javascript.contains("不受支持"));
        let invalid_download = session
            .dispatch(&json!({
                "op": "download",
                "resource": {
                    "url": "javascript:alert(2)",
                    "cookie": "session=must-not-be-vaulted"
                }
            }))
            .unwrap_err();
        assert!(invalid_download.contains("不受支持"));
        let file = session
            .dispatch(&json!({
                "op": "offer",
                "resource": { "url": "file:///C:/Windows/notepad.exe" }
            }))
            .unwrap_err();
        assert!(file.contains("不受支持"));
        for url in [
            "blob:https://site.test/opaque",
            "data:application/octet-stream;base64,AA==",
        ] {
            let rejected = session
                .dispatch(&json!({
                    "op": "offer",
                    "resource": { "url": url }
                }))
                .unwrap_err();
            assert!(rejected.contains("不受支持"));
        }
        let bom = session
            .dispatch(&json!({
                "op": "offer",
                "resource": { "url": "\u{feff}javascript:alert(1)" }
            }))
            .unwrap_err();
        assert!(bom.contains("不受支持"));
        let nul = session
            .dispatch(&json!({
                "op": "offer",
                "resource": { "url": "https://cdn.test/a.bin\u{0000}.gif" }
            }))
            .unwrap_err();
        assert!(nul.contains("不受支持"));
        let msdt = session
            .dispatch(&json!({
                "op": "offer",
                "resource": { "url": "ms-msdt:foo" }
            }))
            .unwrap_err();
        assert!(msdt.contains("不受支持"));
        // 只检查当前会话的副作用；其他测试可以并行生成自己的交接序号。
        assert!(session
            .core
            .query_handoffs(None, None, false)
            .unwrap()
            .is_empty());
        assert!(session.core.snapshot_tasks().unwrap().is_empty());
        assert_eq!(
            session.core.local().latest_sequence(),
            sequence_before_invalid
        );
    }

    #[test]
    fn local_file_offer_requires_an_absolute_existing_file_with_matching_size() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let source = std::env::temp_dir().join(format!("hls-local-offer-{}", next_handoff_id()));
        let bytes = b"browser payload";
        std::fs::write(&source, bytes).unwrap();
        let source_payload = source.to_string_lossy().into_owned();
        let accepted = session
            .dispatch(&json!({
                "op": "offer_local_file",
                "resource": {
                    "url": "blob:https://site.test/opaque",
                    "local_source_path": &source_payload,
                    "filename": "saved.bin",
                    "size": bytes.len()
                }
            }))
            .unwrap();
        let handoff_id = accepted["handoff"]["id"].as_str().unwrap();
        let canonical = std::fs::canonicalize(&source).unwrap();
        let canonical = canonical.to_string_lossy();
        assert_eq!(
            session
                .handoff(handoff_id)
                .unwrap()
                .offer
                .local_source_path
                .as_deref(),
            Some(canonical.as_ref())
        );
        let wrong_size = session
            .dispatch(&json!({
                "op": "offer_local_file",
                "resource": {
                    "url": "data:application/octet-stream;base64,AA==",
                    "local_source_path": &source_payload,
                    "filename": "saved.bin",
                    "size": bytes.len() - 1
                }
            }))
            .unwrap_err();
        assert!(wrong_size.contains("大小不匹配"));
        let relative_path = session
            .dispatch(&json!({
                "op": "offer_local_file",
                "resource": {
                    "url": "blob:https://site.test/opaque",
                    "local_source_path": "relative.bin",
                    "filename": "saved.bin",
                    "size": bytes.len()
                }
            }))
            .unwrap_err();
        assert!(relative_path.contains("绝对路径"));
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn write_response_replaces_oversized_payload_with_correlatable_fallback_error() {
        let mut output = Vec::new();
        let oversized = json!({
            "ok": true,
            "__request_id": "req-oversized-42",
            "blob": "x".repeat(MAX_RESPONSE_BYTES + 1024),
        });
        write_response(&mut output, &oversized).unwrap();
        assert!(output.len() >= 4 && output.len() - 4 <= MAX_RESPONSE_BYTES);
        let decoded = read_message(&mut Cursor::new(output))
            .unwrap()
            .expect("expected fallback response frame");
        assert_eq!(decoded["ok"], false);
        assert_eq!(decoded["error"], "Native Messaging 响应过大");
        assert_eq!(decoded["__request_id"], "req-oversized-42");
    }

    #[test]
    fn broken_pipe_drops_session_and_reconnects_on_retry_or_next_request() {
        let mut session = Some(1u32);
        let mut opened = 1u32;
        let mut observed_policies = Vec::new();
        // Read-only op auto-retries once on Core IPC close / broken pipe, using
        // ExistingOnly so a clean shutdown never respawns Core.
        let response = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "handoff_status"}),
            |msg| core_connect_policy_with(msg, false),
            |policy| {
                observed_policies.push(policy);
                opened += 1;
                Ok(opened)
            },
            |active, _| {
                if *active == 1 {
                    Err("Core IPC closed".into())
                } else {
                    Ok(json!({"ok": true, "session": *active}))
                }
            },
        );
        assert_eq!(response["ok"], true);
        assert_eq!(response["session"], 2);
        assert_eq!(session, Some(2));
        assert_eq!(observed_policies, vec![CoreConnectPolicy::ExistingOnly]);

        // Mutating ops must NOT auto-replay even on pipe write failure (multi-RPC
        // operations may have already committed an earlier step).
        let mut dispatches = 0usize;
        let write_fail = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "media_push"}),
            |msg| core_connect_policy_with(msg, false),
            |policy| {
                observed_policies.push(policy);
                opened += 1;
                Ok(opened)
            },
            |_, _| {
                dispatches += 1;
                Err("Core pipe write: broken pipe".into())
            },
        );
        assert_eq!(dispatches, 1);
        assert_eq!(write_fail["code"], "core_unreachable");
        assert_eq!(session, None);

        // Next `ping` reconnects with ExistingOnly when user-shutdown is active,
        // and AllowAutostart when no user-shutdown marker is present.
        let recovered = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "ping"}),
            |msg| core_connect_policy_with(msg, true),
            |policy| {
                observed_policies.push(policy);
                opened += 1;
                Ok(opened)
            },
            |active, _| Ok(json!({"ok": true, "session": *active})),
        );
        assert_eq!(recovered["ok"], true);
        assert_eq!(recovered["session"], 3);
        assert_eq!(session, Some(3));
        assert_eq!(
            observed_policies,
            vec![
                CoreConnectPolicy::ExistingOnly,
                CoreConnectPolicy::ExistingOnly
            ]
        );
    }

    #[test]
    fn transport_error_response_clears_cached_session() {
        let mut session = Some(1u32);
        let response = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "offer"}),
            |msg| core_connect_policy_with(msg, false),
            |_| Ok(2u32),
            |_, _| Ok(json!({"ok": false, "transport_error": true, "error": "pipe closed"})),
        );
        assert_eq!(response["transport_error"], true);
        assert_eq!(session, None);

        let mut session = Some(1u32);
        let response = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "workbench_status"}),
            |msg| core_connect_policy_with(msg, false),
            |_| Ok(2u32),
            |_, _| Ok(json!({"ok": false, "code": "core_unreachable", "error": "eof"})),
        );
        assert_eq!(response["code"], "core_unreachable");
        assert_eq!(session, None);
    }

    #[test]
    fn core_connect_policy_classifies_passive_and_explicit_ops() {
        // Explicit user launch/action ops always allow autostart, even after tray quit.
        for message in [
            json!({"op": "activate"}),
            json!({"op": "offer"}),
            json!({"op": "download"}),
            json!({"op": "media_push"}),
        ] {
            for user_shutdown in [false, true] {
                assert_eq!(
                    core_connect_policy_with(&message, user_shutdown),
                    CoreConnectPolicy::AllowAutostart,
                    "expected autostart for {message} (user_shutdown={user_shutdown})"
                );
            }
        }

        // `ping` cold-starts Core on fresh/normal startup, but MUST NOT autostart
        // once the user has explicitly quit from the tray.
        for message in [
            json!({"op": "ping"}),
            json!({"op": "ping", "interactive": true}),
        ] {
            assert_eq!(
                core_connect_policy_with(&message, false),
                CoreConnectPolicy::AllowAutostart,
                "cold-start ping must autostart when user has not tray-quit: {message}"
            );
            assert_eq!(
                core_connect_policy_with(&message, true),
                CoreConnectPolicy::ExistingOnly,
                "ping must never respawn Core after tray quit: {message}"
            );
        }

        // Passive/polling/background ops NEVER autostart Core in either state.
        for message in [
            json!({"op": "handoff_status", "handoff_id": "h-1"}),
            json!({"op": "media_push_status", "request_id": "m-1"}),
            json!({"op": "workbench_status", "launch_id": "l-1", "since_sequence": 0}),
            json!({"op": "set_takeover_settings", "enabled": false}),
            json!({"op": "offer_local_file"}),
            json!({"op": "recover_offer"}),
            json!({"op": "subtitle", "request": {"action": "configuration"}}),
            json!({"op": "accept_handoff", "handoff_id": "h-1"}),
            json!({"op": "reject_handoff", "handoff_id": "h-1"}),
            json!({"op": "list_tasks"}),
            json!({"op": "get_settings"}),
            json!({"op": "poll_refresh_requests"}),
            json!({"op": "unknown_op"}),
            json!({}),
        ] {
            for user_shutdown in [false, true] {
                assert_eq!(
                    core_connect_policy_with(&message, user_shutdown),
                    CoreConnectPolicy::ExistingOnly,
                    "passive/background op must never autostart Core: {message} (user_shutdown={user_shutdown})"
                );
            }
        }
    }

    #[test]
    fn passive_ops_when_core_absent_never_autostart_and_return_core_unreachable() {
        // 1. Fresh/cold profile (no tray-quit marker): `ping` cold-starts Core.
        let mut cold_session: Option<NativeHostSession> = None;
        let mut cold_autostarts = 0usize;
        let cold_ping = dispatch_session_message_with_policy(
            &mut cold_session,
            &json!({"op": "ping"}),
            |msg| core_connect_policy_with(msg, false),
            |policy| {
                assert_eq!(policy, CoreConnectPolicy::AllowAutostart);
                cold_autostarts += 1;
                NativeHostSession::in_memory()
            },
            |active, msg| active.dispatch(msg),
        );
        assert_eq!(cold_ping["ok"], true, "cold_ping={cold_ping}");
        assert_eq!(cold_ping["protocol_version"], V7_PROTOCOL_VERSION);
        assert_eq!(cold_autostarts, 1);
        assert!(cold_session.is_some());

        // 2. After tray quit (`user_shutdown_active = true`), `ping` AND all
        // passive/background ops must fail with `core_unreachable` and zero autostart.
        let mut session: Option<NativeHostSession> = None;
        let mut autostart_calls = 0usize;
        let mut existing_only_calls = 0usize;

        for message in [
            json!({"op": "ping"}),
            json!({"op": "ping", "interactive": true}),
            json!({"op": "handoff_status", "handoff_id": "handoff-1"}),
            json!({"op": "media_push_status", "request_id": "media-push-1"}),
            json!({"op": "workbench_status", "launch_id": "launch-1", "since_sequence": 0}),
            json!({"op": "set_takeover_settings", "enabled": true}),
            json!({"op": "offer_local_file", "resource": {"url": "blob:https://site.test/x"}}),
            json!({"op": "recover_offer", "resource": {"url": "https://cdn.test/a.bin", "client_request_id": "r-1"}}),
            json!({"op": "subtitle", "request": {"action": "configuration"}}),
            json!({"op": "list_tasks"}),
            json!({"op": "get_settings"}),
            json!({"op": "poll_refresh_requests"}),
        ] {
            let response = dispatch_session_message_with_policy(
                &mut session,
                &message,
                |msg| core_connect_policy_with(msg, true),
                |policy| match policy {
                    CoreConnectPolicy::ExistingOnly => {
                        existing_only_calls += 1;
                        Err("Core not running".into())
                    }
                    CoreConnectPolicy::AllowAutostart => {
                        autostart_calls += 1;
                        NativeHostSession::in_memory()
                    }
                },
                |active, msg| active.dispatch(msg),
            );
            assert_eq!(
                response["ok"], false,
                "message={message} response={response}"
            );
            assert_eq!(
                response["code"], "core_unreachable",
                "message={message} response={response}"
            );
            assert_eq!(response["stage"], "core");
            assert!(
                !response["error"].as_str().unwrap_or("").is_empty(),
                "message={message} response={response}"
            );
            assert!(session.is_none());
        }

        assert_eq!(autostart_calls, 0);
        assert_eq!(existing_only_calls, 12);
    }

    #[test]
    fn passive_ops_after_core_shutdown_do_not_respawn_core() {
        let server = crate::CoreServer::in_memory().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = server.serve(listener);
        let client = CoreIpcClient::connect_addr(addr).unwrap();
        let mut session = Some(NativeHostSession::from_backend(HostCore::Remote(client)));
        let user_shutdown = std::sync::atomic::AtomicBool::new(false);

        let autostart_calls = std::sync::atomic::AtomicUsize::new(0);
        let mut open = |policy| match policy {
            CoreConnectPolicy::ExistingOnly => {
                CoreIpcClient::connect_addr_timeout(addr, Duration::from_millis(25))
                    .map(|client| NativeHostSession::from_backend(HostCore::Remote(client)))
            }
            CoreConnectPolicy::AllowAutostart => {
                autostart_calls.fetch_add(1, Ordering::SeqCst);
                NativeHostSession::in_memory()
            }
        };

        // Live Core answers ping normally.
        let live_ping = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "ping"}),
            |msg| core_connect_policy_with(msg, user_shutdown.load(Ordering::SeqCst)),
            &mut open,
            |active, msg| active.dispatch(msg),
        );
        assert_eq!(live_ping["ok"], true);
        assert_eq!(autostart_calls.load(Ordering::SeqCst), 0);

        // Simulate Tray "退出下载引擎" (record user-shutdown marker + `CoreCommand::Shutdown`).
        user_shutdown.store(true, Ordering::SeqCst);
        let mut shutdown_client = CoreIpcClient::connect_addr(addr).unwrap();
        shutdown_client.command(CoreCommand::Shutdown).unwrap();
        drop(shutdown_client);
        worker.join().unwrap().unwrap();

        // Passive heartbeats & background ops must drop the dead session,
        // return `core_unreachable`, and never request autostart.
        for message in [
            json!({"op": "ping"}),
            json!({"op": "ping", "interactive": true}),
            json!({"op": "handoff_status", "handoff_id": "handoff-1"}),
            json!({"op": "media_push_status", "request_id": "media-push-1"}),
            json!({"op": "set_takeover_settings", "enabled": false}),
            json!({"op": "subtitle", "request": {"action": "configuration"}}),
        ] {
            let response = dispatch_session_message_with_policy(
                &mut session,
                &message,
                |msg| core_connect_policy_with(msg, user_shutdown.load(Ordering::SeqCst)),
                &mut open,
                |active, msg| active.dispatch(msg),
            );
            assert_eq!(
                response["ok"], false,
                "message={message} response={response}"
            );
            assert_eq!(
                response["code"], "core_unreachable",
                "message={message} response={response}"
            );
            assert_eq!(response["stage"], "core");
            assert!(session.is_none());
        }
        assert_eq!(autostart_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn explicit_launch_and_action_ops_autostart_core_when_absent_or_after_shutdown() {
        // 1. Cold start (session == None) for explicit user-initiated ops, even
        // when the user-shutdown marker is active from an earlier tray quit.
        for (message, check_field) in [
            (json!({"op": "activate"}), "activated"),
            (
                json!({
                    "op": "offer",
                    "resource": {
                        "url": "https://cdn.test/video.mp4",
                        "filename": "video.mp4",
                        "resource_kind": "file",
                        "client_request_id": "explicit-offer-1"
                    }
                }),
                "handoff",
            ),
            (
                json!({
                    "op": "download",
                    "resource": {
                        "url": "https://cdn.test/movie.m3u8",
                        "filename": "movie.mp4",
                        "resource_kind": "hls"
                    }
                }),
                "task",
            ),
            (
                json!({
                    "op": "media_push",
                    "kind": "tvbox",
                    "resource": {
                        "url": "https://cdn.test/live.m3u8",
                        "title": "Live"
                    }
                }),
                "id",
            ),
        ] {
            let mut session: Option<NativeHostSession> = None;
            let mut autostart_calls = 0usize;
            let response = dispatch_session_message_with_policy(
                &mut session,
                &message,
                |msg| core_connect_policy_with(msg, true),
                |policy| {
                    assert_eq!(policy, CoreConnectPolicy::AllowAutostart);
                    autostart_calls += 1;
                    NativeHostSession::in_memory()
                },
                |active, msg| active.dispatch(msg),
            );
            assert_eq!(
                response["ok"], true,
                "message={message} response={response}"
            );
            assert!(
                !response[check_field].is_null(),
                "message={message} response={response}"
            );
            assert_eq!(autostart_calls, 1);
            assert!(session.is_some());
        }

        // 2. Stale session immediately after Tray quit (`CoreCommand::Shutdown`
        // + user-shutdown marker): clicking "主程序" (`activate`) must drop the
        // dead pipe, autostart Core, and succeed on the first request.
        let server = crate::CoreServer::in_memory().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = server.serve(listener);
        let client = CoreIpcClient::connect_addr(addr).unwrap();
        let mut session = Some(NativeHostSession::from_backend(HostCore::Remote(client)));

        let mut shutdown_client = CoreIpcClient::connect_addr(addr).unwrap();
        shutdown_client.command(CoreCommand::Shutdown).unwrap();
        drop(shutdown_client);
        worker.join().unwrap().unwrap();

        let mut autostart_calls = 0usize;
        let activated = dispatch_session_message_with_policy(
            &mut session,
            &json!({"op": "activate"}),
            |msg| core_connect_policy_with(msg, true),
            |policy| {
                assert_eq!(policy, CoreConnectPolicy::AllowAutostart);
                autostart_calls += 1;
                NativeHostSession::in_memory()
            },
            |active, msg| active.dispatch(msg),
        );
        assert_eq!(activated["ok"], true, "activated={activated}");
        assert_eq!(activated["activated"], true);
        assert_eq!(activated["stage"], "launching");
        assert_eq!(autostart_calls, 1);
        assert!(session.is_some());
    }
}
