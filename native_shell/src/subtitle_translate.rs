//! 在线字幕只保留内存会话；API Key 经 DPAPI 存入 Core 的凭据库。
use crate::{CoreCoordinator, CredentialVault};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};

const KEY_REF: &str = "subtitle:dashscope";
const PROVIDER_SETTING: &str = "subtitle_provider";
const MODEL: &str = "qwen3.5-livetranslate-flash-realtime";
const LANGUAGES: &[&str] = &[
    "zh", "en", "ja", "ko", "fr", "de", "es", "ru", "pt", "it", "ar", "id", "vi", "th",
];

#[derive(Default)]
pub(crate) struct SubtitleService {
    sessions: Mutex<BTreeMap<String, Session>>,
}

struct Session {
    owner: String,
    input: mpsc::Sender<Input>,
    state: Arc<Mutex<State>>,
}

enum Input {
    Audio {
        pcm: Vec<u8>,
        position: f64,
        rate: f64,
        epoch: u64,
    },
    Reset(u64),
    Stop,
}

struct State {
    status: &'static str,
    error: String,
    epoch: u64,
    sequence: u64,
    events: VecDeque<Value>,
    last_seen: Instant,
}

impl State {
    fn push(&mut self, mut event: Value) {
        self.sequence += 1;
        event["sequence"] = json!(self.sequence);
        event["epoch"] = json!(self.epoch);
        self.events.push_back(event);
        while self.events.len() > 256 {
            self.events.pop_front();
        }
    }
}

