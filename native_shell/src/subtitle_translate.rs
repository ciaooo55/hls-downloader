//! 在线字幕只保留内存会话；API Key 经 DPAPI 存入 Core 的凭据库。
use crate::{CoreCoordinator, CredentialVault};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};

const KEY_REF: &str = "subtitle:dashscope";
const PROVIDER_SETTING: &str = "subtitle_provider";
const DASHSCOPE: &str = "dashscope";
const QWENCLOUD: &str = "qwencloud";
const MODEL: &str = "qwen3.5-livetranslate-flash-realtime";
const QWENCLOUD_ENDPOINT: &str = "wss://maas.qwencloudapi.com/api-ws/v1/realtime";
const LANGUAGES: &[&str] = &[
    "zh", "en", "ja", "ko", "fr", "de", "es", "ru", "pt", "it", "ar", "id", "vi", "th",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct SubtitlePreferences {
    enabled: bool,
    source: String,
    target: String,
    offset: f64,
    show_source: bool,
    prefetch: bool,
}

impl Default for SubtitlePreferences {
    fn default() -> Self {
        Self {
            enabled: false,
            source: "auto".into(),
            target: "zh".into(),
            offset: 0.0,
            show_source: true,
            prefetch: false,
        }
    }
}

#[derive(Default)]
pub(crate) struct SubtitleService {
    sessions: Mutex<BTreeMap<String, Session>>,
}

struct Session {
    owner: String,
    input: mpsc::Sender<Input>,
    stop: Arc<Notify>,
    state: Arc<Mutex<State>>,
    audio_source: Option<Value>,
}

enum Input {
    Audio {
        pcm: Vec<u8>,
        position: f64,
        rate: f64,
        epoch: u64,
    },
    Reset(u64),
    Commit {
        start: f64,
        end: f64,
        epoch: u64,
    },
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
                let provider = match string(request, "provider") {
                    "" | DASHSCOPE => DASHSCOPE,
                    QWENCLOUD => QWENCLOUD,
                    _ => return Err("不支持的字幕服务提供方".into()),
                };
                let workspace = string(request, "workspace");
                let region = string(request, "region");
                if provider == DASHSCOPE {
                    if workspace.is_empty()
                        || workspace.len() > 128
                        || !workspace
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                    {
                        return Err("请填写有效的百炼业务空间 ID".into());
                    }
                    if !matches!(region, "cn-beijing" | "ap-southeast-1") {
                        return Err("不支持的服务地域".into());
                    }
                }
                let key = string(request, "api_key").trim();
                if key.len() > 4096 || key.contains(['\r', '\n']) {
                    return Err("API Key 格式无效".into());
                }
                let preferences = request.get("preferences").map(|value| {
                    let preferences: SubtitlePreferences = serde_json::from_value(value.clone())
                        .map_err(|_| "字幕选项格式无效")?;
                    if (preferences.source != "auto" && !LANGUAGES.contains(&preferences.source.as_str()))
                        || !LANGUAGES.contains(&preferences.target.as_str())
                    {
                        return Err("字幕语种不受支持");
                    }
                    if !preferences.offset.is_finite() || preferences.offset.abs() > 30.0 {
                        return Err("字幕偏移必须在 -30 到 30 秒之间");
                    }
                    serde_json::to_value(preferences).map_err(|_| "字幕选项格式无效")
                }).transpose()?;
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
                let mut settings = json!({"provider": provider});
                let previous: Value = serde_json::from_str(
                    &locked.store().setting_string(PROVIDER_SETTING, "{}")?
                ).map_err(|_| "字幕配置损坏")?;
                if let Some(preferences) = preferences.or_else(|| previous.get("preferences").cloned()) {
                    settings["preferences"] = preferences;
                }
                if provider == DASHSCOPE {
                    settings["workspace"] = json!(workspace);
                    settings["region"] = json!(region);
                }
                locked.store_mut().apply_credential_with_settings(
                    write,
                    &BTreeMap::from([(PROVIDER_SETTING.into(), json!(settings.to_string()))]),
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
            let provider = string(&config, "provider");
            let workspace = string(&config, "workspace");
            if provider == DASHSCOPE && workspace.is_empty() {
                return Err("请先在主程序“设置 → 字幕翻译”中配置百炼业务空间和 API Key".into());
            }
            let key = core
                .load_credential(KEY_REF)?
                .ok_or("请先在主程序“设置 → 字幕翻译”中配置 API Key")?;
            let key = CredentialVault.unprotect(&key)?;
            if key.is_empty() {
                return Err("请先在主程序“设置 → 字幕翻译”中配置 API Key".into());
            }
            let endpoint = realtime_endpoint(provider, workspace, string(&config, "region"))?;
            let epoch = request.get("epoch").and_then(Value::as_u64).unwrap_or(0);
            let audio_source = request
                .get("audio_source")
                .filter(|v| v.is_object())
                .cloned();
            if let Some(source) = &audio_source {
                crate::subtitle_audio_source::validate(source)?;
            }
            let manual = audio_source.is_some();
            let state = Arc::new(Mutex::new(State {
                status: "connecting",
                error: String::new(),
                epoch,
                sequence: 0,
                events: VecDeque::new(),
                last_seen: Instant::now(),
            }));
            let (input, receiver) = mpsc::channel(16);
            let stop = Arc::new(Notify::new());
            sessions.insert(
                id.into(),
                Session {
                    owner: owner.into(),
                    input,
                    stop: stop.clone(),
                    state: state.clone(),
                    audio_source,
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
                            stop,
                            state.clone(),
                            manual,
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
            "prepare_audio" => {
                let source = session
                    .audio_source
                    .clone()
                    .ok_or("当前会话使用播放器实时采集")?;
                let position = request["position"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or("音轨预取时间无效")?;
                let seconds = request["duration"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v > 0.0 && *v <= 12.0)
                    .ok_or("音轨预取窗口必须在 12 秒以内")?;
                let epoch = request["epoch"].as_u64().ok_or("字幕时间轴无效")?;
                let state = session.state.clone();
                drop(sessions);
                crate::subtitle_audio_source::decode(&source, position, seconds, || {
                    let state = state.lock().unwrap();
                    state.epoch != epoch
                        || matches!(state.status, "stopping" | "stopped" | "failed")
                        || state.last_seen.elapsed() > Duration::from_secs(35)
                })
            }
            "commit" => {
                if session.audio_source.is_none() {
                    return Err("实时采集由服务端断句".into());
                }
                let start = request["start"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or("字幕窗口起点无效")?;
                let end = request["end"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v > start && *v <= start + 12.01)
                    .ok_or("字幕窗口终点无效")?;
                let epoch = request["epoch"].as_u64().ok_or("字幕时间轴无效")?;
                session
                    .input
                    .try_send(Input::Commit { start, end, epoch })
                    .map_err(|_| "字幕提交队列繁忙")?;
                Ok(json!({"ok": true}))
            }
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
                let mut state = session.state.lock().unwrap();
                state.epoch = epoch;
                state.status = "connecting";
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
                let session = sessions.get(id).unwrap();
                let mut state = session.state.lock().map_err(|_| "字幕状态不可用")?;
                if !matches!(state.status, "stopped" | "failed") {
                    state.status = "stopping";
                    session.stop.notify_one();
                }
                Ok(json!({"ok": true, "status": state.status}))
            }
            _ => Err("不支持的字幕操作".into()),
        }
    }

    pub(crate) fn stop_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, session) in std::mem::take(&mut *sessions) {
                if let Ok(mut state) = session.state.lock() {
                    if !matches!(state.status, "stopped" | "failed") {
                        state.status = "stopping";
                        session.stop.notify_one();
                    }
                }
            }
        }
    }
}

