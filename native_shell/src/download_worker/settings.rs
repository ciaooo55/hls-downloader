//! Current settings, their validation and persisted updates.

use super::{
    load_queue_profiles, proxy_url_allowed, reject_scan_shell, validate_helper_executable,
    CoreCoordinator,
};
use crate::output_path::reject_path_escape;
use crate::{CoreEvent, QueueProfile};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CoreSettings {
    pub takeover_enabled: bool,
    pub takeover_minimum_bytes: u64,
    pub legal_accepted: bool,
    pub speed_limit_kib: u64,
    pub hourly_quota_mib: u64,
    pub schedule_enabled: bool,
    pub schedule_start: String,
    pub schedule_end: String,
    pub schedule_kib: u64,
    pub auto_category: bool,
    pub category_dirs: crate::category::CategoryDirs,
    pub queue_max: u64,
    pub queue_profiles: Vec<QueueProfile>,
    pub site_rules: String,
    pub av_scan_enabled: bool,
    pub av_scan_command: String,
    pub torrent_watch: String,
    pub torrent_watch_enabled: bool,
    pub download_dir: String,
    pub temp_dir: String,
    pub default_concurrency: u64,
    pub proxy_url: String,
    pub ffmpeg_path: String,
    pub clipboard_watch: bool,
    pub completion_sound_enabled: bool,
    pub progress_window_enabled: bool,
    pub complete_popup_enabled: bool,
    pub resume_interrupted: bool,
    pub auto_retry_max: u64,
    pub existing_file_policy: String,
    pub live_record_max_minutes: u64,
    pub download_subtitles: bool,
    pub skip_ad_segments: bool,
    pub keep_temp_files: bool,
    pub default_user_agent: String,
    pub tvbox_endpoint: String,
    pub dark_mode: bool,
    pub allow_duplicate: bool,
    pub queue_auto_start_enabled: bool,
    pub queue_auto_start_time: String,
    pub queue_auto_stop_enabled: bool,
    pub queue_auto_stop_time: String,
    pub default_referer: String,
    pub default_origin: String,
    pub allowed_hosts: String,
    pub http_chunk_size_mb: u64,
    pub completion_power_action: String,
    pub start_on_login: bool,
    pub queue_active_days: String,
    pub proxy_mode: String,
    pub proxy_bypass: String,
    pub legal_terms_version: String,
    pub reduce_motion: bool,
    pub harvest_minimum_bytes: u64,
    pub av_scan_fail_on_threat: bool,
    pub bt_upload_limit_kib: u64,
    pub bt_max_connections: u64,
    pub bt_enable_dht: bool,
    pub preferred_cast_device_id: String,
    pub task_column_layout: String,
    pub toolbar_actions: String,
    pub task_sort: String,
}