impl SubtitleService {
    pub(crate) fn dispatch(
        &self,
        core: &CoreCoordinator,
        request: &Value,
    ) -> Result<Value, String> {
        match string(request, "action") {
            "configuration" => return configuration(core),
            "configure" => {
                let workspace = string(request, "workspace");
                if workspace.is_empty()
                    || workspace.len() > 128
                    || !workspace
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                {
                    return Err("请填写有效的百炼业务空间 ID".into());
                }
                let region = string(request, "region");
                if !matches!(region, "cn-beijing" | "ap-southeast-1") {
                    return Err("不支持的服务地域".into());
                }
                let key = string(request, "api_key").trim();
                if key.len() > 4096 || key.contains(['\r', '\n']) {
                    return Err("API Key 格式无效".into());
                }
                let protected = if key.is_empty() {
                    None
                } else {
                    Some(CredentialVault.protect(key)?)
                };
                // 设置和凭据共用一个 SQLite 事务，失败时不会留下半保存配置。
                let mut locked = core.lock()?;
                let write = if request.get("clear_key").and_then(Value::as_bool) == Some(true) {
                    Some(crate::store::CredentialWrite::Delete {
                        credential_ref: KEY_REF,
                    })
                } else {
                    protected
                        .as_ref()
                        .map(|blob| crate::store::CredentialWrite::Store {
                            credential_ref: KEY_REF,
                            protected_blob: blob,
                            kind: "subtitle_api_key",
                        })
                };
                locked.store_mut().apply_credential_with_settings(
                    write,
                    &BTreeMap::from([(
                        PROVIDER_SETTING.into(),
                        json!(json!({"workspace": workspace, "region": region}).to_string()),
                    )]),
                )?;
                drop(locked);
                self.stop_all();
                return configuration(core);
            }
            _ => {}
        }
        let id = string(request, "session_id");
        let owner = string(request, "owner");
        if id.is_empty() || id.len() > 160 || owner.is_empty() || owner.len() > 160 {
            return Err("字幕会话身份无效".into());
        }
        let mut sessions = self.sessions.lock().map_err(|_| "字幕会话锁不可用")?;
        if string(request, "action") == "start" {
            sessions.retain(|_, s| {
                s.state
                    .lock()
                    .map(|s| !matches!(s.status, "stopped" | "failed"))
                    .unwrap_or(false)
            });
            if sessions.contains_key(id) {
                return Err("字幕会话已存在".into());
            }
            if sessions.len() >= 4 {
                return Err("最多同时翻译四个网页，请先停止其他会话".into());
            }
            let source = string(request, "source");
            let target = string(request, "target");
            if source != "auto" && !LANGUAGES.contains(&source) || !LANGUAGES.contains(&target) {
                return Err("字幕语种不受支持".into());
            }
            let config = configuration(core)?;
            let workspace = string(&config, "workspace");
            if workspace.is_empty() {
                return Err("请先在插件字幕设置中配置百炼业务空间和 API Key".into());
            }
            let key = core
                .load_credential(KEY_REF)?
                .ok_or("请先配置字幕服务 API Key")?;
            let key = CredentialVault.unprotect(&key)?;
            if key.is_empty() {
                return Err("请先配置字幕服务 API Key".into());
            }
            let endpoint = format!(
                "wss://{}.{}.maas.aliyuncs.com/api-ws/v1/realtime?model={MODEL}",
                workspace,
                string(&config, "region")
            );
            let epoch = request.get("epoch").and_then(Value::as_u64).unwrap_or(0);
            let state = Arc::new(Mutex::new(State {
                status: "connecting",
                error: String::new(),
                epoch,
                sequence: 0,
                events: VecDeque::new(),
                last_seen: Instant::now(),
            }));
            let (input, receiver) = mpsc::channel(16);
            sessions.insert(
                id.into(),
                Session {
                    owner: owner.into(),
                    input,
                    state: state.clone(),
                },
            );
            let source = source.to_string();
            let target = target.to_string();
            std::thread::spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "无法启动字幕连接".to_string())
                    .and_then(|runtime| {
                        runtime.block_on(run(
                            &endpoint,
                            &key,
                            &source,
                            &target,
                            receiver,
                            state.clone(),
                        ))
                    });
                let mut locked = state.lock().unwrap();
                match result {
                    Ok(()) => locked.status = "stopped",
                    Err(error) => {
                        locked.status = "failed";
                        locked.error = error;
                    }
                }
            });
            return Ok(json!({"ok": true, "status": "connecting"}));
        }
        let Some(session) = sessions.get(id) else {
            return if string(request, "action") == "stop" {
                Ok(json!({"ok": true, "status": "stopped"}))
            } else {
                Err("字幕会话已结束，请重新开始".into())
            };
        };
        if session.owner != owner {
            return Err("字幕会话不属于当前插件".into());
        }
        session
            .state
            .lock()
            .map_err(|_| "字幕状态不可用")?
            .last_seen = Instant::now();
        match string(request, "action") {
            "audio" => {
                let audio = string(request, "audio");
                if audio.len() > 43_000 {
                    return Err("字幕音频块过大".into());
                }
                let pcm = STANDARD.decode(audio).map_err(|_| "字幕音频编码无效")?;
                if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > 32_000 {
                    return Err("字幕音频必须是 16kHz 单声道 PCM16".into());
                }
                let position = request
                    .get("position")
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or("字幕时间无效")?;
                let rate = request
                    .get("rate")
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && *v > 0.0 && *v <= 16.0)
                    .ok_or("播放速率无效")?;
                let epoch = request
                    .get("epoch")
                    .and_then(Value::as_u64)
                    .ok_or("字幕时间轴无效")?;
                session
                    .input
                    .try_send(Input::Audio {
                        pcm,
                        position,
                        rate,
                        epoch,
                    })
                    .map_err(|_| "字幕音频发送拥堵或连接已关闭，请重试")?;
                Ok(json!({"ok": true}))
            }
            "reset" => {
                let epoch = request
                    .get("epoch")
                    .and_then(Value::as_u64)
                    .ok_or("字幕时间轴无效")?;
                session
                    .input
                    .try_send(Input::Reset(epoch))
                    .map_err(|_| "无法重置字幕连接")?;
                session.state.lock().unwrap().epoch = epoch;
                Ok(json!({"ok": true}))
            }
            "poll" => {
                let after = request
                    .get("after_sequence")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let state = session.state.lock().unwrap();
                // Chromium 的 Native Messaging 单条返回上限为 1 MiB。
                let events = state
                    .events
                    .iter()
                    .filter(|e| e["sequence"].as_u64().unwrap_or(0) > after)
                    .take(48)
                    .collect::<Vec<_>>();
                let delivered = events
                    .last()
                    .and_then(|e| e["sequence"].as_u64())
                    .unwrap_or(state.sequence);
                Ok(
                    json!({"ok": true, "status": state.status, "error": state.error,
                    "epoch": state.epoch, "latest_sequence": delivered, "events": events }),
                )
            }
            "stop" => {
                let session = sessions.remove(id).unwrap();
                session.state.lock().unwrap().status = "stopped";
                // 丢弃积压音频；关闭所有发送端也会唤醒接收循环。
                let _ = session.input.try_send(Input::Stop);
                Ok(json!({"ok": true, "status": "stopped"}))
            }
            _ => Err("不支持的字幕操作".into()),
        }
    }

    pub(crate) fn stop_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, session) in std::mem::take(&mut *sessions) {
                session.state.lock().unwrap().status = "stopped";
                let _ = session.input.try_send(Input::Stop);
            }
        }
    }
}

