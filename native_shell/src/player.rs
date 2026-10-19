//! In-process libmpv via LoadLibrary/GetProcAddress.
//!
//! When the UI supplies a child HWND, mpv renders into that window (`wid`).
//! Timeline thumbnails are decoded separately; seeking preserves playback state.

use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Mutex};

pub const PLAYER_WINDOW_TITLE: &str = "HLS Downloader 播放器";
const PLAYER_FRAME_LIMIT: usize = 1024 * 1024;
const PLAYER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(test)]
// 播放器和 Core 控制测试共享此环境变量，修改和使用必须在同一个锁内。
pub(crate) static TEST_PLAYER_ENV: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerMetadata {
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub position_available: bool,
    pub session_active: Option<bool>,
    pub paused: Option<bool>,
    pub speed: Option<f64>,
    pub audio_tracks: u32,
    pub subtitle_tracks: u32,
    #[serde(default)]
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub buffered_end_seconds: f64,
    #[serde(default)]
    pub buffering: bool,
    #[serde(default)]
    pub tracks: Vec<crate::contract::PlayerTrack>,
}

fn null_backend_enabled() -> bool {
    std::env::var_os("HLS_V7_PLAYER_NULL").is_some()
}

#[derive(Default)]
pub struct Player {
    inner: Mutex<Backend>,
    last_url: Mutex<String>,
    embed_wid: Mutex<Option<i64>>,
}

#[derive(Default)]
enum Backend {
    #[default]
    Idle,
    Mpv(MpvSession),
    Child(PlayerChild),
}