impl CoreCoordinator {
    pub fn settings(&self) -> Result<CoreSettings, String> {
        let core = self.lock()?;
        let configured_download_dir = core.store().setting_string("download_dir", "")?;
        let download_dir = if configured_download_dir.trim().is_empty()
            || Path::new(&configured_download_dir).is_relative()
        {
            crate::default_v7_download_dir()
                .to_string_lossy()
                .into_owned()
        } else {
            configured_download_dir
        };
        Ok(CoreSettings {
            takeover_enabled: core
                .store()
                .setting_bool("browser_takeover_enabled", true)?,
            takeover_minimum_bytes: core
                .store()
                .setting_u64("browser_takeover_minimum_bytes", 0)?,
            legal_accepted: core.store().setting_bool("legal_terms_accepted", false)?,
            speed_limit_kib: core.store().setting_u64("download_speed_limit_kib", 0)?,
            hourly_quota_mib: core.store().setting_u64("download_hourly_quota_mib", 0)?,
            schedule_enabled: core
                .store()
                .setting_bool("download_speed_schedule_enabled", false)?,
            schedule_start: core
                .store()
                .setting_string("download_speed_schedule_start", "22:00")?,
            schedule_end: core
                .store()
                .setting_string("download_speed_schedule_end", "08:00")?,
            schedule_kib: core.store().setting_u64("download_speed_schedule_kib", 0)?,
            auto_category: core.store().setting_bool("auto_category_dirs", false)?,
            category_dirs: crate::category::parse_category_dirs(
                &core.store().setting_string("browser_category_dirs", "")?,
            ),
            queue_max: core.store().setting_u64("queue_max_active", 3)?.max(1),
            queue_profiles: load_queue_profiles(core.store())?,
            site_rules: core.store().setting_string("site_rules", "")?,
            av_scan_enabled: core.store().setting_bool("av_scan_enabled", false)?,
            av_scan_command: core.store().setting_string("av_scan_command", "")?,
            torrent_watch: core.store().setting_string("torrent_watch_dir", "")?,
            torrent_watch_enabled: core.store().setting_bool("watch_torrents", false)?,
            download_dir,
            temp_dir: core.store().setting_string("temp_dir", "")?,
            default_concurrency: core.store().setting_u64("default_concurrency", 12)?.max(1),
            proxy_url: core.store().setting_string("proxy_url", "")?,
            ffmpeg_path: core.store().setting_string("ffmpeg_path", "")?,
            clipboard_watch: core.store().setting_bool("clipboard_watch", false)?,
            completion_sound_enabled: core
                .store()
                .setting_bool("completion_sound_enabled", false)?,
            progress_window_enabled: core
                .store()
                .setting_bool("download_progress_window_enabled", true)?,
            complete_popup_enabled: core
                .store()
                .setting_bool("download_complete_popup_enabled", true)?,
            resume_interrupted: core
                .store()
                .setting_bool("resume_interrupted_on_startup", false)?,
            auto_retry_max: core.store().setting_u64("auto_retry_failed_max", 0)?,
            existing_file_policy: core
                .store()
                .setting_string("existing_file_policy", "rename")?,
            live_record_max_minutes: core.store().setting_u64("live_record_max_minutes", 0)?,
            download_subtitles: core.store().setting_bool("download_subtitles", true)?,
            skip_ad_segments: core.store().setting_bool("skip_ad_segments", true)?,
            keep_temp_files: core.store().setting_bool("keep_temp_files", false)?,
            default_user_agent: core.store().setting_string("default_user_agent", "")?,
            tvbox_endpoint: core.store().setting_string("tvbox_endpoint", "")?,
            dark_mode: core.store().setting_bool("dark_mode", false)?,
            allow_duplicate: core.store().setting_bool("allow_duplicate", false)?,
            queue_auto_start_enabled: core
                .store()
                .setting_bool("queue_auto_start_enabled", false)?,
            queue_auto_start_time: core
                .store()
                .setting_string("queue_auto_start_time", "00:00")?,
            queue_auto_stop_enabled: core
                .store()
                .setting_bool("queue_auto_stop_enabled", false)?,
            queue_auto_stop_time: core
                .store()
                .setting_string("queue_auto_stop_time", "07:30")?,
            default_referer: core.store().setting_string("default_referer", "")?,
            default_origin: core.store().setting_string("default_origin", "")?,
            allowed_hosts: core.store().setting_string("allowed_hosts", "")?,
            http_chunk_size_mb: core
                .store()
                .setting_u64("http_chunk_size_mb", 8)?
                .clamp(1, 64),
            completion_power_action: core
                .store()
                .setting_string("completion_power_action", "none")?,
            start_on_login: core.store().setting_bool("start_on_login", false)?,
            queue_active_days: core
                .store()
                .setting_string("queue_active_days", "1,2,3,4,5,6,7")?,
            proxy_mode: core.store().setting_string("proxy_mode", "system")?,
            proxy_bypass: core.store().setting_string("proxy_bypass", "")?,
            legal_terms_version: core.store().setting_string("legal_terms_version", "")?,
            reduce_motion: core.store().setting_bool("reduce_motion", false)?,
            harvest_minimum_bytes: core.store().setting_u64("harvest_minimum_bytes", 0)?,
            av_scan_fail_on_threat: core.store().setting_bool("av_scan_fail_on_threat", true)?,
            bt_upload_limit_kib: core.store().setting_u64("bt_upload_limit_kib", 1024)?,
            bt_max_connections: core
                .store()
                .setting_u64("bt_max_connections", 200)?
                .clamp(10, 1000),
            bt_enable_dht: core.store().setting_bool("bt_enable_dht", true)?,
            preferred_cast_device_id: core
                .store()
                .setting_string("preferred_cast_device_id", "")?,
            task_column_layout: core.store().setting_string("task_column_layout", "")?,
            toolbar_actions: core.store().setting_string("toolbar_actions", "")?,
            task_sort: core.store().setting_string("task_sort", "queue:asc")?,
        })
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
        self.set_settings(BTreeMap::from([(key.to_string(), value)]))
    }

