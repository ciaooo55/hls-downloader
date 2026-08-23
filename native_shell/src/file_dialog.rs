//! Native file pickers for import/export. Windows-first.

use std::path::PathBuf;

pub fn pick_import_paths() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        windows_open()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub fn pick_export_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        windows_save()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_open() -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
        OPENFILENAMEW,
    };

    let mut buffer = vec![0u16; 32_768];
    let mut filter: Vec<u16> = "Download sources\0*.torrent;*.url;*.magnet;*.m3u;*.m3u8;*.mpd;*.html;*.htm;*.meta4;*.metalink;*.txt\0All files\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut ofn = unsafe { std::mem::zeroed::<OPENFILENAMEW>() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_mut_ptr();
    ofn.lpstrFile = buffer.as_mut_ptr();
    ofn.nMaxFile = buffer.len() as u32;
    ofn.Flags = OFN_EXPLORER | OFN_ALLOWMULTISELECT | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return Vec::new();
    }
    parse_multi_select(&buffer)
}

#[cfg(windows)]
fn windows_save() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_OVERWRITEPROMPT, OPENFILENAMEW,
    };

    let mut buffer: Vec<u16> = "hls-links.txt".encode_utf16().collect();
    buffer.resize(1024, 0);
    let mut filter: Vec<u16> = "Link list\0*.txt\0All files\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut ofn = unsafe { std::mem::zeroed::<OPENFILENAMEW>() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_mut_ptr();
    ofn.lpstrFile = buffer.as_mut_ptr();
    ofn.nMaxFile = buffer.len() as u32;
    ofn.Flags = OFN_OVERWRITEPROMPT;
    if unsafe { GetSaveFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..end])))
}

#[cfg(windows)]
fn parse_multi_select(buffer: &[u16]) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, unit) in buffer.iter().enumerate() {
        if *unit == 0 {
            if index == start {
                break;
            }
            parts.push(std::ffi::OsString::from_wide(&buffer[start..index]));
            start = index + 1;
        }
    }
    if parts.is_empty() {
        return Vec::new();
    }
    if parts.len() == 1 {
        return vec![PathBuf::from(&parts[0])];
    }
    let dir = PathBuf::from(&parts[0]);
    parts[1..].iter().map(|name| dir.join(name)).collect()
}
