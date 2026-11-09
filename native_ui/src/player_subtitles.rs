//! Optional desktop translation. PCM stays on the Core connection; the UI never
//! receives source credentials or opens the original site's media URL.
use base64::{engine::general_purpose::STANDARD, Engine};
use hls_client::{CoreIpcClient, CorePipeRequest, CorePipeResponse};
use serde_json::{json, Value};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Playback {
    pub generation: u64,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub buffering: bool,
}

#[derive(Default)]
struct Input {
    playback: Playback,
    revision: u64,
    enabled: bool,
    closed: bool,
}

#[derive(Default)]
struct Captions {
    revision: u64,
    notice: String,
    cues: Vec<Value>,
    offset: f64,
    show_source: bool,
}

impl Captions {
    fn text(&self, position: f64) -> String {
        ["translation", "source"]
            .into_iter()
            .filter(|role| *role != "source" || self.show_source)
            .filter_map(|role| {
                self.cues
                    .iter()
                    .rev()
                    .find(|cue| {
                        cue["role"].as_str() == Some(role)
                            && cue["start"]
                                .as_f64()
                                .is_some_and(|t| t + self.offset <= position)
                            && cue["end"]
                                .as_f64()
                                .is_some_and(|t| t + self.offset > position)
                    })
                    .and_then(|cue| cue["text"].as_str())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn accept(&mut self, events: &[Value]) -> bool {
        let mut ready = false;
        for event in events {
            if event["type"] == "ready" {
                ready = true;
            }
            if event["type"] != "cue" {
                continue;
            }
            if let Some(cue) = self
                .cues
                .iter_mut()
                .find(|cue| cue["id"] == event["id"] && cue["role"] == event["role"])
            {
                *cue = event.clone();
            } else {
                self.cues.push(event.clone());
            }
        }
        if self.cues.len() > 256 {
            self.cues.drain(..self.cues.len() - 256);
        }
        ready
    }
}

pub struct PlayerSubtitles {
    input: Arc<Mutex<Input>>,
    output: Arc<Mutex<Captions>>,
    wake: mpsc::SyncSender<()>,
    started: bool,
}

impl Default for PlayerSubtitles {
    fn default() -> Self {
        let (wake, _) = mpsc::sync_channel(1);
        Self {
            input: Arc::default(),
            output: Arc::default(),
            wake,
            started: false,
        }
    }
}

impl PlayerSubtitles {
    pub fn enabled(&self) -> bool {
        self.input.lock().unwrap().enabled
    }

    pub fn toggle(&mut self) {
        if !self.started {
            let (wake, receiver) = mpsc::sync_channel(1);
            self.wake = wake;
            let input = self.input.clone();
            let output = self.output.clone();
            std::thread::spawn(move || run(input, output, receiver));
            self.started = true;
        }
        let mut input = self.input.lock().unwrap();
        input.enabled = !input.enabled;
        input.revision += 1;
        self.output.lock().unwrap().cues.clear();
        let _ = self.wake.try_send(());
    }

    pub fn update(&self, playback: Playback) {
        let mut input = self.input.lock().unwrap();
        if playback.generation != input.playback.generation {
            input.revision += 1;
            self.output.lock().unwrap().cues.clear();
            let _ = self.wake.try_send(());
        }
        input.playback = playback;
    }

    pub fn reset(&self) {
        let mut input = self.input.lock().unwrap();
        input.revision += 1;
        self.output.lock().unwrap().cues.clear();
        let _ = self.wake.try_send(());
    }

    pub fn suspend(&self) {
        let mut input = self.input.lock().unwrap();
        input.revision += 1;
        input.playback.buffering = true;
        self.output.lock().unwrap().cues.clear();
        let _ = self.wake.try_send(());
    }

    pub fn display(&self, position: f64) -> (String, String) {
        let input = self.input.lock().unwrap();
        let output = self.output.lock().unwrap();
        if input.enabled && output.revision == input.revision {
            (output.text(position), output.notice.clone())
        } else {
            (String::new(), String::new())
        }
    }
}

impl Drop for PlayerSubtitles {
    fn drop(&mut self) {
        self.input.lock().unwrap().closed = true;
        let _ = self.wake.try_send(());
    }
}

struct Session {
    id: String,
    revision: u64,
    sequence: u64,
    next_audio: f64,
    pending: Option<Instant>,
    prefetch: bool,
}

fn request(client: &mut CoreIpcClient, value: Value) -> Result<Value, String> {
    match client.request(&CorePipeRequest::Subtitle {
        request_id: 1,
        request: value,
    })? {
        CorePipeResponse::Subtitle { result, .. } if result["ok"] == true => Ok(result),
        CorePipeResponse::Subtitle { result, .. } => Err(result["error"]
            .as_str()
            .unwrap_or("字幕服务操作失败")
            .into()),
        CorePipeResponse::Error { message, .. } => Err(message),
        _ => Err("字幕服务返回了无效响应".into()),
    }
}

fn run(input: Arc<Mutex<Input>>, output: Arc<Mutex<Captions>>, wake: mpsc::Receiver<()>) {
    let owner = format!(
        "desktop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut client: Option<CoreIpcClient> = None;
    let mut session: Option<Session> = None;
    let mut failed = None;
    loop {
        let (closed, enabled, revision, playback) = {
            let input = input.lock().unwrap();
            (
                input.closed,
                input.enabled,
                input.revision,
                input.playback.clone(),
            )
        };
        if closed
            || !enabled
            || playback.generation == 0
            || session.as_ref().is_some_and(|s| s.revision != revision)
        {
            if let (Some(s), Some(c)) = (session.take(), client.as_mut()) {
                let _ = request(
                    c,
                    json!({"action":"stop", "session_id":s.id, "owner":owner}),
                );
            }
            if closed {
                break;
            }
        }
        if enabled
            && playback.generation != 0
            && failed != Some(revision)
            && (session.is_some() || !playback.buffering)
        {
            let result = (|| -> Result<(), String> {
                if client.is_none() {
                    client = Some(CoreIpcClient::connect()?);
                }
                let client = client.as_mut().unwrap();
                if session.is_none() {
                    let config = request(client, json!({"action":"configuration"}))?;
                    if config["has_key"] != true {
                        return Err("请在设置 → 字幕翻译中配置 API Key".into());
                    }
                    let preferences = &config["preferences"];
                    let id = format!("{owner}-{revision}");
                    request(
                        client,
                        json!({"action":"start", "session_id":id, "owner":owner,
                        "source":preferences["source"].as_str().unwrap_or("auto"),
                        "target":preferences["target"].as_str().unwrap_or("zh"),
                        "player_generation":playback.generation, "epoch":revision}),
                    )?;
                    *output.lock().unwrap() = Captions {
                        revision,
                        notice: "字幕翻译连接中…".into(),
                        cues: Vec::new(),
                        offset: preferences["offset"].as_f64().unwrap_or(0.0),
                        show_source: preferences["showSource"].as_bool().unwrap_or(true),
                    };
                    session = Some(Session {
                        id,
                        revision,
                        sequence: 0,
                        next_audio: playback.position,
                        pending: None,
                        prefetch: preferences["prefetch"].as_bool().unwrap_or(false),
                    });
                }
                let session = session.as_mut().unwrap();
                let poll = request(
                    client,
                    json!({"action":"poll", "session_id":session.id, "owner":owner, "after_sequence":session.sequence}),
                )?;
                if matches!(
                    poll["status"].as_str(),
                    Some("failed" | "stopped" | "stopping")
                ) {
                    return Err(poll["error"]
                        .as_str()
                        .filter(|v| !v.is_empty())
                        .unwrap_or("字幕会话已结束，请重新开启")
                        .into());
                }
                session.sequence = poll["latest_sequence"].as_u64().unwrap_or(session.sequence);
                if let Some(events) = poll["events"].as_array() {
                    let mut output = output.lock().unwrap();
                    if output.accept(events) {
                        session.pending = None;
                        output.notice.clear();
                    }
                }
                if session
                    .pending
                    .is_some_and(|at| at.elapsed() > Duration::from_secs(25))
                {
                    return Err("字幕翻译超时，请检查服务后重新开启".into());
                }
                let ahead = if session.prefetch { 12.0 } else { 4.0 };
                if poll["status"] != "running"
                    || playback.paused
                    || playback.buffering
                    || session.pending.is_some()
                    || session.next_audio > playback.position + ahead
                {
                    return Ok(());
                }
                let start = session.next_audio.max(playback.position);
                let seconds = if playback.duration > 0.0 {
                    (playback.duration - start).min(4.0)
                } else {
                    4.0
                };
                if seconds < 0.05 {
                    return Ok(());
                }
                let prepared = request(
                    client,
                    json!({"action":"prepare_audio", "session_id":session.id, "owner":owner,
                    "position":start, "duration":seconds, "epoch":revision}),
                )?;
                let pcm = STANDARD
                    .decode(prepared["audio"].as_str().unwrap_or(""))
                    .map_err(|_| "字幕音频编码无效")?;
                if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > 128_000 {
                    return Err("字幕音频大小无效".into());
                }
                // UI changes are sampled again after the potentially slow decoder.
                // Late PCM may never be submitted to a replacement timeline.
                let current = input.lock().unwrap();
                if current.closed || !current.enabled || current.revision != revision {
                    return Ok(());
                }
                drop(current);
                for (index, chunk) in pcm.chunks(32_000).enumerate() {
                    request(
                        client,
                        json!({"action":"audio", "session_id":session.id, "owner":owner,
                        "audio":STANDARD.encode(chunk), "position":start + index as f64, "rate":1.0, "epoch":revision}),
                    )?;
                }
                let end = start + pcm.len() as f64 / 32_000.0;
                request(
                    client,
                    json!({"action":"commit", "session_id":session.id, "owner":owner,
                    "start":start, "end":end, "epoch":revision}),
                )?;
                session.next_audio = end;
                session.pending = Some(Instant::now());
                Ok(())
            })();
            if let Err(error) = result {
                if let (Some(s), Some(c)) = (session.take(), client.as_mut()) {
                    let _ = request(
                        c,
                        json!({"action":"stop", "session_id":s.id, "owner":owner}),
                    );
                }
                client = None;
                failed = Some(revision);
                *output.lock().unwrap() = Captions {
                    revision,
                    notice: error,
                    ..Default::default()
                };
            }
        }
        if !enabled || playback.generation == 0 || failed == Some(revision) {
            if wake.recv().is_err() {
                break;
            }
        } else {
            let _ = wake.recv_timeout(Duration::from_millis(200));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_follow_media_time_offset_and_replace_partial_text() {
        let mut captions = Captions {
            offset: 0.5,
            show_source: true,
            ..Default::default()
        };
        captions.accept(&[
            json!({"type":"cue","id":"a","role":"translation","text":"部分","start":2,"end":4}),
            json!({"type":"cue","id":"a","role":"source","text":"source","start":2,"end":4}),
        ]);
        assert!(captions.text(2.4).is_empty());
        assert_eq!(captions.text(2.5), "部分\nsource");
        assert!(captions.accept(&[
            json!({"type":"cue","id":"a","role":"translation","text":"完整","start":2,"end":4}),
            json!({"type":"ready"}),
        ]));
        assert_eq!(captions.text(4.49), "完整\nsource");
        assert!(captions.text(4.5).is_empty());
        captions.show_source = false;
        assert_eq!(captions.text(3.0), "完整");
    }

    #[test]
    fn replacement_timeline_and_disable_never_show_old_captions() {
        let subtitles = PlayerSubtitles::default();
        subtitles.input.lock().unwrap().enabled = true;
        subtitles.output.lock().unwrap().notice = "old".into();
        assert_eq!(subtitles.display(0.0).1, "old");
        subtitles.update(Playback {
            generation: 2,
            ..Default::default()
        });
        assert!(subtitles.display(0.0).1.is_empty());
        subtitles.input.lock().unwrap().enabled = false;
        assert!(subtitles.display(0.0).1.is_empty());
    }
}