    pub fn set_settings(&self, mut values: BTreeMap<String, Value>) -> Result<(), String> {
        for (key, value) in &values {
            Self::validate_setting(key, value)?;
        }
        let queue_profiles = if let Some(value) = values.get("queue_profiles") {
            let profiles: Vec<QueueProfile> = serde_json::from_value(value.clone())
                .map_err(|error| format!("队列配置格式无效: {error}"))?;
            Some(profiles)
        } else {
            None
        };
        if values.get("legal_terms_accepted").and_then(Value::as_bool) == Some(true) {
            values.insert(
                "legal_terms_version".into(),
                Value::String(crate::LEGAL_TERMS_VERSION.into()),
            );
        }
        let start_login = values.get("start_on_login").and_then(Value::as_bool);
        if let Some(flag) = start_login {
            crate::startup::apply(flag)?;
        }
        if let Some(profiles) = &queue_profiles {
            let ids: HashSet<_> = profiles.iter().map(|profile| profile.id.as_str()).collect();
            let mut core = self.lock()?;
            let orphaned = core
                .tasks()
                .into_iter()
                .filter(|task| !ids.contains(task.queue_id.as_str()))
                .map(|task| task.task_id)
                .collect();
            core.assign_queue_and_set_settings(orphaned, crate::DEFAULT_QUEUE_ID.into(), &values)?;
        } else {
            self.lock()?.store_mut().set_settings(&values)?;
            self.lock()?.emit(CoreEvent::SettingsChanged {
                keys: values.keys().cloned().collect(),
            })?;
        }
        if values.keys().any(|key| {
            matches!(
                key.as_str(),
                "download_speed_limit_kib"
                    | "download_speed_schedule_enabled"
                    | "download_speed_schedule_start"
                    | "download_speed_schedule_end"
                    | "download_speed_schedule_kib"
            )
        }) {
            let core = self.lock()?;
            crate::net_policy::configure_global_schedule(
                core.store().setting_u64("download_speed_limit_kib", 0)?,
                core.store()
                    .setting_bool("download_speed_schedule_enabled", false)?,
                &core
                    .store()
                    .setting_string("download_speed_schedule_start", "22:00")?,
                &core
                    .store()
                    .setting_string("download_speed_schedule_end", "08:00")?,
                core.store().setting_u64("download_speed_schedule_kib", 0)?,
            );
        }
        if let Some(limit) = values
            .get("download_hourly_quota_mib")
            .and_then(Value::as_u64)
        {
            crate::net_policy::configure_hourly_quota_mib(limit);
        }
        if let Some(profiles) = queue_profiles {
            crate::net_policy::sync_queue_limits(
                profiles
                    .iter()
                    .map(|profile| (profile.id.as_str(), profile.speed_limit_kib)),
            );
        }
        Ok(())
    }

