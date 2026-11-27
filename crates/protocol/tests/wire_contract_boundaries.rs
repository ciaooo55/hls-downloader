//! Boundary regressions for the v7 Named Pipe wire contract.
use hls_protocol::*;
use serde_json::{json, Value};

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn frame_budget_constants_stay_consistent() {
    assert_eq!(V7_PIPE_MAX_FRAME, 4 * 1024 * 1024);
    assert_eq!(V7_PAGE_PAYLOAD_BUDGET + 64 * 1024, V7_PIPE_MAX_FRAME);
    assert!(u32::try_from(V7_PIPE_MAX_FRAME).is_ok());
    assert_eq!(V7_PIPE_NAME, r"\\.\pipe\HLSDownloader.v7");
    assert_eq!(V7_PROTOCOL_NAME, "hls-downloader-v7-core");
    assert_eq!(V7_PROTOCOL_VERSION, 1);
}

#[test]
fn encoded_frames_use_a_little_endian_length_prefix() {
    let encoded = encode_message(&json!({"a": 1})).unwrap();
    let payload = br#"{"a":1}"#;
    assert_eq!(&encoded[..4], &(payload.len() as u32).to_le_bytes());
    assert_eq!(&encoded[4..], payload);
    assert_eq!(encoded, frame(payload));
}

#[test]
fn decoding_rejects_truncated_headers() {
    for len in 0..4 {
        let error = decode_message::<Value>(&vec![0u8; len]).unwrap_err();
        assert_eq!(error, "Core pipe frame truncated");
    }
}

#[test]
fn decoding_rejects_length_mismatches_in_both_directions() {
    let good = frame(b"\"x\"");
    assert_eq!(decode_message::<String>(&good).unwrap(), "x");

    let mut short = good.clone();
    short.pop();
    assert_eq!(
        decode_message::<String>(&short).unwrap_err(),
        "Core pipe frame invalid length"
    );

    let mut long = good;
    long.push(b' ');
    assert_eq!(
        decode_message::<String>(&long).unwrap_err(),
        "Core pipe frame invalid length"
    );
}

#[test]
fn decoding_rejects_declared_lengths_above_the_frame_limit() {
    for declared in [V7_PIPE_MAX_FRAME as u32 + 1, u32::MAX] {
        let mut bytes = declared.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"{}");
        assert_eq!(
            decode_message::<Value>(&bytes).unwrap_err(),
            "Core pipe frame invalid length"
        );
    }
}

#[test]
fn decoding_accepts_a_payload_of_exactly_the_frame_limit() {
    let text = "x".repeat(V7_PIPE_MAX_FRAME - 2);
    let encoded = encode_message(&text).unwrap();
    assert_eq!(encoded.len(), V7_PIPE_MAX_FRAME + 4);
    assert_eq!(decode_message::<String>(&encoded).unwrap(), text);
}

#[test]
fn decoding_reports_invalid_payloads_without_echoing_content() {
    let empty = decode_message::<Value>(&frame(b"")).unwrap_err();
    assert!(
        empty.starts_with("Core pipe JSON invalid at line"),
        "{empty}"
    );

    for payload in [
        &b"{\"type\":\"hello\",\"protocol\":\"secret-token\""[..],
        &b"\xff\xfe secret-token"[..],
        &b"{\"type\":\"store_credential\",\"request_id\":1,\"credential_ref\":\"secret-token\"}"[..],
    ] {
        let error = decode_message::<CorePipeRequest>(&frame(payload)).unwrap_err();
        assert!(
            error.starts_with("Core pipe JSON invalid at line"),
            "{error}"
        );
        assert!(!error.contains("secret-token"), "{error}");
    }

    let error = decode_message::<CorePipeResponse>(&frame(
        br#"{"type":"credential","request_id":"secret-token"}"#,
    ))
    .unwrap_err();
    assert!(!error.contains("secret-token"), "{error}");
}

