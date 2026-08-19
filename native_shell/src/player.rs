//! In-process libmpv via LoadLibrary/GetProcAddress.
//!
//! When the UI supplies a child HWND, mpv renders into that window (`wid`).
//! Timeline drag preview seeks while paused. The process never spawns `mpv.exe`.

use std::ffi::{c_char, c_void, CString};
use std::sync::Mutex;

pub const PLAYER_WINDOW_TITLE: &str = "HLS Downloader 播放器";

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
        if std::env::var_os("HLS_V6_PLAYER_NULL").is_some() {
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.ensure_backend(&mut inner)?;
        self.command_locked(
            &mut inner,
            &format!("loadfile {} replace", quote_mpv(url)?),
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
        self.command_locked(&mut inner, &format!("set speed {:.3}", speed.max(0.25).min(4.0)))
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

    pub fn seek_relative(&self, seconds: f64) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|_| "player lock")?;
        self.command_locked(&mut inner, &format!("seek {:.3} relative", seconds))
    }

    pub fn last_url(&self) -> String {
        self.last_url.lock().map(|url| url.clone()).unwrap_or_default()
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

    pub fn preview_percent(&self, percent: f64) -> Result<(), String> {
        let percent = percent.clamp(0.0, 100.0);
        *self.last_preview.lock().map_err(|_| "player lock")? = percent;
        if std::env::var_os("HLS_V6_PLAYER_NULL").is_some() {
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
        if std::env::var_os("HLS_V6_PLAYER_NULL").is_some() {
            *self.embed_wid.lock().map_err(|_| "player lock")? = Some(1);
            return Ok(());
        }
        #[cfg(windows)]
        {
            if let Ok(mut slot) = self.embed_wid.lock() {
                if let Some(old) = slot.take() {
                    destroy_previous_host(old);
                }
            }
            if let Ok(wid) = create_child_host(title, x, y, w, h) {
                *self.embed_wid.lock().map_err(|_| "player lock")? = Some(wid);
                if let Ok(mut inner) = self.inner.lock() {
                    *inner = Backend::Idle;
                }
            }
        }
        Ok(())
    }

    pub fn stop(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            *inner = Backend::Idle;
        }
    }

    fn ensure_backend(&self, inner: &mut Backend) -> Result<(), String> {
        if matches!(inner, Backend::Mpv(_)) {
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
            Backend::Idle => {
                if std::env::var_os("HLS_V6_PLAYER_NULL").is_some() {
                    Ok(())
                } else {
                    Err("未加载 libmpv".into())
                }
            }
        }
    }
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
        let mut last = "未找到 libmpv。请将 libmpv-2.dll 放到 HLSDownloader.exe 同目录。".to_string();
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
        for name in names {
            candidates.push(PathBuf::from(name));
        }
        for path in candidates {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                last = format!("{} 无法加载", path.display());
                continue;
            }
            unsafe {
                let create = GetProcAddress(module, b"mpv_create\0".as_ptr());
                let initialize = GetProcAddress(module, b"mpv_initialize\0".as_ptr());
                let command_string = GetProcAddress(module, b"mpv_command_string\0".as_ptr());
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
                let handle = create();
                if handle.is_null() {
                    last = "mpv_create returned null".into();
                    continue;
                }
                if let Some(set_option) = set_option {
                    let set_option: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> i32 =
                        std::mem::transmute(set_option);
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

#[cfg(windows)]
fn destroy_previous_host(wid: i64) {
    if wid <= 1 {
        return;
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(wid as windows_sys::Win32::Foundation::HWND);
    }
}

#[cfg(windows)]
fn create_child_host(title: &str, x: i32, y: i32, w: i32, h: i32) -> Result<i64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, FindWindowW, SetWindowPos, HWND_TOP, SWP_NOZORDER, SWP_SHOWWINDOW,
        WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
    };
    let wide: Vec<u16> = std::ffi::OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let parent = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
    if parent.is_null() {
        return Err("player window not found".into());
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
            parent,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if child.is_null() {
        return Err("failed to create mpv host window".into());
    }
    unsafe {
        SetWindowPos(child, HWND_TOP, x, y, w.max(32), h.max(32), SWP_NOZORDER | SWP_SHOWWINDOW);
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
        player.set_fullscreen(true).unwrap();
        player.set_pip(true).unwrap();
        player.set_pip(false).unwrap();
        player
            .attach_embed_host(PLAYER_WINDOW_TITLE, 0, 48, 720, 220)
            .unwrap();
        assert!(player.last_embed().contains("720,220"));
        player.preview_percent(37.5).unwrap();
        assert!((player.last_preview() - 37.5).abs() < f64::EPSILON);
        assert!(quote_mpv("http://127.0.0.1/a.mp4; run calc").unwrap().starts_with('"'));
        assert!(quote_mpv("http://127.0.0.1/a\nrun").is_err());
    }
}