    pub(super) fn validate_setting(key: &str, value: &Value) -> Result<(), String> {
        if !PUBLIC_SETTING_KEYS.contains(&key) && key != "legal_terms_version" {
            return Err(format!("未知设置项: {key}"));
        }
        if key == "ffmpeg_path" {
            if let Some(path) = value.as_str() {
                validate_helper_executable(path, &["ffmpeg", "ffmpeg.exe"])?;
            }
        }
        if key == "av_scan_command" {
            if let Some(command) = value.as_str() {
                reject_scan_shell(command)?;
            }
        }
        if key == "download_dir" || key == "temp_dir" || key == "torrent_watch_dir" {
            if let Some(path) = value.as_str() {
                if !path.trim().is_empty() {
                    reject_path_escape(path)?;
                }
            }
        }
        if key == "proxy_url" {
            if let Some(url) = value.as_str() {
                if !proxy_url_allowed(url) {
                    return Err("代理地址无效".into());
                }
            }
        }
        if key == "browser_category_dirs" {
            if let Some(raw) = value.as_str() {
                crate::category::validate_category_dirs(raw)?;
                let dirs = crate::category::parse_category_dirs(raw);
                reject_path_escape(&dirs.media)?;
                reject_path_escape(&dirs.program)?;
                reject_path_escape(&dirs.archive)?;
                reject_path_escape(&dirs.other)?;
            }
        }
        if key == "tvbox_endpoint" {
            if let Some(url) = value.as_str() {
                let url = url.trim();
                if !url.is_empty()
                    && (!url.to_ascii_lowercase().starts_with("http://")
                        || url.chars().any(|ch| ch.is_control())
                        || url.contains('\\'))
                {
                    return Err("TVBox 地址必须是局域网 HTTP 地址".into());
                }
            }
        }
        if matches!(
            key,
            "proxy_url"
                | "default_referer"
                | "default_origin"
                | "allowed_hosts"
                | "default_user_agent"
                | "tvbox_endpoint"
                | "proxy_mode"
                | "proxy_bypass"
                | "queue_active_days"
                | "legal_terms_version"
        ) {
            if let Some(text) = value.as_str() {
                if text.chars().any(|ch| ch.is_control()) {
                    return Err("设置值不能包含控制字符".into());
                }
            }
        }
        if key == "proxy_mode" {
            if let Some(mode) = value.as_str() {
                if !matches!(mode, "direct" | "manual" | "system" | "") {
                    return Err("代理模式无效".into());
                }
            }
        }
        if key == "bt_max_connections" {
            let value = value
                .as_u64()
                .ok_or_else(|| "BT 最大连接数必须是整数".to_string())?;
            if !(10..=1000).contains(&value) {
                return Err("BT 最大连接数必须在 10 到 1000 之间".into());
            }
        }
        if key == "bt_upload_limit_kib" {
            let value = value
                .as_u64()
                .ok_or_else(|| "BT 上传限制必须是整数".to_string())?;
            if value > 1_048_576 {
                return Err("BT 上传限制不能超过 1048576 KiB/s".into());
            }
        }
        if key == "download_hourly_quota_mib" {
            let value = value
                .as_u64()
                .ok_or_else(|| "每小时流量配额必须是整数".to_string())?;
            if value > 1_048_576 {
                return Err("每小时流量配额不能超过 1048576 MiB".into());
            }
        }
        if key == "queue_profiles" {
            validate_queue_profiles(value)?;
        }
        if key == "default_origin" {
            if let Some(origin) = value.as_str() {
                let origin = origin.trim().to_ascii_lowercase();
                if !origin.is_empty()
                    && !(origin.starts_with("http://") || origin.starts_with("https://"))
                {
                    return Err("默认 Origin 必须是 HTTP(S) 地址".into());
                }
            }
        }
        if key == "allowed_hosts" {
            if let Some(hosts) = value.as_str() {
                if hosts.chars().any(char::is_control)
                    || hosts
                        .split([',', ';'])
                        .any(|item| item.trim().contains("//"))
                {
                    return Err("允许的域名列表无效".into());
                }
            }
        }
        if key == "site_rules" {
            let raw = value
                .as_str()
                .ok_or_else(|| "站点规则必须是文本".to_string())?;
            crate::site_rules::validate_site_rules(raw)?;
        }
        if key == "task_column_layout" {
            validate_ui_layout(
                value,
                &["name", "progress", "status", "speed", "size", "actions"],
                true,
                "name",
            )?;
        }
        if key == "toolbar_actions" {
            validate_ui_layout(
                value,
                &[
                    "new",
                    "paste",
                    "batch",
                    "harvest",
                    "start_all",
                    "pause_all",
                    "cast",
                    "tvbox",
                    "extension",
                ],
                false,
                "new",
            )?;
        }
        if key == "task_sort" {
            let sort = value
                .as_str()
                .ok_or_else(|| "任务排序设置必须是文本".to_string())?;
            let (field, direction) = sort.split_once(':').unwrap_or((sort, "asc"));
            if !matches!(
                field,
                "queue" | "name" | "progress" | "status" | "speed" | "size"
            ) || !matches!(direction, "asc" | "desc")
            {
                return Err("任务排序设置无效".into());
            }
        }
        Ok(())
    }
}

fn validate_ui_layout(
    value: &Value,
    allowed: &[&str],
    widths: bool,
    required_visible: &str,
) -> Result<(), String> {
    let raw = value
        .as_str()
        .ok_or_else(|| "界面布局设置必须是文本".to_string())?;
    if raw.is_empty() {
        return Ok(());
    }
    if raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err("界面布局设置过长或包含控制字符".into());
    }
    let mut seen = HashSet::new();
    let mut required_enabled = false;
    for entry in raw.split(',') {
        let parts: Vec<_> = entry.split(':').collect();
        if parts.len() != if widths { 3 } else { 2 } {
            return Err("界面布局设置格式无效".into());
        }
        let id = parts[0];
        if !allowed.contains(&id) || !seen.insert(id) {
            return Err("界面布局包含未知或重复项目".into());
        }
        let enabled = parts.last().is_some_and(|flag| matches!(*flag, "0" | "1"));
        if !enabled {
            return Err("界面布局启用状态无效".into());
        }
        if widths {
            let width = parts[1]
                .parse::<u32>()
                .map_err(|_| "任务列宽度必须是整数".to_string())?;
            if !(48..=800).contains(&width) {
                return Err("任务列宽度必须在 48 到 800 之间".into());
            }
        }
        if id == required_visible && parts.last() == Some(&"1") {
            required_enabled = true;
        }
    }
    if !required_enabled {
        return Err(format!("界面布局必须保留 {required_visible}"));
    }
    Ok(())
}

