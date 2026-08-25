use crate::{ResourceKind, TaskSnapshot, TaskSpec, DEFAULT_QUEUE_ID};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const MAX_IMPORT_BYTES: u64 = 1024 * 1024;
const MAX_IMPORT_TASKS: usize = 100;

#[derive(Serialize, Deserialize)]
struct ExportDocument {
    schema: String,
    #[serde(default)]
    product_version: String,
    tasks: Vec<ExportTask>,
}

#[derive(Serialize, Deserialize)]
struct ExportTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    url: String,
    #[serde(default = "default_resource_kind")]
    resource_kind: ResourceKind,
    #[serde(default)]
    status: String,
    #[serde(default)]
    downloaded_bytes: u64,
    #[serde(default)]
    total_bytes: Option<u64>,
    #[serde(default = "default_request_method")]
    request_method: String,
    #[serde(default)]
    download_dir: String,
    #[serde(default)]
    speed_limit_kib: u32,
    #[serde(default)]
    expected_checksum: String,
    #[serde(default)]
    max_workers: u32,
    #[serde(default)]
    mirrors: Vec<String>,
    #[serde(default)]
    scheduled_start_at: String,
    #[serde(default)]
    scheduled_stop_at: String,
    #[serde(default)]
    queue_id: String,
}

fn default_resource_kind() -> ResourceKind {
    ResourceKind::File
}

fn default_request_method() -> String {
    "GET".into()
}

impl From<&TaskSnapshot> for ExportTask {
    fn from(task: &TaskSnapshot) -> Self {
        Self {
            id: task.task_id.clone(),
            title: task.title.clone(),
            filename: task.filename.clone(),
            url: task.url.clone(),
            resource_kind: task.resource_kind.clone(),
            status: task.status.clone(),
            downloaded_bytes: task.downloaded_bytes,
            total_bytes: task.total_bytes,
            request_method: task.request_method.clone(),
            download_dir: task.download_dir.clone(),
            speed_limit_kib: task.speed_limit_kib,
            expected_checksum: task.expected_checksum.clone(),
            max_workers: task.max_workers,
            mirrors: task.mirrors.clone(),
            scheduled_start_at: task.scheduled_start_at.clone(),
            scheduled_stop_at: task.scheduled_stop_at.clone(),
            queue_id: task.queue_id.clone(),
        }
    }
}

pub fn import_tasks_from_source(source: &str) -> Result<Option<Vec<TaskSpec>>, String> {
    let Some(path) = crate::link_file::local_source_path(source) else {
        return Ok(None);
    };
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("json"))
    {
        return Ok(None);
    }
    Ok(Some(import_tasks_from_path(&path)?))
}

fn import_tasks_from_path(path: &Path) -> Result<Vec<TaskSpec>, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("读取任务列表失败: {error}"))?;
    if metadata.len() == 0 || metadata.len() > MAX_IMPORT_BYTES {
        return Err("任务 JSON 为空或超过 1 MiB".into());
    }
    let bytes = fs::read(path).map_err(|error| format!("读取任务列表失败: {error}"))?;
    let text = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .map_err(|_| "任务 JSON 必须使用 UTF-8 编码".to_string())?;
    import_tasks(text)
}