struct PlayerChild {
    process: Child,
    requests: Option<mpsc::SyncSender<String>>,
    responses: mpsc::Receiver<Result<String, String>>,
    io_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PlayerChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

struct MpvSession {
    handle: *mut c_void,
    command_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> i32,
    terminate: unsafe extern "C" fn(*mut c_void),
    get_property_string: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char>,
    free: Option<unsafe extern "C" fn(*mut c_void)>,
    wait_event: unsafe extern "C" fn(*mut c_void, f64) -> *const MpvEvent,
}

#[repr(C)]
struct MpvEvent {
    event_id: i32,
    error: i32,
    reply_userdata: u64,
    data: *mut c_void,
}

fn mpv_has_shutdown(session: &MpvSession) -> bool {
    loop {
        let event = unsafe { (session.wait_event)(session.handle, 0.0).as_ref() };
        match event.map(|event| event.event_id) {
            Some(1) => return true,
            Some(0) | None => return false,
            _ => {}
        }
    }
}

unsafe impl Send for MpvSession {}

impl Drop for MpvSession {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.terminate)(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

impl Player {
    pub fn play(&self, url: &str, title: &str) -> Result<(), String> {
        if null_backend_enabled() {
            *self.last_url.lock().map_err(|_| "player lock")? = url.to_string();
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.ensure_backend(&mut inner)?;
        let title: String = title.chars().filter(|ch| !ch.is_control()).collect();
        let title = if title.trim().is_empty() {
            "视频"
        } else {
            title.trim()
        };
        self.command_locked(
            &mut inner,
            &format!("set force-media-title {}", quote_mpv(title)?),
        )?;
        self.command_locked(&mut inner, "set pause no")?;
        self.command_locked(&mut inner, &format!("loadfile {} replace", quote_mpv(url)?))?;
        *self.last_url.lock().map_err(|_| "player lock")? = url.to_string();
        Ok(())
    }

    pub fn finish_download(&self, path: &str, title: &str) -> Result<(), String> {
        if null_backend_enabled() {
            *self.last_url.lock().map_err(|_| "player lock")? = path.into();
            return Ok(());
        }
        {
            let mut inner = self.inner.lock().map_err(|_| "player lock")?;
            if let Backend::Child(child) = &mut *inner {
                child.request(
                    "finish_download",
                    Some(serde_json::json!({ "path": path, "title": title })),
                )?;
                *self.last_url.lock().map_err(|_| "player lock")? = path.into();
                return Ok(());
            }
        }
        let metadata = self.metadata();
        if metadata.session_active == Some(false) {
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        let Backend::Mpv(session) = &*inner else {
            return Ok(());
        };
        if mpv_has_shutdown(session) {
            *inner = Backend::Idle;
            return Ok(());
        }
        let audio = property_string(session, "aid");
        let subtitle = property_string(session, "sid");
        let has_input = property_string(session, "idle-active").as_deref() != Some("yes");
        self.command_locked(&mut inner, "stop")?;
        // END_FILE 表示旧文件已经卸载；保留窗口和播放器选项，再释放分片缓存。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let Backend::Mpv(session) = &*inner else {
            return Ok(());
        };
        if has_input {
            loop {
                let event = unsafe { (session.wait_event)(session.handle, 0.05).as_ref() };
                match event.map(|event| event.event_id) {
                    Some(7) => break,
                    Some(1) => {
                        *inner = Backend::Idle;
                        return Ok(());
                    }
                    _ if std::time::Instant::now() >= deadline => {
                        return Err("播放器仍占用下载缓存，已保留缓存供稍后清理".into())
                    }
                    _ => {}
                }
            }
        }
        *self.last_url.lock().map_err(|_| "player lock")? = path.to_string();
        self.command_locked(
            &mut inner,
            &format!("set force-media-title {}", quote_mpv(title)?),
        )?;
        let mut options = format!(
            "start={:.3},pause={},speed={:.3}",
            metadata.position_seconds,
            if metadata.paused == Some(true) {
                "yes"
            } else {
                "no"
            },
            metadata.speed.unwrap_or(1.0)
        );
        for (name, track) in [("aid", audio), ("sid", subtitle)] {
            if let Some(track) = track.filter(|track| {
                matches!(track.as_str(), "no" | "auto")
                    || (!track.is_empty() && track.bytes().all(|ch| ch.is_ascii_digit()))
            }) {
                options.push_str(&format!(",{name}={track}"));
            }
        }
        self.command_locked(
            &mut inner,
            &format!(
                "loadfile {} replace -1 {}",
                quote_mpv(path)?,
                quote_mpv(&options)?
            ),
        )
    }

    pub fn pause(&self, paused: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(
            &mut inner,
            &format!("set pause {}", if paused { "yes" } else { "no" }),
        )
    }

    pub fn set_speed(&self, speed: f64) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(
            &mut inner,
            &format!("set speed {:.3}", speed.clamp(0.25, 4.0)),
        )
    }

    pub fn set_fullscreen(&self, on: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(
            &mut inner,
            &format!("set fullscreen {}", if on { "yes" } else { "no" }),
        )
    }

    pub fn set_pip(&self, on: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        if on {
            self.command_locked(&mut inner, "set fullscreen no")?;
            self.command_locked(&mut inner, "set ontop yes")?;
            self.command_locked(&mut inner, "set window-scale 0.42")
        } else {
            self.command_locked(&mut inner, "set ontop no")?;
            self.command_locked(&mut inner, "set window-scale 1")
        }
    }

    pub fn adjust_volume(&self, delta: f64) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("add volume {:.0}", delta))
    }

    pub fn set_volume(&self, volume: f64) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("音量无效".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(
            &mut inner,
            &format!("set volume {:.0}", volume.clamp(0.0, 100.0)),
        )
    }