pub(super) fn validate_queue_profiles(value: &Value) -> Result<(), String> {
    let profiles: Vec<QueueProfile> = serde_json::from_value(value.clone())
        .map_err(|error| format!("队列配置格式无效: {error}"))?;
    if profiles.is_empty() || profiles.len() > 32 {
        return Err("必须保留 1 到 32 个队列".into());
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for profile in &profiles {
        let id = profile.id.trim();
        if id.is_empty()
            || id.len() > 40
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !ids.insert(id.to_ascii_lowercase())
        {
            return Err("队列编号必须唯一，且只能包含字母、数字、连字符和下划线".into());
        }
        let name = profile.name.trim();
        if name.is_empty()
            || name.chars().count() > 40
            || name.chars().any(char::is_control)
            || !names.insert(name.to_lowercase())
        {
            return Err("队列名称必须为 1 到 40 个可见字符".into());
        }
        if !(-100..=100).contains(&profile.priority) {
            return Err("队列优先级必须在 -100 到 100 之间".into());
        }
        if !(1..=64).contains(&profile.max_active) {
            return Err("队列并发数必须在 1 到 64 之间".into());
        }
        if profile.speed_limit_kib > 1_048_576 {
            return Err("队列限速不能超过 1048576 KiB/s".into());
        }
        if !valid_clock(&profile.start_time) || !valid_clock(&profile.stop_time) {
            return Err("队列计划时间必须使用 HH:mm".into());
        }
        let days: Vec<_> = profile.active_days.split(',').map(str::trim).collect();
        let unique_days: HashSet<_> = days.iter().copied().collect();
        if days.is_empty()
            || unique_days.len() != days.len()
            || !days.iter().all(|day| {
                day.parse::<u8>()
                    .is_ok_and(|value| (1..=7).contains(&value))
            })
        {
            return Err("队列活动星期必须使用 1 到 7".into());
        }
        if !matches!(
            profile.completion_action.as_str(),
            "" | "none" | "shutdown" | "sleep" | "hibernate"
        ) {
            return Err("队列完成动作无效".into());
        }
    }
    if !profiles
        .iter()
        .any(|profile| profile.id == crate::DEFAULT_QUEUE_ID)
    {
        return Err("默认队列不能删除".into());
    }
    Ok(())
}

fn valid_clock(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60)
}

pub const PUBLIC_SETTING_KEYS: &[&str] = &[
    "browser_takeover_enabled",
    "browser_takeover_minimum_bytes",
    "legal_terms_accepted",
    "download_speed_limit_kib",
    "download_hourly_quota_mib",
    "download_speed_schedule_enabled",
    "download_speed_schedule_start",
    "download_speed_schedule_end",
    "download_speed_schedule_kib",
    "auto_category_dirs",
    "browser_category_dirs",
    "queue_max_active",
    "queue_profiles",
    "site_rules",
    "av_scan_enabled",
    "av_scan_command",
    "torrent_watch_dir",
    "watch_torrents",
    "download_dir",
    "temp_dir",
    "default_concurrency",
    "proxy_url",
    "ffmpeg_path",
    "clipboard_watch",
    "completion_sound_enabled",
    "download_progress_window_enabled",
    "download_complete_popup_enabled",
    "resume_interrupted_on_startup",
    "auto_retry_failed_max",
    "existing_file_policy",
    "live_record_max_minutes",
    "download_subtitles",
    "skip_ad_segments",
    "keep_temp_files",
    "default_user_agent",
    "tvbox_endpoint",
    "dark_mode",
    "allow_duplicate",
    "queue_auto_start_enabled",
    "queue_auto_start_time",
    "queue_auto_stop_enabled",
    "queue_auto_stop_time",
    "default_referer",
    "default_origin",
    "allowed_hosts",
    "http_chunk_size_mb",
    "completion_power_action",
    "start_on_login",
    "queue_active_days",
    "proxy_mode",
    "proxy_bypass",
    "reduce_motion",
    "harvest_minimum_bytes",
    "av_scan_fail_on_threat",
    "bt_upload_limit_kib",
    "bt_max_connections",
    "bt_enable_dht",
    "preferred_cast_device_id",
    "task_column_layout",
    "toolbar_actions",
    "task_sort",
];