pub fn import_tasks(text: &str) -> Result<Vec<TaskSpec>, String> {
    let document: ExportDocument =
        serde_json::from_str(text).map_err(|error| format!("任务 JSON 格式无效: {error}"))?;
    if document.schema != "hls-downloader.tasks.v1" {
        return Err("任务 JSON 架构不受支持".into());
    }
    if document.tasks.is_empty() || document.tasks.len() > MAX_IMPORT_TASKS {
        return Err("任务 JSON 必须包含 1 到 100 个任务".into());
    }
    document
        .tasks
        .into_iter()
        .map(|task| {
            let url = task.url.trim().to_string();
            if !crate::http_engine::remote_resource_url_allowed(&url) {
                return Err("任务 JSON 包含不受支持的链接".to_string());
            }
            if task.mirrors.len() > 16
                || task
                    .mirrors
                    .iter()
                    .any(|mirror| !crate::http_engine::remote_resource_url_allowed(mirror))
            {
                return Err("任务 JSON 包含无效或过多的镜像".to_string());
            }
            Ok(TaskSpec {
                url,
                resource_kind: task.resource_kind,
                title: task.title,
                filename: task.filename,
                download_dir: task.download_dir,
                request_method: crate::http_engine::sanitize_http_method(&task.request_method),
                concurrency: task.max_workers.min(128),
                checksum: (!task.expected_checksum.trim().is_empty())
                    .then_some(task.expected_checksum),
                expected_size: task.total_bytes.filter(|size| *size > 0),
                mirrors: task.mirrors,
                speed_limit_kib: task.speed_limit_kib.min(1_048_576),
                scheduled_start_at: task.scheduled_start_at,
                scheduled_stop_at: task.scheduled_stop_at,
                queue_id: if task.queue_id.trim().is_empty() {
                    DEFAULT_QUEUE_ID.into()
                } else {
                    task.queue_id
                },
                ..TaskSpec::default()
            })
        })
        .collect()
}

pub fn export_tasks(
    tasks: &[TaskSnapshot],
    task_ids: &[String],
    requested_format: &str,
) -> Result<(String, String, usize), String> {
    let selected: HashSet<&str> = task_ids.iter().map(String::as_str).collect();
    let mut tasks: Vec<&TaskSnapshot> = tasks
        .iter()
        .filter(|task| selected.is_empty() || selected.contains(task.task_id.as_str()))
        .collect();
    tasks.sort_by(|left, right| {
        (left.queue_index, left.task_id.as_str()).cmp(&(right.queue_index, right.task_id.as_str()))
    });
    if tasks.is_empty() {
        return Err("没有可导出的任务".into());
    }

    let format = match requested_format.trim().to_ascii_lowercase().as_str() {
        "json" => "json",
        "csv" => "csv",
        "txt" | "urls" => "urls",
        _ => return Err("导出格式仅支持 JSON、CSV 或 URL 列表".into()),
    };
    let count = tasks.len();
    let data = match format {
        "json" => serde_json::to_string_pretty(&ExportDocument {
            schema: "hls-downloader.tasks.v1".into(),
            product_version: "7.0.0".into(),
            tasks: tasks.into_iter().map(ExportTask::from).collect(),
        })
        .map_err(|error| format!("任务序列化失败: {error}"))?,
        "csv" => export_csv(&tasks),
        _ => tasks
            .iter()
            .map(|task| task.url.trim())
            .filter(|url| !url.is_empty())
            .collect::<Vec<_>>()
            .join("\r\n"),
    };
    Ok((format.into(), data, count))
}

fn export_csv(tasks: &[&TaskSnapshot]) -> String {
    let mut lines = vec!["id,filename,title,status,resource_kind,url,downloaded_bytes,total_bytes,request_method,download_dir,speed_limit_kib,expected_checksum,max_workers,mirrors,scheduled_start_at,scheduled_stop_at".to_string()];
    lines.extend(tasks.iter().map(|task| {
        [
            task.task_id.clone(),
            task.filename.clone(),
            task.title.clone(),
            task.status.clone(),
            serde_json::to_value(&task.resource_kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "file".into()),
            task.url.clone(),
            task.downloaded_bytes.to_string(),
            task.total_bytes
                .map(|value| value.to_string())
                .unwrap_or_default(),
            task.request_method.clone(),
            task.download_dir.clone(),
            task.speed_limit_kib.to_string(),
            task.expected_checksum.clone(),
            task.max_workers.to_string(),
            task.mirrors.join("|"),
            task.scheduled_start_at.clone(),
            task.scheduled_stop_at.clone(),
        ]
        .into_iter()
        .map(csv_cell)
        .collect::<Vec<_>>()
        .join(",")
    }));
    lines.join("\r\n")
}

