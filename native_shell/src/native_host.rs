//! Python-free Native Messaging front-end for the v6 Rust core.
//!
//! Chrome and Firefox keep this process alive through a Native Messaging port.
//! One session therefore owns one durable Core connection plus its short-lived
//! handoff offers. No HTTP request or Python process is required on this path.

use crate::{
    CoreCommand, CoreEvent, CoreIpcClient, CredentialVault, ResourceKind, ResourceOffer, TaskSnapshot,
    TaskSpec, V6_PROTOCOL_NAME, V6_PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const RECOMMENDED_EXTENSION_VERSION: &str = "5.0.14";
const MINIMUM_EXTENSION_VERSION: &str = "2.0.11";
const EXTENSION_RELEASE_URL: &str = "https://github.com/ciaooo55/hls-downloader/releases/latest";
static NEXT_HANDOFF: AtomicU64 = AtomicU64::new(1);

struct NativeHostSession {
    core: HostCore,
    handoffs: BTreeMap<String, Handoff>,
    request_ids: BTreeMap<String, String>,
}

enum HostCore {
    #[cfg(test)]
    Local(crate::PersistentCore),
    Remote(CoreIpcClient),
}

impl HostCore {
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

    fn setting_bool(&mut self, key: &str, fallback: bool) -> Result<bool, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store().setting_bool(key, fallback),
            Self::Remote(client) => match client.load_settings()? {
                crate::CorePipeResponse::Settings {
                    takeover_enabled,
                    legal_accepted,
                    ..
                } => Ok(match key {
                    "browser_takeover_enabled" => takeover_enabled,
                    "legal_terms_accepted" => legal_accepted,
                    _ => fallback,
                }),
                _ => Ok(fallback),
            },
        }
    }

    fn setting_u64(&mut self, key: &str, fallback: u64) -> Result<u64, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store().setting_u64(key, fallback),
            Self::Remote(client) => match client.load_settings()? {
                crate::CorePipeResponse::Settings {
                    takeover_minimum_bytes,
                    speed_limit_kib,
                    harvest_minimum_bytes,
                    ..
                } => Ok(match key {
                    "browser_takeover_minimum_bytes" => takeover_minimum_bytes,
                    "download_speed_limit_kib" => speed_limit_kib,
                    "harvest_minimum_bytes" => harvest_minimum_bytes,
                    _ => fallback,
                }),
                _ => Ok(fallback),
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
            Self::Local(core) => core
                .store_mut()
                .store_credential(credential_ref, protected_blob, kind),
            Self::Remote(client) => client.store_credential(credential_ref, protected_blob, kind),
        }
    }

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
            Self::Local(core) => core.store_mut().save_handoff(
                handoff_id,
                json,
                status,
                task_id,
                created_at_ms,
            ),
            Self::Remote(client) => {
                client.save_handoff(handoff_id, json, status, task_id, created_at_ms)
            }
        }
    }

    fn load_handoffs(&mut self) -> Result<Vec<String>, String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store().load_handoffs(),
            Self::Remote(client) => client.load_handoffs(),
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

#[derive(Clone, Serialize, Deserialize)]
struct Handoff {
    id: String,
    offer: ResourceOffer,
    filename: String,
    title: String,
    mime_type: String,
    size: u64,
    status: String,
    presentation: String,
    task_id: Option<String>,
    created_at_ms: u64,
    #[serde(default)]
    request_id: String,
}

impl NativeHostSession {
    fn open_default() -> Result<Self, String> {
        Self::from_backend(HostCore::Remote(CoreIpcClient::connect()?))
    }

    #[cfg(test)]
    fn in_memory() -> Result<Self, String> {
        Self::from_backend(HostCore::Local(crate::PersistentCore::in_memory()?))
    }

    fn from_backend(mut core: HostCore) -> Result<Self, String> {
        let mut handoffs = BTreeMap::new();
        let mut request_ids = BTreeMap::new();
        for encoded in core.load_handoffs()? {
            if let Ok(handoff) = serde_json::from_str::<Handoff>(&encoded) {
                if !handoff.request_id.is_empty() {
                    request_ids.insert(handoff.request_id.clone(), handoff.id.clone());
                }
                handoffs.insert(handoff.id.clone(), handoff);
            }
        }
        Ok(Self {
            core,
            handoffs,
            request_ids,
        })
    }

