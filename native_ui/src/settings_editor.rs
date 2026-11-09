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
    ("外观", "dark_mode", "深色模式"),
    ("外观", "reduce_motion", "减少动态效果"),
    ("外观", "task_sort", "任务排序"),
];

pub struct SettingsEditor {
    pub window: SettingsWindow,
    pub values: BTreeMap<String, Value>,
    pub dirty: BTreeMap<String, Value>,
    invalid: BTreeMap<String, String>,
    rules: Vec<crate::site_rules::SiteRuleDraft>,
    rules_error: Option<String>,
    rules_changed: bool,
}

impl SettingsEditor {
    pub fn new(response: &CorePipeResponse, dark: bool) -> Result<Self, slint::PlatformError> {
        let window = SettingsWindow::new()?;
        crate::native_window::apply_theme(&window, dark);
        window.set_groups(ModelRc::new(VecModel::from(
            GROUPS
                .iter()
                .map(|group| (*group).into())
                .collect::<Vec<_>>(),
        )));
        let values: BTreeMap<String, Value> = serde_json::to_value(response)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .collect();
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

    pub fn edit(&mut self, key: &str, text: &str) -> Result<(), String> {
        let original = self.values.get(key).ok_or("未知设置项")?;
        let value = match original {
            Value::Bool(_) => Value::Bool(text.parse().map_err(|_| "开关值无效")?),
            Value::Number(_) => {
                let number: u64 = match text.trim().parse() {
                    Ok(number) => number,
                    Err(_) => {
                        self.invalid.insert(key.into(), text.into());
                        return Err("请输入有效的非负整数".into());
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
                let choices: &[&str] = match *key {
                    "proxy_mode" => &["direct", "system", "manual"],
                    "existing_file_policy" => &["rename", "overwrite", "skip"],
                    "completion_power_action" => {
                        &["none", "shutdown", "sleep", "hibernate", "exit"]
                    }
                    "task_sort" => &[
                        "queue:asc",
                        "name:asc",
                        "name:desc",
                        "progress:asc",
                        "size:asc",
                        "size:desc",
                        "status:asc",
                        "speed:desc",
                    ],
                    _ => &[],
                };
                let kind = if value.is_boolean() {
                    "bool"
                } else if value.is_number() {
                    "number"
                } else if !choices.is_empty() {
                    "choice"
                } else if key.ends_with("_dir") || *key == "torrent_watch" {
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
                        .cloned()
                        .unwrap_or_else(|| {
                            value
                                .as_str()
                                .map(str::to_owned)
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
                saved
                    .row
                    .host
                    .trim()
                    .eq_ignore_ascii_case(rule.row.host.trim())
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
        self.values = serde_json::to_value(response)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .collect();
        crate::native_window::apply_theme(
            &self.window,
            self.values
                .get("dark_mode")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
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
        if saved {
            self.render(&self.window.get_selected_group());
        }
    }

    pub fn storage_changes(&self) -> Result<BTreeMap<String, Value>, String> {
        if let Some(key) = self.invalid.keys().next() {
            let label = FIELDS
                .iter()
                .find(|(_, field, _)| *field == key)
                .map(|(_, _, label)| *label)
                .unwrap_or(key);
            return Err(format!("{label}：请输入有效的非负整数"));
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
            "legal_accepted" => "legal_terms_accepted",
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
}
