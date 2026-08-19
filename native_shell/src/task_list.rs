//! Native task list: filter, search, and row paint data.
//!
//! The Win32 window only draws what this module already decided. Headless
//! tests cover the same 全部 / 未完成 / 已完成 + file-category rules.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const ACTIVE: &[&str] = &[
    "queued",
    "awaiting_confirmation",
    "fetching_metadata",
    "awaiting_selection",
    "checking",
    "downloading",
    "downloading_m3u8",
    "parsing",
    "downloading_segments",
    "pausing",
    "paused",
    "merging",
    "remuxing",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusFilter {
    #[default]
    All,
    Unfinished,
    Completed,
}

impl StatusFilter {
    pub fn as_id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unfinished => "unfinished",
            Self::Completed => "completed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Unfinished => "未完成",
            Self::Completed => "已完成",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCategory {
    #[default]
    All,
    Video,
    Music,
    Archive,
    Document,
    Program,
    General,
}

impl FileCategory {
    pub fn as_id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Video => "video",
            Self::Music => "music",
            Self::Archive => "archive",
            Self::Document => "document",
            Self::Program => "program",
            Self::General => "general",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部类型",
            Self::Video => "视频",
            Self::Music => "音乐",
            Self::Archive => "压缩包",
            Self::Document => "文档",
            Self::Program => "程序",
            Self::General => "常规",
        }
    }

    pub fn from_file(path: &str, mime_type: &str, task_type: &str) -> Self {
        let kind = task_type.trim().to_ascii_lowercase();
        if kind == "hls" || kind == "dash" {
            return Self::Video;
        }
        let lower = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, value)| value).unwrap_or("");
        let mime = mime_type.trim().to_ascii_lowercase();
        if matches!(
            ext,
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "ts" | "m3u8" | "mpd" | "flv"
        ) || mime.starts_with("video/")
        {
            return Self::Video;
        }
        if matches!(ext, "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "wma")
            || mime.starts_with("audio/")
        {
            return Self::Music;
        }
        if matches!(
            ext,
            "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "iso"
        ) {
            return Self::Archive;
        }
        if matches!(
            ext,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "epub"
        ) {
            return Self::Document;
        }
        if matches!(
            ext,
            "exe" | "msi" | "msix" | "appx" | "bat" | "cmd" | "apk" | "dmg" | "deb" | "rpm"
        ) {
            return Self::Program;
        }
        Self::General
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub filename: String,
    pub title: String,
    pub url: String,
    pub status: String,
    pub task_type: String,
    pub mime_type: String,
    pub output_path: String,
    pub progress_percent: f64,
    pub downloaded_bytes: i64,
    pub total_bytes: i64,
    pub speed_bytes_per_sec: f64,
    pub available_actions: Vec<String>,
    pub created_at: String,
    pub queue_position: i64,
    pub is_live: bool,
    pub output_missing: bool,
}

impl TaskRow {
    pub fn from_value(value: &Value) -> Option<Self> {
        let id = json_str(value, "id");
        let id = if id.is_empty() {
            json_str(value, "task_id")
        } else {
            id
        };
        if id.is_empty() {
            return None;
        }
        Some(Self {
            id,
            filename: json_str(value, "filename"),
            title: json_str(value, "title"),
            url: json_str(value, "url"),
            status: json_str(value, "status"),
            task_type: json_str_or(value, "task_type", "http"),
            mime_type: json_str(value, "mime_type"),
            output_path: json_str(value, "output_path"),
            progress_percent: json_f64(value, "progress_percent")
                .or_else(|| json_f64(value, "percent"))
                .unwrap_or(0.0),
            downloaded_bytes: json_i64(value, "downloaded_bytes"),
            total_bytes: json_i64(value, "total_bytes").max(json_i64(value, "size")),
            speed_bytes_per_sec: json_f64(value, "speed_bytes_per_sec").unwrap_or(0.0),
            available_actions: json_string_vec(value, "available_actions"),
            created_at: json_str(value, "created_at"),
            queue_position: json_i64(value, "queue_position"),
            is_live: json_bool(value, "is_live"),
            output_missing: json_bool(value, "output_missing"),
        })
    }

