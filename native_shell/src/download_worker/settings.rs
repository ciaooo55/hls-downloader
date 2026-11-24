//! Current settings, their validation and persisted updates.

use super::{
    load_queue_profiles, proxy_url_allowed, reject_scan_shell, validate_helper_executable,
    CoreCoordinator,
};
use crate::output_path::reject_path_escape;
use crate::QueueProfile;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::Ordering;

#[derive(Clone)]
pub(super) struct CachedSettings {
    version: u64,
    settings: CoreSettings,
    has_explicit_download_dir: bool,
    env_download_dir: Option<OsString>,
}

#[derive(Debug, Clone)]
pub struct CoreSettings {
    pub takeover_enabled: bool,
    pub takeover_minimum_bytes: u64,
    pub legal_accepted: bool,
    pub speed_limit_kib: u64,
    pub speed_limit_enabled: bool,
    pub quick_download_enabled: bool,
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
    pub hls_remux_ts_to_mp4: bool,
    pub keep_temp_files: bool,
    pub default_user_agent: String,
    pub tvbox_endpoint: String,
    pub theme_mode: String,
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
    pub task_sort: String,
}

impl CoreCoordinator {
    pub fn settings(&self) -> Result<CoreSettings, String> {
        let version = self.settings_version.load(Ordering::Acquire);
        if let Ok(guard) = self.settings_cache.read() {
            if let Some(cached) = guard.as_ref() {
                if cached.version == version {
                    if cached.has_explicit_download_dir {
                        return Ok(cached.settings.clone());
                    }
                    let current_env = std::env::var_os("HLS_V7_DOWNLOAD_DIR");
                    if current_env == cached.env_download_dir {
                        return Ok(cached.settings.clone());
                    }
                    let mut settings = cached.settings.clone();
                    settings.download_dir = crate::default_v7_download_dir()
                        .to_string_lossy()
                        .into_owned();
                    return Ok(settings);
                }
            }
        }
        let core = self.lock()?;
        let version = self.settings_version.load(Ordering::Acquire);
        let configured_download_dir = core.store().setting_string("download_dir", "")?;
        let has_explicit_download_dir = !(configured_download_dir.trim().is_empty()
            || Path::new(&configured_download_dir).is_relative());
        let env_download_dir = if has_explicit_download_dir {
            None
        } else {
            std::env::var_os("HLS_V7_DOWNLOAD_DIR")
        };
        let download_dir = if has_explicit_download_dir {
            configured_download_dir
        } else {
            crate::default_v7_download_dir()
                .to_string_lossy()
                .into_owned()
        };
        let speed_limit_kib = core.store().setting_u64("download_speed_limit_kib", 0)?;
        let speed_limit_enabled = core
            .store()
            .setting_bool("download_speed_limit_enabled", speed_limit_kib > 0)?;
        let quick_download_enabled = core
            .store()
            .setting_bool("browser_quick_download_enabled", false)?;
        let loaded = CoreSettings {
            takeover_enabled: core
                .store()
                .setting_bool("browser_takeover_enabled", true)?,
            takeover_minimum_bytes: core
                .store()
                .setting_u64("browser_takeover_minimum_bytes", 0)?,
            legal_accepted: core.store().setting_bool("legal_terms_accepted", false)?,
            speed_limit_kib,
            speed_limit_enabled,
            quick_download_enabled,
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
            default_concurrency: core
                .store()
                .setting_u64("default_concurrency", 12)?
                .clamp(1, 64),
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
            hls_remux_ts_to_mp4: core.store().setting_bool("hls_remux_ts_to_mp4", false)?,
            keep_temp_files: core.store().setting_bool("keep_temp_files", false)?,
            default_user_agent: core.store().setting_string("default_user_agent", "")?,
            tvbox_endpoint: core.store().setting_string("tvbox_endpoint", "")?,
            theme_mode: core.store().theme_mode()?,
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
            task_sort: core.store().setting_string("task_sort", "queue:asc")?,
        };
        if let Ok(mut guard) = self.settings_cache.write() {
            *guard = Some(CachedSettings {
                version,
                settings: loaded.clone(),
                has_explicit_download_dir,
                env_download_dir,
            });
        }
        Ok(loaded)
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
        self.set_settings(BTreeMap::from([(key.to_string(), value)]))
    }