    pub fn toggle_mute(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, "cycle mute")
    }

    pub(crate) fn source_url(&self) -> Result<String, String> {
        self.last_url
            .lock()
            .map(|url| url.clone())
            .map_err(|_| "player source lock".into())
    }

    pub fn set_audio_track(&self, track: &str) -> Result<(), String> {
        let track = track.trim();
        if track.is_empty() || track.chars().any(|ch| !ch.is_ascii_digit()) {
            return Err("音轨编号无效".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("set aid {track}"))
    }

    pub fn set_subtitle_track(&self, track: &str) -> Result<(), String> {
        let track = track.trim();
        if track != "no" && (track.is_empty() || track.chars().any(|ch| !ch.is_ascii_digit())) {
            return Err("字幕编号无效".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("set sid {track}"))
    }

    pub fn seek_relative(&self, seconds: f64) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("seek {:.3} relative+exact", seconds))
    }

    pub fn seek_absolute(&self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err("播放时间无效".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("seek {seconds:.3} absolute+exact"))
    }

    pub fn metadata(&self) -> PlayerMetadata {
        if null_backend_enabled() {
            return PlayerMetadata::default();
        }
        let Ok(mut inner) = self.inner.lock() else {
            return PlayerMetadata::default();
        };
        if let Backend::Child(child) = &mut *inner {
            return child
                .request("metadata", None)
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_else(|| PlayerMetadata {
                    session_active: Some(false),
                    ..PlayerMetadata::default()
                });
        }
        if matches!(&*inner, Backend::Mpv(session) if mpv_has_shutdown(session)) {
            *inner = Backend::Idle;
        }
        let Backend::Mpv(session) = &*inner else {
            return PlayerMetadata {
                session_active: Some(false),
                ..PlayerMetadata::default()
            };
        };
        let number = |name: &str| {
            property_string(session, name)
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(0.0)
        };
        let duration_seconds = number("duration");
        let position_seconds = number("time-pos");
        let track_count = number("track-list/count") as u32;
        let mut audio_tracks = 0u32;
        let mut subtitle_tracks = 0u32;
        let mut tracks = Vec::new();
        for index in 0..track_count {
            let kind =
                property_string(session, &format!("track-list/{index}/type")).unwrap_or_default();
            match kind.as_str() {
                "audio" => audio_tracks = audio_tracks.saturating_add(1),
                "sub" => subtitle_tracks = subtitle_tracks.saturating_add(1),
                _ => {}
            }
            if matches!(kind.as_str(), "audio" | "sub") {
                tracks.push(crate::contract::PlayerTrack {
                    id: property_string(session, &format!("track-list/{index}/id"))
                        .unwrap_or_default(),
                    kind,
                    title: property_string(session, &format!("track-list/{index}/title"))
                        .unwrap_or_default(),
                    language: property_string(session, &format!("track-list/{index}/lang"))
                        .unwrap_or_default(),
                    selected: property_string(session, &format!("track-list/{index}/selected"))
                        .as_deref()
                        == Some("yes"),
                });
            }
        }
        PlayerMetadata {
            position_seconds,
            duration_seconds,
            position_available: duration_seconds > 0.0,
            session_active: Some(true),
            paused: property_string(session, "pause").and_then(|value| match value.as_str() {
                "yes" => Some(true),
                "no" => Some(false),
                _ => None,
            }),
            speed: Some(number("speed")).filter(|value| *value > 0.0),
            audio_tracks,
            subtitle_tracks,
            volume: number("volume"),
            muted: property_string(session, "mute").as_deref() == Some("yes"),
            buffered_end_seconds: number("demuxer-cache-time").max(position_seconds),
            buffering: property_string(session, "paused-for-cache").as_deref() == Some("yes"),
            tracks,
        }
    }

    pub fn attach_embed_host(
        &self,
        title: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<(), String> {
        if null_backend_enabled() {
            *self.embed_wid.lock().map_err(|_| "player lock")? = Some(1);
            return Ok(());
        }
        let parent = crate::window_handle_by_title(title).ok_or("播放器宿主窗口尚未就绪")?;
        self.attach_embed_hwnd(parent, x, y, w, h)
    }

    pub fn attach_embed_hwnd(
        &self,
        parent: i64,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<(), String> {
        if null_backend_enabled() {
            *self.embed_wid.lock().map_err(|_| "player lock")? = Some(1);
            return Ok(());
        }
        if parent <= 1 || w <= 0 || h <= 0 || w > 16384 || h > 16384 {
            return Err("播放器宿主区域无效".into());
        }
        // 嵌入参数必须在真正持有 libmpv 的子进程设置，不能只写 Core 的字段。
        if std::env::var_os("HLS_V7_PLAYER_CHILD").is_none() {
            let mut inner = self.inner.lock().map_err(|_| "player lock")?;
            self.ensure_backend(&mut inner)?;
            if let Backend::Child(child) = &mut *inner {
                child.request(
                    "embed",
                    Some(serde_json::json!({"parent":parent,"x":x,"y":y,"w":w,"h":h})),
                )?;
                return Ok(());
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
            use windows_sys::Win32::UI::HiDpi::{
                GetWindowDpiAwarenessContext, SetThreadDpiAwarenessContext,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetParent, IsWindow, SetParent, SetWindowPos, HWND_TOP, SWP_NOACTIVATE,
                SWP_SHOWWINDOW,
            };
            let mut slot = self.embed_wid.lock().map_err(|_| "player embed lock")?;
            if unsafe { IsWindow(parent as _) } == 0 {
                return Err("播放器宿主已关闭".into());
            }
            // 使用宿主的 DPI 模式创建和移动子 HWND，坐标与 Compose 的像素矩形一致。
            if unsafe { SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(parent as _)) }
                .is_null()
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            if let Some(wid) = *slot {
                if unsafe { IsWindow(wid as _) } != 0 {
                    // 调整尺寸、主界面和画中画之间切换，只移动同一个视频子窗口。
                    unsafe {
                        if GetParent(wid as _) != parent as _ {
                            SetLastError(0);
                            if SetParent(wid as _, parent as _).is_null() && GetLastError() != 0 {
                                return Err(std::io::Error::last_os_error().to_string());
                            }
                        }
                        if SetWindowPos(
                            wid as _,
                            HWND_TOP,
                            x,
                            y,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        ) == 0
                        {
                            return Err(std::io::Error::last_os_error().to_string());
                        }
                    }
                    return Ok(());
                }
            }
            *slot = Some(create_child_host_parent(parent, x, y, w, h)?);
        }
        #[cfg(not(windows))]
        {
            let _ = (parent, x, y, w, h);
        }
        Ok(())
    }

    pub fn stop(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            *inner = Backend::Idle;
        }
        #[cfg(windows)]
        if let Ok(mut slot) = self.embed_wid.lock() {
            if let Some(wid) = slot.take().filter(|wid| *wid > 1) {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(wid as _);
                }
            }
        }
    }

    pub fn hide_embed(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        if let Backend::Child(child) = &mut *inner {
            child.request("hide_embed", None)?;
            return Ok(());
        }
        #[cfg(windows)]
        if let Some(wid) = *self.embed_wid.lock().map_err(|_| "player embed lock")? {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(wid as _, 0);
            }
        }
        Ok(())
    }

    fn poll_shutdown(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if matches!(&*inner, Backend::Mpv(session) if mpv_has_shutdown(session)) {
                *inner = Backend::Idle;
            }
        }
    }

    fn ensure_backend(&self, inner: &mut Backend) -> Result<(), String> {
        // quit 后静态属性仍能读取，必须消费真正的 MPV_EVENT_SHUTDOWN。
        let shutdown = match inner {
            Backend::Mpv(session) => mpv_has_shutdown(session),
            Backend::Child(child) => {
                child.requests.is_none() || !matches!(child.process.try_wait(), Ok(None))
            }
            Backend::Idle => false,
        };
        if shutdown {
            *inner = Backend::Idle;
        }
        if matches!(inner, Backend::Mpv(_) | Backend::Child(_)) {
            return Ok(());
        }
        if std::env::var_os("HLS_V7_PLAYER_CHILD").is_none() {
            *inner = Backend::Child(spawn_player_child()?);
            return Ok(());
        }
        let wid = self.embed_wid.lock().ok().and_then(|slot| *slot);
        let session = load_libmpv_session(wid)?;
        *inner = Backend::Mpv(session);
        Ok(())
    }

    fn command_locked(&self, inner: &mut Backend, command: &str) -> Result<(), String> {
        match inner {
            Backend::Mpv(session) => {
                let c_command = CString::new(command).map_err(|error| error.to_string())?;
                let code = unsafe { (session.command_string)(session.handle, c_command.as_ptr()) };
                if code < 0 {
                    Err(format!("mpv_command_string failed: {code}"))
                } else {
                    Ok(())
                }
            }
            Backend::Child(child) => child
                .request("mpv", Some(serde_json::json!({ "command": command })))
                .map(|_| ()),
            Backend::Idle => {
                if null_backend_enabled() {
                    Ok(())
                } else {
                    Err("未加载 libmpv".into())
                }
            }
        }
    }

    fn command_raw(&self, command: &str) -> Result<serde_json::Value, String> {
        if null_backend_enabled() {
            return Ok(serde_json::Value::Null);
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.ensure_backend(&mut inner)?;
        self.command_locked(&mut inner, command)?;
        Ok(serde_json::Value::Null)
    }
}

