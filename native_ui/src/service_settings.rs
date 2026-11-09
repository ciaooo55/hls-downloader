use crate::{native_window, ServiceSettingsWindow};
use hls_client::{CorePipeRequest, CorePipeResponse};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

pub struct ServiceSettings {
    pub window: ServiceSettingsWindow,
    pending: Cell<Option<u64>>,
    initial_provider: RefCell<String>,
    has_key: Cell<bool>,
    request: Rc<dyn Fn(CorePipeRequest) -> bool>,
    id: Rc<dyn Fn() -> u64>,
}

impl ServiceSettings {
    pub fn new(
        mode: &str,
        dark: bool,
        request: Rc<dyn Fn(CorePipeRequest) -> bool>,
        id: Rc<dyn Fn() -> u64>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = ServiceSettingsWindow::new()?;
        crate::native_window::apply_theme(&window, dark);
        window.set_mode(mode.into());
        window.set_loaded(mode != "subtitle");
        let this = Rc::new(Self {
            window,
            pending: Cell::new(None),
            initial_provider: RefCell::new(String::new()),
            has_key: Cell::new(false),
            request,
            id,
        });
        let weak = Rc::downgrade(&this);
        this.window.on_action(move |action| {
            if let Some(this) = weak.upgrade() {
                if let Err(error) = this.action(&action) {
                    this.window.set_status(error.into());
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.clear_secrets();
                let _ = this.window.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        if mode == "subtitle" {
            let _ = this.action("reload");
        }
        Ok(this)
    }

    pub fn show(&self, owner: Option<i64>) {
        let _ = self.window.show();
        native_window::prepare_dialog(&self.window, true, true, owner);
    }

    fn clear_secrets(&self) {
        self.window.set_api_key("".into());
        self.window.set_cookie("".into());
        self.window.set_authorization("".into());
        self.window.set_identity_headers("".into());
    }

    fn action(&self, action: &str) -> Result<(), String> {
        if action == "close" {
            self.clear_secrets();
            let _ = self.window.hide();
            return Ok(());
        }
        if self.window.get_busy() {
            return Ok(());
        }
        let id = (self.id)();
        let request = if action == "reload" {
            CorePipeRequest::Subtitle {
                request_id: id,
                request: serde_json::json!({"action":"configuration"}),
            }
        } else if action == "save" && self.window.get_mode() == "subtitle" {
            let provider = self.window.get_provider();
            let key = self.window.get_api_key();
            if self.has_key.get()
                && provider.as_str() != self.initial_provider.borrow().as_str()
                && key.trim().is_empty()
                && !self.window.get_clear_key()
            {
                return Err("切换服务时，请输入对应服务的 API Key 或清除旧密钥".into());
            }
            let offset = self
                .window
                .get_offset()
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && n.abs() <= 30.0)
                .ok_or("字幕偏移必须在 -30 至 30 秒之间")?;
            CorePipeRequest::Subtitle {
                request_id: id,
                request: serde_json::json!({"action":"configure","provider":provider.as_str(),"workspace":self.window.get_workspace().trim(),"region":self.window.get_region().as_str(),"api_key":key.as_str(),"clear_key":self.window.get_clear_key(),"preferences":{"enabled":self.window.get_translation_enabled(),"source":self.window.get_source().as_str(),"target":self.window.get_target().as_str(),"offset":offset,"showSource":self.window.get_show_source(),"prefetch":self.window.get_prefetch()}}),
            }
        } else if action == "save" && self.window.get_default_cookie() {
            if !self.window.get_clear_credential() && self.window.get_cookie().trim().is_empty() {
                return Err("请输入 Cookie，或选择清除凭据".into());
            }
            CorePipeRequest::SetDefaultCookie {
                request_id: id,
                cookie: if self.window.get_clear_credential() {
                    String::new()
                } else {
                    self.window.get_cookie().to_string()
                },
            }
        } else if action == "save" {
            if self.window.get_host().trim().is_empty() {
                return Err("请输入站点域名".into());
            }
            if !self.window.get_clear_credential()
                && self.window.get_cookie().trim().is_empty()
                && self.window.get_authorization().trim().is_empty()
                && self.window.get_identity_headers().trim().is_empty()
            {
                return Err("请输入 Cookie、Authorization 或其他身份请求头，或选择清除凭据".into());
            }
            let mut headers: BTreeMap<String, String> =
                if self.window.get_identity_headers().trim().is_empty()
                    || self.window.get_clear_credential()
                {
                    BTreeMap::new()
                } else {
                    serde_json::from_str(self.window.get_identity_headers().as_str())
                        .map_err(|_| "其他身份请求头必须是名称和值均为字符串的 JSON 对象")?
                };
            if headers.len() > 64
                || headers.iter().any(|(name, value)| {
                    name.is_empty()
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                        || value.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
                })
            {
                return Err("身份请求头过多或含有无效名称、换行符".into());
            }
            if headers.keys().any(|name| {
                name.eq_ignore_ascii_case("cookie") || name.eq_ignore_ascii_case("authorization")
            }) {
                return Err("Cookie 和 Authorization 请填写在对应的输入框中".into());
            }
            if !self.window.get_authorization().trim().is_empty() {
                headers.insert(
                    "Authorization".into(),
                    self.window.get_authorization().trim().to_owned(),
                );
            }
            CorePipeRequest::SetSiteRuleCredential {
                request_id: id,
                host: self.window.get_host().trim().to_owned(),
                cookie: self.window.get_cookie().to_string(),
                request_headers: headers,
                clear: self.window.get_clear_credential(),
            }
        } else {
            return Ok(());
        };
        if !(self.request)(request) {
            return Err("命令队列已满，请稍后重试".into());
        }
        self.pending.set(Some(id));
        self.window.set_busy(true);
        self.window.set_status("".into());
        self.clear_secrets();
        Ok(())
    }

    pub fn response(&self, response: &CorePipeResponse) {
        let id = match response {
            CorePipeResponse::Subtitle { request_id, .. }
            | CorePipeResponse::Settings { request_id, .. }
            | CorePipeResponse::Events { request_id, .. } => Some(*request_id),
            CorePipeResponse::Error { request_id, .. } => *request_id,
            _ => None,
        };
        if id.is_none() || id != self.pending.get() {
            return;
        }
        self.pending.set(None);
        self.window.set_busy(false);
        match response {
            CorePipeResponse::Subtitle { result, .. } => {
                if let Err(message) = subtitle_result(result) {
                    self.window.set_status(message.into());
                    return;
                }
                self.window.set_loaded(true);
                let provider = result["provider"].as_str().unwrap_or("dashscope");
                *self.initial_provider.borrow_mut() = provider.to_owned();
                self.has_key
                    .set(result["has_key"].as_bool().unwrap_or(false));
                self.window.set_provider(provider.into());
                self.window
                    .set_workspace(result["workspace"].as_str().unwrap_or("").into());
                self.window
                    .set_region(result["region"].as_str().unwrap_or("cn-beijing").into());
                self.window.set_key_status(
                    if self.has_key.get() {
                        "已保存 API Key"
                    } else {
                        "尚未保存 API Key"
                    }
                    .into(),
                );
                self.window.set_clear_key(false);
                let p = &result["preferences"];
                self.window
                    .set_source(p["source"].as_str().unwrap_or("auto").into());
                self.window
                    .set_target(p["target"].as_str().unwrap_or("zh").into());
                self.window
                    .set_offset(p["offset"].as_f64().unwrap_or(0.0).to_string().into());
                self.window
                    .set_translation_enabled(p["enabled"].as_bool().unwrap_or(false));
                self.window
                    .set_show_source(p["showSource"].as_bool().unwrap_or(true));
                self.window
                    .set_prefetch(p["prefetch"].as_bool().unwrap_or(false));
                self.window.set_status("配置已读取，密钥不会回显".into());
            }
            CorePipeResponse::Error { message, .. } => {
                self.window.set_status(message.clone().into())
            }
            CorePipeResponse::Events { events, .. } => self.window.set_status(
                events
                    .iter()
                    .find_map(|e| {
                        if let hls_client::CoreEvent::Error { message, .. } = &e.event {
                            Some(message.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "凭据已保存".into())
                    .into(),
            ),
            _ => self.window.set_status("凭据已保存".into()),
        }
    }
}

fn subtitle_result(result: &serde_json::Value) -> Result<(), String> {
    if result["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(result["error"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("字幕配置未保存，请重试")
            .to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_subtitle_configuration_preserves_the_core_error() {
        let error =
            subtitle_result(&serde_json::json!({"ok":false,"error":"请填写有效的百炼业务空间 ID"}))
                .unwrap_err();
        assert_eq!(error, "请填写有效的百炼业务空间 ID");
        assert!(subtitle_result(&serde_json::json!({})).is_err());
        assert!(subtitle_result(&serde_json::json!({"ok":true,"has_key":true})).is_ok());
    }
}
