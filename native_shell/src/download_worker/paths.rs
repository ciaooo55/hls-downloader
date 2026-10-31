//! Task directories and atomic BT file-selection publication.

use super::{safe_filename, TaskSpec};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct TaskPaths {
    pub output: PathBuf,
    pub final_output: PathBuf,
    pub control: PathBuf,
    pub progress: PathBuf,
    pub torrent_selection: PathBuf,
}

impl TaskPaths {
    pub fn for_task(task_id: &str, spec: &TaskSpec) -> Result<Self, String> {
        if task_id.is_empty() || task_id.contains(['/', '\\', ':']) || matches!(task_id, "." | "..")
        {
            return Err("invalid task directory identifier".into());
        }
        let root = if !spec.download_dir.trim().is_empty() {
            PathBuf::from(&spec.download_dir)
        } else if let Some(root) = std::env::var_os("HLS_V7_DOWNLOAD_DIR") {
            PathBuf::from(root)
        } else {
            PathBuf::from("downloads")
        };
        let work_root = if spec.work_dir.trim().is_empty() {
            root.clone()
        } else {
            PathBuf::from(&spec.work_dir)
        };
        let task_dir = work_root.join(".hls-tasks").join(task_id);
        let mut final_name = if spec.resource_kind == crate::ResourceKind::Torrent
            && !spec.torrent_selection.is_empty()
        {
            format!("{}.files", safe_filename(&spec.filename, &spec.url))
        } else {
            safe_filename(&spec.filename, &spec.url)
        };
        if matches!(
            spec.resource_kind,
            crate::ResourceKind::Hls | crate::ResourceKind::Dash | crate::ResourceKind::Live
        ) {
            let extension = Path::new(&final_name)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            // 合流产物是 MP4，网页标题或清单名称不能直接作为无扩展名的最终文件。
            if extension != "mp4" {
                final_name = Path::new(&final_name)
                    .with_extension("mp4")
                    .to_string_lossy()
                    .into_owned();
            }
        }
        let mut final_output = root.join(&final_name);
        if let Some(source) = spec.local_source_path.as_deref() {
            if fs::canonicalize(source).ok() == fs::canonicalize(&final_output).ok() {
                let source_name = Path::new(&final_name);
                let stem = source_name
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("download");
                let extension = source_name
                    .extension()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty());
                let mut sequence = 2u64;
                loop {
                    let candidate_name = match extension {
                        Some(extension) => format!("{stem} ({sequence}).{extension}"),
                        None => format!("{stem} ({sequence})"),
                    };
                    let candidate = root.join(candidate_name);
                    if !candidate.exists() {
                        final_output = candidate;
                        break;
                    }
                    sequence = sequence.saturating_add(1);
                }
            }
        }
        Ok(Self {
            output: task_dir.join("payload.downloading"),
            final_output,
            control: task_dir.join("control"),
            progress: task_dir.join("progress.json"),
            torrent_selection: task_dir.join("torrent-selection.json"),
        })
    }

    pub fn prepare(&self) -> Result<(), String> {
        if let Some(parent) = self.output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create task directory: {error}"))?;
        }
        if let Some(parent) = self.final_output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create output directory: {error}"))?;
        }
        if !self.control.exists() {
            fs::write(&self.control, "run")
                .map_err(|error| format!("write task control: {error}"))?;
        }
        Ok(())
    }

    pub fn set_control(&self, value: &str) -> Result<(), String> {
        fs::write(&self.control, value).map_err(|error| format!("write task control: {error}"))
    }

    pub fn task_dir(&self) -> PathBuf {
        self.output
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.output.clone())
    }

    pub fn publish(&self) -> Result<(), String> {
        self.publish_with("overwrite", false).map(|_| ())
    }

    pub fn publish_with(&self, policy: &str, keep_temp: bool) -> Result<PathBuf, String> {
        crate::output_path::publish_file(&self.output, &self.final_output, policy, keep_temp)
    }
}

static TORRENT_SELECTION_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TORRENT_SELECTION_TEMP_ID: AtomicU64 = AtomicU64::new(1);

fn write_torrent_selection_locked(
    path: &Path,
    selections: &[crate::TorrentFileSelection],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create BT selection directory: {error}"))?;
    }
    let encoded = serde_json::to_vec(selections)
        .map_err(|error| format!("encode BT file selection: {error}"))?;
    let mut temporary_name = path.file_name().unwrap_or_default().to_os_string();
    temporary_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        TORRENT_SELECTION_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = path.with_file_name(temporary_name);
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        crate::atomic_replace::replace_file(&temporary, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result.map_err(|error| format!("publish BT file selection: {error}"))
}

pub(super) fn write_torrent_selection(
    path: &Path,
    selections: &[crate::TorrentFileSelection],
) -> Result<(), String> {
    let _guard = TORRENT_SELECTION_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "BT file selection write lock poisoned".to_string())?;
    write_torrent_selection_locked(path, selections)
}

pub(super) fn initialize_torrent_selection(
    path: &Path,
    selections: &[crate::TorrentFileSelection],
) -> Result<(), String> {
    let _guard = TORRENT_SELECTION_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "BT file selection write lock poisoned".to_string())?;
    if path.exists() {
        return Ok(());
    }
    write_torrent_selection_locked(path, selections)
}