fn configuration(core: &CoreCoordinator) -> Result<Value, String> {
    let locked = core.lock()?;
    let raw = locked.store().setting_string(PROVIDER_SETTING, "{}")?;
    let value: Value = serde_json::from_str(&raw).map_err(|_| "字幕配置损坏")?;
    let provider = match string(&value, "provider") {
        "" | DASHSCOPE => DASHSCOPE,
        QWENCLOUD => QWENCLOUD,
        _ => return Err("字幕配置损坏".into()),
    };
    Ok(
        json!({"ok": true, "provider": provider, "workspace": string(&value, "workspace"),
        "region": value.get("region").and_then(Value::as_str).unwrap_or("cn-beijing"),
        "preferences": value.get("preferences"),
        "has_key": locked.store().load_credential(KEY_REF)?.is_some() }),
    )
}

fn realtime_endpoint(provider: &str, workspace: &str, region: &str) -> Result<String, String> {
    let base = match provider {
        DASHSCOPE if !workspace.is_empty() && matches!(region, "cn-beijing" | "ap-southeast-1") => {
            format!("wss://{workspace}.{region}.maas.aliyuncs.com/api-ws/v1/realtime")
        }
        QWENCLOUD => QWENCLOUD_ENDPOINT.into(),
        _ => return Err("字幕服务配置无效".into()),
    };
    Ok(format!("{base}?model={MODEL}"))
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
    // 提前模式一次只提交一个窗口；只有对应 response.done 后才能推进就绪范围。
    pending_window: Option<(f64, f64)>,
    window_items: BTreeMap<String, (f64, f64)>,
    response_id: String,
    uncommitted: Option<(f64, f64)>,
    manual: bool,
}