fn sample_requests() -> Vec<(CorePipeRequest, &'static str, &'static str)> {
    let secret = || "secret-token".to_string();
    vec![
        (
            CorePipeRequest::CastControl {
                request_id: 1,
                session_generation: 2,
                action: secret(),
            },
            "cast_control",
            "CastControl",
        ),
        (
            CorePipeRequest::PlayerControl {
                request_id: 1,
                session_generation: 2,
                action: secret(),
            },
            "player_control",
            "PlayerControl",
        ),
        (
            CorePipeRequest::PlayerEmbed {
                request_id: 1,
                embed: PlayerEmbed {
                    session_generation: 1,
                    layout_generation: 2,
                    parent_hwnd: Some(-1),
                    x: i32::MIN,
                    y: i32::MAX,
                    width: 0,
                    height: 0,
                },
            },
            "player_embed",
            "PlayerEmbed",
        ),
        (
            CorePipeRequest::Crawl {
                request_id: 1,
                request: CrawlRequest::Get {
                    project_id: secret(),
                },
            },
            "crawl",
            "Crawl",
        ),
        (
            CorePipeRequest::PreviewZip {
                request_id: 1,
                source: secret(),
            },
            "preview_zip",
            "PreviewZip",
        ),
        (
            CorePipeRequest::Hello {
                protocol: secret(),
                version: V7_PROTOCOL_VERSION,
            },
            "hello",
            "Hello",
        ),
        (
            CorePipeRequest::Command {
                request_id: 1,
                command: CoreCommand::GetTask { task_id: secret() },
            },
            "command",
            "Command",
        ),
        (
            CorePipeRequest::Snapshot { request_id: 1 },
            "snapshot",
            "Snapshot",
        ),
        (
            CorePipeRequest::ListTasks {
                request_id: 1,
                after_task_id: Some(secret()),
                limit: 0,
            },
            "list_tasks",
            "ListTasks",
        ),
        (
            CorePipeRequest::WaitEventBatch {
                request_id: 1,
                after_sequence: u64::MAX,
                timeout_ms: 0,
            },
            "wait_event_batch",
            "WaitEventBatch",
        ),
        (
            CorePipeRequest::Capabilities { request_id: 1 },
            "capabilities",
            "Capabilities",
        ),
        (
            CorePipeRequest::WaitEvents {
                request_id: 1,
                after_sequence: 0,
                timeout_ms: u64::MAX,
            },
            "wait_events",
            "WaitEvents",
        ),
        (
            CorePipeRequest::StoreSetting {
                request_id: 1,
                key: "proxy_url".into(),
                value: json!(secret()),
            },
            "store_setting",
            "StoreSetting",
        ),
        (
            CorePipeRequest::StoreSettings {
                request_id: 1,
                values: [("proxy_url".to_string(), json!(secret()))].into(),
            },
            "store_settings",
            "StoreSettings",
        ),
        (
            CorePipeRequest::LoadSettings { request_id: 1 },
            "load_settings",
            "LoadSettings",
        ),
        (
            CorePipeRequest::SetDefaultCookie {
                request_id: 1,
                cookie: secret(),
            },
            "set_default_cookie",
            "SetDefaultCookie",
        ),
        (
            CorePipeRequest::SetSiteRuleCredential {
                request_id: 1,
                host: "cdn.test".into(),
                cookie: secret(),
                request_headers: [("Authorization".to_string(), secret())].into(),
                clear: false,
            },
            "set_site_rule_credential",
            "SetSiteRuleCredential",
        ),
        (
            CorePipeRequest::StoreCredential {
                request_id: 1,
                credential_ref: "ref".into(),
                protected_blob: secret(),
                kind: "cookie".into(),
            },
            "store_credential",
            "StoreCredential",
        ),
        (
            CorePipeRequest::LoadCredential {
                request_id: 1,
                credential_ref: secret(),
            },
            "load_credential",
            "LoadCredential",
        ),
        (
            CorePipeRequest::DeleteCredential {
                request_id: 1,
                credential_ref: secret(),
            },
            "delete_credential",
            "DeleteCredential",
        ),
        (
            CorePipeRequest::AdmitHandoff {
                request_id: 1,
                handoff_json: secret(),
            },
            "admit_handoff",
            "AdmitHandoff",
        ),
        (
            CorePipeRequest::SaveHandoff {
                request_id: 1,
                handoff_id: "h".into(),
                handoff_json: secret(),
                status: "pending".into(),
                task_id: None,
                created_at_ms: 0,
            },
            "save_handoff",
            "SaveHandoff",
        ),
        (
            CorePipeRequest::LoadHandoffs {
                request_id: 1,
                handoff_id: Some(secret()),
                client_request_id: None,
                pending_only: true,
                paged: true,
                after: Some([i64::MIN, i64::MAX]),
            },
            "load_handoffs",
            "LoadHandoffs",
        ),
        (
            CorePipeRequest::Subtitle {
                request_id: 1,
                request: json!({"api_key": secret()}),
            },
            "subtitle",
            "Subtitle",
        ),
        (
            CorePipeRequest::PlayerThumbnail {
                request_id: 1,
                task_id: secret(),
                seconds: Some(-1.5),
            },
            "player_thumbnail",
            "PlayerThumbnail",
        ),
    ]
}

