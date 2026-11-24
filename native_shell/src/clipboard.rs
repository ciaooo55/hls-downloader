//! Optional clipboard URL watch. Windows only; other targets stay empty.

const MAX_CLIPBOARD_UTF16_UNITS: usize = 32_768;

pub fn looks_like_download_url(text: &str) -> bool {
    let line = text
        .lines()
        .map(str::trim)
        .find(|item| !item.is_empty())
        .unwrap_or("");
    crate::http_engine::remote_resource_url_allowed(line)
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
        let key = line.to_string();
        if seen.insert(key) {
            urls.push(line.to_string());
        }
        if urls.len() >= 100 {
            break;
        }
    }
    urls
}

/// Filters clipboard URLs for automatic background clipboard monitoring.
/// Keeps magnet/FTP/SFTP, playlist/torrent links, and HTTP(S) URLs whose path
/// ends in a recognized downloadable file extension while ignoring normal web
/// pages (`.html`, `.php`, `.aspx`, bare paths, etc.).
pub fn download_urls(text: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut urls = Vec::new();
    for raw in text.split(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'')) {
        let line = raw.trim_matches(|ch: char| matches!(ch, '(' | ')' | ',' | ';' | '.' | '。'));
        if !looks_like_download_url(line) || !is_auto_watch_download_url(line) {
            continue;
        }
        let key = line.to_string();
        if seen.insert(key) {
            urls.push(line.to_string());
        }
        if urls.len() >= 100 {
            break;
        }
    }
    urls
}

fn is_auto_watch_download_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("magnet:")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("sftp://")
    {
        return true;
    }
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    let Some((_, after_scheme)) = lower.split_once("://") else {
        return false;
    };
    let Some(path_start) = after_scheme.find('/') else {
        return false;
    };
    let path_and_query = &after_scheme[path_start..];
    let path = path_and_query.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() || path.ends_with('/') {
        return false;
    }
    let filename = path.rsplit('/').next().unwrap_or("");
    let Some((stem, ext)) = filename.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || ext.is_empty() {
        return false;
    }
    matches!(
        ext,
        // Playlists / streams / torrents / metalink
        "m3u8"
            | "mpd"
            | "m3u"
            | "torrent"
            | "meta4"
            | "metalink"
            // Video & audio
            | "mp4"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "m4v"
            | "ts"
            | "m2ts"
            | "flv"
            | "wmv"
            | "3gp"
            | "mpeg"
            | "mpg"
            | "rmvb"
            | "rm"
            | "vob"
            | "ogv"
            | "mp3"
            | "m4a"
            | "aac"
            | "flac"
            | "wav"
            | "ogg"
            | "oga"
            | "opus"
            | "wma"
            | "ac3"
            | "mka"
            | "aiff"
            | "ape"
            // Archives & disk images
            | "zip"
            | "7z"
            | "rar"
            | "tar"
            | "gz"
            | "tgz"
            | "bz2"
            | "tbz2"
            | "xz"
            | "txz"
            | "zst"
            | "lz"
            | "lzma"
            | "cab"
            | "iso"
            | "img"
            | "vhd"
            | "vhdx"
            | "wim"
            // Installers, packages & binaries
            | "exe"
            | "msi"
            | "msp"
            | "msix"
            | "msixbundle"
            | "appx"
            | "appxbundle"
            | "dmg"
            | "pkg"
            | "apk"
            | "xapk"
            | "ipa"
            | "deb"
            | "rpm"
            | "appimage"
            | "flatpak"
            | "snap"
            | "whl"
            | "jar"
            | "war"
            | "vsix"
            | "xpi"
            | "crx"
            | "nupkg"
            | "bin"
            | "dll"
            | "so"
            | "dylib"
            | "wasm"
            // Documents & ebooks
            | "pdf"
            | "epub"
            | "mobi"
            | "azw3"
            | "djvu"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "odt"
            | "ods"
            | "odp"
            | "rtf"
            | "csv"
            | "txt"
            | "md"
            | "chm"
            | "cbz"
            | "cbr"
            // Images
            | "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "avif"
            | "heic"
            | "heif"
            | "bmp"
            | "tiff"
            | "tif"
            | "ico"
            | "svg"
            | "psd"
            | "raw"
            | "cr2"
            | "nef"
            | "arw"
            | "dng"
            // Fonts / ML / data packages
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "gguf"
            | "safetensors"
            | "onnx"
            | "ckpt"
            | "pt"
            | "pth"
            | "parquet"
            | "sqlite"
            | "db"
    )
}

