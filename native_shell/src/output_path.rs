//! IDM-style output collision policy: rename, overwrite, or skip.

use std::fs;
use std::path::{Path, PathBuf};

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
        if dest.is_file() {
            fs::remove_file(&dest).map_err(|error| format!("overwrite output: {error}"))?;
        }
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

pub fn publish_file(
    source: &Path,
    dest: &Path,
    policy: &str,
    keep_temp: bool,
) -> Result<PathBuf, String> {
    let dest = choose_output_path(dest, policy)?;
    if dest.exists() {
        fs::remove_file(&dest).map_err(|error| format!("replace existing output: {error}"))?;
    }

    if keep_temp {
        // Keeping temporary files must have the same meaning on same-volume and
        // cross-volume work directories. A rename would consume the working
        // payload, so explicitly copy when the user asked to retain it.
        fs::copy(source, &dest)
            .map(|_| ())
            .map_err(|error| format!("publish completed output: {error}"))?;
    } else if fs::rename(source, &dest).is_err() {
        // Cross-volume publication cannot use rename. Copy the completed file,
        // then remove the working payload so a successful download does not
        // leave a second full-size copy under .hls-tasks.
        fs::copy(source, &dest)
            .map(|_| ())
            .map_err(|error| format!("publish completed output: {error}"))?;
        if let Err(error) = fs::remove_file(source) {
            eprintln!(
                "published output but could not remove temporary payload {}: {error}",
                source.display()
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
}
