//! 有界运行诊断；只记录调用点明确选择的状态字段，不写请求、URL 或凭据。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_BYTES: u64 = 5 * 1024 * 1024;
const FILE_COUNT: usize = 4;
static SENDER: OnceLock<mpsc::SyncSender<String>> = OnceLock::new();

pub fn init_runtime_log(data_dir: &Path) -> io::Result<()> {
    if SENDER.get().is_some() {
        return Ok(());
    }
    let mut log = RuntimeLog::open(data_dir.join("logs"), FILE_BYTES)?;
    let (sender, receiver) = mpsc::sync_channel::<String>(128);
    std::thread::Builder::new()
        .name("core-diagnostics".into())
        .spawn(move || {
            while let Ok(line) = receiver.recv() {
                if let Err(error) = log.append(&line) {
                    // 不能把日志盘故障升级为任务失败，也不能无限重试填满队列。
                    let _ = writeln!(
                        std::io::stderr(),
                        "Core diagnostic log unavailable: {error}"
                    );
                    break;
                }
            }
        })?;
    let _ = SENDER.set(sender);
    Ok(())
}

/// fields 仅允许诊断状态：阶段、错误码、任务 ID、耗时、计数。
/// 请求地址、标题、头、正文、密钥和未经处理的第三方错误不得传入。
pub fn record_runtime_log(event: &str, fields: serde_json::Value) {
    let Some(sender) = SENDER.get() else { return };
    let Some(line) = runtime_log_line(event, fields) else {
        return;
    };
    // 调用点可处于任务锁内；诊断拥塞不能阻塞传输或 IPC。
    let _ = sender.try_send(line);
}

fn runtime_log_line(event: &str, mut fields: serde_json::Value) -> Option<String> {
    // 第三方错误可能含有完整请求或密码；保留事件名和结构化错误码。
    let fields = fields.as_object_mut()?;
    fields.retain(|key, value| match key.as_str() {
        "task_id" | "handoff_id" | "stage" | "status" | "code" | "version" | "operation" => {
            value.as_str().is_some_and(|text| {
                text.len() <= 128
                    && text
                        .chars()
                        .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            })
        }
        "elapsed_ms" | "downloaded_bytes" | "total_bytes" | "http_status" | "attempt" | "rows" => {
            value.is_u64() || value.is_null()
        }
        "child_exited" => value.is_boolean(),
        _ => false,
    });
    let record = serde_json::json!({
        "timestamp_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
        "pid": std::process::id(), "event": event, "fields": fields,
    });
    let mut line = record.to_string();
    if line.len() > 4095 {
        return None;
    }
    line.push('\n');
    Some(line)
}

struct RuntimeLog {
    directory: PathBuf,
    file: Option<File>,
    bytes: u64,
    limit: u64,
}

impl RuntimeLog {
    fn open(directory: PathBuf, limit: u64) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("core.jsonl"))?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            directory,
            file: Some(file),
            bytes,
            limit,
        })
    }

    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(if index == 0 {
            "core.jsonl".into()
        } else {
            format!("core.{index}.jsonl")
        })
    }

    fn append(&mut self, line: &str) -> io::Result<()> {
        if line.len() as u64 > self.limit {
            return Ok(());
        }
        if self.bytes.saturating_add(line.len() as u64) > self.limit {
            self.file.take();
            let oldest = self.path(FILE_COUNT - 1);
            match fs::remove_file(oldest) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            for index in (0..FILE_COUNT - 1).rev() {
                match fs::rename(self.path(index), self.path(index + 1)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            self.file = Some(
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(self.path(0))?,
            );
            self.bytes = 0;
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("diagnostic log closed"))?
            .write_all(line.as_bytes())?;
        self.bytes += line.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_bounds_disk_usage_and_keeps_latest_records_across_restart() {
        let root = std::env::temp_dir().join(format!(
            "hls-log-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut log = RuntimeLog::open(root.clone(), 64).unwrap();
        for index in 0..30 {
            log.append(&format!("{index:02} {}\n", "x".repeat(27)))
                .unwrap();
        }
        drop(log);
        let mut log = RuntimeLog::open(root.clone(), 64).unwrap();
        log.append("latest after restart\n").unwrap();
        assert!(fs::read_to_string(root.join("core.jsonl"))
            .unwrap()
            .ends_with("latest after restart\n"));
        let files: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .collect();
        assert_eq!(files.len(), FILE_COUNT);
        assert!(files.iter().all(|bytes| *bytes <= 64));
        drop(log);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rotation_failure_preserves_existing_logs_and_does_not_truncate_them() {
        let root = std::env::temp_dir().join(format!("hls-log-blocked-{}", std::process::id()));
        let mut log = RuntimeLog::open(root.clone(), 8).unwrap();
        log.append("before\n").unwrap();
        fs::create_dir(root.join("core.3.jsonl")).unwrap();
        assert!(log.append("after\n").is_err());
        assert_eq!(
            fs::read_to_string(root.join("core.jsonl")).unwrap(),
            "before\n"
        );
        drop(log);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn error_details_are_omitted_and_individual_fields_remain_valid_json() {
        let line = runtime_log_line(
            "progress_writer_failed",
            serde_json::json!({
                "rows": 3, "error": "https://host/?token=private",
                "stage": "ftp://user:private@host", "status": "stopped", "cookie": "private"
            }),
        )
        .unwrap();
        let record: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(!line.contains("private"));
        assert_eq!(
            record["fields"],
            serde_json::json!({"rows":3, "status":"stopped"})
        );
    }
}
