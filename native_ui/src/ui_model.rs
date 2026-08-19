use hls_native_shell::{sample_cells, CoreEvent, ResourceKind, TaskSnapshot};
use slint::{ModelRc, VecModel};
#[cfg(test)]
use slint::SharedString;
use std::collections::BTreeMap;

use crate::TaskRow;

#[cfg(test)]
pub fn demo_tasks() -> Vec<TaskRow> {
    vec![
        TaskRow {
            task_id: SharedString::from("task-1"),
            title: SharedString::from("官方安装包"),
            filename: SharedString::from("HLSDownloader-v6.0.0-Setup.exe"),
            status: SharedString::from("下载中"),
            progress: 0.72,
            ranges: 0.68,
            speed: SharedString::from("8.4 MB/s"),
            size: SharedString::from("428 MB"),
            workers: SharedString::from("8 连接"),
            eta: SharedString::from("剩余 12s"),
            kind: SharedString::from("HTTP"),
            live: true,
            accent: slint::Color::from_rgb_u8(37, 99, 235),
            picked: false,
            map_cells: ModelRc::new(VecModel::from(vec![2, 2, 2, 1, 0, 0])),
        },
        TaskRow {
            task_id: SharedString::from("task-2"),
            title: SharedString::from("直播录制：演示频道"),
            filename: SharedString::from("demo-live-2026-08-15.mp4"),
            status: SharedString::from("录制中"),
            progress: 0.38,
            ranges: 0.38,
            speed: SharedString::from("3.1 MB/s"),
            size: SharedString::from("进行中"),
            workers: SharedString::from("1 连接"),
            eta: SharedString::from(""),
            kind: SharedString::from("直播"),
            live: true,
            accent: slint::Color::from_rgb_u8(22, 163, 74),
            picked: false,
            map_cells: ModelRc::new(VecModel::from(vec![2, 1, 0, 0])),
        },
        TaskRow {
            task_id: SharedString::from("task-3"),
            title: SharedString::from("项目文档"),
            filename: SharedString::from("architecture.pdf"),
            status: SharedString::from("已完成"),
            progress: 1.0,
            ranges: 1.0,
            speed: SharedString::from("完成"),
            size: SharedString::from("12.8 MB"),
            workers: SharedString::from(""),
            eta: SharedString::from(""),
            kind: SharedString::from("HTTP"),
            live: false,
            accent: slint::Color::from_rgb_u8(100, 116, 139),
            picked: false,
            map_cells: ModelRc::new(VecModel::from(vec![2, 2, 2, 2])),
        },
    ]
}

#[derive(Default)]
pub struct UiBridge {
    snapshots: BTreeMap<String, TaskSnapshot>,
    last_status: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TaskCounts {
    pub all: usize,
    pub downloading: usize,
    pub processing: usize,
    pub queued: usize,
    pub paused: usize,
    pub completed: usize,
    pub failed: usize,
    pub media: usize,
    pub programs: usize,
    pub archives: usize,
    pub other: usize,
}

impl UiBridge {
    pub fn replace(&mut self, snapshots: impl IntoIterator<Item = TaskSnapshot>) {
        self.snapshots.clear();
        for snapshot in snapshots {
            self.snapshots.insert(snapshot.task_id.clone(), snapshot);
        }
    }