    fn dispatch(&mut self, message: &Value) -> Result<Value, String> {
        let operation = message
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "Native Messaging 缺少 op".to_string())?;
        if !matches!(
            operation,
            "ping"
                | "activate"
                | "offer"
                | "download"
                | "handoff_status"
                | "wait_handoff"
                | "accept_handoff"
                | "reject_handoff"
                | "set_takeover_settings"
                | "push_to_tv"
                | "media_push"
                | "media_push_status"
        ) {
            return Err(format!("不支持的 Native Messaging 操作: {operation}"));
        }

        match operation {
            "ping" => self.ping(),
            "activate" => {
                self.core.handle(CoreCommand::OpenMain)?;
                Ok(json!({"ok": true, "activated": true}))
            }
            "offer" => self.offer(message),
            "download" => self.download(message),
            "handoff_status" | "wait_handoff" => self.handoff_status(message),
            "accept_handoff" => self.accept_handoff(message),
            "reject_handoff" => self.reject_handoff(message),
            "set_takeover_settings" => self.set_takeover_settings(message),
            "push_to_tv" | "media_push" => self.media_push(message),
            "media_push_status" => self.media_push_status(message),
            _ => unreachable!(),
        }
    }

    fn ping(&mut self) -> Result<Value, String> {
        self.core.handle(CoreCommand::Ping)?;
        let _ = self.core.handle(CoreCommand::BrowserHello {
            version: env!("CARGO_PKG_VERSION").into(),
            browser: "extension".into(),
        });
        let takeover_enabled = self.core.setting_bool("browser_takeover_enabled", true)?;
        let minimum_bytes = self
            .core
            .setting_u64("browser_takeover_minimum_bytes", 0)?;
        Ok(json!({
            "ok": true,
            "protocol": V6_PROTOCOL_NAME,
            "protocol_version": V6_PROTOCOL_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "takeover_enabled": takeover_enabled,
            "takeover_minimum_bytes": minimum_bytes,
            "recommended_extension_version": RECOMMENDED_EXTENSION_VERSION,
            "minimum_extension_version": MINIMUM_EXTENSION_VERSION,
            "extension_release_url": EXTENSION_RELEASE_URL,
        }))
    }

    fn offer(&mut self, message: &Value) -> Result<Value, String> {
        let payload = resource_payload(message)?;
        let credential_ref = self.persist_browser_context(payload)?;
        let request_id = payload
            .get("client_request_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !request_id.is_empty() {
            if let Some(existing_id) = self.request_ids.get(request_id) {
                if let Some(existing) = self.handoffs.get(existing_id) {
                    return Ok(json!({"ok": true, "handoff": existing.public(None)}));
                }
            }
        }

        let mut offer = parse_offer(payload)?;
        offer.credential_ref = credential_ref;
        let id = next_handoff_id();
        offer.handoff_id = id.clone();
        offer.filename = field(payload, "filename");
        offer.title = field(payload, "title");
        offer.size = payload.get("size").and_then(Value::as_u64).unwrap_or(0);
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
        };
        if !request_id.is_empty() && request_id.len() <= 160 {
            self.request_ids.insert(request_id.to_string(), id.clone());
        }
        self.persist_handoff(&handoff)?;
        self.handoffs.insert(id.clone(), handoff);
        self.core.handle(CoreCommand::OfferResource { offer })?;
        let response = self
            .handoffs
            .get(&id)
            .map(|item| item.public(None))
            .unwrap_or_else(|| json!({"id": id, "status": "pending"}));
        Ok(json!({"ok": true, "handoff": response}))
    }

    fn download(&mut self, message: &Value) -> Result<Value, String> {
        let payload = resource_payload(message)?;
        let credential_ref = self.persist_browser_context(payload)?;
        let offer = parse_offer(payload)?;
        let filename = filename(payload, &offer.url);
        let events = self.core.handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: offer.url,
                resource_kind: offer.resource_kind,
                title: field(payload, "title"),
                filename,
                download_dir: String::new(),
                request_method: offer.request_method,
                credential_ref: credential_ref.or(offer.credential_ref),
                replay_context_ref: offer.replay_context_ref,
                concurrency: 8,
                checksum: None,
                expected_size: payload
                    .get("size")
                    .and_then(Value::as_u64)
                    .filter(|size| *size > 0),
                etag: field(payload, "etag"),
                last_modified: field(payload, "last_modified"),
                ..Default::default()
            },
        })?;
        let snapshot = events
            .into_iter()
            .find_map(|event| match event.event {
                CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .ok_or_else(|| "Rust Core 未返回新建任务快照".to_string())?;
        let _ = self.core.handle(CoreCommand::TaskAction {
            task_id: snapshot.task_id.clone(),
            action: "start".into(),
        });
        Ok(json!({"ok": true, "task": snapshot, "activated": true}))
    }

    fn handoff_status(&mut self, message: &Value) -> Result<Value, String> {
        let _ = self.reload_handoffs();
        let id = message
            .get("handoff_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let handoff = self
            .handoffs
            .get(id)
            .ok_or_else(|| "接管请求不存在或已过期".to_string())?
            .clone();
        let task = match handoff.task_id.as_deref() {
            Some(task_id) => self
                .core
                .snapshot_tasks()?
                .into_iter()
                .find(|item| item.task_id == task_id),
            None => None,
        };
        Ok(json!({"ok": true, "handoff": handoff.public(task.as_ref())}))
    }

    fn accept_handoff(&mut self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("handoff_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let _ = self.reload_handoffs();
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
        })?;
        self.reload_handoffs()?;
        self.handoff_status(&json!({"handoff_id": id}))
    }

    fn reject_handoff(&mut self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("handoff_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let _ = self.reload_handoffs();
        self.core.handle(CoreCommand::RejectHandoff {
            handoff_id: id.clone(),
        })?;
        self.reload_handoffs()?;
        self.handoff_status(&json!({"handoff_id": id}))
    }

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
        let enabled = self.core.setting_bool("browser_takeover_enabled", true)?;
        let minimum_bytes = self
            .core
            .setting_u64("browser_takeover_minimum_bytes", 0)?;
        Ok(json!({
            "ok": true,
            "takeover_enabled": enabled,
            "takeover_minimum_bytes": minimum_bytes,
        }))
    }

    fn media_push(&mut self, message: &Value) -> Result<Value, String> {
        let _ = self.core.handle(CoreCommand::OpenMain);
        let kind = message
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("cast");
        let payload = resource_payload(message)?;
        let url = field(payload, "url");
        let title = field(payload, "title");
        let title = if title.is_empty() {
            filename(payload, &url)
        } else {
            title
        };
        let push = crate::cast::start_browser_push(kind, &url, &title)?;
        Ok(json!({
            "ok": true,
            "id": push.id,
            "kind": push.kind,
            "status": push.status,
            "message": push.message,
            "location": push.location,
        }))
    }

    fn media_push_status(&self, message: &Value) -> Result<Value, String> {
        let id = message
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(push) = crate::cast::browser_push_status(id) else {
            return Ok(json!({
                "ok": false,
                "error": "投送请求不存在或已过期"
            }));
        };
        Ok(json!({
            "ok": true,
            "id": push.id,
            "kind": push.kind,
            "status": push.status,
            "message": push.message,
            "location": push.location,
        }))
    }

    fn persist_browser_context(
        &mut self,
        payload: &Map<String, Value>,
    ) -> Result<Option<String>, String> {
        if !contains_browser_secrets(payload) {
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
        let credential_ref = format!(
            "cred-{:x}-{}",
            unix_time_ms(),
            NEXT_HANDOFF.load(Ordering::Relaxed)
        );
        self.core
            .store_credential(&credential_ref, &protected, "browser_replay")?;
        Ok(Some(credential_ref))
    }

    fn reload_handoffs(&mut self) -> Result<(), String> {
        let mut handoffs = BTreeMap::new();
        let mut request_ids = BTreeMap::new();
        for encoded in self.core.load_handoffs()? {
            if let Ok(handoff) = serde_json::from_str::<Handoff>(&encoded) {
                if !handoff.request_id.is_empty() {
                    request_ids.insert(handoff.request_id.clone(), handoff.id.clone());
                }
                handoffs.insert(handoff.id.clone(), handoff);
            }
        }
        self.handoffs = handoffs;
        self.request_ids = request_ids;
        Ok(())
    }
}

impl Handoff {
    fn public(&self, task: Option<&TaskSnapshot>) -> Value {
        let mut value = json!({
            "id": self.id,
            "url": self.offer.url,
            "filename": self.filename,
            "title": self.title,
            "mime_type": self.mime_type,
            "source_page_url": self.offer.source_page_url,
            "resource_kind": resource_kind_name(self.offer.resource_kind),
            "owner": self.offer.owner,
            "evidence": self.offer.evidence,
            "confidence": self.offer.confidence,
            "request_method": self.offer.request_method,
            "size": self.size,
            "status": self.status,
            "task_id": self.task_id,
            "created_at_ms": self.created_at_ms,
            "presented": self.presentation == "presented",
            "presentation": self.presentation,
            "presentation_mode": "native-rust",
            "presentation_ok": self.presentation != "failed" && self.status != "failed",
            "presentation_queued": self.presentation == "queued",
            "presentable": true,
        });
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
}

pub fn run() -> i32 {
    let mut session = match NativeHostSession::open_default() {
        Ok(session) => session,
        Err(error) => {
            eprintln!("v6 core startup failed: {error}");
            return 1;
        }
    };
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
        let response = match session.dispatch(&message) {
            Ok(value) => value,
            Err(error) => json!({"ok": false, "error": error}),
        };
        let response = attach_request_id(response, request_id);
        if let Err(error) = write_response(&mut output, &response) {
            eprintln!("native messaging write failed: {error}");
            return 1;
        }
    }
}

fn attach_request_id(response: Value, request_id: String) -> Value {
    if request_id.is_empty() {
        return response;
    }
    let mut object = response.as_object().cloned().unwrap_or_default();
    object.insert("__request_id".into(), Value::String(request_id));
    Value::Object(object)
}

fn resource_payload(message: &Value) -> Result<&Map<String, Value>, String> {
    message
        .get("resource")
        .and_then(Value::as_object)
        .ok_or_else(|| "Native Messaging 缺少 resource".to_string())
}

fn parse_offer(payload: &Map<String, Value>) -> Result<ResourceOffer, String> {
    let url = field(payload, "url");
    if url.is_empty() {
        return Err("资源 URL 为空".into());
    }
    if !browser_resource_url_allowed(&url) {
        return Err("资源 URL 不受支持".into());
    }
    let resource_kind = parse_resource_kind(
        payload
            .get("resource_kind")
            .and_then(Value::as_str)
            .unwrap_or("file"),
        &url,
    );
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
        owner: field(payload, "owner").chars().take(160).collect(),
        evidence,
        confidence,
        source_page_url: field(payload, "source_page_url"),
        credential_ref: None,
        replay_context_ref: None,
        request_method: crate::http_engine::sanitize_http_method(&field(
            payload,
            "request_method",
        )),
        handoff_id: String::new(),
        filename: field(payload, "filename"),
        title: field(payload, "title"),
        size: payload.get("size").and_then(Value::as_u64).unwrap_or(0),
    })
}

