//! In-process libmpv via LoadLibrary/GetProcAddress.
//!
//! When the UI supplies a child HWND, mpv renders into that window (`wid`).
//! Timeline drag preview seeks while paused. The process never spawns `mpv.exe`.

use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

pub const PLAYER_WINDOW_TITLE: &str = "HLS Downloader 播放器";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PlayerMetadata {
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub position_available: bool,
    pub audio_tracks: u32,
    pub subtitle_tracks: u32,
}

fn null_backend_enabled() -> bool {
    std::env::var_os("HLS_V7_PLAYER_NULL").is_some()
        || std::env::var_os("HLS_V6_PLAYER_NULL").is_some()
}

#[derive(Default)]
pub struct Player {
    inner: Mutex<Backend>,
    last_url: Mutex<String>,
    embed_wid: Mutex<Option<i64>>,
    last_preview: Mutex<f64>,
    last_embed: Mutex<String>,
}

enum Backend {
    Idle,
    Mpv(MpvSession),
    Child(PlayerChild),
}

struct PlayerChild {
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for PlayerChild {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::Idle
    }
}

struct MpvSession {
    handle: *mut c_void,
    command_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> i32,
    terminate: unsafe extern "C" fn(*mut c_void),
    get_property_string: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char>,
    free: Option<unsafe extern "C" fn(*mut c_void)>,
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
    pub fn play(&self, url: &str) -> Result<(), String> {
        *self.last_url.lock().map_err(|_| "player lock")? = url.to_string();
        if null_backend_enabled() {
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.ensure_backend(&mut inner)?;
        self.command_locked(&mut inner, &format!("loadfile {} replace", quote_mpv(url)?))
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
            &format!("set speed {:.3}", speed.max(0.25).min(4.0)),
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
        if track.is_empty() || track.chars().any(|ch| !ch.is_ascii_digit()) {
            return Err("字幕编号无效".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("set sid {track}"))
    }

    pub fn seek_relative(&self, seconds: f64) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("seek {:.3} relative", seconds))
    }

    pub fn last_url(&self) -> String {
        self.last_url
            .lock()
            .map(|url| url.clone())
            .unwrap_or_default()
    }

    pub fn last_preview(&self) -> f64 {
        self.last_preview.lock().map(|value| *value).unwrap_or(0.0)
    }

    pub fn last_embed(&self) -> String {
        self.last_embed
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
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
                .unwrap_or_default();
        }
        let Backend::Mpv(session) = &*inner else {
            return PlayerMetadata::default();
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
        for index in 0..track_count {
            match property_string(session, &format!("track-list/{index}/type")).as_deref() {
                Some("audio") => audio_tracks = audio_tracks.saturating_add(1),
                Some("sub") => subtitle_tracks = subtitle_tracks.saturating_add(1),
                _ => {}
            }
        }
        PlayerMetadata {
            position_seconds,
            duration_seconds,
            position_available: duration_seconds > 0.0,
            audio_tracks,
            subtitle_tracks,
        }
    }

