//! Download path boundaries and output collision policy: rename, overwrite, or skip.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn is_reserved_dos_component(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim();
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM0"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT0"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn validate_untrusted_dir_syntax(requested: &str) -> Result<(), String> {
    use std::path::Prefix;
    if requested.chars().any(char::is_control) {
        return Err("下载目录包含非法控制字符".into());
    }
    if requested.starts_with(r"\\") || requested.starts_with("//") {
        return Err("下载目录必须位于默认下载根目录内".into());
    }
    let mut components = Path::new(requested).components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(..) => {
                    return Err("下载目录必须位于默认下载根目录内".into());
                }
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {
                    if !matches!(components.peek(), Some(Component::RootDir)) {
                        return Err("下载目录不能使用盘符相对路径".into());
                    }
                }
                Prefix::Verbatim(_) => {
                    return Err("下载目录必须位于默认下载根目录内".into());
                }
            },
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.contains(':')
                    || text.ends_with([' ', '.'])
                    || is_reserved_dos_component(&text)
                {
                    return Err("下载目录包含不支持的路径名".into());
                }
            }
            Component::ParentDir => return Err("下载目录不能包含 ..".into()),
            Component::RootDir | Component::CurDir => {}
        }
    }
    Ok(())
}

fn disk_drive_letter(path: &Path) -> Option<u8> {
    use std::path::Prefix;
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => Some(drive.to_ascii_uppercase()),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn constrain_untrusted_download_dir(
    requested: &str,
    configured: &str,
) -> Result<String, String> {
    reject_path_escape(requested)?;
    let configured = configured.trim();
    let root = PathBuf::from(if configured.is_empty() {
        "downloads"
    } else {
        configured
    });
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(root.to_string_lossy().into_owned());
    }
    validate_untrusted_dir_syntax(requested)?;
    let root_canon = logical_canonical(&root);
    let path = Path::new(requested);
    let candidate = if path.is_absolute() {
        // Reject cross-drive absolute paths before touching the filesystem via canonicalize.
        if let (Some(req_drive), Some(root_drive)) =
            (disk_drive_letter(path), disk_drive_letter(&root_canon))
        {
            if req_drive != root_drive {
                return Err("下载目录必须位于默认下载根目录内".into());
            }
        }
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let candidate_canon = logical_canonical(&candidate);
    if candidate_canon == root_canon || candidate_canon.starts_with(&root_canon) {
        return Ok(candidate.to_string_lossy().into_owned());
    }
    Err("下载目录必须位于默认下载根目录内".into())
}

pub(crate) fn logical_canonical(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        if let Ok(canon) = std::fs::canonicalize(&cur) {
            let mut out = canon;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match cur.file_name() {
            Some(name) => {
                suffix.push(OsString::from(name));
                if !cur.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    path.to_path_buf()
}

pub(crate) fn reject_path_escape(path: &str) -> Result<(), String> {
    // 设置值按 Windows 路径书写：`\\` 在任何平台上都按分隔符检查 `..`（非 Windows 的
    // `Path` 会把 `..\\Windows` 当成单个普通名字而放行）。
    if Path::new(path)
        .components()
        .any(|item| matches!(item, Component::ParentDir))
        || path.split(['/', '\\']).any(|part| part == "..")
    {
        return Err("下载目录不能包含 ..".into());
    }
    Ok(())
}

static OUTPUT_PUBLISH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn typed_output_path(
    payload: &Path,
    requested: &Path,
    mime: &str,
    disposition: &str,
) -> Result<PathBuf, String> {
    let mut head = [0u8; 4096];
    let count = fs::File::open(payload)
        .and_then(|mut file| file.read(&mut head))
        .map_err(|error| format!("read downloaded file type: {error}"))?;
    let head = &head[..count];
    let current = requested
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let server_ext = disposition_extension(disposition);
    let mime_ext = match mime.as_str() {
        "video/mp4" | "application/mp4" => "mp4",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "video/webm" | "audio/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "video/mp2t" => "ts",
        "video/x-msvideo" => "avi",
        "audio/mpeg" => "mp3",
        "audio/aac" => "aac",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" | "application/ogg" => "ogg",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/epub+zip" => "epub",
        "application/gzip" => "gz",
        "text/html" => "html",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "application/json" => "json",
        _ => "",
    };
    let zip = head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06");
    let detected = if head.len() >= 12 && &head[4..8] == b"ftyp" {
        match &head[8..12] {
            b"qt  " => "mov",
            b"M4A " | b"M4B " => "m4a",
            b"avif" | b"avis" => "avif",
            b"heic" | b"heix" => "heic",
            b"3gp4" | b"3gp5" | b"3gp6" => "3gp",
            _ => "mp4",
        }
    } else if head.starts_with(b"\x1a\x45\xdf\xa3") {
        if head.windows(4).any(|bytes| bytes == b"webm") {
            "webm"
        } else {
            "mkv"
        }
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if head.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        "gif"
    } else if head.starts_with(b"%PDF-") {
        "pdf"
    } else if head.len() >= 12 && head.starts_with(b"RIFF") {
        match &head[8..12] {
            b"WAVE" => "wav",
            b"AVI " => "avi",
            b"WEBP" => "webp",
            _ => "",
        }
    } else if head.starts_with(b"fLaC") {
        "flac"
    } else if head.starts_with(b"OggS") {
        "ogg"
    } else if looks_like_mpeg_ts(head) {
        "ts"
    } else if head.starts_with(b"ID3")
        || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe6 == 0xe2)
    {
        "mp3"
    } else if head.len() >= 2 && head[0] == 0xff && head[1] & 0xf6 == 0xf0 {
        "aac"
    } else if head.starts_with(b"\x1f\x8b") {
        "gz"
    } else if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
        "7z"
    } else if head.starts_with(b"Rar!\x1a\x07") {
        "rar"
    } else if zip {
        "zip"
    } else {
        ""
    };
    // ZIP 也是浏览器扩展和安装包的容器，改成 .zip 会破坏正常安装入口。
    let zip_subtype = |ext: &str| {
        matches!(
            ext,
            "docx"
                | "xlsx"
                | "pptx"
                | "docm"
                | "xlsm"
                | "pptm"
                | "epub"
                | "apk"
                | "jar"
                | "odt"
                | "ods"
                | "odp"
                | "xpi"
                | "whl"
                | "vsix"
                | "nupkg"
                | "msix"
                | "msixbundle"
                | "appx"
                | "appxbundle"
                | "cbz"
                | "kmz"
                | "3mf"
                | "crx"
                | "ipa"
                | "aab"
                | "xapk"
                | "kra"
                | "ora"
                | "dwfx"
                | "mcpack"
                | "mcworld"
                | "mrpack"
                | "pk3"
                | "pk4"
                | "love"
        )
    };
    let extension = if zip {
        if zip_subtype(&current) {
            current.as_str()
        } else if zip_subtype(mime_ext) {
            mime_ext
        } else if zip_subtype(&server_ext) {
            server_ext.as_str()
        } else {
            "zip"
        }
    } else if !detected.is_empty() {
        if (detected == "mp4" && matches!(current.as_str(), "m4v" | "m4a" | "m4b"))
            || (detected == "jpg" && current == "jpeg")
            || (detected == "ogg" && matches!(current.as_str(), "oga" | "ogv" | "opus"))
            || (detected == "ts" && matches!(current.as_str(), "m2ts" | "mts"))
        {
            current.as_str()
        } else {
            detected
        }
    } else if matches!(
        current.as_str(),
        "" | "bin" | "dat" | "php" | "asp" | "aspx" | "jsp" | "cgi" | "html" | "htm"
    ) {
        if !mime_ext.is_empty() {
            mime_ext
        } else if is_dangerous_executable_extension(&server_ext) {
            ""
        } else {
            server_ext.as_str()
        }
    } else {
        ""
    };
    Ok(if extension.is_empty() {
        requested.to_path_buf()
    } else {
        requested.with_extension(extension)
    })
}

fn looks_like_mpeg_ts(head: &[u8]) -> bool {
    let offset = if head.len() >= 10
        && head.starts_with(b"ID3")
        && head[3] < 0xff
        && head[4] < 0xff
        && head[6..10].iter().all(|byte| byte & 0x80 == 0)
    {
        let tag_len = ((head[6] as usize) << 21)
            | ((head[7] as usize) << 14)
            | ((head[8] as usize) << 7)
            | (head[9] as usize);
        let header_and_footer: usize = if head[5] & 0x10 != 0 { 20 } else { 10 };
        header_and_footer.saturating_add(tag_len)
    } else {
        0
    };
    let Some(rest) = head.get(offset..) else {
        return false;
    };
    (rest.len() > 376 && rest[0] == 0x47 && rest[188] == 0x47 && rest[376] == 0x47)
        || (rest.len() > 388 && rest[4] == 0x47 && rest[196] == 0x47 && rest[388] == 0x47)
}

fn is_dangerous_executable_extension(ext: &str) -> bool {
    matches!(
        ext,
        "exe"
            | "com"
            | "bat"
            | "cmd"
            | "scr"
            | "pif"
            | "msi"
            | "msp"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "ps1"
            | "psm1"
            | "psd1"
            | "hta"
            | "cpl"
            | "msc"
            | "lnk"
            | "url"
            | "reg"
            | "inf"
            | "dll"
            | "ocx"
            | "sys"
            | "drv"
            | "chm"
            | "application"
            | "gadget"
            | "scf"
            | "sct"
            | "shb"
            | "shs"
            | "pyz"
            | "pyzw"
    )
}

fn disposition_extension(value: &str) -> String {
    let filename = disposition_filename(value);
    let extension = Path::new(&filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension.len() <= 12 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        extension
    } else {
        String::new()
    }
}

pub(crate) fn disposition_filename(value: &str) -> String {
    let mut plain = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for part in value
        .split(|ch| {
            if escaped {
                escaped = false;
                return false;
            }
            if quoted && ch == '\\' {
                escaped = true;
                return false;
            }
            if ch == '"' {
                quoted = !quoted;
            }
            ch == ';' && !quoted
        })
        .skip(1)
    {
        let Some((key, raw)) = part.trim().split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !key.eq_ignore_ascii_case("filename") && !key.eq_ignore_ascii_case("filename*") {
            continue;
        }
        let raw = raw.trim().trim_matches('"');
        let name = if key.eq_ignore_ascii_case("filename*") {
            let Some(encoded) = raw.splitn(3, '\'').nth(2) else {
                continue;
            };
            let mut decoded = Vec::new();
            let bytes = encoded.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    if let (Some(a), Some(b)) = (
                        (bytes[i + 1] as char).to_digit(16),
                        (bytes[i + 2] as char).to_digit(16),
                    ) {
                        decoded.push((a * 16 + b) as u8);
                        i += 3;
                        continue;
                    }
                }
                decoded.push(bytes[i]);
                i += 1;
            }
            String::from_utf8_lossy(&decoded).into_owned()
        } else {
            raw.to_string()
        };
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            continue;
        }
        let name = crate::download_worker::safe_filename(&name, "");
        if key.eq_ignore_ascii_case("filename*") {
            return name;
        }
        plain = name;
    }
    plain
}