    pub fn apply(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::TaskCreated { snapshot }
            | CoreEvent::TaskUpdated { snapshot }
            | CoreEvent::TaskProgress { snapshot } => {
                self.snapshots.insert(snapshot.task_id.clone(), snapshot);
            }
            CoreEvent::TaskDeleted { task_id } => {
                self.snapshots.remove(&task_id);
            }
            CoreEvent::Ready { .. }
            | CoreEvent::HandoffOffered { .. }
            | CoreEvent::HandoffResolved { .. }
            | CoreEvent::UiShow { .. }
            | CoreEvent::ProbeResult { .. }
            | CoreEvent::CastDevices { .. } => {}
            CoreEvent::DuplicateOffered { message, .. }
            | CoreEvent::Toast { message, .. }
            | CoreEvent::BrowserStatus { message, .. }
            | CoreEvent::CastSession { status: message, .. } => {
                self.last_status = message;
            }
            CoreEvent::HarvestResult { links, .. } => {
                self.last_status = format!("页面抓取到 {} 条链接", links.len());
            }
            CoreEvent::TaskLog { lines, .. } => {
                if let Some(line) = lines.last() {
                    self.last_status = line.clone();
                }
            }
            CoreEvent::Error { code, message } => {
                self.last_status = if code.is_empty() {
                    message
                } else {
                    format!("{code}: {message}")
                };
            }
        }
    }

    pub fn last_status(&self) -> Option<&str> {
        if self.last_status.is_empty() {
            None
        } else {
            Some(self.last_status.as_str())
        }
    }

    pub fn snapshot(&self, task_id: &str) -> Option<&TaskSnapshot> {
        self.snapshots.get(task_id)
    }

    pub fn all(&self) -> Vec<TaskSnapshot> {
        self.snapshots.values().cloned().collect()
    }

    #[cfg(test)]
    pub fn rows(&self) -> Vec<TaskRow> {
        self.snapshots.values().map(task_row).collect()
    }

    pub fn filtered_rows(&self, query: &str, filter: &str) -> Vec<TaskRow> {
        let query = query.trim().to_lowercase();
        let mut snapshots: Vec<_> = self
            .snapshots
            .values()
            .filter(|snapshot| matches_filter(snapshot, filter))
            .filter(|snapshot| {
                query.is_empty()
                    || snapshot.title.to_lowercase().contains(&query)
                    || snapshot.filename.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        snapshots.sort_by_key(|item| (item.queue_index, item.task_id.clone()));
        snapshots.into_iter().map(|snapshot| task_row(&snapshot)).collect()
    }

    pub fn counts(&self) -> TaskCounts {
        let mut counts = TaskCounts {
            all: self.snapshots.len(),
            ..TaskCounts::default()
        };
        for snapshot in self.snapshots.values() {
            match snapshot.status.as_str() {
                status if is_active_transfer(status) => counts.downloading += 1,
                status if is_local_processing(status) => counts.processing += 1,
                "queued" => counts.queued += 1,
                "paused" => counts.paused += 1,
                "completed" | "done" => counts.completed += 1,
                "failed" | "error" => counts.failed += 1,
                _ => {}
            }
            if is_media(snapshot) {
                counts.media += 1;
            }
            if is_program(snapshot) {
                counts.programs += 1;
            }
            if is_archive(snapshot) {
                counts.archives += 1;
            }
            if is_other(snapshot) {
                counts.other += 1;
            }
        }
        counts
    }

    pub fn detail_line(&self, task_id: &str) -> String {
        let Some(snapshot) = self.snapshots.get(task_id) else {
            return "选择任务查看连接、分段、日志和媒体轨道".into();
        };
        let downloaded = format_bytes(snapshot.downloaded_bytes);
        let total = snapshot
            .total_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "未知".into());
        let playback = if snapshot.playback_ready {
            "就绪"
        } else {
            "未就绪"
        };
        let missing = if snapshot.output_missing {
            " · 文件已删除，可重新下载"
        } else {
            ""
        };
        let parts = if snapshot.connection_hint.is_empty() {
            String::new()
        } else {
            format!(" · {}", snapshot.connection_hint)
        };
        let error = snapshot
            .error_message
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| format!(" · {text}"))
            .unwrap_or_default();
        let eta = snapshot
            .eta_seconds
            .map(|seconds| format!(" · 剩余 {seconds}s"))
            .unwrap_or_default();
        let url = if snapshot.url.is_empty() {
            String::new()
        } else {
            format!(" · {}", snapshot.url)
        };
        format!(
            "阶段 {} · {} / {} · 速度 {}{eta} · 连接 {} · 分段 {}/{}{parts} · 边下边播 {playback}{missing}{error}{url}",
            snapshot.stage,
            downloaded,
            total,
            format_speed(snapshot.speed_bytes_per_sec),
            snapshot.active_workers,
            snapshot.completed_ranges,
            snapshot.total_ranges
        )
    }
}