fn nul_terminated_len(units: &[u16]) -> Option<usize> {
    units.iter().position(|unit| *unit == 0)
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
    if text.contains('\0') {
        return Err("剪贴板文本不能包含 NUL".into());
    }
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
    hls_client::file_transfer::write_files(paths)
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
        let units =
            (GlobalSize(handle) / std::mem::size_of::<u16>()).min(MAX_CLIPBOARD_UTF16_UNITS);
        if units == 0 {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }
        let slice = std::slice::from_raw_parts(ptr, units);
        let Some(len) = nul_terminated_len(slice) else {
            GlobalUnlock(handle);
            CloseClipboard();
            return None;
        };
        let text = String::from_utf16_lossy(&slice[..len]);
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
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE,
    };
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * 2;
    unsafe {
        let handle = GlobalAlloc(0x0002, bytes);
        if handle == 0 {
            return Err("无法分配剪贴板内存".into());
        }
        let ptr = GlobalLock(handle) as *mut u16;
        if ptr.is_null() {
            GlobalFree(handle);
            return Err("无法锁定剪贴板内存".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(handle);
        // EmptyClipboard makes this window the owner before SetClipboardData.
        let class: Vec<_> = "STATIC\0".encode_utf16().collect();
        let owner = CreateWindowExW(
            0,
            class.as_ptr(),
            null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            null_mut(),
            null(),
        );
        if owner.is_null() {
            GlobalFree(handle);
            return Err("无法创建剪贴板窗口".into());
        }
        let result = if OpenClipboard(owner as isize) == 0 {
            Err("无法打开剪贴板".into())
        } else {
            let written = EmptyClipboard() != 0 && SetClipboardData(13, handle) != 0;
            CloseClipboard();
            if written {
                Ok(())
            } else {
                Err("无法写入剪贴板".into())
            }
        };
        DestroyWindow(owner);
        if result.is_err() {
            GlobalFree(handle);
        }
        result
    }
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
    fn GlobalSize(handle: isize) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_nul_is_rejected_before_opening_clipboard() {
        assert!(write_text("before\0after").unwrap_err().contains("NUL"));
    }

    #[test]
    fn accepts_http_and_magnet() {
        assert!(looks_like_download_url("https://cdn.test/a.bin"));
        assert!(looks_like_download_url("magnet:?xt=urn:btih:abc"));
        assert!(!looks_like_download_url("not a url"));
        assert!(!looks_like_download_url("javascript:alert(1)"));
        assert!(!looks_like_download_url("file:///C:/Windows/win.ini"));
        assert_eq!(
            first_url("note\nhttps://cdn.test/a.bin\n"),
            Some("https://cdn.test/a.bin".into())
        );
    }

    #[test]
    fn preserves_case_distinct_http_resources() {
        assert_eq!(
            all_urls(
                "https://cdn.test/Video.MP4 https://cdn.test/video.mp4 https://cdn.test/Video.MP4"
            ),
            vec![
                "https://cdn.test/Video.MP4".to_string(),
                "https://cdn.test/video.mp4".to_string(),
            ]
        );
    }

    #[test]
    fn bounded_clipboard_text_requires_nul_termination() {
        assert_eq!(nul_terminated_len(&[b'a' as u16, 0, b'b' as u16]), Some(1));
        assert_eq!(nul_terminated_len(&[b'a' as u16, b'b' as u16]), None);
    }

    #[test]
    fn download_urls_filters_web_pages_and_keeps_downloadable_resources() {
        let sample = "\
            https://example.com/\n\
            https://example.com\n\
            https://site.test/article/123\n\
            https://site.test/page.html\n\
            https://site.test/index.htm\n\
            https://site.test/view.php?file=movie.mp4\n\
            https://site.test/legacy.asp\n\
            https://site.test/legacy.aspx\n\
            https://site.test/portal.jsp\n\
            https://my.zip\n\
            javascript:alert(1)\n\
            file:///C:/a.zip\n\
            https://cdn.test/live/master.m3u8?token=abc\n\
            https://cdn.test/stream/manifest.mpd\n\
            https://cdn.test/list.m3u\n\
            https://cdn.test/release.torrent\n\
            https://cdn.test/archive.tar.gz\n\
            https://cdn.test/setup.EXE\n\
            https://cdn.test/video.MP4#t=10\n\
            https://cdn.test/book.pdf\n\
            https://cdn.test/photo.png\n\
            https://cdn.test/firmware.bin\n\
            magnet:?xt=urn:btih:0123456789abcdef\n\
            ftp://mirror.test/pub/file\n\
            ftps://mirror.test/pub/file\n\
            sftp://mirror.test/pub/file\n";
        let matched = download_urls(sample);
        assert_eq!(
            matched,
            vec![
                "https://cdn.test/live/master.m3u8?token=abc",
                "https://cdn.test/stream/manifest.mpd",
                "https://cdn.test/list.m3u",
                "https://cdn.test/release.torrent",
                "https://cdn.test/archive.tar.gz",
                "https://cdn.test/setup.EXE",
                "https://cdn.test/video.MP4#t=10",
                "https://cdn.test/book.pdf",
                "https://cdn.test/photo.png",
                "https://cdn.test/firmware.bin",
                "magnet:?xt=urn:btih:0123456789abcdef",
                "ftp://mirror.test/pub/file",
                "ftps://mirror.test/pub/file",
                "sftp://mirror.test/pub/file",
            ]
        );
        // Existing manual Paste Links helper stays permissive for ordinary HTTP(S) URLs.
        assert!(all_urls("https://example.com/").contains(&"https://example.com/".to_string()));
        assert!(looks_like_download_url("https://site.test/article/123"));
    }
}