fn configuration(core: &CoreCoordinator) -> Result<Value, String> {
    let locked = core.lock()?;
    let raw = locked.store().setting_string(PROVIDER_SETTING, "{}")?;
    let value: Value = serde_json::from_str(&raw).map_err(|_| "字幕配置损坏")?;
    Ok(json!({"ok": true, "workspace": string(&value, "workspace"),
        "region": value.get("region").and_then(Value::as_str).unwrap_or("cn-beijing"),
        "has_key": locked.store().load_credential(KEY_REF)?.is_some() }))
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn setup(source: &str, target: &str) -> Value {
    let mut transcription = json!({"model": "qwen3-asr-flash-realtime"});
    if source != "auto" {
        transcription["language"] = json!(source);
    }
    json!({"event_id": "hls_setup", "type": "session.update", "session": {
        "modalities": ["text"], "sample_rate": 16_000, "input_audio_format": "pcm",
        "input_audio_transcription": transcription, "translation": {"language": target}
    }})
}

struct AudioMark {
    start_ms: f64,
    end_ms: f64,
    position: f64,
    rate: f64,
}

#[derive(Default)]
struct Decoder {
    marks: VecDeque<AudioMark>,
    audio_ms: f64,
    links: BTreeMap<String, String>,
    starts: BTreeMap<String, f64>,
    ends: BTreeMap<String, f64>,
}

impl Decoder {
    fn audio(&mut self, bytes: usize, position: f64, rate: f64) {
        let duration_ms = bytes as f64 / 32.0;
        self.marks.push_back(AudioMark {
            start_ms: self.audio_ms,
            end_ms: self.audio_ms + duration_ms,
            position,
            rate,
        });
        self.audio_ms += duration_ms;
        while self.marks.len() > 1200 {
            self.marks.pop_front();
        }
    }

    fn position(&self, audio_ms: f64) -> Option<f64> {
        let mark = self.marks.iter().rev().find(|m| m.start_ms <= audio_ms)?;
        Some(
            mark.position
                + (audio_ms.min(mark.end_ms) - mark.start_ms).max(0.0) / 1000.0 * mark.rate,
        )
    }

    fn decode(&mut self, event: &Value) -> Option<Value> {
        let kind = string(event, "type");
        let id = event
            .get("item_id")
            .and_then(Value::as_str)
            .or_else(|| event.pointer("/item/id").and_then(Value::as_str))
            .unwrap_or("");
        if kind == "conversation.item.created" {
            if let Some(previous) = event.get("previous_item_id").and_then(Value::as_str) {
                if event.pointer("/item/role").and_then(Value::as_str) == Some("assistant") {
                    self.links.insert(id.into(), previous.into());
                }
            }
        }
        if kind == "input_audio_buffer.speech_started" {
            if let Some(position) = event
                .get("audio_start_ms")
                .and_then(Value::as_f64)
                .and_then(|t| self.position(t))
            {
                self.starts.insert(id.into(), position);
            }
        }
        if kind == "input_audio_buffer.speech_stopped" {
            if let Some(position) = event
                .get("audio_end_ms")
                .and_then(Value::as_f64)
                .and_then(|t| self.position(t))
            {
                self.ends.insert(id.into(), position);
            }
        }
        let source = kind.starts_with("conversation.item.input_audio_transcription.");
        let translation =
            kind.starts_with("response.text.") || kind.starts_with("response.audio_transcript.");
        if !source && !translation {
            return None;
        }
        if !kind.ends_with(".text") && !kind.ends_with(".done") && !kind.ends_with(".completed") {
            return None;
        }
        // 服务端响应项通过 previous_item_id 指向原语音，不能按两个 final 的到达顺序配对。
        let utterance = if source {
            id
        } else {
            self.links.get(id).map(String::as_str).unwrap_or(id)
        }
        .to_string();
        if utterance.is_empty() {
            return None;
        }
        if utterance.len() > 160 {
            return None;
        }
        let final_text = kind.ends_with(".done") || kind.ends_with(".completed");
        let text = event
            .get("text")
            .or_else(|| event.get("transcript"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = if final_text {
            text.to_string()
        } else {
            format!("{}{}", text, string(event, "stash"))
        };
        let start = self
            .starts
            .get(&utterance)
            .copied()
            .or_else(|| self.marks.back().map(|m| m.position))
            .unwrap_or(0.0);
        let end = self
            .ends
            .get(&utterance)
            .copied()
            .unwrap_or(start + 5.0)
            .max(start + 0.5);
        // 服务端长会话会产生无限项 ID，只保留当前活跃窗口。
        if self.starts.len() > 256 {
            self.starts.clear();
            self.ends.clear();
            self.links.clear();
        }
        Some(
            json!({"type": "cue", "id": utterance, "role": if source { "source" } else { "translation" },
            "text": text.chars().take(4096).collect::<String>(), "final": final_text,
            "start": start, "end": end, "language": event.get("language").and_then(Value::as_str).map(|s| s.chars().take(16).collect::<String>()) }),
        )
    }
}

async fn run(
    endpoint: &str,
    key: &str,
    source: &str,
    target: &str,
    mut input: mpsc::Receiver<Input>,
    state: Arc<Mutex<State>>,
) -> Result<(), String> {
    let mut epoch = state.lock().unwrap().epoch;
    'connection: loop {
        if state.lock().unwrap().status == "stopped" {
            return Ok(());
        }
        state.lock().unwrap().status = "connecting";
        let mut request = endpoint
            .into_client_request()
            .map_err(|_| "字幕服务地址无效")?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {key}")
                .parse()
                .map_err(|_| "API Key 格式无效")?,
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(128 * 1024))
            .max_frame_size(Some(128 * 1024));
        let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(15), connect)
            .await
            .map_err(|_| "字幕连接超时，请检查网络")?
            .map_err(|_| "字幕服务连接失败，请检查业务空间、地域、API Key 和网络")?;
        send(&mut socket, setup(source, target)).await?;
        let ready = tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(frame) = socket.next().await {
                match frame.map_err(|_| "字幕连接中断")? {
                    Message::Text(text) => {
                        let event: Value =
                            serde_json::from_str(&text).map_err(|_| "字幕服务返回无效数据")?;
                        if string(&event, "type") == "session.updated" {
                            return Ok::<_, String>(());
                        }
                        if string(&event, "type") == "error" {
                            return Err(
                                "字幕服务拒绝会话配置，请检查 API Key、模型权限和语种".into()
                            );
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            Err("字幕服务在初始化时断开".into())
        })
        .await
        .map_err(|_| "字幕服务初始化超时")?;
        ready?;
        if state.lock().unwrap().status == "stopped" {
            let _ = socket.close(None).await;
            return Ok(());
        }
        state.lock().unwrap().status = "running";
        let mut decoder = Decoder::default();
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        let mut last_received = Instant::now();
        loop {
            tokio::select! {
                message = input.recv() => match message {
                    Some(Input::Audio { pcm, position, rate, epoch: audio_epoch }) => {
                        if state.lock().unwrap().status == "stopped" { let _ = socket.close(None).await; return Ok(()); }
                        if audio_epoch != epoch { continue; }
                        decoder.audio(pcm.len(), position, rate);
                        send(&mut socket, json!({"type": "input_audio_buffer.append", "audio": STANDARD.encode(pcm)})).await?;
                    }
                    Some(Input::Reset(next)) => {
                        epoch = next;
                        let _ = socket.close(None).await;
                        continue 'connection;
                    }
                    Some(Input::Stop) | None => {
                        let _ = send(&mut socket, json!({"type": "session.finish"})).await;
                        let _ = tokio::time::timeout(Duration::from_secs(2), async {
                            while let Some(Ok(Message::Text(text))) = socket.next().await {
                                if serde_json::from_str::<Value>(&text).ok().is_some_and(|v| string(&v, "type") == "session.finished") { break; }
                            }
                        }).await;
                        let _ = socket.close(None).await;
                        return Ok(());
                    }
                },
                frame = socket.next() => {
                    let frame = frame.ok_or("字幕连接已关闭，请重新开始")?.map_err(|_| "字幕连接中断，请重新开始")?;
                    last_received = Instant::now();
                    match frame {
                        Message::Text(text) => {
                            let event: Value = serde_json::from_str(&text).map_err(|_| "字幕服务返回无效数据")?;
                            if string(&event, "type") == "error" || string(&event, "type").ends_with(".failed") {
                                return Err("字幕服务处理失败，请检查额度、模型权限和输入音频".into());
                            }
                            if let Some(cue) = decoder.decode(&event) {
                                let mut state = state.lock().unwrap();
                                if state.epoch == epoch { state.push(cue); }
                            }
                        }
                        Message::Close(_) => return Err("字幕服务已断开，请重新开始".into()),
                        _ => {}
                    }
                },
                _ = heartbeat.tick() => {
                    if state.lock().unwrap().last_seen.elapsed() > Duration::from_secs(35) { return Ok(()); }
                    if last_received.elapsed() > Duration::from_secs(30) { return Err("字幕连接无响应，请重新开始".into()); }
                    tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Ping(Vec::new().into())))
                        .await.map_err(|_| "字幕心跳超时")?.map_err(|_| "字幕连接中断")?;
                }
            }
        }
    }
}

