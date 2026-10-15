//! IDM-style output collision policy: rename, overwrite, or skip.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static OUTPUT_PUBLISH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn typed_output_path(payload: &Path, requested: &Path, mime: &str, disposition: &str) -> Result<PathBuf, String> {
    let mut head = [0u8; 4096];
    let count = fs::File::open(payload).and_then(|mut file| file.read(&mut head))
        .map_err(|error| format!("read downloaded file type: {error}"))?;
    let head = &head[..count];
    let current = requested.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let server_ext = disposition_extension(disposition);
    let mime_ext = match mime.as_str() {
        "video/mp4" | "application/mp4" => "mp4", "audio/mp4" | "audio/x-m4a" => "m4a",
        "video/webm" | "audio/webm" => "webm", "video/x-matroska" => "mkv",
        "video/quicktime" => "mov", "video/mp2t" => "ts", "video/x-msvideo" => "avi",
        "audio/mpeg" => "mp3", "audio/aac" => "aac", "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav", "audio/ogg" | "application/ogg" => "ogg",
        "image/jpeg" => "jpg", "image/png" => "png", "image/gif" => "gif", "image/webp" => "webp",
        "image/avif" => "avif", "application/pdf" => "pdf", "application/zip" => "zip",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/epub+zip" => "epub", "application/gzip" => "gz",
        "text/html" => "html", "text/plain" => "txt", "text/csv" => "csv",
        "application/json" => "json", _ => "",
    };
    let zip = head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06");
    let detected = if head.len() >= 12 && &head[4..8] == b"ftyp" {
        match &head[8..12] {
            b"qt  " => "mov", b"M4A " | b"M4B " => "m4a",
            b"avif" | b"avis" => "avif", b"heic" | b"heix" => "heic",
            b"3gp4" | b"3gp5" | b"3gp6" => "3gp", _ => "mp4",
        }
    } else if head.starts_with(b"\x1a\x45\xdf\xa3") {
        if head.windows(4).any(|bytes| bytes == b"webm") { "webm" } else { "mkv" }
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") { "png"
    } else if head.starts_with(b"\xff\xd8\xff") { "jpg"
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") { "gif"
    } else if head.starts_with(b"%PDF-") { "pdf"
    } else if head.len() >= 12 && head.starts_with(b"RIFF") {
        match &head[8..12] { b"WAVE" => "wav", b"AVI " => "avi", b"WEBP" => "webp", _ => "" }
    } else if head.starts_with(b"fLaC") { "flac"
    } else if head.starts_with(b"OggS") { "ogg"
    } else if head.starts_with(b"ID3") || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe6 == 0xe2) { "mp3"
    } else if head.len() >= 2 && head[0] == 0xff && head[1] & 0xf6 == 0xf0 { "aac"
    } else if head.len() > 376 && head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47 { "ts"
    } else if head.starts_with(b"\x1f\x8b") { "gz"
    } else if head.starts_with(b"7z\xbc\xaf\x27\x1c") { "7z"
    } else if head.starts_with(b"Rar!\x1a\x07") { "rar"
    } else if zip { "zip"
    } else { "" };
    // ZIP 也是浏览器扩展和安装包的容器，改成 .zip 会破坏正常安装入口。
    let zip_subtype = |ext: &str| matches!(ext, "docx" | "xlsx" | "pptx" | "docm" | "xlsm" | "pptm" | "epub" | "apk" | "jar" | "odt" | "ods" | "odp"
        | "xpi" | "whl" | "vsix" | "nupkg" | "msix" | "msixbundle" | "appx" | "appxbundle" | "cbz");
    let extension = if zip {
        if zip_subtype(&current) { current.as_str() }
        else if zip_subtype(mime_ext) { mime_ext }
        else if zip_subtype(&server_ext) { server_ext.as_str() }
        else { "zip" }
    } else if !detected.is_empty() {
        if (detected == "mp4" && matches!(current.as_str(), "m4v" | "m4a" | "m4b"))
            || (detected == "jpg" && current == "jpeg")
            || (detected == "ogg" && matches!(current.as_str(), "oga" | "ogv" | "opus")) { current.as_str() }
        else { detected }
    } else if matches!(current.as_str(), "" | "bin" | "dat" | "php" | "asp" | "aspx" | "jsp" | "cgi" | "html" | "htm") {
        if !mime_ext.is_empty() { mime_ext } else { server_ext.as_str() }
    } else { "" };
    Ok(if extension.is_empty() { requested.to_path_buf() } else { requested.with_extension(extension) })
}

