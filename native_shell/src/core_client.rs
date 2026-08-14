//! Loopback HTTP client for the Python download core.
//!
//! The supervisor never GETs `/browser/handoffs/{id}` to paint a confirmation
//! window. Filename/url/size come from the event snapshot.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct CoreClient {
    pub host: String,
    pub port: u16,
    pub base_path: String,
    pub token: String,
}

impl CoreClient {
    pub fn parse(url: &str, token: &str) -> Result<Self, String> {
        let url = url.trim().trim_end_matches('/');
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| "core url must be http://127.0.0.1".to_string())?;
        let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = if let Some((host, port)) = hostport.split_once(':') {
            (
                host.to_string(),
                port.parse::<u16>().map_err(|_| "invalid core port")?,
            )
        } else {
            (hostport.to_string(), 80)
        };
        if host != "127.0.0.1" && host != "localhost" {
            return Err("core url must be loopback".into());
        }
        Ok(Self {
            host,
            port,
            base_path: if path.is_empty() {
                String::new()
            } else {
                format!("/{path}")
            },
            token: token.to_string(),
        })
    }

    pub fn boot(&self) -> Result<Value, String> {
        self.request("POST", "/desktop/native-shell/boot", None, 4.0)
    }

    pub fn wait_events(&self, after: u64, timeout: f64) -> Result<Value, String> {
        let path = format!("/desktop/native-shell/events?after={after}&timeout={timeout}");
        self.request("GET", &path, None, timeout + 2.0)
    }

    pub fn accept(&self, handoff_id: &str) -> Result<Value, String> {
        self.request(
            "POST",
            &format!("/browser/handoffs/{handoff_id}/accept"),
            Some(json!({})),
            8.0,
        )
    }

    pub fn reject(&self, handoff_id: &str) -> Result<Value, String> {
        self.request(
            "POST",
            &format!("/browser/handoffs/{handoff_id}/reject"),
            None,
            8.0,
        )
    }

    pub fn health(&self) -> Result<Value, String> {
        self.request("GET", "/health", None, 2.0)
    }

    pub fn pause_task(&self, task_id: &str) -> Result<Value, String> {
        self.request("POST", &format!("/tasks/{task_id}/pause"), None, 8.0)
    }

    pub fn open_explorer(&self, task_id: &str) -> Result<Value, String> {
        self.request(
            "POST",
            "/open-explorer",
            Some(json!({"task_id": task_id})),
            8.0,
        )
    }

    pub fn launch_file(&self, task_id: &str, confirm_executable: bool) -> Result<Value, String> {
        self.request(
            "POST",
            "/launch-file",
            Some(json!({
                "task_id": task_id,
                "confirm_executable": confirm_executable
            })),
            8.0,
        )
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeout_secs: f64,
    ) -> Result<Value, String> {
        if path.contains("/browser/handoffs/") && method == "GET" && !path.contains("/reject") {
            return Err("native shell must not GET a handoff to paint the first frame".into());
        }
        let payload = body
            .as_ref()
            .map(|value| serde_json::to_vec(value).map_err(|err| err.to_string()))
            .transpose()?;
        let full_path = format!("{}{}", self.base_path, path);
        let mut header = format!(
            "{method} {full_path} HTTP/1.1\r\nHost: {host}:{port}\r\nX-Token: {token}\r\nConnection: close\r\n",
            host = self.host,
            port = self.port,
            token = self.token,
        );
        if let Some(bytes) = &payload {
            header.push_str("Content-Type: application/json\r\n");
            header.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
        } else if method != "GET" {
            header.push_str("Content-Length: 0\r\n");
        }
        header.push_str("\r\n");
        let mut stream = TcpStream::connect((self.host.as_str(), self.port))
            .map_err(|err| format!("core connect: {err}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs_f64(timeout_secs.max(1.0))))
            .map_err(|err| err.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(8)))
            .map_err(|err| err.to_string())?;
        stream
            .write_all(header.as_bytes())
            .map_err(|err| err.to_string())?;
        if let Some(bytes) = payload {
            stream.write_all(&bytes).map_err(|err| err.to_string())?;
        }
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|err| format!("core read: {err}"))?;
        let text = String::from_utf8_lossy(&raw);
        let (head, body_text) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(0);
        if status == 0 {
            return Err("core returned an empty HTTP response".into());
        }
        if !(200..300).contains(&status) {
            return Err(format!("core HTTP {status}: {body_text}"));
        }
        if body_text.trim().is_empty() {
            return Ok(json!({"ok": true}));
        }
        serde_json::from_str(body_text).map_err(|err| format!("core json: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_core() {
        let err = CoreClient::parse("http://example.test/api", "token").unwrap_err();
        assert!(err.contains("loopback"));
    }

    #[test]
    fn parses_loopback_api_root() {
        let client = CoreClient::parse("http://127.0.0.1:8765/api", "secret").unwrap();
        assert_eq!(client.port, 8765);
        assert_eq!(client.base_path, "/api");
    }

    #[test]
    fn refuses_handoff_get_on_the_paint_path() {
        let client = CoreClient::parse("http://127.0.0.1:8765/api", "x").unwrap();
        let err = client
            .request("GET", "/browser/handoffs/abc", None, 1.0)
            .unwrap_err();
        assert!(err.contains("must not GET"));
    }
}
