//! Optional clipboard URL watch. Windows only; other targets stay empty.

pub fn looks_like_download_url(text: &str) -> bool {
    let line = text
        .lines()
        .map(str::trim)
        .find(|item| !item.is_empty())
        .unwrap_or("");
    let lower = line.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("magnet:")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
}

pub fn first_url(text: &str) -> Option<String> {
    all_urls(text).into_iter().next()
}

pub fn all_urls(text: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut urls = Vec::new();
    for raw in text.split(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'')) {
        let line = raw.trim_matches(|ch: char| matches!(ch, '(' | ')' | ',' | ';' | '.' | '。'));
        if !looks_like_download_url(line) {
            continue;
        }
        let key = line.to_ascii_lowercase();
        if seen.insert(key) {
            urls.push(line.to_string());
        }
        if urls.len() >= 100 {
            break;
        }
    }
    urls
}

pub fn read_text() -> Option<String> {
    #[cfg(windows)]
    {
        windows_clipboard()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn write_text(text: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_write(text)
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err("当前系统不支持写入剪贴板".into())
    }
}

pub fn write_files(paths: &[std::path::PathBuf]) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_write_files(paths)
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err("当前系统不支持复制文件到剪贴板".into())
    }
}

#[cfg(windows)]
fn windows_clipboard() -> Option<String> {
    unsafe {
        if OpenClipboard(0) == 0 {
            return None;
        }
        let handle = GetClipboardData(13);
        if handle == 0 {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 32_768 {
                break;
            }
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        GlobalUnlock(handle);
        CloseClipboard();
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
}

#[cfg(windows)]
fn windows_write(text: &str) -> Result<(), String> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * 2;
    unsafe {
        if OpenClipboard(0) == 0 {
            return Err("无法打开剪贴板".into());
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return Err("无法清空剪贴板".into());
        }
        let handle = GlobalAlloc(0x0002, bytes);
        if handle == 0 {
            CloseClipboard();
            return Err("无法分配剪贴板内存".into());
        }
        let ptr = GlobalLock(handle) as *mut u16;
        if ptr.is_null() {
            GlobalFree(handle);
            CloseClipboard();
            return Err("无法锁定剪贴板内存".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(handle);
        if SetClipboardData(13, handle) == 0 {
            GlobalFree(handle);
            CloseClipboard();
            return Err("无法写入剪贴板".into());
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(windows)]
fn windows_write_files(paths: &[std::path::PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("没有可复制的文件".into());
    }
    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        wide.extend(path.to_string_lossy().encode_utf16());
        wide.push(0);
    }
    wide.push(0);
    let header = 20usize;
    let bytes = header + wide.len() * 2;
    unsafe {
        if OpenClipboard(0) == 0 {
            return Err("无法打开剪贴板".into());
        }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return Err("无法清空剪贴板".into());
        }
        let handle = GlobalAlloc(0x0002, bytes);
        if handle == 0 {
            CloseClipboard();
            return Err("无法分配剪贴板内存".into());
        }
        let ptr = GlobalLock(handle) as *mut u8;
        if ptr.is_null() {
            GlobalFree(handle);
            CloseClipboard();
            return Err("无法锁定剪贴板内存".into());
        }
        std::ptr::write_bytes(ptr, 0, header);
        let p_files = header as u32;
        std::ptr::copy_nonoverlapping(p_files.to_le_bytes().as_ptr(), ptr, 4);
        *ptr.add(16) = 1;
        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr.add(header), wide.len() * 2);
        GlobalUnlock(handle);
        if SetClipboardData(15, handle) == 0 {
            GlobalFree(handle);
            CloseClipboard();
            return Err("无法写入文件到剪贴板".into());
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(owner: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, mem: isize) -> isize;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalLock(handle: isize) -> *mut core::ffi::c_void;
    fn GlobalUnlock(handle: isize) -> i32;
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalFree(handle: isize) -> isize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_magnet() {
        assert!(looks_like_download_url("https://cdn.test/a.bin"));
        assert!(looks_like_download_url("magnet:?xt=urn:btih:abc"));
        assert!(!looks_like_download_url("not a url"));
        assert_eq!(
            first_url("note\nhttps://cdn.test/a.bin\n"),
            Some("https://cdn.test/a.bin".into())
        );
    }
}
