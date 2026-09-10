//! IDM-style output collision policy: rename, overwrite, or skip.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static OUTPUT_PUBLISH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
        eprintln!(
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
            eprintln!(
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