#[test]
fn every_request_variant_keeps_its_wire_tag_and_roundtrips_through_a_frame() {
    let samples = sample_requests();
    assert_eq!(samples.len(), 25);
    for (request, tag, _) in samples {
        assert_eq!(to_json(&request)["type"], tag);
        let decoded: CorePipeRequest = decode_message(&encode_message(&request).unwrap()).unwrap();
        assert_eq!(decoded, request, "{tag}");
        let expected_id = (tag != "hello").then_some(1);
        assert_eq!(decoded.request_id(), expected_id, "{tag}");
    }
}

#[test]
fn request_debug_output_is_the_variant_name_only() {
    for (request, _, name) in sample_requests() {
        let debug = format!("{request:?}");
        assert_eq!(debug, name);
        assert!(!debug.contains("secret-token"));
    }
}

#[test]
fn request_defaults_keep_older_clients_compatible() {
    let list: CorePipeRequest =
        serde_json::from_value(json!({"type": "list_tasks", "request_id": 3})).unwrap();
    assert_eq!(
        list,
        CorePipeRequest::ListTasks {
            request_id: 3,
            after_task_id: None,
            limit: 128,
        }
    );

    let handoffs: CorePipeRequest =
        serde_json::from_value(json!({"type": "load_handoffs", "request_id": 4})).unwrap();
    assert_eq!(
        handoffs,
        CorePipeRequest::LoadHandoffs {
            request_id: 4,
            handoff_id: None,
            client_request_id: None,
            pending_only: false,
            paged: false,
            after: None,
        }
    );
    let wire = to_json(&handoffs);
    assert!(wire.get("handoff_id").is_none());
    assert!(wire.get("client_request_id").is_none());

    let site: CorePipeRequest = serde_json::from_value(json!({
        "type": "set_site_rule_credential",
        "request_id": 5,
        "host": "cdn.test"
    }))
    .unwrap();
    assert_eq!(
        site,
        CorePipeRequest::SetSiteRuleCredential {
            request_id: 5,
            host: "cdn.test".into(),
            cookie: String::new(),
            request_headers: Default::default(),
            clear: false,
        }
    );
}