pub(crate) fn is_active_transfer(status: &str) -> bool {
    matches!(status, "downloading" | "recording")
}

pub(crate) fn is_local_processing(status: &str) -> bool {
    matches!(status, "merging" | "checking")
}

fn matches_filter(snapshot: &TaskSnapshot, filter: &str) -> bool {
    match filter {
        "下载中" => is_active_transfer(&snapshot.status),
        "排队中" => snapshot.status == "queued",
        "已暂停" => snapshot.status == "paused",
        "本地处理中" => is_local_processing(&snapshot.status),
        "已完成" => matches!(snapshot.status.as_str(), "completed" | "done"),
        "失败" => matches!(snapshot.status.as_str(), "failed" | "error"),
        "媒体" => is_media(snapshot),
        "程序" => is_program(snapshot),
        "压缩包" => is_archive(snapshot),
        "其他" => is_other(snapshot),
        _ => true,
    }
}

fn is_media(snapshot: &TaskSnapshot) -> bool {
    matches!(
        snapshot.resource_kind,
        ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live
    ) || has_extension(
        &snapshot.filename,
        &["mp4", "mkv", "webm", "mov", "mp3", "flac", "m4a", "ts"],
    )
}

fn is_program(snapshot: &TaskSnapshot) -> bool {
    has_extension(&snapshot.filename, &["exe", "msi", "msix", "appx"])
}

fn is_archive(snapshot: &TaskSnapshot) -> bool {
    has_extension(
        &snapshot.filename,
        &["zip", "7z", "rar", "tar", "gz", "bz2", "xz"],
    )
}

fn is_other(snapshot: &TaskSnapshot) -> bool {
    !is_media(snapshot) && !is_program(snapshot) && !is_archive(snapshot)
}