fn spawn_player_child() -> Result<PlayerChild, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("player executable: {error}"))?;
    let mut process = Command::new(executable)
        .arg("--player-process")
        .env("HLS_V7_PLAYER_CHILD", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("启动播放器进程失败: {error}"))?;
    let mut stdin = process
        .stdin
        .take()
        .ok_or_else(|| "播放器 stdin 不可用".to_string())?;
    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| "播放器 stdout 不可用".to_string())?;
    let (requests, input) = mpsc::sync_channel::<String>(1);
    let (output, responses) = mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("player-io".into())
        .spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Ok(line) = input.recv() {
                let response = (|| {
                    writeln!(stdin, "{line}")
                        .and_then(|_| stdin.flush())
                        .map_err(|error| format!("播放器命令写入失败: {error}"))?;
                    read_player_frame(&mut stdout)?.ok_or_else(|| "播放器进程已退出".to_string())
                })();
                let failed = response.is_err();
                if output.send(response).is_err() || failed {
                    break;
                }
            }
        });
    match worker {
        Ok(io_thread) => Ok(PlayerChild {
            process,
            requests: Some(requests),
            responses,
            io_thread: Some(io_thread),
        }),
        Err(error) => {
            let _ = process.kill();
            let _ = process.wait();
            Err(format!("无法启动播放器通信线程: {error}"))
        }
    }
}