fn csv_cell(value: String) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, queue_index: i64, filename: &str, url: &str) -> TaskSnapshot {
        TaskSnapshot {
            task_id: id.into(),
            queue_index,
            filename: filename.into(),
            title: filename.into(),
            url: url.into(),
            status: "paused".into(),
            ..TaskSnapshot::default()
        }
    }

    #[test]
    fn json_export_is_stable_filtered_and_excludes_runtime_logs() {
        let mut later = task("later", 2, "later.bin", "https://example.test/later");
        later.log_tail = vec!["private runtime detail".into()];
        let earlier = task("earlier", 1, "earlier.bin", "https://example.test/earlier");
        let (_, data, count) =
            export_tasks(&[later, earlier], &["earlier".into()], "json").unwrap();
        assert_eq!(count, 1);
        assert!(data.contains("hls-downloader.tasks.v1"));
        assert!(data.contains("earlier.bin"));
        assert!(!data.contains("later.bin"));
        assert!(!data.contains("log_tail"));
    }

    #[test]
    fn csv_quotes_hostile_cells_and_url_export_uses_queue_order() {
        let later = task(
            "later",
            2,
            "quote\"and,comma.bin",
            "https://example.test/later",
        );
        let earlier = task("earlier", 1, "earlier.bin", "https://example.test/earlier");
        let (_, csv, _) = export_tasks(&[later.clone(), earlier.clone()], &[], "csv").unwrap();
        assert!(csv.contains("\"quote\"\"and,comma.bin\""));
        let (format, urls, count) = export_tasks(&[later, earlier], &[], "txt").unwrap();
        assert_eq!(format, "urls");
        assert_eq!(count, 2);
        assert_eq!(
            urls,
            "https://example.test/earlier\r\nhttps://example.test/later"
        );
    }

    #[test]
    fn export_rejects_unknown_formats_and_empty_selection() {
        assert!(export_tasks(
            &[task("one", 0, "one.bin", "https://example.test/one")],
            &[],
            "xml"
        )
        .is_err());
        assert!(export_tasks(&[], &[], "json").is_err());
    }

    #[test]
    fn exported_json_imports_task_configuration_without_runtime_state() {
        let mut source = task("one", 0, "video.mp4", "https://cdn.test/video.mp4");
        source.download_dir = "downloads".into();
        source.speed_limit_kib = 512;
        source.max_workers = 6;
        source.expected_checksum = "sha256:abc".into();
        source.mirrors = vec!["https://mirror.test/video.mp4".into()];
        source.scheduled_start_at = "22:30".into();
        source.queue_id = "default".into();
        let (_, data, _) = export_tasks(&[source], &[], "json").unwrap();

        let imported = import_tasks(&data).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].url, "https://cdn.test/video.mp4");
        assert_eq!(imported[0].filename, "video.mp4");
        assert_eq!(imported[0].concurrency, 6);
        assert_eq!(imported[0].speed_limit_kib, 512);
        assert_eq!(imported[0].checksum.as_deref(), Some("sha256:abc"));
        assert_eq!(imported[0].mirrors, vec!["https://mirror.test/video.mp4"]);
        assert_eq!(imported[0].scheduled_start_at, "22:30");
        assert!(imported[0].credential_ref.is_none());
        assert!(imported[0].replay_context_ref.is_none());
    }

    #[test]
    fn task_import_rejects_foreign_schema_and_unsafe_urls() {
        assert!(import_tasks(r#"{"schema":"other","tasks":[{}]}"#).is_err());
        assert!(import_tasks(
            r#"{"schema":"hls-downloader.tasks.v1","tasks":[{"url":"file:///secret"}]}"#
        )
        .is_err());
    }
}