fn has_extension(filename: &str, extensions: &[&str]) -> bool {
    filename
        .rsplit_once('.')
        .map(|(_, extension)| {
            extensions
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
        .unwrap_or(false)
}

fn task_row(snapshot: &TaskSnapshot) -> TaskRow {
    let progress = snapshot
        .total_bytes
        .filter(|total| *total > 0)
        .map(|total| snapshot.downloaded_bytes as f32 / total as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let accent = match snapshot.resource_kind {
        ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live => {
            slint::Color::from_rgb_u8(22, 163, 74)
        }
        ResourceKind::Torrent => slint::Color::from_rgb_u8(124, 58, 237),
        ResourceKind::Ftp | ResourceKind::Sftp => slint::Color::from_rgb_u8(217, 119, 6),
        _ => slint::Color::from_rgb_u8(37, 99, 235),
    };
    let range_progress = if snapshot.total_ranges > 0 {
        snapshot.completed_ranges as f32 / snapshot.total_ranges as f32
    } else {
        progress
    }
    .clamp(0.0, 1.0);
    let live = is_active_transfer(&snapshot.status);
    let workers = if live && snapshot.active_workers > 0 {
        format!("{} 连接", snapshot.active_workers)
    } else if !snapshot.connection_parts.is_empty() {
        format!("{} 段", snapshot.connection_parts.len())
    } else {
        String::new()
    };
    let eta = snapshot
        .eta_seconds
        .filter(|_| live)
        .map(|seconds| format!("剩余 {seconds}s"))
        .unwrap_or_default();
    let speed = if snapshot.status == "completed" || snapshot.status == "done" {
        "完成".to_string()
    } else {
        format_speed(snapshot.speed_bytes_per_sec)
    };
    TaskRow {
        task_id: snapshot.task_id.clone().into(),
        title: snapshot.title.clone().into(),
        filename: snapshot.filename.clone().into(),
        status: status_label(&snapshot.status, snapshot.output_missing).into(),
        progress,
        ranges: range_progress,
        speed: speed.into(),
        size: snapshot
            .total_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "未知大小".to_string())
            .into(),
        workers: workers.into(),
        eta: eta.into(),
        kind: kind_label(snapshot.resource_kind).into(),
        live,
        accent,
        picked: false,
        map_cells: ModelRc::new(VecModel::from(sample_cells(
            &snapshot.connection_parts,
            snapshot.total_bytes.unwrap_or(0),
            snapshot.downloaded_bytes,
            32,
        ))),
    }
}

fn kind_label(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::File => "HTTP",
        ResourceKind::Hls => "HLS",
        ResourceKind::Dash => "DASH",
        ResourceKind::Live => "直播",
        ResourceKind::Ftp => "FTP",
        ResourceKind::Sftp => "SFTP",
        ResourceKind::Torrent => "BT",
    }
}

fn status_label(status: &str, missing: bool) -> String {
    if missing && matches!(status, "completed" | "done") {
        return "文件已删除".into();
    }
    match status {
        "queued" => "等待中",
        "downloading" => "下载中",
        "recording" => "录制中",
        "paused" => "已暂停",
        "merging" => "本地处理中",
        "checking" => "本地处理中",
        "completed" | "done" => "已完成",
        "failed" | "error" => "失败",
        "canceled" => "已取消",
        other => other,
    }
    .to_string()
}

fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1024 * 1024 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1024.0 / 1024.0)
    } else if bytes_per_sec >= 1024 {
        format!("{:.1} KB/s", bytes_per_sec as f64 / 1024.0)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::{demo_tasks, UiBridge};
    use hls_native_shell::{CoreEvent, ResourceKind, TaskSnapshot};

    #[test]
    fn demo_model_has_visible_rows() {
        let rows = demo_tasks();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].filename, "HLSDownloader-v6.0.0-Setup.exe");
    }

    #[test]
    fn core_progress_updates_only_the_matching_row() {
        let mut bridge = UiBridge::default();
        bridge.apply(CoreEvent::TaskProgress {
            snapshot: TaskSnapshot {
                task_id: "task-1".into(),
                resource_kind: ResourceKind::File,
                status: "downloading".into(),
                stage: "transfer".into(),
                title: "A".into(),
                filename: "a.bin".into(),
                downloaded_bytes: 50,
                total_bytes: Some(100),
                speed_bytes_per_sec: 2048,
                eta_seconds: Some(1),
                active_workers: 2,
                completed_ranges: 1,
                total_ranges: 2,
                    playback_ready: false,
                    is_live: false,
                    available_actions: vec!["pause".into()],
                    url: String::new(),
                    error_code: None,
                    error_message: None,
                    queue_index: 0,
                    output_missing: false,
                    connection_hint: String::new(),
                    connection_parts: Vec::new(),
                    log_tail: Vec::new(),
                },
        });
        let rows = bridge.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].progress, 0.5);
        assert_eq!(rows[0].speed, "2.0 KB/s");
        assert_eq!(rows[0].kind, "HTTP");
        assert_eq!(rows[0].workers, "2 连接");
        assert!(rows[0].live);
    }

    #[test]
    fn search_and_category_filters_use_core_snapshots() {
        let mut bridge = UiBridge::default();
        for (id, filename, kind, status) in [
            ("one", "setup.exe", ResourceKind::File, "downloading"),
            ("two", "movie.mp4", ResourceKind::Hls, "completed"),
        ] {
            bridge.apply(CoreEvent::TaskUpdated {
                snapshot: TaskSnapshot {
                    task_id: id.into(),
                    resource_kind: kind,
                    status: status.into(),
                    stage: "transfer".into(),
                    title: filename.into(),
                    filename: filename.into(),
                    downloaded_bytes: 0,
                    total_bytes: None,
                    speed_bytes_per_sec: 0,
                    eta_seconds: None,
                    active_workers: 0,
                    completed_ranges: 0,
                    total_ranges: 0,
                    playback_ready: false,
                    is_live: false,
                    available_actions: Vec::new(),
                    url: String::new(),
                    error_code: None,
                    error_message: None,
                    queue_index: 0,
                    output_missing: false,
                    connection_hint: String::new(),
                    connection_parts: Vec::new(),
                    log_tail: Vec::new(),
                },
            });
        }
        assert_eq!(bridge.filtered_rows("setup", "全部").len(), 1);
        assert_eq!(bridge.filtered_rows("", "媒体").len(), 1);
        assert_eq!(bridge.counts().programs, 1);
        assert_eq!(bridge.counts().completed, 1);
        assert!(bridge.detail_line("two").contains("边下边播"));
        bridge.apply(CoreEvent::Error {
            code: "update_current".into(),
            message: "已是最新版本 6.0.0-dev".into(),
        });
        assert!(bridge.last_status().unwrap().contains("update_current"));
    }

    #[test]
    fn downloading_filter_excludes_local_mux_and_verify() {
        let mut bridge = UiBridge::default();
        for (id, filename, kind, status) in [
            ("dl", "movie.ts", ResourceKind::Hls, "downloading"),
            ("mux", "movie.ts", ResourceKind::Hls, "merging"),
            ("hash", "setup.exe", ResourceKind::File, "checking"),
            ("rec", "live.ts", ResourceKind::Live, "recording"),
        ] {
            bridge.apply(CoreEvent::TaskUpdated {
                snapshot: TaskSnapshot {
                    task_id: id.into(),
                    resource_kind: kind,
                    status: status.into(),
                    stage: "transfer".into(),
                    title: filename.into(),
                    filename: filename.into(),
                    downloaded_bytes: 0,
                    total_bytes: None,
                    speed_bytes_per_sec: 0,
                    eta_seconds: None,
                    active_workers: 0,
                    completed_ranges: 0,
                    total_ranges: 0,
                    playback_ready: false,
                    is_live: false,
                    available_actions: Vec::new(),
                    url: String::new(),
                    error_code: None,
                    error_message: None,
                    queue_index: 0,
                    output_missing: false,
                    connection_hint: String::new(),
                    connection_parts: Vec::new(),
                    log_tail: Vec::new(),
                },
            });
        }
        assert_eq!(bridge.filtered_rows("", "下载中").len(), 2);
        assert_eq!(bridge.filtered_rows("", "本地处理中").len(), 2);
        assert_eq!(bridge.counts().downloading, 2);
        assert_eq!(bridge.counts().processing, 2);
        assert_eq!(
            bridge.filtered_rows("", "本地处理中")[0].status,
            "本地处理中"
        );
    }

    #[test]
    fn queued_and_paused_filters_match_5x_sidebar() {
        let mut bridge = UiBridge::default();
        for (id, status) in [("q", "queued"), ("p", "paused"), ("d", "downloading")] {
            bridge.apply(CoreEvent::TaskUpdated {
                snapshot: TaskSnapshot {
                    task_id: id.into(),
                    status: status.into(),
                    filename: format!("{id}.bin"),
                    title: id.into(),
                    ..TaskSnapshot::default()
                },
            });
        }
        assert_eq!(bridge.filtered_rows("", "排队中").len(), 1);
        assert_eq!(bridge.filtered_rows("", "已暂停").len(), 1);
        assert_eq!(bridge.filtered_rows("", "其他").len(), 3);
        assert_eq!(bridge.counts().queued, 1);
        assert_eq!(bridge.counts().paused, 1);
        assert_eq!(bridge.counts().other, 3);
    }
}
