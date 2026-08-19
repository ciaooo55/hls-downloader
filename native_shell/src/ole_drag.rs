//! Explorer drag-out. HDROP goes on the clipboard; Windows then DoDragDrop
//! from `OleGetClipboard` so we do not hand a live IDataObject we constructed.

use std::path::{Path, PathBuf};

pub fn hdrop_bytes(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    if paths.is_empty() {
        return Err("没有可拖出的文件".into());
    }
    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            return Err("拖出路径必须是绝对路径".into());
        }
        wide.extend(path.to_string_lossy().encode_utf16());
        wide.push(0);
    }
    wide.push(0);
    let header = 20usize;
    let mut bytes = vec![0u8; header + wide.len() * 2];
    bytes[..4].copy_from_slice(&(header as u32).to_le_bytes());
    bytes[16] = 1;
    let payload = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    bytes[header..].copy_from_slice(payload);
    Ok(bytes)
}

pub fn begin_file_drag(paths: &[PathBuf]) -> Result<(), String> {
    crate::write_clipboard_files(paths)?;
    #[cfg(windows)]
    {
        windows_drag_from_clipboard()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub fn completed_file_drag(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("文件不存在，无法拖出".into());
    }
    begin_file_drag(&[path.to_path_buf()])
}

#[cfg(windows)]
fn windows_drag_from_clipboard() -> Result<(), String> {
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut core::ffi::c_void) -> i32;
        fn OleUninitialize();
        fn OleGetClipboard(ppdataobj: *mut *mut core::ffi::c_void) -> i32;
        fn DoDragDrop(
            pdataobj: *mut core::ffi::c_void,
            pdropsource: *mut core::ffi::c_void,
            dwokeffects: u32,
            pdweffect: *mut u32,
        ) -> i32;
    }
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_LINK: u32 = 4;
    unsafe {
        let hr = OleInitialize(std::ptr::null_mut());
        if hr < 0 && hr != 1 {
            return Err(format!("OleInitialize {hr:#x}"));
        }
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        let got = OleGetClipboard(&mut data);
        if got < 0 || data.is_null() {
            OleUninitialize();
            return Err("剪贴板没有可拖出的文件".into());
        }
        let source = Box::into_raw(Box::new(DropSource::new()));
        let mut effect: u32 = 0;
        let _ = DoDragDrop(
            data,
            source as *mut core::ffi::c_void,
            DROPEFFECT_COPY | DROPEFFECT_LINK,
            &mut effect,
        );
        drop_release(source);
        let vtbl = *(data as *mut *const usize);
        let release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));
        release(data);
        OleUninitialize();
    }
    Ok(())
}

#[cfg(windows)]
#[repr(C)]
struct DropSource {
    vtbl: *const DropSourceVtbl,
    refs: std::sync::atomic::AtomicU32,
}

#[cfg(windows)]
#[repr(C)]
struct DropSourceVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut DropSource,
        iid: *const u8,
        out: *mut *mut core::ffi::c_void,
    ) -> i32,
    add_ref: unsafe extern "system" fn(this: *mut DropSource) -> u32,
    release: unsafe extern "system" fn(this: *mut DropSource) -> u32,
    query_continue_drag: unsafe extern "system" fn(
        this: *mut DropSource,
        escape: i32,
        key_state: u32,
    ) -> i32,
    give_feedback: unsafe extern "system" fn(this: *mut DropSource, effect: u32) -> i32,
}

#[cfg(windows)]
static DROP_SOURCE_VTBL: DropSourceVtbl = DropSourceVtbl {
    query_interface: drop_query_interface,
    add_ref: drop_add_ref,
    release: drop_release,
    query_continue_drag: drop_query_continue,
    give_feedback: drop_give_feedback,
};

#[cfg(windows)]
impl DropSource {
    fn new() -> Self {
        Self {
            vtbl: &DROP_SOURCE_VTBL,
            refs: std::sync::atomic::AtomicU32::new(1),
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn drop_query_interface(
    this: *mut DropSource,
    iid: *const u8,
    out: *mut *mut core::ffi::c_void,
) -> i32 {
    const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
    if this.is_null() || iid.is_null() || out.is_null() {
        return E_NOINTERFACE;
    }
    if iid_is(iid, &IID_IUNKNOWN) || iid_is(iid, &IID_IDROPSOURCE) {
        *out = this as *mut core::ffi::c_void;
        drop_add_ref(this);
        return 0;
    }
    *out = std::ptr::null_mut();
    E_NOINTERFACE
}

#[cfg(windows)]
const IID_IUNKNOWN: [u8; 16] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

#[cfg(windows)]
const IID_IDROPSOURCE: [u8; 16] = [
    0x21, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

#[cfg(windows)]
fn iid_is(iid: *const u8, expected: &[u8; 16]) -> bool {
    unsafe { std::slice::from_raw_parts(iid, 16) == expected }
}

#[cfg(windows)]
unsafe extern "system" fn drop_add_ref(this: *mut DropSource) -> u32 {
    (*this)
        .refs
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1
}

#[cfg(windows)]
unsafe extern "system" fn drop_release(this: *mut DropSource) -> u32 {
    if this.is_null() {
        return 0;
    }
    let previous = (*this)
        .refs
        .fetch_sub(1, std::sync::atomic::Ordering::Release);
    if previous == 1 {
        drop(Box::from_raw(this));
        0
    } else {
        previous.saturating_sub(1)
    }
}

#[cfg(windows)]
unsafe extern "system" fn drop_query_continue(
    _this: *mut DropSource,
    escape: i32,
    key_state: u32,
) -> i32 {
    const DRAGDROP_S_DROP: i32 = 0x0004_0100;
    const DRAGDROP_S_CANCEL: i32 = 0x0004_0101;
    const MK_LBUTTON: u32 = 0x0001;
    if escape != 0 {
        return DRAGDROP_S_CANCEL;
    }
    if key_state & MK_LBUTTON == 0 {
        return DRAGDROP_S_DROP;
    }
    0
}

#[cfg(windows)]
unsafe extern "system" fn drop_give_feedback(_this: *mut DropSource, _effect: u32) -> i32 {
    const DRAGDROP_S_USEDEFAULTCURSORS: i32 = 0x0004_0102;
    DRAGDROP_S_USEDEFAULTCURSORS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdrop_is_unicode_dropfiles() {
        let path = std::env::temp_dir().join("hls-hdrop-probe.bin");
        std::fs::write(&path, b"x").unwrap();
        let bytes = hdrop_bytes(&[path.clone()]).unwrap();
        assert_eq!(&bytes[..4], &20u32.to_le_bytes());
        assert_eq!(bytes[16], 1);
        #[cfg(windows)]
        {
            assert!(iid_is(IID_IUNKNOWN.as_ptr(), &IID_IUNKNOWN));
            assert!(iid_is(IID_IDROPSOURCE.as_ptr(), &IID_IDROPSOURCE));
            assert!(!iid_is(IID_IUNKNOWN.as_ptr(), &IID_IDROPSOURCE));
        }
        let wide: Vec<u16> = bytes[20..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&wide);
        assert!(text.contains(&path.to_string_lossy().to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(hdrop_bytes(&[]).is_err());
    }
}
