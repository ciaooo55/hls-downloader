use crate::{QueueWindow, WorkbenchWindow};
use hls_client::{QueueProfile, DEFAULT_QUEUE_ID};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

pub fn ordered_profiles(profiles: &[QueueProfile]) -> Vec<QueueProfile> {
    let mut items = if profiles.is_empty() {
        vec![QueueProfile::default()]
    } else {
        profiles.to_vec()
    };
    items.sort_by_key(|profile| profile.priority);
    if !items.iter().any(|profile| profile.id == DEFAULT_QUEUE_ID) {
        items.insert(0, QueueProfile::default());
    }
    items
}

pub fn queue_option_names(profiles: &[QueueProfile]) -> Vec<SharedString> {
    profiles.iter().map(|p| p.name.clone().into()).collect()
}

pub fn queue_index_for_id(profiles: &[QueueProfile], queue_id: &str) -> i32 {
    let target = if queue_id.trim().is_empty() {
        DEFAULT_QUEUE_ID
    } else {
        queue_id.trim()
    };
    profiles
        .iter()
        .position(|p| p.id == target)
        .or_else(|| profiles.iter().position(|p| p.id == DEFAULT_QUEUE_ID))
        .unwrap_or(0) as i32
}

pub fn queue_id_at(profiles: &[QueueProfile], index: i32) -> String {
    profiles
        .get(index.max(0) as usize)
        .map(|p| p.id.clone())
        .unwrap_or_else(|| DEFAULT_QUEUE_ID.to_string())
}

pub fn parse_active_days(text: &str) -> Result<[bool; 7], String> {
    if text.trim().is_empty() {
        return Err("至少选择一天".into());
    }
    let days = text
        .split(',')
        .map(|s| s.trim().parse::<u8>().ok().filter(|n| (1..=7).contains(n)))
        .collect::<Option<std::collections::BTreeSet<_>>>()
        .ok_or_else(|| "星期应为 1 至 7，以逗号分隔".to_string())?;
    if days.is_empty() {
        return Err("至少选择一天".into());
    }
    let mut flags = [false; 7];
    for day in days {
        flags[(day - 1) as usize] = true;
    }
    Ok(flags)
}