    pub fn preview_percent(&self, percent: f64) -> Result<(), String> {
        let percent = percent.clamp(0.0, 100.0);
        *self.last_preview.lock().map_err(|_| "player lock")? = percent;
        if null_backend_enabled() {
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.ensure_backend(&mut inner)?;
        self.command_locked(&mut inner, "set pause yes")?;
        self.command_locked(&mut inner, &format!("seek {percent:.3} absolute-percent"))
    }

    pub fn attach_embed_host(
        &self,
        title: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Result<(), String> {
        *self.last_embed.lock().map_err(|_| "player lock")? = format!("{title}:{x},{y},{w},{h}");
        if null_backend_enabled() {
            *self.embed_wid.lock().map_err(|_| "player lock")? = Some(1);
            return Ok(());
        }
        let parent = crate::window_handle_by_title(title).unwrap_or(0);
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
        *self.last_embed.lock().map_err(|_| "player lock")? =
            format!("embed_hwnd:{parent}:{x},{y},{w},{h}");
        if null_backend_enabled() {
            *self.embed_wid.lock().map_err(|_| "player lock")? = Some(1);
            return Ok(());
        }
        #[cfg(windows)]
        {
            if let Ok(mut slot) = self.embed_wid.lock() {
                if let Some(old) = slot.take() {
                    destroy_previous_host(old, parent);
                }
            }
            if parent > 1 {
                if let Ok(wid) = create_child_host_parent(parent, x, y, w, h) {
                    *self.embed_wid.lock().map_err(|_| "player lock")? = Some(wid);
                    if let Ok(mut inner) = self.inner.lock() {
                        *inner = Backend::Idle;
                    }
                }
            }
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
    }

    fn ensure_backend(&self, inner: &mut Backend) -> Result<(), String> {
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
    let stdin = process
        .stdin
        .take()
        .ok_or_else(|| "播放器 stdin 不可用".to_string())?;
    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| "播放器 stdout 不可用".to_string())?;
    Ok(PlayerChild {
        process,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

impl PlayerChild {
    fn request(
        &mut self,
        op: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let mut request = serde_json::Map::new();
        request.insert("op".into(), serde_json::Value::String(op.to_string()));
        if let Some(payload) = payload {
            if let serde_json::Value::Object(fields) = payload {
                request.extend(fields);
            }
        }
        let line = serde_json::Value::Object(request).to_string();
        writeln!(self.stdin, "{line}").map_err(|error| format!("播放器命令写入失败: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("播放器命令刷新失败: {error}"))?;
        let mut response = String::new();
        self.stdout
            .read_line(&mut response)
            .map_err(|error| format!("播放器响应读取失败: {error}"))?;
        if response.trim().is_empty() {
            return Err("播放器进程已退出".into());
        }
        let value: serde_json::Value = serde_json::from_str(&response)
            .map_err(|error| format!("播放器响应格式无效: {error}"))?;
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
    let player = Player::default();
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout());
    for line in stdin.lock().lines() {
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
        if let Ok(explicit) = std::env::var("HLS_V6_LIBMPV") {
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
                let set_option = GetProcAddress(module, b"mpv_set_option_string\0".as_ptr());
                let terminate = GetProcAddress(module, b"mpv_terminate_destroy\0".as_ptr());
                let (Some(create), Some(initialize), Some(command_string), Some(terminate)) =
                    (create, initialize, command_string, terminate)
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
                if let Some(set_option) = set_option {
                    let set_option: unsafe extern "C" fn(
                        *mut c_void,
                        *const c_char,
                        *const c_char,
                    ) -> i32 = std::mem::transmute(set_option);
                    let keep = CString::new("keep-open").unwrap();
                    let yes = CString::new("yes").unwrap();
                    let _ = set_option(handle, keep.as_ptr(), yes.as_ptr());
                    if let Some(wid) = wid {
                        let name = CString::new("wid").unwrap();
                        let value = CString::new(wid.to_string()).unwrap();
                        let force = CString::new("force-window").unwrap();
                        let no = CString::new("no").unwrap();
                        let _ = set_option(handle, name.as_ptr(), value.as_ptr());
                        let _ = set_option(handle, force.as_ptr(), no.as_ptr());
                    } else {
                        let force = CString::new("force-window").unwrap();
                        let _ = set_option(handle, force.as_ptr(), yes.as_ptr());
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
fn destroy_previous_host(wid: i64, parent: i64) {
    if wid <= 1 || wid == parent {
        return;
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(
            wid as windows_sys::Win32::Foundation::HWND,
        );
    }
}

#[cfg(windows)]
fn create_child_host_parent(parent: i64, x: i32, y: i32, w: i32, h: i32) -> Result<i64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, SetWindowPos, HWND_TOP, SWP_NOZORDER, SWP_SHOWWINDOW, WS_CHILD,
        WS_CLIPSIBLINGS, WS_VISIBLE,
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
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
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
        SetWindowPos(
            child,
            HWND_TOP,
            x,
            y,
            w.max(32),
            h.max(32),
            SWP_NOZORDER | SWP_SHOWWINDOW,
        );
    }
    Ok(child as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_backend_records_url_embed_and_preview() {
        std::env::set_var("HLS_V6_PLAYER_NULL", "1");
        let player = Player::default();
        player.play("http://127.0.0.1:9/media/task-1").unwrap();
        assert!(player.last_url().contains("task-1"));
        player.pause(true).unwrap();
        player.set_speed(1.5).unwrap();
        player.set_audio_track("1").unwrap();
        player.set_subtitle_track("2").unwrap();
        assert!(player.set_audio_track("bad").is_err());
        player.set_fullscreen(true).unwrap();
        player.set_pip(true).unwrap();
        player.set_pip(false).unwrap();
        player.attach_embed_hwnd(42, 0, 48, 720, 220).unwrap();
        assert!(player.last_embed().contains("embed_hwnd:42"));
        player
            .attach_embed_host(PLAYER_WINDOW_TITLE, 0, 48, 720, 220)
            .unwrap();
        assert!(player.last_embed().contains("720,220"));
        player.preview_percent(37.5).unwrap();
        assert!((player.last_preview() - 37.5).abs() < f64::EPSILON);
        assert!(quote_mpv("http://127.0.0.1/a.mp4; run calc")
            .unwrap()
            .starts_with('"'));
        assert!(quote_mpv("http://127.0.0.1/a\nrun").is_err());
    }

    #[test]
    fn null_backend_reports_unavailable_metadata_instead_of_fake_progress() {
        std::env::set_var("HLS_V7_PLAYER_NULL", "1");
        let metadata = Player::default().metadata();
        assert!(!metadata.position_available);
        assert_eq!(metadata.position_seconds, 0.0);
        assert_eq!(metadata.duration_seconds, 0.0);
        assert_eq!(metadata.audio_tracks, 0);
        assert_eq!(metadata.subtitle_tracks, 0);
        std::env::remove_var("HLS_V7_PLAYER_NULL");
    }
}