    pub fn merge(&mut self, other: &Self) {
        if !other.filename.is_empty() {
            self.filename.clone_from(&other.filename);
        }
        if !other.title.is_empty() {
            self.title.clone_from(&other.title);
        }
        if !other.url.is_empty() {
            self.url.clone_from(&other.url);
        }
        if !other.status.is_empty() {
            self.status.clone_from(&other.status);
        }
        if !other.task_type.is_empty() {
            self.task_type.clone_from(&other.task_type);
        }
        if !other.mime_type.is_empty() {
            self.mime_type.clone_from(&other.mime_type);
        }
        if !other.output_path.is_empty() {
            self.output_path.clone_from(&other.output_path);
        }
        if other.progress_percent > 0.0 || other.status == "done" {
            self.progress_percent = other.progress_percent;
        }
        if other.downloaded_bytes > 0 {
            self.downloaded_bytes = other.downloaded_bytes;
        }
        if other.total_bytes > 0 {
            self.total_bytes = other.total_bytes;
        }
        self.speed_bytes_per_sec = other.speed_bytes_per_sec;
        if !other.available_actions.is_empty() {
            self.available_actions.clone_from(&other.available_actions);
        }
        if !other.created_at.is_empty() {
            self.created_at.clone_from(&other.created_at);
        }
        if other.queue_position > 0 {
            self.queue_position = other.queue_position;
        }
        self.is_live = other.is_live;
        self.output_missing = other.output_missing;
    }

    pub fn category(&self) -> FileCategory {
        let path = if self.output_path.is_empty() {
            &self.filename
        } else {
            &self.output_path
        };
        FileCategory::from_file(path, &self.mime_type, &self.task_type)
    }

    pub fn is_completed(&self) -> bool {
        self.status == "done"
    }

    pub fn display_name(&self) -> &str {
        if !self.filename.is_empty() {
            &self.filename
        } else if !self.title.is_empty() {
            &self.title
        } else {
            &self.id
        }
    }

