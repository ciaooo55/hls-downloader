use crate::{SettingField, SettingsWindow};
use hls_client::CorePipeResponse;
use serde_json::Value;
use slint::{Model, ModelRc, VecModel};
use std::collections::BTreeMap;

pub const GROUPS: &[&str] = &[
    "通用",
    "下载",
    "计划",
    "网络",
    "媒体",
    "字幕翻译",
    "BT",
    "投屏与推送",
    "安全",
    "浏览器",
    "外观",
];

// Each entry names an existing Core setting. Saving sends only edited keys so
// settings changed by the browser or another UI window are preserved.
const FIELDS: &[(&str, &str, &str)] = &[
    ("通用", "download_dir", "默认下载目录"),
    ("通用", "temp_dir", "缓存与过程文件目录"),
    ("通用", "auto_category", "自动分类目录"),
    ("通用", "category_dir_media", "媒体目录"),
    ("通用", "category_dir_program", "程序目录"),
    ("通用", "category_dir_archive", "压缩包目录"),
    ("通用", "category_dir_other", "其他目录"),
    ("通用", "clipboard_watch", "监视剪贴板"),
    ("通用", "resume_interrupted", "恢复未完成任务"),
    ("通用", "start_on_login", "开机启动"),
    ("通用", "completion_sound_enabled", "完成提示音"),
    ("通用", "progress_window_enabled", "下载进度窗口"),
    ("通用", "complete_popup_enabled", "完成通知"),
    ("下载", "default_concurrency", "默认并发连接数"),
    ("下载", "queue_max", "最大同时任务数"),
    ("下载", "speed_limit_enabled", "启用全局限速"),
    ("下载", "speed_limit_kib", "全局限速 KiB/s（0 不限制）"),
    (
        "下载",
        "hourly_quota_mib",
        "滚动每小时流量配额 MiB（0 不限制）",
    ),
    ("下载", "http_chunk_size_mb", "HTTP 分段大小 MiB"),
    ("下载", "auto_retry_max", "失败自动重试次数"),
    ("下载", "existing_file_policy", "同名文件处理"),
    ("下载", "allow_duplicate", "默认允许重复"),
    ("下载", "keep_temp_files", "保留过程文件"),
    ("计划", "schedule_enabled", "启用定时限速"),
    ("计划", "schedule_start", "限速开始 HH:mm"),
    ("计划", "schedule_end", "限速结束 HH:mm"),
    ("计划", "schedule_kib", "计划限速 KiB/s"),
    ("计划", "queue_auto_start_enabled", "队列自动开始"),
    ("计划", "queue_auto_start_time", "自动开始时间 HH:mm"),
    ("计划", "queue_auto_stop_enabled", "队列自动停止"),
    ("计划", "queue_auto_stop_time", "自动停止时间 HH:mm"),
    (
        "计划",
        "queue_active_days",
        "活动日（周一为 1，以逗号分隔）",
    ),
    ("计划", "completion_power_action", "全部完成后"),
    ("网络", "proxy_mode", "代理模式"),
    ("网络", "proxy_url", "代理地址"),
    ("网络", "proxy_bypass", "直连地址列表"),
    ("网络", "default_user_agent", "默认 User-Agent"),
    ("网络", "default_referer", "默认 Referer"),
    ("网络", "default_origin", "默认 Origin"),
    ("网络", "allowed_hosts", "允许使用手动凭据的站点"),
    ("媒体", "ffmpeg_path", "FFmpeg 路径（留空使用随包组件）"),
    (
        "媒体",
        "live_record_max_minutes",
        "直播最大录制分钟数（0 不限制）",
    ),
    ("媒体", "download_subtitles", "下载关联字幕"),
    ("媒体", "skip_ad_segments", "跳过已识别广告分段"),
    ("媒体", "hls_remux_ts_to_mp4", "将 TS 点播无损封装为 MP4"),
    ("BT", "torrent_watch", "种子监视目录"),
    ("BT", "torrent_watch_enabled", "监视新种子"),
    ("BT", "bt_upload_limit_kib", "BT 上传上限 KiB/s"),
    ("BT", "bt_max_connections", "最大 Peer 连接数"),
    ("BT", "bt_enable_dht", "启用 DHT 节点发现"),
    ("投屏与推送", "tvbox_endpoint", "TVBox 接收端地址"),
    ("投屏与推送", "preferred_cast_device_id", "默认投屏设备 ID"),
    ("安全", "av_scan_enabled", "下载后扫描"),
    ("安全", "av_scan_command", "扫描命令"),
    ("安全", "av_scan_fail_on_threat", "发现威胁时阻止打开"),
    ("浏览器", "takeover_enabled", "启用浏览器接管"),
    ("浏览器", "takeover_minimum_bytes", "自动接管最小字节数"),
    (
        "浏览器",
        "quick_download_enabled",
        "浏览器接管静默直下（不弹确认框）",
    ),
    ("外观", "theme_mode", "主题 / Theme"),
    ("外观", "reduce_motion", "减少动态效果"),
    ("外观", "task_sort", "任务排序"),
];