fn read_player_frame(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut frame = String::new();
    let count = reader
        .take((PLAYER_FRAME_LIMIT + 1) as u64)
        .read_line(&mut frame)
        .map_err(|error| format!("播放器帧读取失败: {error}"))?;
    if count == 0 {
        return Ok(None);
    }
    if count > PLAYER_FRAME_LIMIT || !frame.ends_with('\n') {
        return Err("播放器帧过大或不完整".into());
    }
    Ok(Some(frame))
}

impl PlayerChild {
    fn terminate(&mut self) {
        // 先断开队列，再结束进程，使等待命令、读和写三种状态都能退出。
        self.requests.take();
        let _ = self.process.kill();
        let _ = self.process.wait();
        if let Some(worker) = self.io_thread.take() {
            let _ = worker.join();
        }
    }

    fn request(
        &mut self,
        op: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let mut request = serde_json::Map::new();
        request.insert("op".into(), serde_json::Value::String(op.to_string()));
        if let Some(serde_json::Value::Object(fields)) = payload {
            request.extend(fields);
        }
        let line = serde_json::Value::Object(request).to_string();
        if line.len() + 1 > PLAYER_FRAME_LIMIT {
            return Err("播放器请求过大".into());
        }
        let result = (|| {
            self.requests
                .as_ref()
                .ok_or("播放器连接已关闭")?
                .try_send(line)
                .map_err(|_| "播放器通信不可用".to_string())?;
            let response = self
                .responses
                .recv_timeout(PLAYER_REQUEST_TIMEOUT)
                .map_err(|_| "播放器响应超时或连接已关闭".to_string())??;
            serde_json::from_str::<serde_json::Value>(&response)
                .map_err(|error| format!("播放器响应格式无效: {error}"))
        })();
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.terminate();
                return Err(error);
            }
        };
        if value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
            return Err(value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("播放器命令失败")
                .to_string());
        }
        Ok(value
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}