    pub fn set_settings(&self, mut values: BTreeMap<String, Value>) -> Result<(), String> {
        for (key, value) in &values {
            Self::validate_setting(key, value)?;
        }
        let caller_supplied_profiles = values.contains_key("queue_profiles");
        let mut queue_profiles = if let Some(value) = values.get("queue_profiles") {
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
        if let Some(limit_kib) = values
            .get("download_speed_limit_kib")
            .and_then(Value::as_u64)
        {
            if !values.contains_key("download_speed_limit_enabled") {
                values.insert(
                    "download_speed_limit_enabled".into(),
                    Value::Bool(limit_kib > 0),
                );
            }
        }
        let mut core = self.lock()?;
        let normalize_power = |raw: &str| -> String {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                "none".into()
            } else {
                trimmed.to_string()
            }
        };
        // Keep global `queue_max_active` and `completion_power_action` synchronized
        // with the "default" queue profile so updating either side always takes effect.
        if let Some(profiles) = queue_profiles.as_mut() {
            let existing_profiles = load_queue_profiles(core.store())?;
            let existing_default = existing_profiles
                .iter()
                .find(|profile| profile.id == crate::DEFAULT_QUEUE_ID);
            let existing_max = core
                .store()
                .setting_u64("queue_max_active", 3)?
                .clamp(1, 64) as u32;
            let existing_power = core
                .store()
                .setting_string("completion_power_action", "none")?;
            if let Some(default_profile) = profiles
                .iter_mut()
                .find(|profile| profile.id == crate::DEFAULT_QUEUE_ID)
            {
                let mut profiles_modified = false;
                if let Some(req_max) = values.get("queue_max_active").and_then(Value::as_u64) {
                    let req_max = req_max.clamp(1, 64) as u32;
                    let profile_changed = existing_default
                        .is_none_or(|prev| prev.max_active != default_profile.max_active);
                    let global_changed = req_max != existing_max;
                    if !profile_changed && global_changed {
                        default_profile.max_active = req_max;
                        profiles_modified = true;
                    } else {
                        values.insert(
                            "queue_max_active".into(),
                            Value::from(default_profile.max_active),
                        );
                    }
                } else {
                    values.insert(
                        "queue_max_active".into(),
                        Value::from(default_profile.max_active),
                    );
                }
                if let Some(req_power) = values
                    .get("completion_power_action")
                    .and_then(Value::as_str)
                {
                    let req_power = normalize_power(req_power);
                    let profile_power = normalize_power(&default_profile.completion_action);
                    let prev_profile_power = existing_default
                        .map(|prev| normalize_power(&prev.completion_action))
                        .unwrap_or_else(|| normalize_power(&existing_power));
                    let profile_changed = profile_power != prev_profile_power;
                    let global_changed = req_power != normalize_power(&existing_power);
                    if !profile_changed && global_changed {
                        default_profile.completion_action = req_power;
                        profiles_modified = true;
                    } else {
                        values.insert(
                            "completion_power_action".into(),
                            Value::String(profile_power),
                        );
                    }
                } else {
                    let profile_power = normalize_power(&default_profile.completion_action);
                    values.insert(
                        "completion_power_action".into(),
                        Value::String(profile_power),
                    );
                }
                if profiles_modified {
                    values.insert(
                        "queue_profiles".into(),
                        serde_json::to_value(&*profiles)
                            .map_err(|error| format!("序列化队列配置失败: {error}"))?,
                    );
                }
            }
        } else if values.contains_key("queue_max_active")
            || values.contains_key("completion_power_action")
        {
            let stored_raw = core.store().setting_string("queue_profiles", "")?;
            if !stored_raw.trim().is_empty() {
                let mut profiles = load_queue_profiles(core.store())?;
                if let Some(default_profile) = profiles
                    .iter_mut()
                    .find(|profile| profile.id == crate::DEFAULT_QUEUE_ID)
                {
                    if let Some(max_active) = values.get("queue_max_active").and_then(Value::as_u64)
                    {
                        default_profile.max_active = max_active.clamp(1, 64) as u32;
                    }
                    if let Some(action) = values
                        .get("completion_power_action")
                        .and_then(Value::as_str)
                    {
                        default_profile.completion_action = normalize_power(action);
                    }
                    values.insert(
                        "queue_profiles".into(),
                        serde_json::to_value(&profiles)
                            .map_err(|error| format!("序列化队列配置失败: {error}"))?,
                    );
                    queue_profiles = Some(profiles);
                }
            }
        }
        let orphaned = if caller_supplied_profiles {
            if let Some(profiles) = &queue_profiles {
                let ids: HashSet<_> = profiles.iter().map(|profile| profile.id.as_str()).collect();
                core.tasks()
                    .into_iter()
                    .filter(|task| !ids.contains(task.queue_id.as_str()))
                    .map(|task| task.task_id)
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let previous_startup =
            if let Some(flag) = values.get("start_on_login").and_then(Value::as_bool) {
                let previous = core.store().setting_bool("start_on_login", false)?;
                crate::startup::apply(flag)?;
                Some(previous)
            } else {
                None
            };
        if let Err(error) =
            core.assign_queue_and_set_settings(orphaned, crate::DEFAULT_QUEUE_ID.into(), &values)
        {
            if let Some(previous) = previous_startup {
                crate::startup::apply(previous)
                    .map_err(|rollback| format!("{error}；恢复开机启动失败：{rollback}"))?;
            }
            return Err(error);
        }
        self.invalidate_settings_cache();
        if values.contains_key("ffmpeg_path") {
            super::sync_configured_ffmpeg(core.store());
        }
        if values.keys().any(|key| {
            matches!(
                key.as_str(),
                "download_speed_limit_kib"
                    | "download_speed_limit_enabled"
                    | "download_speed_schedule_enabled"
                    | "download_speed_schedule_start"
                    | "download_speed_schedule_end"
                    | "download_speed_schedule_kib"
            )
        }) {
            let configured_limit_kib = core.store().setting_u64("download_speed_limit_kib", 0)?;
            let speed_limit_enabled = core
                .store()
                .setting_bool("download_speed_limit_enabled", configured_limit_kib > 0)?;
            crate::net_policy::configure_global_schedule(
                crate::net_policy::effective_global_speed_limit_kib(
                    configured_limit_kib,
                    speed_limit_enabled,
                ),
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
        match key {
            "browser_takeover_enabled"
            | "browser_quick_download_enabled"
            | "legal_terms_accepted"
            | "download_speed_limit_enabled"
            | "download_speed_schedule_enabled"
            | "auto_category_dirs"
            | "av_scan_enabled"
            | "watch_torrents"
            | "clipboard_watch"
            | "completion_sound_enabled"
            | "download_progress_window_enabled"
            | "download_complete_popup_enabled"
            | "resume_interrupted_on_startup"
            | "download_subtitles"
            | "skip_ad_segments"
            | "hls_remux_ts_to_mp4"
            | "keep_temp_files"
            | "allow_duplicate"
            | "queue_auto_start_enabled"
            | "queue_auto_stop_enabled"
            | "start_on_login"
            | "reduce_motion"
            | "av_scan_fail_on_threat"
            | "bt_enable_dht"
                if !value.is_boolean() =>
            {
                return Err(format!("设置项 {key} 必须是布尔值"))
            }
            "queue_max_active" | "default_concurrency" | "http_chunk_size_mb"
                if !value.as_u64().is_some_and(|n| (1..=64).contains(&n)) =>
            {
                return Err(format!("设置项 {key} 必须是 1 到 64 的整数"))
            }
            "browser_takeover_minimum_bytes"
            | "download_speed_limit_kib"
            | "download_speed_schedule_kib"
            | "auto_retry_failed_max"
            | "live_record_max_minutes"
            | "harvest_minimum_bytes"
                if !value.is_u64() =>
            {
                return Err(format!("设置项 {key} 必须是非负整数"))
            }
            _ => {}
        }
        if matches!(
            key,
            "ffmpeg_path"
                | "av_scan_command"
                | "download_dir"
                | "temp_dir"
                | "torrent_watch_dir"
                | "proxy_url"
                | "tvbox_endpoint"
                | "default_referer"
                | "default_origin"
                | "allowed_hosts"
                | "default_user_agent"
                | "proxy_mode"
                | "proxy_bypass"
                | "queue_active_days"
                | "queue_auto_start_time"
                | "queue_auto_stop_time"
                | "existing_file_policy"
                | "completion_power_action"
                | "legal_terms_version"
                | "preferred_cast_device_id"
        ) && !value.is_string()
        {
            return Err(format!("设置项 {key} 必须是文本"));
        }
        if key == "queue_active_days" {
            validate_active_days(value.as_str().unwrap())?;
        }
        if key == "existing_file_policy"
            && !matches!(value.as_str(), Some("rename" | "overwrite" | "skip"))
        {
            return Err("已有文件处理策略无效".into());
        }
        if key == "completion_power_action"
            && !matches!(
                value.as_str(),
                Some("" | "none" | "shutdown" | "sleep" | "hibernate")
            )
        {
            return Err("完成动作无效".into());
        }
        if matches!(
            key,
            "download_speed_schedule_start"
                | "download_speed_schedule_end"
                | "queue_auto_start_time"
                | "queue_auto_stop_time"
        ) && !value.as_str().is_some_and(valid_clock)
        {
            return Err("限速计划时间必须使用有效的 HH:mm（00:00 到 23:59）".into());
        }
        if key == "theme_mode" && !matches!(value.as_str(), Some("system" | "light" | "dark")) {
            return Err("主题必须为 System / Light / Dark".into());
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
            let raw = match value {
                Value::Null => None,
                Value::String(raw) => Some(raw.clone()),
                Value::Object(_) => Some(value.to_string()),
                _ => return Err("分类目录格式无效".into()),
            };
            if let Some(raw) = raw {
                crate::category::validate_category_dirs(&raw)?;
                let dirs = crate::category::parse_category_dirs(&raw);
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
                | "download_dir"
                | "temp_dir"
                | "torrent_watch_dir"
                | "ffmpeg_path"
                | "av_scan_command"
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
            || id != profile.id
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
            || profile.name.chars().any(char::is_control)
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
        validate_active_days(&profile.active_days)?;
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

fn validate_active_days(value: &str) -> Result<(), String> {
    let mut days = HashSet::new();
    for day in value.split(',').map(str::trim) {
        let day = day
            .parse::<u8>()
            .map_err(|_| "队列活动星期必须使用 1 到 7".to_string())?;
        if !(1..=7).contains(&day) || !days.insert(day) {
            return Err("队列活动星期必须使用唯一的 1 到 7".into());
        }
    }
    Ok(())
}

fn valid_clock(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.bytes().all(|byte| byte.is_ascii_digit())
        && minute.bytes().all(|byte| byte.is_ascii_digit())
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60)
}

pub const PUBLIC_SETTING_KEYS: &[&str] = &[
    "browser_takeover_enabled",
    "browser_takeover_minimum_bytes",
    "browser_quick_download_enabled",
    "legal_terms_accepted",
    "download_speed_limit_kib",
    "download_speed_limit_enabled",
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
    "hls_remux_ts_to_mp4",
    "keep_temp_files",
    "default_user_agent",
    "tvbox_endpoint",
    "theme_mode",
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
    "task_sort",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_settings_event_preserves_the_previous_value() {
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("clipboard_watch", json!(true))
            .unwrap();
        {
            let core = coordinator.lock().unwrap();
            core.store().connection.execute_batch(
                "CREATE TRIGGER reject_settings_checkpoint BEFORE INSERT ON event_checkpoints BEGIN SELECT RAISE(ABORT, 'reject'); END;"
            ).unwrap();
        }
        assert!(coordinator
            .set_setting("clipboard_watch", json!(false))
            .is_err());
        assert!(coordinator.settings().unwrap().clipboard_watch);
    }

    #[test]
    fn malformed_policy_settings_are_rejected_before_persistence() {
        for (key, value) in [
            ("av_scan_enabled", json!("true")),
            ("download_speed_limit_kib", json!(-1)),
            ("download_speed_schedule_kib", json!("128")),
            ("default_concurrency", json!(1_u64 << 32)),
            ("temp_dir", json!("temp\u{0}dir")),
        ] {
            assert!(
                CoreCoordinator::validate_setting(key, &value).is_err(),
                "{key}"
            );
        }
        assert!(CoreCoordinator::validate_setting("download_speed_limit_kib", &json!(0)).is_ok());
    }

    #[test]
    fn settings_cache_serves_without_sqlite_and_invalidates_on_every_mutation() {
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        // Warm the cache.
        assert!(!coordinator.settings().unwrap().clipboard_watch);
        // Holding the core mutex proves a cache hit never locks PersistentCore or SQLite.
        {
            let _guard = coordinator.lock().unwrap();
            assert!(!coordinator.settings().unwrap().clipboard_watch);
        }
        // Mutation via set_setting invalidates cache immediately.
        coordinator
            .set_setting("clipboard_watch", json!(true))
            .unwrap();
        assert!(coordinator.settings().unwrap().clipboard_watch);

        // Mutation via set_site_rule_credential invalidates cache immediately.
        coordinator
            .set_setting("site_rules", json!(r#"[{"host":"cdn.test"}]"#))
            .unwrap();
        coordinator
            .set_site_rule_credential("cdn.test", "sid=1", &BTreeMap::new(), false)
            .unwrap();
        let rules = crate::parse_site_rules(&coordinator.settings().unwrap().site_rules);
        assert!(!rules[0].credential_ref.is_empty());

        // Direct store_mut().set_setting also bumps settings_version and invalidates cache.
        coordinator
            .lock()
            .unwrap()
            .store_mut()
            .set_setting("theme_mode", "dark")
            .unwrap();
        assert_eq!(coordinator.settings().unwrap().theme_mode, "dark");
    }

    #[test]
    fn global_queue_and_power_settings_stay_synced_with_default_queue_profile() {
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        // Save custom queue_profiles (creating the stored JSON that previously shadowed globals).
        coordinator
            .set_setting(
                "queue_profiles",
                json!([
                    QueueProfile {
                        id: "default".into(),
                        name: "默认队列".into(),
                        max_active: 3,
                        completion_action: "none".into(),
                        ..Default::default()
                    },
                    QueueProfile {
                        id: "night".into(),
                        name: "夜间队列".into(),
                        max_active: 1,
                        completion_action: "hibernate".into(),
                        ..Default::default()
                    }
                ]),
            )
            .unwrap();

        // Updating global queue_max_active and completion_power_action must update "default" profile.
        coordinator
            .set_settings(BTreeMap::from([
                ("queue_max_active".into(), json!(6)),
                ("completion_power_action".into(), json!("sleep")),
            ]))
            .unwrap();
        let settings = coordinator.settings().unwrap();
        assert_eq!(settings.queue_max, 6);
        assert_eq!(settings.completion_power_action, "sleep");
        let default_profile = settings
            .queue_profiles
            .iter()
            .find(|p| p.id == "default")
            .unwrap();
        assert_eq!(default_profile.max_active, 6);
        assert_eq!(default_profile.completion_action, "sleep");
        let night_profile = settings
            .queue_profiles
            .iter()
            .find(|p| p.id == "night")
            .unwrap();
        assert_eq!(night_profile.max_active, 1);
        assert_eq!(night_profile.completion_action, "hibernate");

        // Updating "default" inside queue_profiles must sync back to global keys.
        let mut updated_profiles = settings.queue_profiles;
        updated_profiles[0].max_active = 2;
        updated_profiles[0].completion_action = "shutdown".into();
        coordinator
            .set_setting("queue_profiles", json!(updated_profiles))
            .unwrap();
        let synced = coordinator.settings().unwrap();
        assert_eq!(synced.queue_max, 2);
        assert_eq!(synced.completion_power_action, "shutdown");
        assert_eq!(synced.queue_profiles[0].max_active, 2);
        assert_eq!(synced.queue_profiles[0].completion_action, "shutdown");
    }

    #[test]
    fn speed_limit_enabled_toggle_preserves_kib_and_updates_throttle() {
        let coordinator = CoreCoordinator::new(crate::PersistentCore::in_memory().unwrap());
        assert!(
            CoreCoordinator::validate_setting("download_speed_limit_enabled", &json!("true"))
                .is_err()
        );
        assert!(
            CoreCoordinator::validate_setting("browser_quick_download_enabled", &json!(1)).is_err()
        );
        assert!(
            CoreCoordinator::validate_setting("download_speed_limit_enabled", &json!(false))
                .is_ok()
        );
        assert!(
            CoreCoordinator::validate_setting("browser_quick_download_enabled", &json!(true))
                .is_ok()
        );

        coordinator
            .set_setting("browser_quick_download_enabled", json!(true))
            .unwrap();
        assert!(coordinator.settings().unwrap().quick_download_enabled);

        // Setting download_speed_limit_kib > 0 auto-enables throttling.
        coordinator
            .set_setting("download_speed_limit_kib", json!(512))
            .unwrap();
        let settings = coordinator.settings().unwrap();
        assert_eq!(settings.speed_limit_kib, 512);
        assert!(settings.speed_limit_enabled);
        assert_eq!(crate::net_policy::active_global_limit_bps(), 512.0 * 1024.0);

        // Toggling download_speed_limit_enabled OFF disables throttling without zeroing speed_limit_kib.
        coordinator
            .set_setting("download_speed_limit_enabled", json!(false))
            .unwrap();
        let paused = coordinator.settings().unwrap();
        assert_eq!(paused.speed_limit_kib, 512);
        assert!(!paused.speed_limit_enabled);
        assert_eq!(crate::net_policy::active_global_limit_bps(), 0.0);

        // Toggling download_speed_limit_enabled back ON restores the exact saved limit.
        coordinator
            .set_setting("download_speed_limit_enabled", json!(true))
            .unwrap();
        let resumed = coordinator.settings().unwrap();
        assert_eq!(resumed.speed_limit_kib, 512);
        assert!(resumed.speed_limit_enabled);
        assert_eq!(crate::net_policy::active_global_limit_bps(), 512.0 * 1024.0);

        // Explicit batch with speed_limit_kib > 0 and speed_limit_enabled = false preserves disabled state.
        coordinator
            .set_settings(BTreeMap::from([
                ("download_speed_limit_kib".into(), json!(256)),
                ("download_speed_limit_enabled".into(), json!(false)),
            ]))
            .unwrap();
        let explicit_off = coordinator.settings().unwrap();
        assert_eq!(explicit_off.speed_limit_kib, 256);
        assert!(!explicit_off.speed_limit_enabled);
        assert_eq!(crate::net_policy::active_global_limit_bps(), 0.0);

        // Reset global throttle so other parallel tests observe clean state.
        coordinator
            .set_setting("download_speed_limit_kib", json!(0))
            .unwrap();
    }
}