impl Decoder {
    fn manual_audio(&mut self, bytes: usize, position: f64, rate: f64) -> Result<(), String> {
        if self.pending_window.is_some()
            || rate != 1.0
            || self
                .uncommitted
                .is_some_and(|(_, end)| (position - end).abs() > 0.001)
        {
            return Err("提前音频时间轴不连续或上一窗口尚未完成".into());
        }
        let start = self.uncommitted.map_or(position, |v| v.0);
        self.uncommitted = Some((start, position + bytes as f64 / 32000.0));
        self.audio(bytes, position, rate);
        Ok(())
    }

    fn commit_window(&mut self, start: f64, end: f64) -> Result<(), String> {
        if self.pending_window.is_some()
            || !self
                .uncommitted
                .is_some_and(|(s, e)| (s - start).abs() < 0.001 && (e - end).abs() < 0.001)
        {
            return Err("字幕提交范围与实际音频不一致".into());
        }
        self.uncommitted = None;
        self.pending_window = Some((start, end));
        Ok(())
    }
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
        // QwenCloud 手动提交实测不会创建 user 项，音频项 ID 由 committed 确认。
        if self.manual && kind == "input_audio_buffer.committed" && !id.is_empty() {
            if let Some(window) = self.pending_window {
                self.window_items.insert(id.into(), window);
            }
        }
        if kind == "conversation.item.created" {
            if self.manual && event.pointer("/item/role").and_then(Value::as_str) == Some("user") {
                if let Some(window) = self.pending_window {
                    self.window_items.insert(id.into(), window);
                }
            }
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
        let window = if self.manual {
            Some(*self.window_items.get(&utterance)?)
        } else {
            None
        };
        let utterance = if let Some((start, _)) = window {
            format!("window-{start:.6}")
        } else {
            utterance
        };
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
            .or_else(|| window.map(|v| v.0))
            .or_else(|| self.marks.back().map(|m| m.position))
            .unwrap_or(0.0);
        let end = self
            .ends
            .get(&utterance)
            .copied()
            .or_else(|| window.map(|v| v.1))
            .unwrap_or(start + 5.0)
            .max(start + 0.5);
        // 服务端长会话会产生无限项 ID，只保留当前活跃窗口。
        if self.starts.len() > 256 {
            self.starts.clear();
            self.ends.clear();
            self.links.clear();
        }
        if self.window_items.len() > 256 {
            self.window_items
                .retain(|_, v| Some(*v) == self.pending_window);
        }
        Some(
            json!({"type": "cue", "id": utterance, "role": if source { "source" } else { "translation" },
            "text": text.chars().take(4096).collect::<String>(), "final": final_text,
            "start": start, "end": end, "language": event.get("language").and_then(Value::as_str).map(|s| s.chars().take(16).collect::<String>()) }),
        )
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    endpoint: &str,
    key: &str,
    source: &str,
    target: &str,
    mut input: mpsc::Receiver<Input>,
    stop: Arc<Notify>,
    state: Arc<Mutex<State>>,
    manual: bool,
) -> Result<(), String> {
    let mut epoch = state.lock().unwrap().epoch;
    'connection: loop {
        {
            let mut state = state.lock().unwrap();
            if matches!(state.status, "stopped" | "stopping") {
                return Ok(());
            }
            state.status = "connecting";
        }
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
        let connected = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(15), connect) => result,
            _ = stop.notified() => return Ok(()),
        };
        let (mut socket, _) = connected
            .map_err(|_| "字幕连接超时，请检查网络")?
            .map_err(|_| "字幕服务连接失败，请检查服务配置、API Key 和网络")?;
        let mut configuration = setup(source, target);
        if manual {
            configuration["session"]["turn_detection"] = Value::Null;
        }
        send(&mut socket, configuration).await?;
        let ready_result = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(8), async {
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
            }) => result,
            _ = stop.notified() => { let _ = socket.close(None).await; return Ok(()) },
        };
        let ready = ready_result.map_err(|_| "字幕服务初始化超时")?;
        ready?;
        if state.lock().unwrap().status == "stopped" {
            let _ = socket.close(None).await;
            return Ok(());
        }
        {
            let mut session_state = state.lock().unwrap();
            if session_state.status != "stopping" {
                session_state.status = "running";
            }
        }
        let mut decoder = Decoder {
            manual,
            ..Decoder::default()
        };
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        let mut last_received = Instant::now();
        loop {
            if state.lock().unwrap().status == "stopping" && input.is_empty() {
                // 预取尾部是尚未提交的未来音频；停止时丢弃，避免结束会话又生成一个无时间范围的响应。
                if manual && decoder.uncommitted.take().is_some() {
                    send(&mut socket, json!({"type": "input_audio_buffer.clear"})).await?;
                }
                send(&mut socket, json!({"type": "session.finish"})).await?;
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let frame = socket
                            .next()
                            .await
                            .ok_or("字幕服务在结束会话时断开")?
                            .map_err(|_| "字幕连接中断")?;
                        let Message::Text(text) = frame else { continue };
                        let event: Value =
                            serde_json::from_str(&text).map_err(|_| "字幕服务返回无效数据")?;
                        if string(&event, "type") == "session.finished" {
                            return Ok::<_, String>(());
                        }
                        accept_event(&event, &mut decoder, &state, epoch)?;
                    }
                })
                .await
                .map_err(|_| "字幕结束处理超时")??;
                let _ = socket.close(None).await;
                return Ok(());
            }
            tokio::select! {
                message = input.recv() => match message {
                    Some(Input::Audio { pcm, position, rate, epoch: audio_epoch }) => {
                        if state.lock().unwrap().status == "stopped" { let _ = socket.close(None).await; return Ok(()); }
                        if audio_epoch != epoch { continue; }
                        if manual { decoder.manual_audio(pcm.len(), position, rate)?; }
                        else { decoder.audio(pcm.len(), position, rate); }
                        send(&mut socket, json!({"type": "input_audio_buffer.append", "audio": STANDARD.encode(pcm)})).await?;
                    }
                    Some(Input::Reset(next)) => {
                        epoch = next;
                        let _ = socket.close(None).await;
                        continue 'connection;
                    }
                    Some(Input::Commit { start, end, epoch: commit_epoch }) => {
                        if commit_epoch != epoch { continue; }
                        decoder.commit_window(start, end)?;
                        send(&mut socket, json!({"type": "input_audio_buffer.commit"})).await?;
                    }
                    None => {},
                },
                _ = stop.notified() => {},
                frame = socket.next() => {
                    let frame = frame.ok_or("字幕连接已关闭，请重新开始")?.map_err(|_| "字幕连接中断，请重新开始")?;
                    last_received = Instant::now();
                    match frame {
                        Message::Text(text) => {
                            let event: Value = serde_json::from_str(&text).map_err(|_| "字幕服务返回无效数据")?;
                            accept_event(&event, &mut decoder, &state, epoch)?;
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

fn accept_event(
    event: &Value,
    decoder: &mut Decoder,
    state: &Arc<Mutex<State>>,
    epoch: u64,
) -> Result<(), String> {
    let kind = string(event, "type");
    if kind == "error" || kind.ends_with(".failed") {
        return Err("字幕服务处理失败，请检查额度、模型权限和输入音频".into());
    }
    // 服务端在收到流式音频后即可创建响应，不会等客户端提交整个窗口。
    if decoder.manual
        && kind == "response.created"
        && (decoder.pending_window.is_some() || decoder.uncommitted.is_some())
    {
        decoder.response_id = event
            .pointer("/response/id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
    }
    if let Some(cue) = decoder.decode(event) {
        let mut state = state.lock().unwrap();
        if state.epoch == epoch {
            state.push(cue);
        }
    }
    if decoder.manual && kind == "response.done" {
        if decoder.response_id.is_empty()
            || event.pointer("/response/id").and_then(Value::as_str)
                != Some(decoder.response_id.as_str())
        {
            return Err("字幕服务响应无法对应当前音频窗口".into());
        }
        if event.pointer("/response/status").and_then(Value::as_str) != Some("completed") {
            return Err("字幕窗口翻译未完成，请重新开始".into());
        }
        if let Some((start, end)) = decoder.pending_window {
            // 最终响应内的文本也是有效结果，不能依赖流式 text.done 一定先到达。
            let text = event
                .pointer("/response/output")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item["content"].as_array())
                .flatten()
                .filter_map(|part| {
                    part["text"]
                        .as_str()
                        .or_else(|| part["transcript"].as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut state = state.lock().unwrap();
            if state.epoch == epoch {
                if !text.is_empty() {
                    state.push(json!({"type":"cue", "id":format!("window-{start:.6}"),
                    "role":"translation", "text":text.chars().take(4096).collect::<String>(),
                    "final":true, "start":start, "end":end}));
                }
                state.push(json!({"type": "ready", "start": start, "end": end}));
            }
            decoder.pending_window = None;
            decoder.response_id.clear();
        }
    }
    Ok(())
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
    fn manual_stop_discards_uncommitted_future_audio_before_finish() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
                    let appended = Arc::new(Notify::new());
                    let received = appended.clone();
                    let provider = tokio::spawn(async move {
                        let (tcp, _) = listener.accept().await.unwrap();
                        let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
                        let mut types = Vec::new();
                        while let Some(Ok(frame)) = socket.next().await {
                            let Message::Text(text) = frame else { continue };
                            let event: Value = serde_json::from_str(&text).unwrap();
                            let kind = string(&event, "type").to_string();
                            types.push(kind.clone());
                            match kind.as_str() {
                                "session.update" => {
                                    assert!(event["session"]["turn_detection"].is_null());
                                    socket
                                        .send(Message::Text(
                                            json!({"type":"session.updated"}).to_string().into(),
                                        ))
                                        .await
                                        .unwrap();
                                }
                                "input_audio_buffer.append" => received.notify_one(),
                                "session.finish" => {
                                    assert_eq!(
                                        &types[types.len() - 2..],
                                        &["input_audio_buffer.clear", "session.finish"]
                                    );
                                    socket
                                        .send(Message::Text(
                                            json!({"type":"session.finished"}).to_string().into(),
                                        ))
                                        .await
                                        .unwrap();
                                    break;
                                }
                                _ => {}
                            }
                        }
                    });
                    let state = Arc::new(Mutex::new(State {
                        status: "connecting",
                        error: String::new(),
                        epoch: 0,
                        sequence: 0,
                        events: VecDeque::new(),
                        last_seen: Instant::now(),
                    }));
                    let (tx, rx) = mpsc::channel(16);
                    let stop = Arc::new(Notify::new());
                    let worker_state = state.clone();
                    let worker_stop = stop.clone();
                    let worker = tokio::spawn(async move {
                        run(
                            &endpoint,
                            "test",
                            "en",
                            "zh",
                            rx,
                            worker_stop,
                            worker_state,
                            true,
                        )
                        .await
                    });
                    tx.send(Input::Audio {
                        pcm: vec![0; 8000],
                        position: 20.0,
                        rate: 1.0,
                        epoch: 0,
                    })
                    .await
                    .unwrap();
                    appended.notified().await;
                    state.lock().unwrap().status = "stopping";
                    stop.notify_one();
                    worker.await.unwrap().unwrap();
                    provider.await.unwrap();
                    assert!(state.lock().unwrap().events.is_empty());
                })
                .await
                .expect("manual stop timed out");
            });
    }

    #[test]
    fn prepared_window_requires_contiguous_original_audio_and_completed_translation() {
        let state = Arc::new(Mutex::new(State {
            status: "running",
            error: String::new(),
            epoch: 2,
            sequence: 0,
            events: VecDeque::new(),
            last_seen: Instant::now(),
        }));
        let mut decoder = Decoder {
            manual: true,
            ..Decoder::default()
        };
        decoder.manual_audio(8000, 10.0, 1.0).unwrap();
        assert!(decoder.manual_audio(8000, 10.5, 1.0).is_err());
        assert!(decoder.manual_audio(8000, 10.25, 2.0).is_err());
        accept_event(
            &json!({"type":"response.created", "response":{"id":"response-1"}}),
            &mut decoder,
            &state,
            2,
        )
        .unwrap();
        decoder.manual_audio(8000, 10.25, 1.0).unwrap();
        assert!(decoder.commit_window(10.0, 11.0).is_err());
        decoder.commit_window(10.0, 10.5).unwrap();
        assert!(decoder.manual_audio(8000, 10.5, 1.0).is_err());
        for event in [
            json!({"type":"conversation.item.input_audio_transcription.text", "item_id":"input", "text":"partial"}),
            json!({"type":"conversation.item.created", "item":{"id":"output", "role":"assistant"}, "previous_item_id":"input"}),
            json!({"type":"input_audio_buffer.committed", "item_id":"input"}),
            json!({"type":"conversation.item.input_audio_transcription.completed", "item_id":"input", "transcript":"source"}),
            json!({"type":"response.text.done", "item_id":"output", "text":"译文"}),
        ] {
            accept_event(&event, &mut decoder, &state, 2).unwrap();
        }
        assert!(state
            .lock()
            .unwrap()
            .events
            .iter()
            .all(|e| e["type"] != "ready"));
        accept_event(
            &json!({"type":"response.done", "response":{"id":"response-1", "status":"completed"}}),
            &mut decoder,
            &state,
            2,
        )
        .unwrap();
        let locked = state.lock().unwrap();
        let ready = locked.events.back().unwrap();
        assert_eq!(ready["type"], "ready");
        assert_eq!(ready["start"], 10.0);
        assert_eq!(ready["end"], 10.5);
        assert!(locked
            .events
            .iter()
            .filter(|e| e["type"] == "cue")
            .all(|e| e["start"] == 10.0 && e["end"] == 10.5));
        drop(locked);
        assert!(decoder.response_id.is_empty());
        decoder.manual_audio(8000, 10.5, 1.0).unwrap();
        decoder.commit_window(10.5, 10.75).unwrap();
        accept_event(
            &json!({"type":"response.created", "response":{"id":"response-2"}}),
            &mut decoder,
            &state,
            2,
        )
        .unwrap();
        accept_event(
            &json!({"type":"response.done", "response":{"id":"response-2", "status":"completed"}}),
            &mut decoder,
            &state,
            2,
        )
        .unwrap();
        assert_eq!(state.lock().unwrap().events.back().unwrap()["end"], 10.75);
    }

    #[test]
    fn incomplete_or_stale_responses_never_mark_a_window_ready() {
        for (response_id, status, event_epoch) in [
            ("wrong", "completed", 1),
            ("current", "failed", 1),
            ("current", "completed", 0),
        ] {
            let state = Arc::new(Mutex::new(State {
                status: "running",
                error: String::new(),
                epoch: 1,
                sequence: 0,
                events: VecDeque::new(),
                last_seen: Instant::now(),
            }));
            let mut decoder = Decoder {
                manual: true,
                ..Decoder::default()
            };
            decoder.manual_audio(8000, 0.0, 1.0).unwrap();
            decoder.commit_window(0.0, 0.25).unwrap();
            decoder.response_id = "current".into();
            let result = accept_event(
                &json!({"type":"response.done", "response":{"id":response_id, "status":status}}),
                &mut decoder,
                &state,
                event_epoch,
            );
            if event_epoch == 1 {
                assert!(result.is_err());
            }
            assert!(state.lock().unwrap().events.is_empty());
        }
    }

    #[test]
    fn rustls_client_config_uses_a_crypto_provider() {
        let _config = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }

    #[test]
    fn provider_configuration_validation_and_endpoints_are_provider_specific() {
        let service = SubtitleService::default();
        let core = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        assert_eq!(configuration(&core).unwrap()["provider"], DASHSCOPE);
        assert!(realtime_endpoint(DASHSCOPE, "space", "cn-beijing")
            .unwrap()
            .starts_with("wss://space.cn-beijing.maas.aliyuncs.com/"));
        assert_eq!(
            realtime_endpoint(QWENCLOUD, "", "").unwrap(),
            format!("{QWENCLOUD_ENDPOINT}?model={MODEL}")
        );
        assert!(realtime_endpoint("unknown", "space", "cn-beijing").is_err());

        assert!(service
            .dispatch(&core, &json!({"action":"configure","provider":"unknown"}))
            .is_err());
        assert!(service
            .dispatch(&core, &json!({"action":"configure","provider":"dashscope","workspace":"","region":"cn-beijing"}))
            .is_err());
        assert!(service
            .dispatch(&core, &json!({"action":"configure","provider":"dashscope","workspace":"space","region":"unknown"}))
            .is_err());
        let configured = service
            .dispatch(&core, &json!({"action":"configure","provider":"qwencloud"}))
            .unwrap();
        assert_eq!(configured["provider"], QWENCLOUD);
        assert_eq!(configured["workspace"], "");
        assert!(service
            .dispatch(&core, &json!({"action":"configure","provider":"dashscope","workspace":"","region":"cn-beijing"}))
            .is_err());
        assert_eq!(configuration(&core).unwrap()["provider"], QWENCLOUD);
    }

    #[test]
    fn desktop_subtitle_preferences_persist_and_invalid_edits_do_not_replace_them() {
        let service = SubtitleService::default();
        let core = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        let preferences = json!({"enabled": true, "source": "ja", "target": "zh",
            "offset": -0.4, "showSource": false, "prefetch": true});
        let saved = service.dispatch(&core, &json!({"action": "configure",
            "provider": "qwencloud", "preferences": preferences})).unwrap();
        assert_eq!(saved["preferences"], preferences);
        assert!(saved.get("api_key").is_none());
        for invalid in [json!({"target": "unsupported"}), json!({"offset": 31}),
            json!({"enabled": "true"}), json!({"api_key": "must-not-be-stored-here"})] {
            assert!(service.dispatch(&core, &json!({"action": "configure", "provider": "qwencloud",
                "preferences": invalid})).is_err());
            assert_eq!(configuration(&core).unwrap()["preferences"], preferences);
        }
        service.dispatch(&core, &json!({"action": "configure", "provider": "qwencloud"})).unwrap();
        assert_eq!(configuration(&core).unwrap()["preferences"], preferences);
    }

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
        let stop = Arc::new(Notify::new());
        service.sessions.lock().unwrap().insert(
            "test".into(),
            Session {
                owner: "owner".into(),
                input,
                stop: stop.clone(),
                state: state.clone(),
                audio_source: None,
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
                &json!({"action":"audio","session_id":"test","owner":"owner",
            "audio":STANDARD.encode([0,0]),"position":121,"rate":1,"epoch":8}),
            )
            .unwrap();
        state
            .lock()
            .unwrap()
            .push(json!({"type":"cue","text":"尾部字幕"}));
        service
            .dispatch(
                &core,
                &json!({"action":"stop","session_id":"test","owner":"owner"}),
            )
            .unwrap();
        assert_eq!(state.lock().unwrap().status, "stopping");
        assert_eq!(service.sessions.lock().unwrap().len(), 1);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Input::Audio { epoch: 8, .. }
        ));
        let polled = service
            .dispatch(
                &core,
                &json!({"action":"poll","session_id":"test","owner":"owner","after_sequence":0}),
            )
            .unwrap();
        assert_eq!(polled["events"][0]["text"], "尾部字幕");
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
                stop: Arc::new(Notify::new()),
                state: Arc::new(Mutex::new(state)),
                audio_source: None,
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
                    assert!(run(
                        &endpoint,
                        "test",
                        "auto",
                        "zh",
                        rx,
                        Arc::new(Notify::new()),
                        state.clone(),
                        false
                    )
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
        let partial = d
            .decode(&json!({"type":"response.text.text","item_id":"answer","text":"你好"}))
            .unwrap();
        assert_eq!(partial["text"], "你好");
        assert_eq!(partial["final"], false);
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
        let configured = service
            .dispatch(
                &core,
                &json!({"action":"configure","provider":"qwencloud","api_key":"secret-for-test"}),
            )
            .unwrap();
        assert_eq!(configured["provider"], QWENCLOUD);
        assert_eq!(configured["has_key"], true);
        assert!(!configured.to_string().contains("secret-for-test"));
        assert!(core
            .load_credential(KEY_REF)
            .unwrap()
            .unwrap()
            .starts_with("dpapi:"));
        let cleared = service
            .dispatch(
                &core,
                &json!({"action":"configure","provider":"qwencloud","clear_key":true}),
            )
            .unwrap();
        assert_eq!(cleared["has_key"], false);
    }

    #[test]
    // tungstenite 握手回调的错误类型由库固定为 HTTP Response，无法在调用处装箱。
    #[allow(clippy::result_large_err)]
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
                        let events = [
                            json!({"type":"input_audio_buffer.speech_started","item_id":"source","audio_start_ms":0}),
                            json!({"type":"input_audio_buffer.speech_stopped","item_id":"source","audio_end_ms":250}),
                            json!({"type":"conversation.item.created","item":{"id":"target","role":"assistant"},"previous_item_id":"source"}),
                            json!({"type":"response.text.done","item_id":"target","text":"真实链路"}),
                            json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"source","transcript":"transport"}),
                        ];
                        if epoch == 0 {
                            for event in &events {
                                socket.send(Message::Text(event.to_string().into())).await.unwrap();
                            }
                            while let Some(Ok(frame)) = socket.next().await { if matches!(frame, Message::Close(_)) { break; } }
                        } else {
                            loop {
                                if let Some(Ok(Message::Text(text))) = socket.next().await {
                                    let event: Value = serde_json::from_str(&text).unwrap();
                                    if event["type"] == "session.finish" { break; }
                                }
                            }
                            for event in events {
                                socket.send(Message::Text(event.to_string().into())).await.unwrap();
                            }
                            socket.send(Message::Text(json!({"type":"session.finished"}).to_string().into())).await.unwrap();
                        }
                    }
                });
                let state = Arc::new(Mutex::new(State { status:"connecting", error:String::new(), epoch:0, sequence:0, events:VecDeque::new(), last_seen:Instant::now() }));
                let (tx, rx) = mpsc::channel(16);
                let worker_state = state.clone();
                let stop = Arc::new(Notify::new());
                let worker_stop = stop.clone();
                let worker = tokio::spawn(async move { run(&endpoint,"test-secret","auto","zh",rx,worker_stop,worker_state,false).await });
                for epoch in 0..2 {
                    if epoch > 0 {
                        {
                            let mut state = state.lock().unwrap();
                            state.epoch = epoch;
                            state.status = "connecting";
                        }
                        tx.send(Input::Reset(epoch)).await.unwrap();
                        while state.lock().unwrap().status != "running" {
                            tokio::task::yield_now().await;
                        }
                    }
                    tx.send(Input::Audio { pcm:vec![0;8000], position:if epoch == 0 { 10.0 } else { 120.0 }, rate:1.0, epoch }).await.unwrap();
                    if epoch == 0 {
                        loop {
                            let received = {
                                let state = state.lock().unwrap();
                                state.events.iter().filter(|e| e["epoch"] == epoch).cloned().collect::<Vec<_>>()
                            };
                            if received.len() >= 2 {
                                assert_eq!(received[0]["text"], "真实链路");
                                assert_eq!(received[0]["id"], received[1]["id"]);
                                assert_eq!(received[0]["start"], 10.0);
                                break;
                            }
                            tokio::task::yield_now().await;
                        }
                    }
                }
                state.lock().unwrap().status = "stopping";
                stop.notify_one();
                worker.await.unwrap().unwrap();
                provider.await.unwrap();
                let received = state.lock().unwrap().events.iter()
                    .filter(|event| event["epoch"] == 1)
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(received.len(), 2);
                assert_eq!(received[0]["text"], "真实链路");
                assert_eq!(received[0]["id"], received[1]["id"]);
            }).await.expect("subtitle transport test timed out");
        });
    }
}