/// Entry point for the isolated child process hosted by the Rust engine.
/// Stdin/stdout carry only bounded JSON commands; all diagnostics stay out of
/// the protocol stream so a crashed player cannot corrupt Core IPC.
pub fn run_player_process() -> i32 {
    std::env::set_var("HLS_V7_PLAYER_CHILD", "1");
    #[cfg(windows)]
    unsafe {
        // mpv 的窗口线程由其内部创建，仅设置当前线程不会改变它的 DPI 默认值。
        // 必须在创建任何窗口前设置进程默认值，否则嵌入画面会被再次缩小。
        use windows_sys::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            eprintln!(
                "player DPI initialization failed: {}",
                std::io::Error::last_os_error()
            );
            return 1;
        }
    }
    let player = Player::default();
    // stdin 不阻塞事件处理。视频子 HWND 归此线程，必须持续泵送窗口消息。
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            match read_player_frame(&mut stdin) {
                Ok(Some(line)) => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        }
    });
    let mut stdout = std::io::BufWriter::new(std::io::stdout());
    loop {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
            };
            let mut message: MSG = std::mem::zeroed();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let line = match receiver.recv_timeout(std::time::Duration::from_millis(16)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                player.poll_shutdown();
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("player stdin: {error}");
                return 1;
            }
        };
        let result = (|| -> Result<serde_json::Value, String> {
            let request: serde_json::Value =
                serde_json::from_str(&line).map_err(|error| error.to_string())?;
            match request
                .get("op")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
            {
                "metadata" => {
                    serde_json::to_value(player.metadata()).map_err(|error| error.to_string())
                }
                "mpv" => player.command_raw(
                    request
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(""),
                ),
                "finish_download" => {
                    let path = request
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .ok_or("缺少成品路径")?;
                    let title = request
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("视频");
                    player.finish_download(path, title)?;
                    Ok(serde_json::Value::Null)
                }
                "embed" => {
                    let integer = |key| {
                        request
                            .get(key)
                            .and_then(serde_json::Value::as_i64)
                            .ok_or("缺少播放器区域参数")
                    };
                    player.attach_embed_hwnd(
                        integer("parent")?,
                        integer("x")? as i32,
                        integer("y")? as i32,
                        integer("w")? as i32,
                        integer("h")? as i32,
                    )?;
                    Ok(serde_json::Value::Null)
                }
                "hide_embed" => {
                    player.hide_embed()?;
                    Ok(serde_json::Value::Null)
                }
                "stop" => {
                    player.stop();
                    Ok(serde_json::Value::Null)
                }
                _ => Err("未知播放器命令".into()),
            }
        })();
        let response = match result {
            Ok(value) => serde_json::json!({ "ok": true, "value": value }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        };
        if serde_json::to_writer(&mut stdout, &response).is_err() || writeln!(stdout).is_err() {
            return 1;
        }
        if stdout.flush().is_err() {
            return 1;
        }
    }
    0
}