fn contains_browser_secrets(payload: &Map<String, Value>) -> bool {
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
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
        || lower.starts_with("magnet:"))
        && !lower.contains('\r')
        && !lower.contains('\n')
}

fn next_handoff_id() -> String {
    let sequence = NEXT_HANDOFF.fetch_add(1, Ordering::Relaxed);
    format!("handoff-{:x}-{sequence:x}", unix_time_ms())
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
    let payload = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err("Native Messaging 响应过大".into());
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
    fn native_message_frame_roundtrip_is_little_endian() {
        let mut output = Vec::new();
        write_response(&mut output, &json!({"ok": true})).unwrap();
        let mut input = Cursor::new(output);
        let decoded = read_message(&mut input).unwrap().unwrap();
        assert_eq!(decoded["ok"], true);
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
                    "evidence": ["manifest_mime"],
                    "confidence": 0.99
                }
            }))
            .unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["task"]["resource_kind"], "hls");
        assert_eq!(session.core.local().tasks().len(), 1);
        assert_eq!(session.core.local().store().load_tasks().unwrap().len(), 1);
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
        let second = session.dispatch(&request).unwrap();
        assert_eq!(first["handoff"]["id"], second["handoff"]["id"]);
        assert_eq!(session.handoffs.len(), 1);
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
    fn v6_ping_does_not_advertise_fastapi_loopback() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let response = session.dispatch(&json!({"op": "ping"})).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["protocol"], "hls-downloader-v6-core");
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
        assert_eq!(status["status"], "ready");
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
        assert!(status["handoff"]["task_id"].as_str().unwrap().starts_with("task-"));
    }

    #[test]
    fn browser_offer_rejects_javascript_and_file_urls() {
        let mut session = NativeHostSession::in_memory().unwrap();
        let javascript = session
            .dispatch(&json!({
                "op": "offer",
                "resource": { "url": "javascript:alert(1)" }
            }))
            .unwrap_err();
        assert!(javascript.contains("不受支持"));
        let file = session
            .dispatch(&json!({
                "op": "offer",
                "resource": { "url": "file:///C:/Windows/notepad.exe" }
            }))
            .unwrap_err();
        assert!(file.contains("不受支持"));
    }
}
