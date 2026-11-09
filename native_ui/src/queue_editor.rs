use crate::QueueWindow;
use hls_client::QueueProfile;
use slint::{ComponentHandle, ModelRc, VecModel};

pub struct QueueEditor {
    pub window: QueueWindow,
    profiles: Vec<QueueProfile>,
    selected: usize,
}

impl QueueEditor {
    pub fn new(
        mut profiles: Vec<QueueProfile>,
        dark: bool,
        assign: bool,
    ) -> Result<Self, slint::PlatformError> {
        if profiles.is_empty() {
            profiles.push(QueueProfile::default());
        }
        let window = QueueWindow::new()?;
        crate::native_window::apply_theme(&window, dark);
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
                let number = (1..)
                    .find(|n| {
                        !self
                            .profiles
                            .iter()
                            .any(|profile| profile.name == format!("新队列 {n}"))
                    })
                    .unwrap();
                self.profiles.push(QueueProfile {
                    id: format!("queue-{nonce:x}"),
                    name: format!("新队列 {number}"),
                    ..Default::default()
                });
                self.selected = self.profiles.len() - 1;
            }
            "remove" if self.profiles[self.selected].id != "default" => {
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
            let Some((hour, minute)) = time.split_once(':') else {
                return Err("时间格式应为 HH:mm".into());
            };
            if time.len() != 5
                || !hour.parse::<u8>().is_ok_and(|n| n < 24)
                || !minute.parse::<u8>().is_ok_and(|n| n < 60)
            {
                return Err("请输入有效的 HH:mm 时间".into());
            }
        }
        let days = w
            .get_active_days()
            .split(',')
            .map(|s| s.trim().parse::<u8>().ok().filter(|n| (1..=7).contains(n)))
            .collect::<Option<std::collections::BTreeSet<_>>>()
            .ok_or("星期应为 1 至 7，以逗号分隔")?;
        if days.is_empty() {
            return Err("至少选择一天".into());
        }
        let profile = &mut self.profiles[self.selected];
        profile.name = name;
        profile.priority = priority;
        profile.max_active = max_active;
        profile.speed_limit_kib = speed_limit_kib;
        profile.enabled = w.get_queue_enabled();
        profile.schedule_enabled = w.get_schedule_enabled();
        profile.start_time = w.get_start_time().into();
        profile.stop_time = w.get_stop_time().into();
        profile.active_days = days.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
        profile.completion_action = match w.get_completion_action().as_str() {
            "关机" => "shutdown",
            "睡眠" => "sleep",
            "休眠" => "hibernate",
            "退出程序" => "exit",
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
        w.set_active_days(p.active_days.clone().into());
        w.set_completion_action(
            match p.completion_action.as_str() {
                "shutdown" => "关机",
                "sleep" => "睡眠",
                "hibernate" => "休眠",
                "exit" => "退出程序",
                _ => "无操作",
            }
            .into(),
        );
        w.set_can_remove(p.id != "default");
        w.set_status("".into());
    }
}