fn quote_mpv(url: &str) -> Result<String, String> {
    if url.is_empty() || url.contains('\0') || url.chars().any(char::is_control) {
        return Err("播放地址无效".into());
    }
    Ok(format!(
        "\"{}\"",
        url.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[allow(clippy::manual_c_str_literals)]
fn load_libmpv_session(wid: Option<i64>) -> Result<MpvSession, String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::path::PathBuf;
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let mut last =
            "未找到 libmpv。请将 libmpv-2.dll 放到 HLSDownloader.exe 同目录。".to_string();
        let names = ["libmpv-2.dll", "mpv-2.dll", "libmpv-1.dll"];
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in names {
                    candidates.push(dir.join(name));
                }
            }
        }
        if let Ok(explicit) = std::env::var("HLS_V7_LIBMPV") {
            candidates.insert(0, PathBuf::from(explicit));
        }
        if let Ok(cwd) = std::env::current_dir() {
            for name in names {
                candidates.push(cwd.join(name));
                candidates.push(cwd.join("presenter_ui").join(name));
            }
        }
        for name in names {
            candidates.push(PathBuf::from(name));
        }
        for path in candidates {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                last = format!("{} 无法加载", path.display());
                continue;
            }
            unsafe {
                let create = GetProcAddress(module, b"mpv_create\0".as_ptr());
                let initialize = GetProcAddress(module, b"mpv_initialize\0".as_ptr());
                let command_string = GetProcAddress(module, b"mpv_command_string\0".as_ptr());
                let get_property_string =
                    GetProcAddress(module, b"mpv_get_property_string\0".as_ptr());
                let free = GetProcAddress(module, b"mpv_free\0".as_ptr());
                let wait_event = GetProcAddress(module, b"mpv_wait_event\0".as_ptr());
                let set_option = GetProcAddress(module, b"mpv_set_option_string\0".as_ptr());
                let terminate = GetProcAddress(module, b"mpv_terminate_destroy\0".as_ptr());
                let (
                    Some(create),
                    Some(initialize),
                    Some(command_string),
                    Some(terminate),
                    Some(set_option),
                    Some(wait_event),
                ) = (
                    create,
                    initialize,
                    command_string,
                    terminate,
                    set_option,
                    wait_event,
                )
                else {
                    last = format!("{} 缺少 mpv 符号", path.display());
                    continue;
                };
                let create: unsafe extern "C" fn() -> *mut c_void = std::mem::transmute(create);
                let initialize: unsafe extern "C" fn(*mut c_void) -> i32 =
                    std::mem::transmute(initialize);
                let command_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> i32 =
                    std::mem::transmute(command_string);
                let terminate: unsafe extern "C" fn(*mut c_void) = std::mem::transmute(terminate);
                let wait_event: unsafe extern "C" fn(*mut c_void, f64) -> *const MpvEvent =
                    std::mem::transmute(wait_event);
                let get_property_string = get_property_string.map(|value| {
                    std::mem::transmute::<
                        _,
                        unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
                    >(value)
                });
                let free = free.map(|value| {
                    std::mem::transmute::<_, unsafe extern "C" fn(*mut c_void)>(value)
                });
                let handle = create();
                if handle.is_null() {
                    last = "mpv_create returned null".into();
                    continue;
                }
                {
                    let set_option: unsafe extern "C" fn(
                        *mut c_void,
                        *const c_char,
                        *const c_char,
                    ) -> i32 = std::mem::transmute(set_option);
                    // 输入只来自本地播放服务或成品；站点分片可能使用 jpeg/bin 后缀。
                    let mut options = vec![
                        ("keep-open", "yes".to_string()),
                        ("idle", "yes".into()),
                        (
                            "demuxer-lavf-o",
                            "allowed_extensions=ALL,extension_picky=0".into(),
                        ),
                    ];
                    if let Some(wid) = wid {
                        options.extend([("wid", wid.to_string()), ("force-window", "no".into())]);
                    } else {
                        // libmpv 默认不启用 OSC 和输入；本地播放的控制必须在视频内。
                        options.extend([
                            ("force-window", "yes".into()),
                            ("title", PLAYER_WINDOW_TITLE.into()),
                            ("osc", "yes".into()),
                            ("input-default-bindings", "yes".into()),
                            ("input-vo-keyboard", "yes".into()),
                            ("input-cursor", "yes".into()),
                            ("border", "no".into()),
                            // 分片与成品的显示参数可能略有差别，不能在收尾切换时重排窗口。
                            ("auto-window-resize", "no".into()),
                            ("keepaspect-window", "no".into()),
                            ("ontop", "yes".into()),
                            ("autofit", "70%x70%".into()),
                            ("geometry", "50%:50%".into()),
                            ("script-opts", "osc-layout=bottombar,osc-visibility=always,osc-vidscale=no,osc-boxvideo=yes,osc-windowcontrols=yes,osc-deadzonesize=0,osc-timetotal=yes,osc-custom_button_1_content=速,osc-custom_button_1_mbtn_left_command=cycle-values speed 1 1.25 1.5 2; show-text ${speed}x,osc-custom_button_1_mbtn_right_command=set speed 1; show-text ${speed}x".into()),
                        ]);
                    }
                    for (name, value) in options {
                        let option = CString::new(name).unwrap();
                        let value = CString::new(value).unwrap();
                        let code = set_option(handle, option.as_ptr(), value.as_ptr());
                        if code < 0 {
                            terminate(handle);
                            return Err(format!("播放器选项 {name} 设置失败: {code}"));
                        }
                    }
                }
                if initialize(handle) < 0 {
                    terminate(handle);
                    last = "mpv_initialize failed".into();
                    continue;
                }
                return Ok(MpvSession {
                    handle,
                    command_string,
                    terminate,
                    get_property_string,
                    free,
                    wait_event,
                });
            }
        }
        Err(last)
    }
    #[cfg(not(windows))]
    {
        let _ = wid;
        Err("libmpv is Windows-first in v6".into())
    }
}

fn property_string(session: &MpvSession, name: &str) -> Option<String> {
    let getter = session.get_property_string?;
    let property = CString::new(name).ok()?;
    let pointer = unsafe { getter(session.handle, property.as_ptr()) };
    if pointer.is_null() {
        return None;
    }
    let value = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    if let Some(free) = session.free {
        unsafe { free(pointer.cast()) };
    }
    Some(value)
}