pub fn normalize_policy(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "overwrite" => "overwrite",
        "skip" => "skip",
        _ => "rename",
    }
}

pub fn choose_output_path(path: &Path, policy: &str) -> Result<PathBuf, String> {
    let dest = path.to_path_buf();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("create output directory: {error}"))?;
    }
    let policy = normalize_policy(policy);
    let existing = fs::symlink_metadata(&dest).ok();
    let Some(meta) = existing else {
        return Ok(dest);
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing to publish to symlink: {}",
            dest.display()
        ));
    }
    let size = if meta.is_file() { meta.len() } else { 1 };
    if policy == "skip" && size > 0 {
        return Err(format!("target already exists: {}", dest.display()));
    }
    if policy == "overwrite" || (policy == "skip" && size == 0) {
        return Ok(dest);
    }
    let stem = dest
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let ext = dest
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    for index in 1..10_000 {
        let candidate = parent.join(format!("{stem}_{index}{ext}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "cannot allocate unique output name: {}",
        dest.display()
    ))
}

pub(crate) fn stage_copy_and_replace(source: &Path, dest: &Path) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_STAGE: AtomicU64 = AtomicU64::new(1);
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let staging = parent.join(format!(
        ".hls-stage-{}-{}.tmp",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut reader =
        fs::File::open(source).map_err(|error| format!("publish completed output: {error}"))?;
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .map_err(|error| format!("create output staging file: {error}"))?;
    let result = (|| -> std::io::Result<()> {
        std::io::copy(&mut reader, &mut writer)?;
        writer.sync_all()?;
        drop(writer);
        crate::atomic_replace::replace_file(&staging, dest)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&staging);
        return Err(format!("publish completed output: {error}"));
    }
    Ok(())
}

fn publish_payload(source: &Path, dest: &Path, keep_temp: bool) -> Result<(), String> {
    if keep_temp {
        // Keeping temporary files must have the same meaning on same-volume and
        // cross-volume work directories. Stage into a sibling temp file, sync,
        // and atomically replace so partial copies are never exposed.
        return stage_copy_and_replace(source, dest);
    }

    if crate::atomic_replace::replace_file(source, dest).is_ok() {
        return Ok(());
    }

    // Cross-volume publication cannot use rename. Stage-copy into dest's
    // directory, sync, atomically replace, then remove the working payload.
    stage_copy_and_replace(source, dest)?;
    if let Err(error) = fs::remove_file(source) {
        let _ = writeln!(
            std::io::stderr(),
            "published output but could not remove temporary payload {}: {error}",
            source.display()
        );
    }
    Ok(())
}

pub fn publish_file(
    source: &Path,
    dest: &Path,
    policy: &str,
    keep_temp: bool,
) -> Result<PathBuf, String> {
    // Selection and publication must be one process-wide operation. Otherwise two
    // downloads finishing together can both select the same rename suffix and the
    // later publisher can replace the first task's completed file.
    let _publish_guard = OUTPUT_PUBLISH_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "output publication lock poisoned".to_string())?;

    let dest = choose_output_path(dest, policy)?;
    if let Ok(meta) = fs::symlink_metadata(&dest) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing to publish to symlink: {}",
                dest.display()
            ));
        }
        if !meta.is_file() {
            return Err(format!("target is not a file: {}", dest.display()));
        }
    }

    publish_payload(source, &dest, keep_temp)?;

    if !keep_temp {
        if let Some(parent) = source.parent() {
            let _ = fs::remove_file(parent.join("control"));
            let _ = fs::remove_file(parent.join("progress.json"));
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_directory_rejects_device_names_and_streams() {
        for path in ["NUL.txt", "data:stream", "folder/../elsewhere"] {
            assert!(constrain_untrusted_download_dir(path, "downloads").is_err());
        }
    }

    #[test]
    fn untrusted_download_dir_must_stay_under_configured_root() {
        let root = std::env::temp_dir().join("hls-v6-dl-root");
        let root = root.to_string_lossy().into_owned();
        assert!(
            constrain_untrusted_download_dir("", &root)
                .unwrap()
                .replace('\\', "/")
                .ends_with("hls-v6-dl-root")
                || constrain_untrusted_download_dir("", &root).unwrap() == root
        );
        assert!(constrain_untrusted_download_dir("nested", &root)
            .unwrap()
            .contains("nested"));
        assert!(constrain_untrusted_download_dir("../escape", &root).is_err());
        #[cfg(windows)]
        assert!(constrain_untrusted_download_dir(r"C:\Windows", &root).is_err());
        #[cfg(not(windows))]
        assert!(constrain_untrusted_download_dir("/etc", &root).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn untrusted_download_dir_rejects_junction_escape() {
        use std::process::Command;
        let stamp = std::process::id();
        let root = std::env::temp_dir().join(format!("hls-v6-junc-root-{stamp}"));
        let outside = std::env::temp_dir().join(format!("hls-v6-junc-out-{stamp}"));
        let junction = root.join("escape");
        let _ = fs::create_dir_all(&root);
        let _ = fs::create_dir_all(&outside);
        let _ = fs::remove_dir(&junction);
        let ok = Command::new("cmd")
            .args([
                "/D",
                "/C",
                "mklink",
                "/J",
                &junction.to_string_lossy(),
                &outside.to_string_lossy(),
            ])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !ok {
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_dir_all(&outside);
            return;
        }
        let requested = junction.to_string_lossy().into_owned();
        let configured = root.to_string_lossy().into_owned();
        assert!(constrain_untrusted_download_dir(&requested, &configured).is_err());
        let _ = fs::remove_dir(&junction);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hls-v7-output-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn rename_allocates_suffix() {
        let dir = test_dir("rename");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("clip.mp4");
        fs::write(&first, b"one").unwrap();
        let next = choose_output_path(&first, "rename").unwrap();
        assert_eq!(next.file_name().unwrap(), "clip_1.mp4");
        let skip = choose_output_path(&first, "skip").unwrap_err();
        assert!(skip.contains("already exists"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn keep_temp_copies_output_without_consuming_working_payload() {
        let dir = test_dir("keep-temp");
        let work = dir.join("work");
        let output = dir.join("downloads").join("clip.mp4");
        fs::create_dir_all(&work).unwrap();
        let source = work.join("payload.downloading");
        fs::write(&source, b"payload").unwrap();
        fs::write(work.join("control"), b"run").unwrap();
        fs::write(work.join("progress.json"), b"{}").unwrap();

        let published = publish_file(&source, &output, "overwrite", true).unwrap();
        assert_eq!(published, output);
        assert_eq!(fs::read(&published).unwrap(), b"payload");
        assert_eq!(fs::read(&source).unwrap(), b"payload");
        assert!(work.join("control").exists());
        assert!(work.join("progress.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn normal_publish_consumes_working_payload_and_control_files() {
        let dir = test_dir("consume-temp");
        let work = dir.join("work");
        let output = dir.join("downloads").join("clip.mp4");
        fs::create_dir_all(&work).unwrap();
        let source = work.join("payload.downloading");
        fs::write(&source, b"payload").unwrap();
        fs::write(work.join("control"), b"run").unwrap();
        fs::write(work.join("progress.json"), b"{}").unwrap();

        let published = publish_file(&source, &output, "overwrite", false).unwrap();
        assert_eq!(published, output);
        assert_eq!(fs::read(&published).unwrap(), b"payload");
        assert!(!source.exists());
        assert!(!work.join("control").exists());
        assert!(!work.join("progress.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn overwrite_replaces_existing_file_only_after_publish_can_start() {
        let dir = test_dir("overwrite-success");
        let work = dir.join("work");
        let output = dir.join("downloads").join("clip.mp4");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        let source = work.join("payload.downloading");
        fs::write(&source, b"new-payload").unwrap();
        fs::write(&output, b"old-payload").unwrap();

        let published = publish_file(&source, &output, "overwrite", false).unwrap();
        assert_eq!(published, output);
        assert_eq!(fs::read(&published).unwrap(), b"new-payload");
        assert!(!source.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn overwrite_failure_restores_existing_file() {
        let dir = test_dir("overwrite-restore");
        let output = dir.join("downloads").join("clip.mp4");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, b"old-payload").unwrap();
        let missing_source = dir.join("work").join("missing.downloading");

        let error = publish_file(&missing_source, &output, "overwrite", false).unwrap_err();
        assert!(error.contains("publish completed output"));
        assert_eq!(fs::read(&output).unwrap(), b"old-payload");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_rename_publication_keeps_every_completed_file() {
        let dir = test_dir("concurrent-rename");
        let work = dir.join("work");
        let output = dir.join("downloads").join("clip.mp4");
        fs::create_dir_all(&work).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
        let mut workers = Vec::new();
        for index in 0..4 {
            let source = work.join(format!("payload-{index}.downloading"));
            fs::write(&source, format!("payload-{index}")).unwrap();
            let output = output.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                publish_file(&source, &output, "rename", false).unwrap()
            }));
        }
        barrier.wait();

        let published: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let unique: std::collections::HashSet<_> = published.iter().cloned().collect();
        assert_eq!(unique.len(), 4);
        for path in published {
            assert!(path.is_file());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn typed_output_path_preserves_safe_zip_container_extensions_and_blocks_dangerous_ones() {
        let dir = test_dir("zip-containers");
        fs::create_dir_all(&dir).unwrap();
        let payload = dir.join("payload.downloading");
        fs::write(&payload, b"PK\x03\x04\x14\x00\x00\x00\x08\x00test-zip-body").unwrap();

        for safe_name in [
            "map.kmz",
            "model.3mf",
            "extension.crx",
            "app.ipa",
            "bundle.aab",
            "bundle.xapk",
            "painting.kra",
            "layered.ora",
            "drawing.dwfx",
            "addon.mcpack",
            "world.mcworld",
            "modpack.mrpack",
            "pak0.pk3",
            "pak1.pk4",
            "game.love",
        ] {
            let requested = dir.join(safe_name);
            let resolved =
                typed_output_path(&payload, &requested, "application/octet-stream", "").unwrap();
            assert_eq!(resolved, requested, "must preserve {safe_name}");
        }

        for (disp_name, expected_ext) in [("map.kmz", "kmz"), ("model.3mf", "3mf")] {
            let resolved = typed_output_path(
                &payload,
                &dir.join("download.bin"),
                "application/octet-stream",
                &format!("attachment; filename=\"{disp_name}\""),
            )
            .unwrap();
            assert_eq!(
                resolved.extension().and_then(|e| e.to_str()),
                Some(expected_ext),
                "Content-Disposition {disp_name} must preserve .{expected_ext}"
            );
        }

        for repair_name in [
            "archive",
            "download.bin",
            "export.php",
            "temp.tmp",
            "release-v1.2",
            "export.do",
            "wrong.mp4",
        ] {
            let requested = dir.join(repair_name);
            let resolved =
                typed_output_path(&payload, &requested, "application/octet-stream", "").unwrap();
            assert_eq!(
                resolved.extension().and_then(|e| e.to_str()),
                Some("zip"),
                "{repair_name} should resolve to .zip"
            );
        }

        let docx_from_mime = typed_output_path(
            &payload,
            &dir.join("download.bin"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "",
        )
        .unwrap();
        assert_eq!(
            docx_from_mime.extension().and_then(|e| e.to_str()),
            Some("docx")
        );

        for dangerous_name in [
            "disguised.exe",
            "evil.pyz",
            "evil.pyzw",
            "runner.bat",
            "script.ps1",
            "setup.msi",
        ] {
            let requested = dir.join(dangerous_name);
            let resolved =
                typed_output_path(&payload, &requested, "application/octet-stream", "").unwrap();
            assert_eq!(
                resolved.extension().and_then(|e| e.to_str()),
                Some("zip"),
                "dangerous {dangerous_name} must never pass through unchanged"
            );
        }

        let unknown_payload = dir.join("unknown.downloading");
        fs::write(&unknown_payload, b"\x00\x01\x02\x03opaque-bytes").unwrap();
        let blocked_pyz_disp = typed_output_path(
            &unknown_payload,
            &dir.join("download.bin"),
            "application/octet-stream",
            "attachment; filename=\"evil.pyz\"",
        )
        .unwrap();
        assert_eq!(
            blocked_pyz_disp,
            dir.join("download.bin"),
            "dangerous .pyz Content-Disposition must be blocked"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn typed_output_path_detects_id3_prefixed_mpeg_ts_and_m2ts_without_breaking_mp3() {
        let dir = test_dir("id3-ts");
        fs::create_dir_all(&dir).unwrap();

        // 1. ID3v2 header (10-byte header + 32-byte body) followed by 3 x 188-byte MPEG-TS packets.
        let mut id3_ts = Vec::new();
        id3_ts.extend_from_slice(b"ID3\x04\x00\x00\x00\x00\x00\x20");
        id3_ts.extend_from_slice(&[0u8; 32]);
        for _ in 0..3 {
            let mut pkt = [0u8; 188];
            pkt[0] = 0x47;
            id3_ts.extend_from_slice(&pkt);
        }
        let ts_payload = dir.join("seg.downloading");
        fs::write(&ts_payload, &id3_ts).unwrap();
        let resolved_ts = typed_output_path(
            &ts_payload,
            &dir.join("segment.bin"),
            "application/octet-stream",
            "",
        )
        .unwrap();
        assert_eq!(resolved_ts.extension().and_then(|e| e.to_str()), Some("ts"));

        // 2. ID3v2 with 0x10 footer flag (+20 header/footer) + 192-byte M2TS packets (sync at +4).
        let mut id3_m2ts = Vec::new();
        id3_m2ts.extend_from_slice(b"ID3\x04\x00\x10\x00\x00\x00\x10");
        id3_m2ts.extend_from_slice(&[0u8; 26]); // 16-byte body + 10-byte footer
        for _ in 0..3 {
            let mut pkt = [0u8; 192];
            pkt[4] = 0x47;
            id3_m2ts.extend_from_slice(&pkt);
        }
        let m2ts_payload = dir.join("bdav.downloading");
        fs::write(&m2ts_payload, &id3_m2ts).unwrap();
        let resolved_m2ts = typed_output_path(
            &m2ts_payload,
            &dir.join("clip.bin"),
            "application/octet-stream",
            "",
        )
        .unwrap();
        assert_eq!(
            resolved_m2ts.extension().and_then(|e| e.to_str()),
            Some("ts")
        );

        // 3. Regular ID3v2 MP3 must still detect as mp3.
        let mut mp3_bytes = Vec::from(&b"ID3\x03\x00\x00\x00\x00\x00\x10"[..]);
        mp3_bytes.extend_from_slice(&[0u8; 16]);
        mp3_bytes.extend_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
        let mp3_payload = dir.join("song.downloading");
        fs::write(&mp3_payload, &mp3_bytes).unwrap();
        let resolved_mp3 = typed_output_path(
            &mp3_payload,
            &dir.join("track.bin"),
            "application/octet-stream",
            "",
        )
        .unwrap();
        assert_eq!(
            resolved_mp3.extension().and_then(|e| e.to_str()),
            Some("mp3")
        );

        let _ = fs::remove_dir_all(dir);
    }
}