// Choice settings keep Core values in storage and show product copy in the UI.
pub(crate) fn choice_options(key: &str) -> &'static [(&'static str, &'static str)] {
    match key {
        "theme_mode" => &[
            ("system", "跟随系统 / System"),
            ("light", "浅色 / Light"),
            ("dark", "深色 / Dark"),
        ],
        "proxy_mode" => &[
            ("direct", "直连"),
            ("system", "系统代理"),
            ("manual", "手动代理"),
        ],
        "existing_file_policy" => &[
            ("rename", "自动重命名"),
            ("overwrite", "覆盖已有文件"),
            ("skip", "跳过下载"),
        ],
        "completion_power_action" => &[
            ("none", "无操作"),
            ("shutdown", "关机"),
            ("sleep", "睡眠"),
            ("hibernate", "休眠"),
        ],
        "task_sort" => &[
            ("queue:asc", "队列顺序"),
            ("name:asc", "名称 A→Z"),
            ("name:desc", "名称 Z→A"),
            ("progress:asc", "进度从低到高"),
            ("size:asc", "大小从小到大"),
            ("size:desc", "大小从大到小"),
            ("status:asc", "按状态"),
            ("speed:desc", "速度从快到慢"),
        ],
        _ => &[],
    }
}

fn choice_label<'a>(key: &str, code: &'a str) -> &'a str {
    choice_options(key)
        .iter()
        .find(|(value, _)| *value == code)
        .map(|(_, label)| *label)
        .unwrap_or(code)
}

pub struct SettingsEditor {
    pub window: SettingsWindow,
    pub values: BTreeMap<String, Value>,
    pub dirty: BTreeMap<String, Value>,
    invalid: BTreeMap<String, (String, String)>,
    rules: Vec<crate::site_rules::SiteRuleDraft>,
    rules_error: Option<String>,
    rules_changed: bool,
}

fn settings_values(response: &CorePipeResponse) -> BTreeMap<String, Value> {
    let mut values: BTreeMap<_, _> = serde_json::to_value(response)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
        .into_iter()
        .collect();
    if let Some(Value::String(url)) = values.get_mut("proxy_url") {
        // Older Core versions can return the full URL. Keep only the editable vault identity.
        *url = hls_client::url_credentials::map_url_passwords(url, true, |_, _| Ok(()))
            .expect("redaction cannot fail");
    }
    values
}