fn disposition_extension(value: &str) -> String {
    let mut plain = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for part in value.split(|ch| {
        if escaped { escaped = false; return false; }
        if quoted && ch == '\\' { escaped = true; return false; }
        if ch == '"' { quoted = !quoted; }
        ch == ';' && !quoted
    }).skip(1) {
        let Some((key, raw)) = part.trim().split_once('=') else { continue; };
        let key = key.trim();
        if !key.eq_ignore_ascii_case("filename") && !key.eq_ignore_ascii_case("filename*") { continue; }
        let raw = raw.trim().trim_matches('"');
        let name = if key.eq_ignore_ascii_case("filename*") {
            let Some(encoded) = raw.splitn(3, '\'').nth(2) else { continue; };
            let mut decoded = Vec::new();
            let bytes = encoded.as_bytes();let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    if let (Some(a),Some(b)) = ((bytes[i+1] as char).to_digit(16),(bytes[i+2] as char).to_digit(16)) {
                        decoded.push((a*16+b) as u8);i+=3;continue;
                    }
                }
                decoded.push(bytes[i]);i+=1;
            }
            String::from_utf8_lossy(&decoded).into_owned()
        } else { raw.to_string() };
        let extension = Path::new(&name).extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
        if extension.is_empty() || extension.len()>12 || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) { continue; }
        if key.eq_ignore_ascii_case("filename*") { return extension; }
        plain=extension;
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
    if !dest.exists() {
        return Ok(dest);
    }
    let size = fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0);
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
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "cannot allocate unique output name: {}",
        dest.display()
    ))
}

fn backup_path(dest: &Path) -> Result<PathBuf, String> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let original = dest.file_name().unwrap_or_default();
    for index in 0..10_000 {
        let mut name = original.to_os_string();
        name.push(format!(".hls-backup-{}-{index}", std::process::id()));
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "cannot allocate output backup name: {}",
        dest.display()
    ))
}

fn publish_payload(source: &Path, dest: &Path, keep_temp: bool) -> Result<(), String> {
    if keep_temp {
        // Keeping temporary files must have the same meaning on same-volume and
        // cross-volume work directories. A rename would consume the working
        // payload, so explicitly copy when the user asked to retain it.
        return fs::copy(source, dest)
            .map(|_| ())
            .map_err(|error| format!("publish completed output: {error}"));
    }

    if fs::rename(source, dest).is_ok() {
        return Ok(());
    }

    // Cross-volume publication cannot use rename. Copy the completed file,
    // then remove the working payload so a successful download does not leave
    // a second full-size copy under .hls-tasks.
    fs::copy(source, dest)
        .map(|_| ())
        .map_err(|error| format!("publish completed output: {error}"))?;
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
    let backup = if dest.exists() {
        if !dest.is_file() {
            return Err(format!("target is not a file: {}", dest.display()));
        }
        let backup = backup_path(&dest)?;
        fs::rename(&dest, &backup)
            .map_err(|error| format!("prepare existing output replacement: {error}"))?;
        Some(backup)
    } else {
        None
    };

    if let Err(error) = publish_payload(source, &dest, keep_temp) {
        if dest.exists() {
            let _ = fs::remove_file(&dest);
        }
        if let Some(backup) = &backup {
            if let Err(restore_error) = fs::rename(backup, &dest) {
                return Err(format!(
                    "{error}; restore existing output {} failed: {restore_error}; backup kept at {}",
                    dest.display(),
                    backup.display()
                ));
            }
        }
        return Err(error);
    }

    if let Some(backup) = backup {
        if let Err(error) = fs::remove_file(&backup) {
            let _ = writeln!(
                std::io::stderr(),
                "published output but could not remove previous-output backup {}: {error}",
                backup.display()
            );
        }
    }

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
}