pub fn format_active_days(flags: &[bool; 7]) -> String {
    flags
        .iter()
        .enumerate()
        .filter(|&(_, &active)| active)
        .map(|(index, _)| (index + 1).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub struct QueueEditor {
    pub window: QueueWindow,
    profiles: Vec<QueueProfile>,
    selected: usize,
}

impl QueueEditor {
    pub fn new(
        mut profiles: Vec<QueueProfile>,
        parent: &WorkbenchWindow,
        assign: bool,
    ) -> Result<Self, slint::PlatformError> {
        if profiles.is_empty() {
            profiles.push(QueueProfile::default());
        }
        let window = QueueWindow::new()?;
        crate::native_window::inherit_appearance(&window, parent);
        window.set_can_assign(assign);
        let editor = Self {
            window,
            profiles,
            selected: 0,
        };
        editor.render();
        Ok(editor)
    }

    pub fn select(&mut self, index: usize) -> Result<(), String> {
        self.flush()?;
        if index >= self.profiles.len() {
            return Err("队列不存在".into());
        }
        self.selected = index;
        self.render();
        Ok(())
    }

    pub fn edit(&mut self, action: &str) -> Result<(), String> {
        if action == "close" {
            let _ = self.window.hide();
            return Ok(());
        }
        self.flush()?;
        match action {
            "add" => {
                if self.profiles.len() >= 32 {
                    return Err("最多 32 个队列".into());
                }
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let next_slot = self.profiles.len() + 1;
                let number = (1..=next_slot)
                    .find(|n| {
                        !self
                            .profiles
                            .iter()
                            .any(|profile| profile.name == format!("新队列 {n}"))
                    })
                    .unwrap_or(next_slot);
                self.profiles.push(QueueProfile {
                    id: format!("queue-{nonce:x}"),
                    name: format!("新队列 {number}"),
                    ..Default::default()
                });
                self.selected = self.profiles.len() - 1;
            }
            "remove" if self.profiles.len() > 1 && self.profiles[self.selected].id != "default" => {
                self.profiles.remove(self.selected);
                self.selected = self.selected.min(self.profiles.len() - 1);
            }
            "up" | "down" => {
                let next = if action == "up" {
                    self.selected.saturating_sub(1)
                } else {
                    (self.selected + 1).min(self.profiles.len() - 1)
                };
                self.profiles.swap(self.selected, next);
                self.selected = next;
            }
            _ => {}
        }
        self.render();
        Ok(())
    }

    pub fn toggle_day(&mut self, day: i32) {
        if !(1..=7).contains(&day) {
            return;
        }
        let mut flags = [false; 7];
        let current = self.window.get_active_day_flags();
        for (index, slot) in flags.iter_mut().enumerate() {
            *slot = current.row_data(index).unwrap_or(false);
        }
        let slot = (day - 1) as usize;
        flags[slot] = !flags[slot];
        self.window
            .set_active_day_flags(ModelRc::new(VecModel::from(flags.to_vec())));
        self.window
            .set_active_days(format_active_days(&flags).into());
        self.window.set_status("".into());
    }

    pub fn values(&mut self) -> Result<(Vec<QueueProfile>, String), String> {
        self.flush()?;
        let mut names = std::collections::HashSet::new();
        if self
            .profiles
            .iter()
            .any(|p| !names.insert(p.name.to_lowercase()))
        {
            return Err("队列名称不能重复".into());
        }
        Ok((
            self.profiles.clone(),
            self.profiles[self.selected].id.clone(),
        ))
    }

    pub fn receive_saved_profiles(&mut self, mut profiles: Vec<QueueProfile>) {
        let previous_id = self.profiles.get(self.selected).map(|p| p.id.clone());
        if profiles.is_empty() {
            profiles.push(QueueProfile::default());
        }
        self.profiles = profiles;
        if let Some(id) = previous_id {
            if let Some(pos) = self.profiles.iter().position(|p| p.id == id) {
                self.selected = pos;
            } else {
                self.selected = self.selected.min(self.profiles.len() - 1);
            }
        } else {
            self.selected = self.selected.min(self.profiles.len() - 1);
        }
        self.render();
    }

    fn flush(&mut self) -> Result<(), String> {
        let w = &self.window;
        let name = w.get_queue_name().trim().to_owned();
        if name.is_empty() || name.chars().count() > 40 {
            return Err("队列名称应为 1 至 40 个字符".into());
        }
        let priority = w
            .get_priority()
            .parse::<i32>()
            .ok()
            .filter(|n| (-100..=100).contains(n))
            .ok_or("优先级应为 -100 至 100")?;
        let max_active = w
            .get_max_active()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=64).contains(n))
            .ok_or("最大活动任务应为 1 至 64")?;
        let speed_limit_kib = w
            .get_speed_limit()
            .parse::<u64>()
            .ok()
            .filter(|n| *n <= 1_048_576)
            .ok_or("限速应为 0 至 1048576 KiB/s")?;
        for time in [w.get_start_time(), w.get_stop_time()] {
            if !crate::settings_editor::valid_clock(&time) {
                return Err("请输入有效的 HH:mm 时间".into());
            }
        }
        let day_flags = parse_active_days(&w.get_active_days())?;
        let profile = &mut self.profiles[self.selected];
        profile.name = name;
        profile.priority = priority;
        profile.max_active = max_active;
        profile.speed_limit_kib = speed_limit_kib;
        profile.enabled = w.get_queue_enabled();
        profile.schedule_enabled = w.get_schedule_enabled();
        profile.start_time = w.get_start_time().into();
        profile.stop_time = w.get_stop_time().into();
        profile.active_days = format_active_days(&day_flags);
        profile.completion_action = match w.get_completion_action().as_str() {
            "关机" => "shutdown",
            "睡眠" => "sleep",
            "休眠" => "hibernate",
            _ => "",
        }
        .into();
        Ok(())
    }

    fn render(&self) {
        let w = &self.window;
        let p = &self.profiles[self.selected];
        w.set_profiles(ModelRc::new(VecModel::from(
            self.profiles
                .iter()
                .map(|p| p.name.clone().into())
                .collect::<Vec<_>>(),
        )));
        w.set_selected(self.selected as i32);
        w.set_queue_name(p.name.clone().into());
        w.set_priority(p.priority.to_string().into());
        w.set_max_active(p.max_active.to_string().into());
        w.set_speed_limit(p.speed_limit_kib.to_string().into());
        w.set_queue_enabled(p.enabled);
        w.set_schedule_enabled(p.schedule_enabled);
        w.set_start_time(p.start_time.clone().into());
        w.set_stop_time(p.stop_time.clone().into());
        let day_flags = parse_active_days(&p.active_days).unwrap_or([true; 7]);
        w.set_active_days(format_active_days(&day_flags).into());
        w.set_active_day_flags(ModelRc::new(VecModel::from(day_flags.to_vec())));
        w.set_completion_action(
            match p.completion_action.as_str() {
                "shutdown" => "关机",
                "sleep" => "睡眠",
                "hibernate" => "休眠",
                _ => "无操作",
            }
            .into(),
        );
        w.set_can_remove(self.profiles.len() > 1 && p.id != "default");
        w.set_status("".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_pill_helpers_roundtrip_wire_format_and_reject_invalid_values() {
        assert_eq!(parse_active_days("1,2,3,4,5,6,7").unwrap(), [true; 7]);
        let weekdays = parse_active_days("5, 1, 3, 1").unwrap();
        assert_eq!(weekdays, [true, false, true, false, true, false, false]);
        assert_eq!(format_active_days(&weekdays), "1,3,5");
        assert!(parse_active_days("").is_err());
        assert!(parse_active_days("0,8").is_err());
        assert!(parse_active_days("mon").is_err());
    }

    #[test]
    fn queue_editor_weekday_pills_sync_wire_format_and_validate_empty_selection() {
        crate::offscreen::OffscreenPlatform::init_test();
        let parent = WorkbenchWindow::new().unwrap();
        let mut editor = QueueEditor::new(
            vec![QueueProfile {
                active_days: "1,3,5".into(),
                ..Default::default()
            }],
            &parent,
            false,
        )
        .unwrap();
        assert_eq!(editor.window.get_active_days().as_str(), "1,3,5");
        assert_eq!(
            editor
                .window
                .get_active_day_flags()
                .iter()
                .collect::<Vec<_>>(),
            vec![true, false, true, false, true, false, false]
        );

        // Toggle Sunday (7) on and Wednesday (3) off.
        editor.toggle_day(7);
        editor.toggle_day(3);
        assert_eq!(editor.window.get_active_days().as_str(), "1,5,7");
        let (profiles, _) = editor.values().unwrap();
        assert_eq!(profiles[0].active_days, "1,5,7");

        // Unchecking all remaining days is rejected with a clear validation error.
        editor.toggle_day(1);
        editor.toggle_day(5);
        editor.toggle_day(7);
        assert_eq!(editor.window.get_active_days().as_str(), "");
        assert_eq!(editor.values().unwrap_err(), "至少选择一天");
    }

    #[test]
    fn queue_picker_helpers_order_by_priority_and_map_names_to_ids() {
        let profiles = ordered_profiles(&[
            QueueProfile {
                id: "night".into(),
                name: "夜间限速".into(),
                priority: 10,
                ..Default::default()
            },
            QueueProfile {
                id: "high".into(),
                name: "高速优先".into(),
                priority: -5,
                ..Default::default()
            },
            QueueProfile::default(),
        ]);
        assert_eq!(
            queue_option_names(&profiles)
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>(),
            vec!["高速优先", "默认队列", "夜间限速"]
        );
        assert_eq!(queue_index_for_id(&profiles, "default"), 1);
        assert_eq!(queue_index_for_id(&profiles, ""), 1);
        assert_eq!(queue_index_for_id(&profiles, "night"), 2);
        assert_eq!(queue_index_for_id(&profiles, "unknown"), 1);
        assert_eq!(queue_id_at(&profiles, 0), "high");
        assert_eq!(queue_id_at(&profiles, 1), "default");
        assert_eq!(queue_id_at(&profiles, 99), "default");
    }

    #[test]
    fn receive_saved_profiles_preserves_selected_queue_by_id_and_clamps_removed_selection() {
        crate::offscreen::OffscreenPlatform::init_test();
        let parent = WorkbenchWindow::new().unwrap();
        let mut editor = QueueEditor::new(
            vec![
                QueueProfile::default(),
                QueueProfile {
                    id: "vip".into(),
                    name: "高速优先".into(),
                    priority: -5,
                    ..Default::default()
                },
            ],
            &parent,
            false,
        )
        .unwrap();
        editor.select(1).unwrap();
        assert_eq!(editor.window.get_queue_name().as_str(), "高速优先");

        // Reordered / canonicalized profiles from Core preserve selection by queue ID.
        editor.receive_saved_profiles(vec![
            QueueProfile {
                id: "vip".into(),
                name: "极速通道".into(),
                priority: -10,
                max_active: 8,
                ..Default::default()
            },
            QueueProfile::default(),
        ]);
        assert_eq!(editor.window.get_selected(), 0);
        assert_eq!(editor.window.get_queue_name().as_str(), "极速通道");
        assert_eq!(editor.window.get_max_active().as_str(), "8");

        // Select index 1, then receive a single-profile list: selection clamps cleanly to 0.
        editor.select(1).unwrap();
        editor.receive_saved_profiles(Vec::new());
        assert_eq!(editor.window.get_selected(), 0);
        assert_eq!(editor.window.get_queue_name().as_str(), "默认队列");
    }
}