impl SettingsEditor {
    pub fn new(
        response: &CorePipeResponse,
        parent: &crate::WorkbenchWindow,
    ) -> Result<Self, slint::PlatformError> {
        let window = SettingsWindow::new()?;
        crate::native_window::inherit_appearance(&window, parent);
        window.set_groups(ModelRc::new(VecModel::from(
            GROUPS
                .iter()
                .map(|group| (*group).into())
                .collect::<Vec<_>>(),
        )));
        let values = settings_values(response);
        let parsed = crate::site_rules::SiteRuleDraft::parse(
            values
                .get("site_rules")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let (rules, rules_error) = match parsed {
            Ok(rules) => (rules, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let editor = Self {
            window,
            values,
            dirty: BTreeMap::new(),
            invalid: BTreeMap::new(),
            rules,
            rules_error,
            rules_changed: false,
        };
        editor.render_rules();
        editor.render("通用");
        Ok(editor)
    }

    pub fn discard_unsaved_secrets(&mut self) {
        if self.dirty.remove("proxy_url").is_some() || self.invalid.remove("proxy_url").is_some() {
            self.render(&self.window.get_selected_group());
        }
    }

    pub fn edit(&mut self, key: &str, text: &str) -> Result<(), String> {
        let original = self.values.get(key).ok_or("未知设置项")?;
        if let Err(error) = validate_setting_text(key, text) {
            self.dirty.remove(key);
            let draft = if key == "proxy_url" { "" } else { text };
            self.invalid
                .insert(key.into(), (draft.into(), error.clone()));
            if key == "proxy_url" {
                self.render(&self.window.get_selected_group());
            }
            return Err(error);
        }
        // 下拉框显示中文选项，保存时换回 Core 使用的取值。
        let text = choice_options(key)
            .iter()
            .find(|(_, label)| *label == text)
            .map(|(code, _)| *code)
            .unwrap_or(text);
        let value = match original {
            Value::Bool(_) => Value::Bool(text.parse().map_err(|_| "开关值无效")?),
            Value::Number(_) => {
                let number: u64 = match text.trim().parse() {
                    Ok(number) => number,
                    Err(_) => {
                        let error = "请输入有效的非负整数";
                        self.invalid.insert(key.into(), (text.into(), error.into()));
                        return Err(error.into());
                    }
                };
                Value::from(number)
            }
            _ => Value::String(text.to_owned()),
        };
        self.invalid.remove(key);
        if &value == original {
            self.dirty.remove(key);
        } else {
            self.dirty.insert(key.to_owned(), value);
        }
        Ok(())
    }

    pub fn render(&self, group: &str) {
        let fields = FIELDS
            .iter()
            .filter(|(category, _, _)| *category == group)
            .filter_map(|(_, key, label)| {
                let value = self.dirty.get(*key).or_else(|| self.values.get(*key))?;
                let options = choice_options(key);
                let choices: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
                let kind = if value.is_boolean() {
                    "bool"
                } else if value.is_number() {
                    "number"
                } else if !choices.is_empty() {
                    "choice"
                } else if key.ends_with("_dir")
                    || key.starts_with("category_dir_")
                    || *key == "torrent_watch"
                {
                    "directory"
                } else {
                    "text"
                };
                Some(SettingField {
                    key: (*key).into(),
                    label: (*label).into(),
                    value: self
                        .invalid
                        .get(*key)
                        .map(|(draft, _)| draft.clone())
                        .unwrap_or_else(|| {
                            value
                                .as_str()
                                .map(|code| choice_label(key, code).to_owned())
                                .unwrap_or_else(|| value.to_string())
                        })
                        .into(),
                    checked: value.as_bool().unwrap_or(false),
                    kind: kind.into(),
                    choices: ModelRc::new(VecModel::from(
                        choices
                            .iter()
                            .map(|choice| (*choice).into())
                            .collect::<Vec<_>>(),
                    )),
                })
            })
            .collect::<Vec<_>>();
        self.window.set_fields(ModelRc::new(VecModel::from(fields)));
    }

    fn render_rules(&self) {
        self.window.set_site_rules(ModelRc::new(VecModel::from(
            self.rules
                .iter()
                .map(|rule| rule.row.clone())
                .collect::<Vec<_>>(),
        )));
        if let Some(error) = &self.rules_error {
            self.window.set_status_tone("warning".into());
            self.window.set_status(error.clone().into());
        }
    }

    pub fn edit_rule(&mut self, index: i32, key: &str, value: &str) {
        if self.window.get_busy() {
            return;
        }
        if let Some(rule) = self.rules.get_mut(index as usize) {
            rule.edit(key, value);
            self.rules_changed = true;
            rule.row.has_credential = crate::site_rules::SiteRuleDraft::parse(
                self.values
                    .get("site_rules")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_default()
            .iter()
            .any(|saved| {
                crate::site_rules::host_identity(&saved.row.host)
                    == crate::site_rules::host_identity(&rule.row.host)
                    && saved.row.has_credential
            });
            self.window
                .get_site_rules()
                .set_row_data(index as usize, rule.row.clone());
        }
    }

    pub fn rule_action(&mut self, index: i32, action: &str) -> Result<(), String> {
        if let Some(error) = &self.rules_error {
            return Err(error.clone());
        }
        if self.window.get_busy() {
            return Ok(());
        }
        let index = index as usize;
        match action {
            "add" if self.rules.len() < 100 => {
                self.rules.push(crate::site_rules::SiteRuleDraft::new())
            }
            "remove" if index < self.rules.len() => {
                self.rules.remove(index);
            }
            "up" if index > 0 && index < self.rules.len() => self.rules.swap(index, index - 1),
            "down" if index < self.rules.len() && index + 1 < self.rules.len() => {
                self.rules.swap(index, index + 1)
            }
            _ => return Ok(()),
        }
        self.rules_changed = true;
        self.render_rules();
        Ok(())
    }

    pub fn credential_host(&self, index: i32) -> Result<String, String> {
        if self.rules_changed {
            return Err("请先保存站点规则，再管理登录信息".into());
        }
        self.rules
            .get(index as usize)
            .map(|rule| rule.row.host.trim().to_owned())
            .filter(|host| !host.is_empty())
            .ok_or_else(|| "请先填写并保存站点域名".into())
    }

    pub fn receive_settings(&mut self, response: &CorePipeResponse, saved: bool) {
        let values = settings_values(response);
        let refresh_proxy = values.get("proxy_url") != self.values.get("proxy_url")
            && self.window.get_selected_group() == "网络"
            && !self.dirty.contains_key("proxy_url")
            && !self.invalid.contains_key("proxy_url");
        self.values = values;
        let mode = crate::theme::ThemeMode::from_response(Some(response));
        self.values.insert("theme_mode".into(), mode.code().into());
        crate::native_window::apply_theme(&self.window, mode.is_dark());
        if saved {
            self.dirty.clear();
            self.invalid.clear();
            self.rules_changed = false;
        }
        if !self.rules_changed {
            match crate::site_rules::SiteRuleDraft::parse(
                self.values
                    .get("site_rules")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ) {
                Ok(rules) => {
                    self.rules = rules;
                    self.rules_error = None;
                }
                Err(error) => self.rules_error = Some(error),
            }
            self.render_rules();
        }
        if saved || refresh_proxy {
            self.render(&self.window.get_selected_group());
        }
    }

    pub fn storage_changes(&self) -> Result<BTreeMap<String, Value>, String> {
        if let Some((key, (_, error))) = self.invalid.first_key_value() {
            let label = FIELDS
                .iter()
                .find(|(_, field, _)| *field == key)
                .map(|(_, _, label)| *label)
                .unwrap_or(key);
            return Err(format!("{label}：{error}"));
        }
        let mut changes = storage_changes(&self.values, &self.dirty);
        if self.rules_changed {
            changes.insert(
                "site_rules".into(),
                crate::site_rules::encode(
                    &self.rules,
                    self.values
                        .get("site_rules")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )?
                .into(),
            );
        }
        Ok(changes)
    }
}

fn validate_setting_text(key: &str, text: &str) -> Result<(), String> {
    if matches!(key, "proxy_url" | "download_dir" | "temp_dir")
        && text.chars().any(char::is_control)
    {
        return Err("设置值不能包含控制字符".into());
    }
    if matches!(
        key,
        "schedule_start" | "schedule_end" | "queue_auto_start_time" | "queue_auto_stop_time"
    ) && !valid_clock(text)
    {
        return Err("计划时间必须使用有效的 HH:mm（00:00 到 23:59）".into());
    }
    Ok(())
}

pub(crate) fn valid_clock(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && bytes[..2].iter().chain(&bytes[3..]).all(u8::is_ascii_digit)
        && &bytes[..2] <= b"23".as_slice()
        && &bytes[3..] <= b"59".as_slice()
}

fn storage_changes(
    values: &BTreeMap<String, Value>,
    dirty: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let mut result = BTreeMap::new();
    let mut categories_changed = false;
    for (key, value) in dirty {
        if key.starts_with("category_dir_") {
            categories_changed = true;
            continue;
        }
        let storage_key = match key.as_str() {
            "takeover_enabled" => "browser_takeover_enabled",
            "takeover_minimum_bytes" => "browser_takeover_minimum_bytes",
            "quick_download_enabled" => "browser_quick_download_enabled",
            "legal_accepted" => "legal_terms_accepted",
            "speed_limit_enabled" => "download_speed_limit_enabled",
            "speed_limit_kib" => "download_speed_limit_kib",
            "hourly_quota_mib" => "download_hourly_quota_mib",
            "schedule_enabled" => "download_speed_schedule_enabled",
            "schedule_start" => "download_speed_schedule_start",
            "schedule_end" => "download_speed_schedule_end",
            "schedule_kib" => "download_speed_schedule_kib",
            "auto_category" => "auto_category_dirs",
            "queue_max" => "queue_max_active",
            "torrent_watch" => "torrent_watch_dir",
            "torrent_watch_enabled" => "watch_torrents",
            "progress_window_enabled" => "download_progress_window_enabled",
            "complete_popup_enabled" => "download_complete_popup_enabled",
            "resume_interrupted" => "resume_interrupted_on_startup",
            "auto_retry_max" => "auto_retry_failed_max",
            other => other,
        };
        result.insert(storage_key.to_owned(), value.clone());
    }
    if categories_changed {
        let category_dirs: serde_json::Map<_, _> = ["media", "program", "archive", "other"]
            .iter()
            .map(|category| {
                let key = format!("category_dir_{category}");
                (
                    (*category).to_owned(),
                    dirty
                        .get(&key)
                        .or_else(|| values.get(&key))
                        .cloned()
                        .unwrap_or_else(|| Value::String(String::new())),
                )
            })
            .collect();
        result.insert("browser_category_dirs".into(), Value::Object(category_dirs));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_times_require_a_valid_hhmm() {
        for key in [
            "schedule_start",
            "schedule_end",
            "queue_auto_start_time",
            "queue_auto_stop_time",
        ] {
            for valid in ["00:00", "23:59", "08:30", "22:00"] {
                assert!(validate_setting_text(key, valid).is_ok());
            }
            for invalid in [
                "", "26:99", "25:99", "24:00", "12:60", "1:00", "01:0", "+1:00", " 01:00",
                "01:00 ", "ab:cd", "中文a", "1:000",
            ] {
                assert!(
                    validate_setting_text(key, invalid).is_err(),
                    "{key}: {invalid}"
                );
            }
        }
        assert!(validate_setting_text("download_dir", "").is_ok());
    }

    #[test]
    fn proxy_and_directory_edits_reject_controls_before_trimming() {
        for key in ["proxy_url", "download_dir", "temp_dir"] {
            for value in ["\rvalue", "a\0b", "value\n", "a\u{0085}b"] {
                assert!(validate_setting_text(key, value).is_err());
            }
        }
    }

    #[test]
    fn settings_keep_only_the_passwordless_proxy_identity() {
        let clean = "http://user@proxy:8080";
        for proxy in [clean, "http://user:p%40ss@proxy:8080"] {
            let response = serde_json::from_value(serde_json::json!({
                "type": "settings", "request_id": 1, "proxy_url": proxy,
                "takeover_enabled": true, "takeover_minimum_bytes": 0,
                "legal_accepted": true, "speed_limit_kib": 0, "download_dir": ""
            }))
            .unwrap();
            let values = settings_values(&response);
            assert_eq!(values["proxy_url"], clean);
            assert!(validate_setting_text("proxy_url", clean).is_ok());
            assert!(!storage_changes(&values, &BTreeMap::new()).contains_key("proxy_url"));
        }
    }
    #[test]
    fn editing_one_category_keeps_the_other_directories_and_uses_core_storage_keys() {
        let values = BTreeMap::from([
            ("category_dir_media".into(), Value::from("D:/media")),
            ("category_dir_archive".into(), Value::from("D:/archives")),
        ]);
        let dirty = BTreeMap::from([
            ("category_dir_media".into(), Value::from("E:/media")),
            ("speed_limit_kib".into(), Value::from(400)),
        ]);
        let result = storage_changes(&values, &dirty);
        assert_eq!(result["browser_category_dirs"]["archive"], "D:/archives");
        assert_eq!(result["browser_category_dirs"]["media"], "E:/media");
        assert_eq!(result["download_speed_limit_kib"], 400);
        assert!(!result.contains_key("category_dir_media"));
        assert!(!result.contains_key("speed_limit_kib"));
    }

    #[test]
    fn speed_limit_toggle_and_quick_download_map_to_core_storage_keys() {
        let values = BTreeMap::from([
            ("speed_limit_enabled".into(), Value::from(false)),
            ("quick_download_enabled".into(), Value::from(false)),
        ]);
        let dirty = BTreeMap::from([
            ("speed_limit_enabled".into(), Value::from(true)),
            ("quick_download_enabled".into(), Value::from(true)),
        ]);
        let result = storage_changes(&values, &dirty);
        assert_eq!(result["download_speed_limit_enabled"], Value::from(true));
        assert_eq!(result["browser_quick_download_enabled"], Value::from(true));
        assert!(!result.contains_key("speed_limit_enabled"));
        assert!(!result.contains_key("quick_download_enabled"));
        assert!(FIELDS
            .iter()
            .any(|(group, key, _)| *group == "下载" && *key == "speed_limit_enabled"));
        assert!(FIELDS
            .iter()
            .any(|(group, key, _)| *group == "浏览器" && *key == "quick_download_enabled"));
    }
}
