//! Length-prefixed JSON codec matching `backend.app.native_shell`.

use serde_json::{Map, Value};

pub const PROTOCOL_NAME: &str = "hls-downloader-native-shell";
pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const PAINT_KEYS: [&str; 8] = [
    "id",
    "url",
    "filename",
    "title",
    "mime_type",
    "size",
    "resource_kind",
    "status",
];

pub fn paint_snapshot(handoff: &Value) -> Value {
    let source = handoff.as_object().cloned().unwrap_or_default();
    let mut snapshot = Map::new();
    snapshot.insert("id".into(), json_string(&source, "id"));
    snapshot.insert("url".into(), json_string(&source, "url"));
    snapshot.insert("filename".into(), json_string(&source, "filename"));
    snapshot.insert("title".into(), json_string(&source, "title"));
    snapshot.insert("mime_type".into(), json_string(&source, "mime_type"));
    snapshot.insert(
        "resource_kind".into(),
        json_string_or(&source, "resource_kind", "file"),
    );
    snapshot.insert(
        "status".into(),
        json_string_or(&source, "status", "pending"),
    );
    let size = source
        .get("size")
        .and_then(Value::as_i64)
        .or_else(|| {
            source
                .get("size")
                .and_then(Value::as_u64)
                .and_then(|value| i64::try_from(value).ok())
        })
        .or_else(|| {
            source
                .get("size")
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0)
        .max(0);
    snapshot.insert("size".into(), Value::from(size));
    Value::Object(snapshot)
}

pub fn encode_frame(message: &Value) -> Result<Vec<u8>, String> {
    let payload = serde_json::to_vec(message).map_err(|err| err.to_string())?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err("native shell frame too large".into());
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_frame(buffer: &[u8]) -> Result<Value, String> {
    if buffer.len() < 4 {
        return Err("native shell frame truncated".into());
    }
    let length = u32::from_le_bytes(buffer[0..4].try_into().unwrap()) as usize;
    if length > MAX_FRAME_BYTES {
        return Err("native shell frame too large".into());
    }
    if buffer.len() < 4 + length {
        return Err("native shell frame truncated".into());
    }
    let message: Value =
        serde_json::from_slice(&buffer[4..4 + length]).map_err(|err| err.to_string())?;
    if !message.is_object() {
        return Err("native shell frame is not an object".into());
    }
    Ok(message)
}

fn json_string(source: &Map<String, Value>, key: &str) -> Value {
    Value::String(
        source
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    )
}

fn json_string_or(source: &Map<String, Value>, key: &str, fallback: &str) -> Value {
    let value = source.get(key).and_then(Value::as_str).unwrap_or("");
    Value::String(if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_drops_cookie_and_unknown_size() {
        let painted = paint_snapshot(&json!({
            "id": "x",
            "filename": "doc.pdf",
            "size": "nope",
            "cookie": "secret"
        }));
        assert_eq!(painted["filename"], "doc.pdf");
        assert_eq!(painted["size"], 0);
        assert!(painted.get("cookie").is_none());
    }

    #[test]
    fn frame_roundtrip_keeps_presentable_handoff() {
        let event = json!({
            "protocol": PROTOCOL_NAME,
            "version": PROTOCOL_VERSION,
            "kind": "handoff",
            "presentable": true,
            "snapshot": paint_snapshot(&json!({"id": "h1", "filename": "setup.exe", "url": "https://cdn.test/setup.exe"}))
        });
        let restored = decode_frame(&encode_frame(&event).unwrap()).unwrap();
        assert_eq!(restored["snapshot"]["filename"], "setup.exe");
        assert_eq!(restored["presentable"], true);
    }
}
