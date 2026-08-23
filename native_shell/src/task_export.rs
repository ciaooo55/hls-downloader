use crate::{ResourceKind, TaskSnapshot};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize)]
struct ExportDocument {
    schema: &'static str,
    product_version: &'static str,
    tasks: Vec<ExportTask>,
}

#[derive(Serialize)]
struct ExportTask {
    id: String,
    title: String,
    filename: String,
    url: String,
    resource_kind: ResourceKind,
    status: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    request_method: String,
    download_dir: String,
    speed_limit_kib: u32,
    expected_checksum: String,
    max_workers: u32,
    mirrors: Vec<String>,
    scheduled_start_at: String,
    scheduled_stop_at: String,
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
        }
    }
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
            schema: "hls-downloader.tasks.v1",
            product_version: "7.0.0",
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
}