#[test]
fn requests_tolerate_unknown_fields_but_reject_unknown_tags_and_missing_ids() {
    let request: CorePipeRequest = serde_json::from_value(json!({
        "type": "snapshot",
        "request_id": 9,
        "future_field": true
    }))
    .unwrap();
    assert_eq!(request, CorePipeRequest::Snapshot { request_id: 9 });

    for invalid in [
        json!({"type": "drop_database", "request_id": 1}),
        json!({"request_id": 1}),
        json!({"type": "snapshot"}),
        json!({"type": "snapshot", "request_id": -1}),
        json!({"type": "snapshot", "request_id": "1"}),
        json!({"type": "Snapshot", "request_id": 1}),
        json!({"type": "load_handoffs", "request_id": 1, "after": [1]}),
        json!({"type": "list_tasks", "request_id": 1, "limit": -1}),
    ] {
        assert!(
            serde_json::from_value::<CorePipeRequest>(invalid.clone()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn response_request_ids_and_tags_follow_the_contract() {
    let error: CorePipeResponse = serde_json::from_value(json!({
        "type": "error",
        "code": "busy",
        "message": "retry"
    }))
    .unwrap();
    assert_eq!(error.request_id(), None);
    assert_eq!(to_json(&error)["request_id"], Value::Null);

    let error = CorePipeResponse::Error {
        request_id: Some(u64::MAX),
        code: "x".into(),
        message: "secret-token".into(),
    };
    assert_eq!(error.request_id(), Some(u64::MAX));
    assert_eq!(format!("{error:?}"), "Error");

    let hello = CorePipeResponse::Hello {
        protocol: V7_PROTOCOL_NAME.into(),
        version: V7_PROTOCOL_VERSION,
        pid: u32::MAX,
    };
    assert_eq!(hello.request_id(), None);
    assert_eq!(to_json(&hello)["type"], "hello");

    let samples = [
        (
            CorePipeResponse::TaskPage {
                request_id: 1,
                page: TaskPage {
                    tasks: vec![],
                    next: Some("t".into()),
                    sequence: 1,
                    browser_connected: true,
                },
            },
            "task_page",
        ),
        (
            CorePipeResponse::EventBatch {
                request_id: 1,
                batch: EventBatch {
                    events: vec![EventEnvelope {
                        sequence: 7,
                        event: CoreEvent::Ready {
                            protocol: V7_PROTOCOL_NAME.into(),
                            version: V7_PROTOCOL_VERSION,
                        },
                    }],
                    latest_sequence: 7,
                    resync_required: true,
                },
            },
            "event_batch",
        ),
        (
            CorePipeResponse::Crawl {
                request_id: 1,
                projects: vec![],
            },
            "crawl",
        ),
        (
            CorePipeResponse::ZipPreview {
                request_id: 1,
                preview: ZipPreview {
                    entries: vec![],
                    archive_bytes: u64::MAX,
                    metadata_bytes_read: 0,
                    zip64: true,
                },
            },
            "zip_preview",
        ),
        (
            CorePipeResponse::Events {
                request_id: 1,
                events: vec![],
            },
            "events",
        ),
        (
            CorePipeResponse::Snapshot {
                request_id: 1,
                tasks: vec![],
                latest_sequence: 0,
                browser_connected: false,
            },
            "snapshot",
        ),
        (
            CorePipeResponse::Capabilities {
                request_id: 1,
                product_version: "10.1.0".into(),
                protocol_version: V7_PROTOCOL_VERSION,
                commands: vec![],
                settings: vec![],
                max_frame_bytes: V7_PIPE_MAX_FRAME as u64,
            },
            "capabilities",
        ),
        (
            CorePipeResponse::Credential {
                request_id: 1,
                protected_blob: None,
            },
            "credential",
        ),
        (
            CorePipeResponse::Handoffs {
                request_id: 1,
                items: vec!["secret-token".into()],
                next: None,
            },
            "handoffs",
        ),
        (
            CorePipeResponse::Subtitle {
                request_id: 1,
                result: json!({"text": "secret-token"}),
            },
            "subtitle",
        ),
        (
            CorePipeResponse::PlayerThumbnail {
                request_id: 1,
                jpeg_base64: String::new(),
            },
            "player_thumbnail",
        ),
    ];
    for (response, tag) in samples {
        let wire = to_json(&response);
        assert_eq!(wire["type"], tag);
        assert_eq!(response.request_id(), Some(1), "{tag}");
        assert!(!format!("{response:?}").contains("secret-token"));
        let decoded: CorePipeResponse =
            decode_message(&encode_message(&response).unwrap()).unwrap();
        assert_eq!(decoded, response, "{tag}");
    }
    let handoffs = to_json(&CorePipeResponse::Handoffs {
        request_id: 1,
        items: vec![],
        next: None,
    });
    assert!(handoffs.get("next").is_none());
}

#[test]
fn snapshot_response_defaults_sequence_and_browser_state() {
    let snapshot: CorePipeResponse = serde_json::from_value(json!({
        "type": "snapshot",
        "request_id": 2,
        "tasks": []
    }))
    .unwrap();
    assert_eq!(
        snapshot,
        CorePipeResponse::Snapshot {
            request_id: 2,
            tasks: vec![],
            latest_sequence: 0,
            browser_connected: false,
        }
    );
}

#[test]
fn minimal_settings_response_fills_documented_defaults() {
    let settings: CorePipeResponse = serde_json::from_value(json!({
        "type": "settings",
        "request_id": 1,
        "takeover_enabled": true,
        "takeover_minimum_bytes": 0,
        "legal_accepted": false,
        "speed_limit_kib": 0
    }))
    .unwrap();
    let wire = to_json(&settings);
    for (key, expected) in [
        ("schedule_start", json!("22:00")),
        ("schedule_end", json!("08:00")),
        ("queue_max", json!(3)),
        ("default_concurrency", json!(12)),
        ("existing_file_policy", json!("rename")),
        ("http_chunk_size_mb", json!(8)),
        ("completion_power_action", json!("none")),
        ("queue_active_days", json!("1,2,3,4,5,6,7")),
        ("proxy_mode", json!("system")),
        ("task_sort", json!("queue:asc")),
        ("bt_upload_limit_kib", json!(1024)),
        ("bt_max_connections", json!(200)),
        ("progress_window_enabled", json!(true)),
        ("complete_popup_enabled", json!(true)),
        ("download_subtitles", json!(true)),
        ("skip_ad_segments", json!(true)),
        ("hls_remux_ts_to_mp4", json!(true)),
        ("av_scan_fail_on_threat", json!(true)),
        ("bt_enable_dht", json!(true)),
        ("speed_limit_enabled", json!(false)),
        ("default_cookie_configured", json!(false)),
        ("proxy_url", json!("")),
        ("queue_profiles", json!([])),
    ] {
        assert_eq!(wire[key], expected, "{key}");
    }

    for required in [
        "takeover_enabled",
        "takeover_minimum_bytes",
        "legal_accepted",
        "speed_limit_kib",
    ] {
        let mut partial = wire.clone();
        partial.as_object_mut().unwrap().remove(required);
        assert!(
            serde_json::from_value::<CorePipeResponse>(partial).is_err(),
            "{required}"
        );
    }
}

#[test]
fn crawl_spec_rejects_unknown_fields_and_defaults_missing_ones() {
    let spec: CrawlSpec = serde_json::from_value(json!({"entry_url": "https://a.test/"})).unwrap();
    assert_eq!(
        spec,
        CrawlSpec {
            entry_url: "https://a.test/".into(),
            ..CrawlSpec::default()
        }
    );
    assert_eq!(spec.queue_id, DEFAULT_QUEUE_ID);
    assert!(spec.same_origin);
    assert_eq!(
        (spec.max_depth, spec.max_pages, spec.page_concurrency),
        (2, 500, 2)
    );

    assert!(serde_json::from_value::<CrawlSpec>(json!({"follow_external": true})).is_err());
    assert!(serde_json::from_value::<CrawlSpec>(json!({"max_depth": -1})).is_err());
    assert!(serde_json::from_value::<CrawlSpec>(json!({"max_pages": 4_294_967_296u64})).is_err());

    let nested = json!({
        "type": "crawl",
        "request_id": 1,
        "request": {"action": "create", "spec": {"same_origin": false, "unknown": 1}}
    });
    assert!(serde_json::from_value::<CorePipeRequest>(nested).is_err());
}

#[test]
fn crawl_requests_keep_action_tags() {
    for (request, action) in [
        (
            CrawlRequest::Create {
                spec: CrawlSpec::default(),
            },
            "create",
        ),
        (CrawlRequest::List, "list"),
        (
            CrawlRequest::Get {
                project_id: "p".into(),
            },
            "get",
        ),
        (
            CrawlRequest::Pause {
                project_id: "p".into(),
            },
            "pause",
        ),
        (
            CrawlRequest::Resume {
                project_id: "p".into(),
            },
            "resume",
        ),
        (
            CrawlRequest::Enqueue {
                project_id: "p".into(),
                urls: vec![],
            },
            "enqueue",
        ),
        (
            CrawlRequest::Sync {
                project_id: "p".into(),
            },
            "sync",
        ),
    ] {
        let wire = to_json(&request);
        assert_eq!(wire["action"], action);
        assert_eq!(
            serde_json::from_value::<CrawlRequest>(wire).unwrap(),
            request
        );
    }
    assert!(serde_json::from_value::<CrawlRequest>(json!({"action": "delete"})).is_err());
}

#[test]
fn resource_kinds_use_snake_case_and_default_to_file() {
    for (kind, wire) in [
        (ResourceKind::File, "file"),
        (ResourceKind::Hls, "hls"),
        (ResourceKind::Dash, "dash"),
        (ResourceKind::Live, "live"),
        (ResourceKind::Ftp, "ftp"),
        (ResourceKind::Sftp, "sftp"),
        (ResourceKind::Torrent, "torrent"),
    ] {
        assert_eq!(to_json(&kind), json!(wire));
        assert_eq!(
            serde_json::from_value::<ResourceKind>(json!(wire)).unwrap(),
            kind
        );
    }
    assert_eq!(ResourceKind::default(), ResourceKind::File);
    assert!(serde_json::from_value::<ResourceKind>(json!("HLS")).is_err());
    assert!(serde_json::from_value::<ResourceKind>(json!("magnet")).is_err());
}

#[test]
fn oversized_requests_fail_to_encode_instead_of_truncating() {
    let request = CorePipeRequest::AdmitHandoff {
        request_id: 1,
        handoff_json: "x".repeat(V7_PIPE_MAX_FRAME),
    };
    assert_eq!(
        encode_message(&request).unwrap_err(),
        "Core pipe frame too large"
    );
}
