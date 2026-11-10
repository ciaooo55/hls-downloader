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
    pub speed: f64,
}

#[derive(Default)]
struct Input {
    playback: Playback,
    revision: u64,
    enabled: bool,
    closed: bool,
    pause: PauseControl,
}

#[derive(Default)]
struct PauseControl {
    // Only a pause issued by this UI may be automatically released.
    owned: bool,
    user_paused: bool,
    pending: Option<u64>,
    failed_revision: Option<u64>,
}

#[derive(Default)]
struct Captions {
    revision: u64,
    notice: String,
    cues: Vec<Value>,
    offset: f64,
    show_source: bool,
    prefetch: bool,
    ready_ranges: Vec<(f64, f64)>,
}

impl Captions {
    fn ready_ahead(&self, position: f64) -> f64 {
        let mut end = position;
        for &(start, ready_end) in &self.ready_ranges {
            if start > end + 0.01 {
                break;
            }
            end = end.max(ready_end);
        }
        (end - position).max(0.0)
    }

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
                if let (Some(start), Some(end)) = (event["start"].as_f64(), event["end"].as_f64()) {
                    if start.is_finite() && end.is_finite() && start >= 0.0 && end > start {
                        self.ready_ranges.push((start, end));
                    }
                }
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
        self.ready_ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
        if self.ready_ranges.len() > 256 {
            self.ready_ranges.drain(..self.ready_ranges.len() - 256);
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
            input.pause = PauseControl::default();
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
            let notice = if input.pause.owned && output.prefetch {
                format!(
                    "等待字幕 · 已准备 {:.1} 秒",
                    output.ready_ahead((position - output.offset).max(0.0))
                )
            } else {
                output.notice.clone()
            };
            (output.text(position), notice)
        } else {
            (String::new(), String::new())
        }
    }

    pub fn user_control(&self, action: &str) {
        if matches!(action, "pause" | "resume" | "play" | "stop") {
            let mut input = self.input.lock().unwrap();
            input.pause.owned = false;
            input.pause.user_paused = action == "pause";
        }
    }

    /// Return an absolute pause state; the caller sends a generation-bound command.
    pub fn playback_action(&self, position: f64) -> Option<bool> {
        let mut input = self.input.lock().unwrap();
        let output = self.output.lock().unwrap();
        if input.playback.generation == 0
            || input.pause.user_paused
            || input.pause.pending.is_some()
            || input.pause.failed_revision == Some(input.revision)
        {
            return None;
        }
        if !input.enabled || (output.revision == input.revision && !output.prefetch) {
            return (input.pause.owned && input.playback.paused).then_some(false);
        }
        // A seek/track change must prepare its own windows before any resume.
        if output.revision != input.revision || input.playback.buffering {
            return None;
        }
        let position = (position - output.offset).max(0.0);
        let remaining = if input.playback.duration > 0.0 {
            (input.playback.duration - position).max(0.0)
        } else {
            f64::INFINITY
        };
        let speed = input.playback.speed.clamp(0.25, 4.0);
        let ahead = output.ready_ahead(position);
        if input.playback.paused {
            (input.pause.owned
                && (remaining <= 0.05
                    || (ahead > 0.0 && ahead >= (8.0 * speed).min(16.0).min(remaining) - 0.01)))
                .then_some(false)
        } else {
            input.pause.owned = false;
            (remaining > 0.05 && ahead < remaining.min(1.0)).then_some(true)
        }
    }

    pub fn playback_command_sent(&self, request_id: u64, paused: bool) {
        let mut input = self.input.lock().unwrap();
        input.pause.owned = paused;
        input.pause.pending = Some(request_id);
    }

    pub fn playback_response(&self, request_id: u64, success: bool) -> bool {
        let mut input = self.input.lock().unwrap();
        if input.pause.pending != Some(request_id) {
            return false;
        }
        input.pause.pending = None;
        if !success {
            input.pause.owned = false;
            input.pause.failed_revision = Some(input.revision);
        }
        true
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
    offset: f64,
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
                    let prefetch = preferences["prefetch"].as_bool().unwrap_or(false);
                    let offset = preferences["offset"].as_f64().unwrap_or(0.0);
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
                        offset,
                        show_source: preferences["showSource"].as_bool().unwrap_or(true),
                        prefetch,
                        ready_ranges: Vec::new(),
                    };
                    session = Some(Session {
                        id,
                        revision,
                        sequence: 0,
                        next_audio: if prefetch {
                            (playback.position - offset).max(0.0)
                        } else {
                            playback.position
                        },
                        pending: None,
                        prefetch,
                        offset,
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
                let ahead = if session.prefetch {
                    (8.0 * playback.speed).clamp(12.0, 16.0)
                } else {
                    4.0
                };
                let position = if session.prefetch {
                    (playback.position - session.offset).max(0.0)
                } else {
                    playback.position
                };
                if poll["status"] != "running"
                    || (playback.paused && !session.prefetch)
                    || playback.buffering
                    || session.pending.is_some()
                    || session.next_audio > position + ahead
                {
                    return Ok(());
                }
                let start = session.next_audio.max(position);
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

    fn synchronized_player() -> PlayerSubtitles {
        let subtitles = PlayerSubtitles::default();
        subtitles.update(Playback {
            generation: 7,
            duration: 30.0,
            speed: 1.0,
            ..Default::default()
        });
        subtitles.input.lock().unwrap().enabled = true;
        let revision = subtitles.input.lock().unwrap().revision;
        *subtitles.output.lock().unwrap() = Captions {
            revision,
            prefetch: true,
            ..Default::default()
        };
        subtitles
    }

    fn acknowledge_pause(subtitles: &PlayerSubtitles) {
        assert_eq!(subtitles.playback_action(0.0), Some(true));
        subtitles.playback_command_sent(10, true);
        assert_eq!(subtitles.playback_action(0.0), None);
        assert!(subtitles.playback_response(10, true));
        subtitles.input.lock().unwrap().playback.paused = true;
    }

    #[test]
    fn prefetch_waits_for_contiguous_ready_windows_including_silence() {
        let subtitles = synchronized_player();
        acknowledge_pause(&subtitles);
        subtitles.output.lock().unwrap().accept(&[
            json!({"type":"cue","id":"a","role":"translation","text":"partial","start":0,"end":12}),
        ]);
        assert_eq!(subtitles.playback_action(0.0), None);
        // The middle window is silent; cue text cannot establish completeness.
        subtitles.output.lock().unwrap().accept(&[
            json!({"type":"ready","start":8,"end":12}),
            json!({"type":"ready","start":0,"end":4}),
        ]);
        assert_eq!(subtitles.playback_action(0.0), None);
        subtitles
            .output
            .lock()
            .unwrap()
            .accept(&[json!({"type":"ready","start":4,"end":8})]);
        assert_eq!(subtitles.playback_action(0.0), Some(false));
        subtitles.playback_command_sent(11, false);
        assert_eq!(subtitles.playback_action(0.0), None);
        assert!(subtitles.playback_response(11, true));
        assert_eq!(subtitles.playback_action(0.0), None);
    }

    #[test]
    fn user_pause_wins_over_inflight_automatic_pause_and_ready_events() {
        let subtitles = synchronized_player();
        subtitles.playback_command_sent(10, true);
        subtitles.user_control("pause");
        assert!(subtitles.playback_response(10, true));
        // The next player snapshot has not arrived yet: do not reclaim ownership.
        assert_eq!(subtitles.playback_action(0.0), None);
        subtitles.input.lock().unwrap().playback.paused = true;
        subtitles
            .output
            .lock()
            .unwrap()
            .accept(&[json!({"type":"ready","start":0,"end":12})]);
        assert_eq!(subtitles.playback_action(0.0), None);
        subtitles.user_control("resume");
        assert_eq!(subtitles.playback_action(0.0), None);
        subtitles.input.lock().unwrap().playback.paused = false;
        assert_eq!(subtitles.playback_action(0.0), None);
        assert_eq!(subtitles.playback_action(11.5), Some(true));
    }

    #[test]
    fn replacement_timeline_cannot_resume_using_old_ready_windows_or_receipts() {
        let subtitles = synchronized_player();
        acknowledge_pause(&subtitles);
        subtitles
            .output
            .lock()
            .unwrap()
            .accept(&[json!({"type":"ready","start":0,"end":30})]);
        subtitles.suspend();
        assert_eq!(subtitles.playback_action(20.0), None);
        let revision = subtitles.input.lock().unwrap().revision;
        *subtitles.output.lock().unwrap() = Captions {
            revision,
            prefetch: true,
            ..Default::default()
        };
        subtitles.input.lock().unwrap().playback.buffering = false;
        assert_eq!(subtitles.playback_action(20.0), None);
        subtitles
            .output
            .lock()
            .unwrap()
            .accept(&[json!({"type":"ready","start":20,"end":30})]);
        assert_eq!(subtitles.playback_action(20.0), Some(false));
        subtitles.playback_command_sent(11, false);
        subtitles.update(Playback {
            generation: 8,
            paused: true,
            duration: 30.0,
            speed: 1.0,
            ..Default::default()
        });
        assert!(!subtitles.playback_response(11, true));
        assert_eq!(subtitles.playback_action(0.0), None);
    }

    #[test]
    fn disable_or_service_failure_releases_only_an_owned_pause() {
        for disabled in [false, true] {
            let subtitles = synchronized_player();
            acknowledge_pause(&subtitles);
            if disabled {
                let mut input = subtitles.input.lock().unwrap();
                input.enabled = false;
                input.revision += 1;
            } else {
                subtitles.output.lock().unwrap().prefetch = false;
            }
            assert_eq!(subtitles.playback_action(0.0), Some(false));
            subtitles.user_control("pause");
            assert_eq!(subtitles.playback_action(0.0), None);
        }
        let subtitles = synchronized_player();
        subtitles.output.lock().unwrap().prefetch = false;
        assert_eq!(subtitles.playback_action(0.0), None);
    }

    #[test]
    fn failed_player_control_is_not_retried_on_every_tick() {
        let subtitles = synchronized_player();
        subtitles.playback_command_sent(10, true);
        assert!(!subtitles.playback_response(9, false));
        assert_eq!(subtitles.playback_action(0.0), None);
        assert!(subtitles.playback_response(10, false));
        assert_eq!(subtitles.playback_action(0.0), None);
        subtitles.reset();
        subtitles.output.lock().unwrap().revision = subtitles.input.lock().unwrap().revision;
        assert_eq!(subtitles.playback_action(0.0), Some(true));
    }

    #[test]
    fn prefetch_resume_threshold_tracks_speed_and_the_last_partial_window() {
        let subtitles = synchronized_player();
        acknowledge_pause(&subtitles);
        subtitles.input.lock().unwrap().playback.speed = 2.0;
        subtitles
            .output
            .lock()
            .unwrap()
            .accept(&[json!({"type":"ready","start":0,"end":8})]);
        assert_eq!(subtitles.playback_action(0.0), None);
        subtitles
            .output
            .lock()
            .unwrap()
            .accept(&[json!({"type":"ready","start":8,"end":16})]);
        assert_eq!(subtitles.playback_action(0.0), Some(false));
        subtitles.output.lock().unwrap().ready_ranges = vec![(28.0, 30.0)];
        assert_eq!(subtitles.playback_action(28.0), Some(false));
        assert_eq!(subtitles.playback_action(30.0), Some(false));
    }

    #[test]
    fn prefetch_uses_the_shifted_caption_timeline_for_offsets_and_end_of_media() {
        for (offset, position, ready_start) in [(-6.0, 0.0, 6.0), (6.0, 10.0, 4.0)] {
            let subtitles = synchronized_player();
            acknowledge_pause(&subtitles);
            let mut output = subtitles.output.lock().unwrap();
            output.offset = offset;
            output.ready_ranges = vec![(ready_start, ready_start + 8.0)];
            drop(output);
            assert_eq!(subtitles.playback_action(position), Some(false));
        }
        let subtitles = synchronized_player();
        acknowledge_pause(&subtitles);
        subtitles.output.lock().unwrap().offset = -6.0;
        assert_eq!(subtitles.playback_action(28.0), Some(false));
    }

    #[cfg(windows)]
    #[test]
    fn paused_prefetch_worker_prepares_audio_over_its_own_core_pipe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let pipe = format!(r"\\.\pipe\HLSSubtitleWorkerTest.{}", std::process::id());
        let stop = Arc::new(AtomicBool::new(false));
        let windows = Arc::new(Mutex::new(Vec::<Value>::new()));
        let (committed, commits) = mpsc::channel();
        let (ready, listening) = mpsc::sync_channel(1);
        let handler = Arc::new(move |request| match request {
            CorePipeRequest::Hello { .. } => CorePipeResponse::Hello {
                protocol: hls_client::V7_PROTOCOL_NAME.into(),
                version: hls_client::V7_PROTOCOL_VERSION,
                pid: 0,
            },
            CorePipeRequest::Subtitle {
                request_id,
                request,
            } => {
                let result = match request["action"].as_str().unwrap() {
                    "configuration" => {
                        json!({"ok":true,"has_key":true,"preferences":{"prefetch":true,"offset":-2.0}})
                    }
                    "start" => {
                        assert_eq!(request["player_generation"], 7);
                        json!({"ok":true})
                    }
                    "poll" => {
                        let windows = windows.lock().unwrap();
                        let after = request["after_sequence"].as_u64().unwrap() as usize;
                        json!({"ok":true,"status":"running","latest_sequence":windows.len(),"events":windows[after..]})
                    }
                    "prepare_audio" => json!({"ok":true,"audio":STANDARD.encode(vec![0;128_000])}),
                    "audio" => json!({"ok":true}),
                    "commit" => {
                        windows.lock().unwrap().push(
                            json!({"type":"ready","start":request["start"],"end":request["end"]}),
                        );
                        committed
                            .send((
                                request["start"].as_f64().unwrap(),
                                request["end"].as_f64().unwrap(),
                            ))
                            .unwrap();
                        json!({"ok":true})
                    }
                    "stop" => json!({"ok":true}),
                    action => panic!("unexpected subtitle action: {action}"),
                };
                CorePipeResponse::Subtitle { request_id, result }
            }
            request => panic!("unexpected Core request: {request:?}"),
        });
        let server_stop = stop.clone();
        let server_pipe = pipe.clone();
        let server = std::thread::spawn(move || {
            hls_client::NamedPipeServer::new(server_pipe).serve_loop_with_ready(
                server_stop,
                handler,
                ready,
            )
        });
        listening
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let previous = std::env::var_os("HLS_V7_PIPE");
        std::env::set_var("HLS_V7_PIPE", pipe);
        let input = Arc::new(Mutex::new(Input {
            playback: Playback {
                generation: 7,
                duration: 14.0,
                speed: 1.0,
                paused: true,
                ..Default::default()
            },
            enabled: true,
            ..Default::default()
        }));
        let output = Arc::new(Mutex::new(Captions::default()));
        let worker_input = input.clone();
        let (wake, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || run(worker_input, output, receiver));
        let result = std::panic::catch_unwind(|| {
            for expected in [(2.0, 6.0), (6.0, 10.0), (10.0, 14.0)] {
                assert_eq!(
                    commits.recv_timeout(Duration::from_secs(5)).unwrap(),
                    expected
                );
            }
        });
        input.lock().unwrap().closed = true;
        let _ = wake.try_send(());
        let worker_result = worker.join();
        stop.store(true, Ordering::SeqCst);
        let server_result = server.join();
        match previous {
            Some(value) => std::env::set_var("HLS_V7_PIPE", value),
            None => std::env::remove_var("HLS_V7_PIPE"),
        }
        result.unwrap();
        worker_result.unwrap();
        server_result.unwrap().unwrap();
    }

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
