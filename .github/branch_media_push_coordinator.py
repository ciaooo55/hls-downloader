from pathlib import Path

path = Path("native_shell/src/download_worker.rs")
text = path.read_text(encoding="utf-8")

old_request = '''    fn request_media_push(&self, request: MediaPushRequest) -> Result<Vec<EventEnvelope>, String> {
        if request.id.trim().is_empty() || request.id.len() > 160 {
            return Err("媒体推送请求编号无效".into());
        }
        if !matches!(request.push_kind.as_str(), "cast" | "tvbox") {
            return Err("媒体推送类型无效".into());
        }
        let lower = request.url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://"))
            || request.url.chars().any(char::is_control)
        {
            return Err("媒体推送地址无效".into());
        }
        let json = serde_json::to_string(&request)
            .map_err(|error| format!("encode media push {}: {error}", request.id))?;
        self.save_handoff(
            &request.id,
            &json,
            &request.status,
            None,
            request.created_at_ms,
        )?;
        self.lock()?.emit(CoreEvent::MediaPushRequested { request })
    }
'''
new_request = '''    fn request_media_push(&self, request: MediaPushRequest) -> Result<Vec<EventEnvelope>, String> {
        if request.id.trim().is_empty() || request.id.len() > 160 {
            return Err("媒体推送请求编号无效".into());
        }
        if !matches!(request.push_kind.as_str(), "cast" | "tvbox") {
            return Err("媒体推送类型无效".into());
        }
        if request.status != "pending" {
            return Err("媒体推送请求状态无效".into());
        }
        let lower = request.url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://"))
            || request.url.chars().any(char::is_control)
        {
            return Err("媒体推送地址无效".into());
        }
        self.lock()?.handle(CoreCommand::RequestMediaPush { request })
    }
'''
if text.count(old_request) != 1:
    raise SystemExit(f"request_media_push anchor count={text.count(old_request)}")
text = text.replace(old_request, new_request, 1)

old_resolve = '''    fn resolve_media_push(
        &self,
        request_id: &str,
        status: &str,
        message: &str,
        location: &str,
    ) -> Result<Vec<EventEnvelope>, String> {
        if !matches!(status, "done" | "failed" | "canceled") {
            return Err("媒体推送结果状态无效".into());
        }
        let mut request = self
            .load_handoffs()?
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == request_id)
            .ok_or_else(|| "媒体推送请求不存在或已过期".to_string())?;
        request.status = status.to_string();
        request.message = message.trim().chars().take(300).collect();
        request.location = location.trim().chars().take(2048).collect();
        let json = serde_json::to_string(&request)
            .map_err(|error| format!("encode media push {}: {error}", request.id))?;
        self.save_handoff(
            &request.id,
            &json,
            &request.status,
            None,
            request.created_at_ms,
        )?;
        self.lock()?.emit(CoreEvent::MediaPushResolved { request })
    }
'''
new_resolve = '''    fn resolve_media_push(
        &self,
        request_id: &str,
        status: &str,
        message: &str,
        location: &str,
    ) -> Result<Vec<EventEnvelope>, String> {
        if !matches!(status, "done" | "failed" | "canceled") {
            return Err("媒体推送结果状态无效".into());
        }
        self.lock()?.handle(CoreCommand::ResolveMediaPush {
            request_id: request_id.to_string(),
            status: status.to_string(),
            message: message.trim().chars().take(300).collect(),
            location: location.trim().chars().take(2048).collect(),
        })
    }
'''
if text.count(old_resolve) != 1:
    raise SystemExit(f"resolve_media_push anchor count={text.count(old_resolve)}")
text = text.replace(old_resolve, new_resolve, 1)

marker = '''    #[test]
    fn local_url_shortcut_expands_to_http_task() {'''
addition = '''    #[test]
    fn browser_media_push_coordinator_enforces_single_pending_and_first_terminal_wins() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let first = MediaPushRequest {
            id: "media-push-first".into(),
            push_kind: "cast".into(),
            url: "https://cdn.test/first.mp4".into(),
            title: "First".into(),
            status: "pending".into(),
            message: "waiting".into(),
            location: String::new(),
            created_at_ms: 100,
        };
        let second = MediaPushRequest {
            id: "media-push-second".into(),
            push_kind: "cast".into(),
            url: "https://cdn.test/second.mp4".into(),
            title: "Second".into(),
            status: "pending".into(),
            message: "waiting".into(),
            location: String::new(),
            created_at_ms: 101,
        };
        coordinator
            .dispatch(CoreCommand::RequestMediaPush {
                request: first.clone(),
            })
            .unwrap();
        let overlap = coordinator
            .dispatch(CoreCommand::RequestMediaPush {
                request: second.clone(),
            })
            .unwrap();
        assert!(overlap.iter().any(|envelope| matches!(
            &envelope.event,
            CoreEvent::MediaPushResolved { request }
                if request.id == second.id && request.status == "failed"
        )));

        coordinator
            .dispatch(CoreCommand::ResolveMediaPush {
                request_id: first.id.clone(),
                status: "done".into(),
                message: "sent".into(),
                location: "http://192.168.1.8/media/first".into(),
            })
            .unwrap();
        assert!(coordinator
            .dispatch(CoreCommand::ResolveMediaPush {
                request_id: first.id.clone(),
                status: "failed".into(),
                message: "late failure".into(),
                location: String::new(),
            })
            .unwrap()
            .is_empty());

        let rows = coordinator
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
            .collect::<Vec<_>>();
        let persisted_first = rows.iter().find(|item| item.id == first.id).unwrap();
        let persisted_second = rows.iter().find(|item| item.id == second.id).unwrap();
        assert_eq!(persisted_first.status, "done");
        assert_eq!(persisted_first.message, "sent");
        assert_eq!(persisted_first.location, "http://192.168.1.8/media/first");
        assert_eq!(persisted_second.status, "failed");
    }

    #[test]
    fn browser_media_push_coordinator_rejects_nonpending_request_state() {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let request = MediaPushRequest {
            id: "media-push-invalid-state".into(),
            push_kind: "cast".into(),
            url: "https://cdn.test/video.mp4".into(),
            title: "Video".into(),
            status: "done".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 102,
        };
        assert!(coordinator
            .dispatch(CoreCommand::RequestMediaPush { request })
            .is_err());
    }

'''
if text.count(marker) != 1:
    raise SystemExit(f"test insertion anchor count={text.count(marker)}")
text = text.replace(marker, addition + marker, 1)

path.write_text(text, encoding="utf-8")