async fn send<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    message: Value,
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(message.to_string().into())),
    )
    .await
    .map_err(|_| "字幕音频发送超时")?
    .map_err(|_| "字幕音频发送失败".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_requests_cannot_change_another_owner_or_reset_a_full_queue() {
        let core = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        let service = SubtitleService::default();
        let state = Arc::new(Mutex::new(State {
            status: "running",
            error: String::new(),
            epoch: 7,
            sequence: 0,
            events: VecDeque::new(),
            last_seen: Instant::now(),
        }));
        let (input, mut receiver) = mpsc::channel(1);
        service.sessions.lock().unwrap().insert(
            "test".into(),
            Session {
                owner: "owner".into(),
                input,
                state: state.clone(),
            },
        );
        for action in ["audio", "reset", "poll", "stop"] {
            assert!(service
                .dispatch(
                    &core,
                    &json!({"action":action,"session_id":"test","owner":"other","epoch":8})
                )
                .is_err());
        }
        for audio in [
            "not-base64!".to_string(),
            STANDARD.encode([0]),
            STANDARD.encode(vec![0; 32_002]),
        ] {
            assert!(service
                .dispatch(
                    &core,
                    &json!({"action":"audio","session_id":"test","owner":"owner",
                "audio":audio,"position":0,"rate":1,"epoch":7})
                )
                .is_err());
        }
        service
            .dispatch(
                &core,
                &json!({"action":"audio","session_id":"test","owner":"owner",
            "audio":STANDARD.encode([0,0]),"position":120,"rate":2,"epoch":7}),
            )
            .unwrap();
        assert!(service
            .dispatch(
                &core,
                &json!({"action":"reset","session_id":"test","owner":"owner","epoch":8})
            )
            .is_err());
        assert_eq!(state.lock().unwrap().epoch, 7);
        assert!(
            matches!(receiver.try_recv().unwrap(), Input::Audio { position, epoch: 7, .. } if position == 120.0)
        );
        service
            .dispatch(
                &core,
                &json!({"action":"reset","session_id":"test","owner":"owner","epoch":8}),
            )
            .unwrap();
        assert!(matches!(receiver.try_recv().unwrap(), Input::Reset(8)));
        service
            .dispatch(
                &core,
                &json!({"action":"stop","session_id":"test","owner":"owner"}),
            )
            .unwrap();
        assert!(matches!(receiver.try_recv().unwrap(), Input::Stop));
        assert_eq!(state.lock().unwrap().status, "stopped");
    }

    #[test]
    fn long_unicode_cues_paginate_below_native_message_limit_without_loss() {
        let core = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        let service = SubtitleService::default();
        let mut state = State {
            status: "running",
            error: String::new(),
            epoch: 0,
            sequence: 0,
            events: VecDeque::new(),
            last_seen: Instant::now(),
        };
        for index in 0..100 {
            state.push(json!({"type":"cue","id":index.to_string(),"text":"😀".repeat(4096),"start":index,"end":index+1}));
        }
        let (input, _receiver) = mpsc::channel(1);
        service.sessions.lock().unwrap().insert(
            "test".into(),
            Session {
                owner: "owner".into(),
                input,
                state: Arc::new(Mutex::new(state)),
            },
        );
        let mut after = 0;
        let mut sequences = Vec::new();
        while after < 100 {
            let response = service.dispatch(&core, &json!({"action":"poll","session_id":"test","owner":"owner","after_sequence":after})).unwrap();
            assert!(serde_json::to_vec(&response).unwrap().len() < 1024 * 1024);
            sequences.extend(
                response["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|event| event["sequence"].as_u64().unwrap()),
            );
            after = response["latest_sequence"].as_u64().unwrap();
        }
        assert_eq!(sequences, (1..=100).collect::<Vec<_>>());
    }

    #[test]
    fn websocket_disconnect_and_invalid_payload_fail_without_fabricated_cues() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for terminal in [Message::Close(None), Message::Text("invalid JSON".into())] {
                tokio::time::timeout(Duration::from_secs(5), async {
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let endpoint = format!("ws://{}/realtime", listener.local_addr().unwrap());
                    let provider = tokio::spawn(async move {
                        let (tcp, _) = listener.accept().await.unwrap();
                        let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
                        assert!(matches!(
                            socket.next().await.unwrap().unwrap(),
                            Message::Text(_)
                        ));
                        socket
                            .send(Message::Text(
                                json!({"type":"session.updated"}).to_string().into(),
                            ))
                            .await
                            .unwrap();
                        while let Some(Ok(frame)) = socket.next().await {
                            if matches!(frame, Message::Text(_)) {
                                break;
                            }
                        }
                        socket.send(terminal).await.unwrap();
                    });
                    let state = Arc::new(Mutex::new(State {
                        status: "connecting",
                        error: String::new(),
                        epoch: 0,
                        sequence: 0,
                        events: VecDeque::new(),
                        last_seen: Instant::now(),
                    }));
                    let (tx, rx) = mpsc::channel(1);
                    tx.send(Input::Audio {
                        pcm: vec![0; 8000],
                        position: 0.0,
                        rate: 1.0,
                        epoch: 0,
                    })
                    .await
                    .unwrap();
                    assert!(run(&endpoint, "test", "auto", "zh", rx, state.clone())
                        .await
                        .is_err());
                    assert!(state.lock().unwrap().events.is_empty());
                    provider.await.unwrap();
                })
                .await
                .expect("failure boundary timed out");
            }
        });
    }

    #[test]
    fn source_and_translation_are_paired_by_item_at_nonzero_seek_and_rate() {
        let mut d = Decoder::default();
        d.audio(8000, 120.0, 2.0);
        d.audio(8000, 120.5, 2.0);
        d.decode(&json!({"type":"input_audio_buffer.speech_started","item_id":"speech","audio_start_ms":100}));
        d.decode(&json!({"type":"input_audio_buffer.speech_stopped","item_id":"speech","audio_end_ms":400}));
        d.decode(&json!({"type":"conversation.item.created","item":{"id":"answer","role":"assistant"},"previous_item_id":"speech"}));
        let t = d
            .decode(&json!({"type":"response.text.done","item_id":"answer","text":"你好"}))
            .unwrap();
        let s = d.decode(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"speech","transcript":"hello"})).unwrap();
        assert_eq!(t["id"], s["id"]);
        assert_eq!(t["start"], json!(120.2));
        assert_eq!(t["end"], json!(120.8));
    }

    #[test]
    #[cfg(windows)]
    fn configuration_does_not_return_the_key_and_clear_removes_it() {
        let service = SubtitleService::default();
        let core = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        let configured = service.dispatch(&core, &json!({"action":"configure","workspace":"test-space","region":"cn-beijing","api_key":"secret-for-test"})).unwrap();
        assert_eq!(configured["has_key"], true);
        assert!(!configured.to_string().contains("secret-for-test"));
        assert!(core
            .load_credential(KEY_REF)
            .unwrap()
            .unwrap()
            .starts_with("dpapi:"));
        let cleared = service.dispatch(&core, &json!({"action":"configure","workspace":"test-space","region":"cn-beijing","clear_key":true})).unwrap();
        assert_eq!(cleared["has_key"], false);
    }

    #[test]
    fn websocket_pcm_translation_and_seek_reset_use_real_transport() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(8), async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("ws://{}/realtime", listener.local_addr().unwrap());
                let provider = tokio::spawn(async move {
                    for epoch in 0..2 {
                        let (tcp, _) = listener.accept().await.unwrap();
                        let mut socket = tokio_tungstenite::accept_hdr_async(tcp, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                            assert_eq!(request.headers()["Authorization"], "Bearer test-secret");
                            Ok(response)
                        }).await.unwrap();
                        let Message::Text(text) = socket.next().await.unwrap().unwrap() else { panic!("missing setup"); };
                        let update: Value = serde_json::from_str(&text).unwrap();
                        assert!(update.pointer("/session/input_audio_transcription/language").is_none());
                        assert_eq!(update.pointer("/session/translation/language").unwrap(), "zh");
                        socket.send(Message::Text(json!({"type":"session.updated"}).to_string().into())).await.unwrap();
                        loop {
                            let frame = socket.next().await.unwrap().unwrap();
                            if let Message::Text(text) = frame {
                                let message: Value = serde_json::from_str(&text).unwrap();
                                assert_eq!(message["type"], "input_audio_buffer.append");
                                assert_eq!(STANDARD.decode(string(&message,"audio")).unwrap(), vec![0; 8000]);
                                break;
                            }
                        }
                        for event in [
                            json!({"type":"input_audio_buffer.speech_started","item_id":"source","audio_start_ms":0}),
                            json!({"type":"input_audio_buffer.speech_stopped","item_id":"source","audio_end_ms":250}),
                            json!({"type":"conversation.item.created","item":{"id":"target","role":"assistant"},"previous_item_id":"source"}),
                            json!({"type":"response.text.done","item_id":"target","text":"真实链路"}),
                            json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"source","transcript":"transport"}),
                        ] { socket.send(Message::Text(event.to_string().into())).await.unwrap(); }
                        if epoch == 0 {
                            while let Some(Ok(frame)) = socket.next().await { if matches!(frame, Message::Close(_)) { break; } }
                        } else {
                            loop {
                                if let Some(Ok(Message::Text(text))) = socket.next().await {
                                    let event: Value = serde_json::from_str(&text).unwrap();
                                    if event["type"] == "session.finish" { break; }
                                }
                            }
                            socket.send(Message::Text(json!({"type":"session.finished"}).to_string().into())).await.unwrap();
                        }
                    }
                });
                let state = Arc::new(Mutex::new(State { status:"connecting", error:String::new(), epoch:0, sequence:0, events:VecDeque::new(), last_seen:Instant::now() }));
                let (tx, rx) = mpsc::channel(16);
                let worker_state = state.clone();
                let worker = tokio::spawn(async move { run(&endpoint,"test-secret","auto","zh",rx,worker_state).await });
                for epoch in 0..2 {
                    if epoch > 0 {
                        state.lock().unwrap().epoch = epoch;
                        tx.send(Input::Reset(epoch)).await.unwrap();
                    }
                    tx.send(Input::Audio { pcm:vec![0;8000], position:if epoch == 0 { 10.0 } else { 120.0 }, rate:1.0, epoch }).await.unwrap();
                    loop {
                        let received = {
                            let state = state.lock().unwrap();
                            state.events.iter().filter(|e| e["epoch"] == epoch).cloned().collect::<Vec<_>>()
                        };
                        if received.len() >= 2 {
                            assert_eq!(received[0]["text"], "真实链路");
                            assert_eq!(received[0]["id"], received[1]["id"]);
                            assert_eq!(received[0]["start"], if epoch == 0 { 10.0 } else { 120.0 });
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                }
                tx.send(Input::Stop).await.unwrap();
                worker.await.unwrap().unwrap();
                provider.await.unwrap();
            }).await.expect("subtitle transport test timed out");
        });
    }
}