    pub fn status_label(&self) -> &'static str {
        if self.status == "done" && self.output_missing {
            return "文件已删除";
        }
        if self.is_live && self.status == "downloading_segments" {
            return "直播录制";
        }
        match self.status.as_str() {
            "queued" => "排队中",
            "awaiting_confirmation" => "等待确认",
            "fetching_metadata" => "获取元数据",
            "awaiting_selection" => "等待选择文件",
            "checking" => "校验文件",
            "downloading" => "准备下载",
            "downloading_m3u8" => "获取清单",
            "parsing" => "解析中",
            "downloading_segments" => "下载分片",
            "pausing" => "正在暂停",
            "paused" => "已暂停",
            "merging" => "合并中",
            "remuxing" => "转封装",
            "done" => "已完成",
            "failed" => "失败",
            "canceled" => "已取消",
            "unsupported" => "不支持",
            "interrupted" => "上次运行中断",
            _ => "进行中",
        }
    }

    pub fn display_line(&self) -> String {
        let percent = if self.is_completed() {
            "100%".to_string()
        } else if self.progress_percent > 0.0 {
            format!("{:.0}%", self.progress_percent.clamp(0.0, 100.0))
        } else {
            "--".to_string()
        };
        format!(
            "{}    {}    {}    {}",
            self.display_name(),
            percent,
            format_speed(self.speed_bytes_per_sec),
            self.status_label()
        )
    }

    pub fn has_action(&self, action: &str) -> bool {
        self.available_actions.iter().any(|item| item == action)
    }

    pub fn start_kind(&self) -> Option<&'static str> {
        if self.has_action("start") {
            Some("start")
        } else if self.has_action("resume") {
            Some("resume")
        } else if self.has_action("retry") {
            Some("retry")
        } else {
            None
        }
    }

    fn sort_rank(&self) -> i32 {
        if ACTIVE.contains(&self.status.as_str()) {
            0
        } else if self.status == "failed" || self.status == "unsupported" {
            1
        } else if self.status == "canceled" {
            2
        } else {
            3
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskList {
    pub tasks: Vec<TaskRow>,
    pub status_filter: StatusFilter,
    pub category: FileCategory,
    pub query: String,
    pub selected_id: String,
    pub needs_refresh: bool,
}

impl TaskList {
    pub fn replace(&mut self, items: Vec<Value>) {
        let selected = self.selected_id.clone();
        self.tasks = items.iter().filter_map(TaskRow::from_value).collect();
        self.needs_refresh = false;
        self.restore_selection(&selected);
    }

    pub fn upsert(&mut self, items: &[Value]) {
        for item in items {
            let Some(row) = TaskRow::from_value(item) else {
                continue;
            };
            if let Some(existing) = self.tasks.iter_mut().find(|task| task.id == row.id) {
                existing.merge(&row);
            } else {
                self.tasks.insert(0, row);
            }
        }
    }

    pub fn remove(&mut self, task_id: &str) {
        self.tasks.retain(|task| task.id != task_id);
        if self.selected_id == task_id {
            self.selected_id.clear();
            if let Some(first) = self.visible().into_iter().next() {
                self.selected_id = first.id.clone();
            }
        }
    }

    pub fn set_status_filter(&mut self, filter: StatusFilter) {
        self.status_filter = filter;
        self.keep_selection_visible();
    }

    pub fn set_category(&mut self, category: FileCategory) {
        self.category = category;
        self.keep_selection_visible();
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.keep_selection_visible();
    }

    pub fn select_id(&mut self, task_id: &str) {
        if self.visible().iter().any(|task| task.id == task_id) {
            self.selected_id = task_id.to_string();
        }
    }

    pub fn select_visible_index(&mut self, index: i32) {
        let visible = self.visible();
        if index < 0 {
            return;
        }
        if let Some(task) = visible.get(index as usize) {
            self.selected_id = task.id.clone();
        }
    }

    pub fn visible(&self) -> Vec<&TaskRow> {
        let needle = self.query.trim().to_lowercase();
        let mut rows: Vec<&TaskRow> = self
            .tasks
            .iter()
            .filter(|task| self.matches_status(task) && self.matches_category(task))
            .filter(|task| {
                if needle.is_empty() {
                    return true;
                }
                [&task.id, &task.title, &task.filename, &task.url]
                    .into_iter()
                    .any(|value| value.to_lowercase().contains(&needle))
            })
            .collect();
        rows.sort_by(|a, b| {
            let rank = a.sort_rank().cmp(&b.sort_rank());
            if rank != std::cmp::Ordering::Equal {
                return rank;
            }
            if a.status == "queued" && b.status == "queued" {
                let left = if a.queue_position > 0 {
                    a.queue_position
                } else {
                    i64::MAX
                };
                let right = if b.queue_position > 0 {
                    b.queue_position
                } else {
                    i64::MAX
                };
                let position = left.cmp(&right);
                if position != std::cmp::Ordering::Equal {
                    return position;
                }
            }
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        rows
    }

    pub fn selected(&self) -> Option<&TaskRow> {
        let selected = self.selected_id.as_str();
        self.visible().into_iter().find(|task| task.id == selected)
    }

    pub fn selected_index(&self) -> i32 {
        let selected = self.selected_id.as_str();
        self.visible()
            .iter()
            .position(|task| task.id == selected)
            .map(|index| index as i32)
            .unwrap_or(-1)
    }

    pub fn summary(&self) -> String {
        let visible = self.visible().len();
        let total = self.tasks.len();
        if visible == 0 {
            if !self.query.trim().is_empty() {
                "没有匹配的任务".into()
            } else if self.status_filter != StatusFilter::All || self.category != FileCategory::All
            {
                "当前分类没有任务 · 关闭窗口回到托盘".into()
            } else {
                "暂无任务 · 关闭窗口回到托盘".into()
            }
        } else if visible == total {
            format!("{visible} 项 · 关闭窗口回到托盘")
        } else {
            format!("{visible} / {total} 项 · 关闭窗口回到托盘")
        }
    }

    fn matches_status(&self, task: &TaskRow) -> bool {
        match self.status_filter {
            StatusFilter::All => true,
            StatusFilter::Unfinished => !task.is_completed(),
            StatusFilter::Completed => task.is_completed(),
        }
    }

    fn matches_category(&self, task: &TaskRow) -> bool {
        self.category == FileCategory::All || task.category() == self.category
    }

    fn restore_selection(&mut self, previous: &str) {
        if !previous.is_empty() && self.visible().iter().any(|task| task.id == previous) {
            self.selected_id = previous.to_string();
            return;
        }
        self.keep_selection_visible();
    }

    fn keep_selection_visible(&mut self) {
        let visible = self.visible();
        if visible.iter().any(|task| task.id == self.selected_id) {
            return;
        }
        self.selected_id = visible
            .first()
            .map(|task| task.id.clone())
            .unwrap_or_default();
    }
}

pub fn format_speed(speed: f64) -> String {
    if speed <= 0.0 {
        "0 B/s".into()
    } else if speed < 1024.0 {
        format!("{speed:.0} B/s")
    } else if speed < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", speed / 1024.0)
    } else {
        format!("{:.1} MB/s", speed / (1024.0 * 1024.0))
    }
}

fn json_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn json_str_or(value: &Value, key: &str, fallback: &str) -> String {
    let text = json_str(value, key);
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn json_i64(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|item| {
            item.as_i64()
                .or_else(|| item.as_u64().and_then(|n| i64::try_from(n).ok()))
                .or_else(|| item.as_f64().map(|n| n as i64))
        })
        .unwrap_or(0)
}

fn json_f64(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|item| {
        item.as_f64()
            .or_else(|| item.as_i64().map(|n| n as f64))
            .or_else(|| item.as_u64().map(|n| n as f64))
    })
}

fn json_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn json_string_vec(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(filename: &str, status: &str, extra: Value) -> Value {
        let mut item = json!({
            "id": filename,
            "filename": filename,
            "status": status,
            "created_at": "2026-01-01T00:00:00",
        });
        if let Value::Object(extra) = extra {
            if let Value::Object(target) = &mut item {
                target.extend(extra);
            }
        }
        item
    }

    #[test]
    fn unfinished_hides_done_and_search_matches_filename() {
        let mut list = TaskList::default();
        list.replace(vec![
            row(
                "movie.mp4",
                "downloading_segments",
                json!({"progress_percent": 40.0, "available_actions": ["pause", "cancel"]}),
            ),
            row(
                "done.zip",
                "done",
                json!({"available_actions": ["open", "delete"]}),
            ),
            row(
                "setup.exe",
                "paused",
                json!({"available_actions": ["resume", "delete"]}),
            ),
        ]);
        list.set_status_filter(StatusFilter::Unfinished);
        let names: Vec<&str> = list
            .visible()
            .iter()
            .map(|task| task.filename.as_str())
            .collect();
        assert_eq!(names, vec!["movie.mp4", "setup.exe"]);
        list.set_query("setup");
        assert_eq!(list.visible().len(), 1);
        assert_eq!(list.selected().unwrap().filename, "setup.exe");
        assert_eq!(list.selected().unwrap().start_kind(), Some("resume"));
        list.set_query("");
        list.set_status_filter(StatusFilter::Completed);
        assert_eq!(list.visible()[0].filename, "done.zip");
        assert!(list.selected().unwrap().has_action("open"));
    }

    #[test]
    fn file_category_follows_extension_and_hls_type() {
        assert_eq!(
            FileCategory::from_file("a.mp4", "", "http"),
            FileCategory::Video
        );
        assert_eq!(
            FileCategory::from_file("track.m3u8", "", "hls"),
            FileCategory::Video
        );
        assert_eq!(
            FileCategory::from_file("song.flac", "", "http"),
            FileCategory::Music
        );
        assert_eq!(
            FileCategory::from_file("pack.zip", "", "http"),
            FileCategory::Archive
        );
        assert_eq!(
            FileCategory::from_file("notes.pdf", "", "http"),
            FileCategory::Document
        );
        assert_eq!(
            FileCategory::from_file("setup.exe", "", "http"),
            FileCategory::Program
        );
        assert_eq!(
            FileCategory::from_file("blob.bin", "", "http"),
            FileCategory::General
        );
        let mut list = TaskList::default();
        list.replace(vec![
            row("a.mp4", "done", json!({})),
            row("b.zip", "done", json!({})),
            row("c.exe", "queued", json!({})),
        ]);
        list.set_category(FileCategory::Program);
        assert_eq!(list.visible().len(), 1);
        assert_eq!(list.visible()[0].filename, "c.exe");
    }

    #[test]
    fn hide_selection_stays_when_row_still_visible() {
        let mut list = TaskList::default();
        list.replace(vec![
            row("keep.bin", "paused", json!({"created_at": "2026-02-01"})),
            row("other.bin", "done", json!({"created_at": "2026-02-02"})),
        ]);
        list.select_id("keep.bin");
        list.set_status_filter(StatusFilter::Unfinished);
        assert_eq!(list.selected_id, "keep.bin");
        list.remove("keep.bin");
        assert_eq!(list.selected_id, "");
        assert!(list.visible().is_empty());
    }

    #[test]
    fn progress_upsert_keeps_existing_actions_until_full_refresh() {
        let mut list = TaskList::default();
        list.replace(vec![row(
            "a.bin",
            "downloading",
            json!({"available_actions": ["pause"], "progress_percent": 10.0}),
        )]);
        list.upsert(&[json!({
            "id": "a.bin",
            "filename": "a.bin",
            "status": "downloading_segments",
            "progress_percent": 55.0,
            "speed_bytes_per_sec": 2048.0
        })]);
        let row = &list.tasks[0];
        assert_eq!(row.status, "downloading_segments");
        assert_eq!(row.progress_percent, 55.0);
        assert_eq!(row.available_actions, vec!["pause"]);
        assert!(row.display_line().contains("下载分片"));
    }
}