#[cfg(windows)]
fn create_child_host_parent(parent: i64, x: i32, y: i32, w: i32, h: i32) -> Result<i64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_DISABLED, WS_VISIBLE,
    };
    if parent <= 1 {
        return Err("player hwnd missing".into());
    }
    let class: Vec<u16> = std::ffi::OsStr::new("STATIC")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let child: HWND = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_DISABLED,
            x,
            y,
            w.max(32),
            h.max(32),
            parent as HWND,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if child.is_null() {
        return Err("failed to create mpv host window".into());
    }
    unsafe {
        if SetWindowPos(
            child,
            HWND_TOP,
            x,
            y,
            w.max(32),
            h.max(32),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        ) == 0
        {
            let error = std::io::Error::last_os_error().to_string();
            DestroyWindow(child);
            return Err(error);
        }
    }
    Ok(child as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_frames_reject_truncation_oversize_and_invalid_utf8() {
        let mut valid = std::io::Cursor::new(b"{\"ok\":true}\n{\"ok\":false}\n");
        assert_eq!(
            read_player_frame(&mut valid).unwrap().unwrap(),
            "{\"ok\":true}\n"
        );
        assert_eq!(
            read_player_frame(&mut valid).unwrap().unwrap(),
            "{\"ok\":false}\n"
        );
        assert!(read_player_frame(&mut valid).unwrap().is_none());
        for bytes in [
            b"{\"ok\":true}".to_vec(),
            vec![b'x'; PLAYER_FRAME_LIMIT + 1],
            vec![0xff, b'\n'],
        ] {
            assert!(read_player_frame(&mut std::io::Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn null_backend_accepts_embed_and_rejects_invalid_controls() {
        let _guard = TEST_PLAYER_ENV.lock().unwrap();
        let previous = std::env::var_os("HLS_V7_PLAYER_NULL");
        std::env::set_var("HLS_V7_PLAYER_NULL", "1");
        let player = Player::default();
        player
            .play("http://127.0.0.1:9/media/task-1", "测试影片")
            .unwrap();
        player.pause(true).unwrap();
        player.set_speed(1.5).unwrap();
        player.set_audio_track("1").unwrap();
        player.set_subtitle_track("2").unwrap();
        assert!(player.set_audio_track("bad").is_err());
        player.set_fullscreen(true).unwrap();
        player.set_pip(true).unwrap();
        player.set_pip(false).unwrap();
        player.attach_embed_hwnd(42, 0, 48, 720, 220).unwrap();
        player
            .attach_embed_host(PLAYER_WINDOW_TITLE, 0, 48, 720, 220)
            .unwrap();
        player.seek_absolute(37.5).unwrap();
        assert!(player.seek_absolute(f64::NAN).is_err());
        assert!(player.seek_absolute(-1.0).is_err());
        player.set_subtitle_track("no").unwrap();
        assert!(quote_mpv("http://127.0.0.1/a.mp4; run calc")
            .unwrap()
            .starts_with('"'));
        assert!(quote_mpv("http://127.0.0.1/a\nrun").is_err());
        match previous {
            Some(value) => std::env::set_var("HLS_V7_PLAYER_NULL", value),
            None => std::env::remove_var("HLS_V7_PLAYER_NULL"),
        }
    }

    #[test]
    fn null_backend_reports_unavailable_metadata_instead_of_fake_progress() {
        let _guard = TEST_PLAYER_ENV.lock().unwrap();
        let previous = std::env::var_os("HLS_V7_PLAYER_NULL");
        std::env::set_var("HLS_V7_PLAYER_NULL", "1");
        let metadata = Player::default().metadata();
        assert!(!metadata.position_available);
        assert_eq!(metadata.position_seconds, 0.0);
        assert_eq!(metadata.duration_seconds, 0.0);
        assert_eq!(metadata.audio_tracks, 0);
        assert_eq!(metadata.subtitle_tracks, 0);
        match previous {
            Some(value) => std::env::set_var("HLS_V7_PLAYER_NULL", value),
            None => std::env::remove_var("HLS_V7_PLAYER_NULL"),
        }
    }
}
