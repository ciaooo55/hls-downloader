use super::*;
use crate::{ResourceKind, TaskSpec};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

#[test]
fn oversized_filename_component_has_an_actionable_error() {
    for filename in [
        format!("{}.bin", "测".repeat(256)),
        format!("{}.bin", "😀".repeat(128)),
    ] {
        let error = TaskPaths::for_task(
            "task-1",
            &TaskSpec {
                filename,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("255 个 UTF-16 单位"), "{error}");
    }
}

#[test]
fn oversized_filenames_are_rejected_before_creation_and_rename() {
    let mut core = PersistentCore::in_memory().unwrap();
    for filename in ["测".repeat(256), "😀".repeat(128)] {
        let error = core
            .handle(CoreCommand::CreateTask {
                spec: TaskSpec {
                    filename,
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(error.contains("255 个 UTF-16 单位"), "{error}");
        assert!(core.tasks().is_empty());
    }
    for filename in [String::new(), "valid.bin".into(), "测".repeat(255)] {
        core.handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                filename: filename.clone(),
                ..Default::default()
            },
        })
        .unwrap();
        core.set_task_filename("task-1", filename).unwrap();
    }
    let original = core.task_spec("task-1").unwrap().clone();
    let mut invalid = original.clone();
    invalid.filename = "😀".repeat(128);
    assert!(core
        .replace_spec("task-1", invalid.clone())
        .unwrap_err()
        .contains("255 个 UTF-16 单位"));
    assert!(core
        .set_task_filename("task-1", invalid.filename)
        .unwrap_err()
        .contains("255 个 UTF-16 单位"));
    assert_eq!(
        core.task_spec("task-1").unwrap().filename,
        original.filename
    );
    assert_eq!(core.snapshot("task-1").unwrap().filename, "测".repeat(255));
}

#[test]
fn legacy_oversized_queued_filename_does_not_block_next_download() {
    let root = lifecycle_test_root("legacy-oversized-name");
    let address = serve_http_fixture(b"next task payload");
    let mut runtime = crate::CoreRuntime::default();
    let mut store = crate::CoreStore::in_memory().unwrap();
    for filename in ["😀".repeat(128), "valid.bin".into()] {
        let spec = TaskSpec {
            url: format!("http://{address}/{filename}"),
            filename,
            download_dir: root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let events = runtime.handle(CoreCommand::CreateTask { spec: spec.clone() });
        store.apply_events_and_spec(&events, Some(&spec)).unwrap();
    }
    let coordinator = CoreCoordinator::new(PersistentCore::from_store(store).unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .unwrap();
    coordinator.start_next_queued(false).unwrap();
    let failed = wait_for_task_status(&coordinator, "failed");
    assert!(format!("{failed:?}").contains("255 个 UTF-16 单位"));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let tasks = coordinator.tasks().unwrap();
        if tasks
            .iter()
            .any(|task| task.task_id == "task-2" && task.status == "completed")
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{tasks:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read(root.join("valid.bin")).unwrap(),
        b"next task payload"
    );
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn http_download_publishes_in_a_deep_unicode_directory() {
    let root = std::env::temp_dir().join(format!("hls-long-http-{}", std::process::id()));
    let mut directory = root.clone();
    for _ in 0..6 {
        directory.push(format!("deep-{}", "测".repeat(35)));
    }
    assert!(directory.to_string_lossy().encode_utf16().count() > 295);
    fs::create_dir_all(&directory).unwrap();
    let address = serve_http_fixture(b"deep unicode payload");
    let coordinator = direct_http_coordinator(&directory);
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address}/file.bin"),
                filename: "深目录验证.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let task = wait_for_task_status(&coordinator, "completed");
    let output = PathBuf::from(&task.output_path);
    assert_eq!(output.file_name().unwrap(), "深目录验证.bin");
    assert_eq!(fs::read(&output).unwrap(), b"deep unicode payload");
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn speed_schedule_times_are_validated_before_persistence() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    for key in [
        "download_speed_schedule_start",
        "download_speed_schedule_end",
    ] {
        for valid in ["00:00", "23:59", "08:30"] {
            coordinator
                .dispatch(CoreCommand::SetSetting {
                    key: key.into(),
                    value: serde_json::json!(valid),
                })
                .unwrap();
            assert_eq!(
                coordinator
                    .lock()
                    .unwrap()
                    .store()
                    .setting_string(key, "")
                    .unwrap(),
                valid
            );
        }
        for invalid in [
            "", "26:99", "25:99", "24:00", "12:60", "1:00", "01:0", "+1:00", " 01:00", "01:00 ",
            "ab:cd", "中文a",
        ] {
            assert!(
                coordinator
                    .dispatch(CoreCommand::SetSetting {
                        key: key.into(),
                        value: serde_json::json!(invalid),
                    })
                    .is_err(),
                "{key}: {invalid}"
            );
            assert_eq!(
                coordinator
                    .lock()
                    .unwrap()
                    .store()
                    .setting_string(key, "")
                    .unwrap(),
                "08:30"
            );
        }
        for invalid in [Value::Null, Value::Bool(true), serde_json::json!(1200)] {
            assert!(coordinator.set_setting(key, invalid).is_err());
        }
    }
    assert!(coordinator
        .set_settings(BTreeMap::from([
            (
                "download_speed_schedule_start".into(),
                serde_json::json!("12:00")
            ),
            (
                "download_speed_schedule_end".into(),
                serde_json::json!("25:99")
            ),
        ]))
        .is_err());
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .store()
            .setting_string("download_speed_schedule_start", "")
            .unwrap(),
        "08:30"
    );
}

static TEST_CAST_LOCK: Mutex<()> = Mutex::new(());
static TEST_POWER_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn stale_cast_control_cannot_stop_a_replacement_session() {
    let _cast_guard = TEST_CAST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let previous = {
        let _operation = cast_operation().lock().unwrap();
        crate::cast::remember_lan_share("first fixture");
        let previous = crate::cast::session_generation();
        crate::cast::remember_lan_share("replacement fixture");
        remember_cast_mount(
            "replacement-token",
            "http://192.0.2.1/media/replacement-token/media.mp4",
            "replacement media",
            "task-replacement",
        );
        previous
    };
    assert!(coordinator
        .cast_control(previous, "stop")
        .unwrap_err()
        .contains("会话已经改变"));
    assert_eq!(crate::cast::last_device_label(), "replacement fixture");
    let current = crate::cast::session_generation();
    let events = coordinator.cast_control(current, "status").unwrap();
    assert!(matches!(&events.last().unwrap().event,
        CoreEvent::CastSession {active:true,session_generation,device_kind,title,task_id,media_url,..}
        if *session_generation==current && device_kind=="lan" && title=="replacement media"
            && task_id=="task-replacement" && media_url=="http://192.0.2.1/media/replacement-token/media.mp4"));
    let stopped = coordinator.cast_control(current, "stop").unwrap();
    assert!(
        matches!(&stopped.last().unwrap().event,CoreEvent::CastSession{active:false,media_url,..} if media_url.is_empty())
    );
    assert!(crate::cast::last_device_label().is_empty());
    assert!(active_cast_mount().lock().unwrap().is_none());
    let idle = coordinator
        .dispatch(CoreCommand::ControlCast {
            action: "status".into(),
        })
        .unwrap();
    assert!(matches!(&idle.last().unwrap().event,
        CoreEvent::CastSession { active:false, session_generation, media_url, supported_actions, .. }
        if *session_generation==current && media_url.is_empty() && supported_actions.is_empty()));
    assert!(coordinator
        .cast_control(current, "play")
        .unwrap_err()
        .contains("当前没有投屏会话"));
}

#[test]
fn saving_category_dirs_in_the_pipe_shape_is_refused_instead_of_dropped() {
    // 这正是 Compose 主界面曾经发过来的形状。之前 validate_setting 对它毫无意见，
    // parse_category_dirs 静默回退成四个空目录，用户填的分类目录整段消失。
    let refused = CoreCoordinator::validate_setting(
        "browser_category_dirs",
        &serde_json::json!("E:/Videos|E:/Apps|E:/Archives|E:/Other"),
    );
    assert!(
        refused.is_err(),
        "竖线拼接的形状必须被拒绝，不能悄悄存进去: {refused:?}"
    );

    // 合法形状照旧放行（设置编辑器发送 JSON 对象，确认窗发送 JSON 文本，null 清空）。
    for stored in [
        serde_json::json!({"media":"E:/Videos","program":"E:/Apps","archive":"E:/Archives","other":"E:/Other"}),
        serde_json::json!(serde_json::json!({"media":"E:/Videos","program":"E:/Apps","archive":"E:/Archives","other":"E:/Other"}).to_string()),
        serde_json::json!(null),
        serde_json::json!(""),
    ] {
        assert!(
            CoreCoordinator::validate_setting("browser_category_dirs", &stored).is_ok(),
            "合法形状被误拒: {stored}"
        );
    }
    for invalid in [
        serde_json::json!({"media":"../escape"}),
        serde_json::json!({"media":123}),
        serde_json::json!(1),
        serde_json::json!(true),
        serde_json::json!([]),
    ] {
        assert!(
            CoreCoordinator::validate_setting("browser_category_dirs", &invalid).is_err(),
            "非法分类目录形状必须被拒绝: {invalid}"
        );
    }
}
#[test]
fn size_disagreement_classifies_as_size_mismatch() {
    let failure = task_failure_from_error(
        "response body length mismatch: expected 10, got 4",
        "transfer",
        "https://cdn.test/file.bin",
        1,
        false,
    );
    assert_eq!(failure.code, "SIZE_MISMATCH");
    assert!(
        failure.hint.contains("服务端文件内容已变化"),
        "{}",
        failure.hint
    );
    assert_eq!(
        failure_stage("transfer", "file length mismatch, expected 10, got 4"),
        "size"
    );
}

#[test]
fn every_transport_size_disagreement_is_recognised() {
    // Wording copied from the producers so a rename in any engine re-breaks
    // this: http_engine response body / range checks, sftp_engine, ftp_engine,
    // torrent_engine web seeds, and the task spec check itself.
    for engine_error in [
        "response body length mismatch: expected 10, got 4",
        "file length mismatch, expected 10, got 4",
        "FTP transfer size mismatch: expected 10, received 4",
        "web seed payload length mismatch: expected 10, got 4",
        "size mismatch: expected 10, got 4",
        "文件长度不匹配，期望 10，实际 4",
        "文件大小不匹配，期望 10，实际 4",
    ] {
        assert!(
            is_size_disagreement(&engine_error.to_ascii_lowercase()),
            "not recognised: {engine_error}"
        );
    }
    for unrelated in [
        "connection reset by peer",
        "HTTP 403",
        "no space left on device",
        "checksum mismatch: abc",
    ] {
        assert!(
            !is_size_disagreement(&unrelated.to_ascii_lowercase()),
            "false positive: {unrelated}"
        );
    }
}

#[test]
fn duplicate_identity_requires_same_safe_request_shape() {
    let plain = TaskSpec {
        url: "https://cdn.test/file.bin?token=one".into(),
        request_method: "GET".into(),
        headers: BTreeMap::from([("Accept".into(), "application/octet-stream".into())]),
        ..Default::default()
    };
    let mut same = plain.clone();
    same.url = "https://cdn.test/file.bin?token=two".into();
    same.headers = BTreeMap::from([("accept".into(), "application/octet-stream".into())]);
    assert!(CoreCoordinator::duplicate_request_identity_matches(
        &same, &plain
    ));

    let mut changed_quality = same.clone();
    changed_quality.preferred_bandwidth = 2_500_000;
    changed_quality.preferred_height = 720;
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &changed_quality,
        &plain
    ));
    let mut changed_variant = changed_quality.clone();
    changed_quality.preferred_variant_id = "v1".into();
    changed_variant.preferred_variant_id = "v2".into();
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &changed_variant,
        &changed_quality
    ));
    let mut changed_audio = same.clone();
    changed_audio.preferred_audio = "English".into();
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &changed_audio,
        &plain
    ));
    let mut changed_method = same.clone();
    changed_method.request_method = "HEAD".into();
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &changed_method,
        &plain
    ));

    let mut changed_header = same.clone();
    changed_header
        .headers
        .insert("Referer".into(), "https://page.test/".into());
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &changed_header,
        &plain
    ));

    let mut credentialed = same.clone();
    credentialed.credential_ref = Some("browser-replay-1".into());
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &credentialed,
        &plain
    ));

    let mut sensitive = same.clone();
    sensitive.headers.insert("Cookie".into(), "sid=next".into());
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &sensitive, &plain
    ));

    let mut post = same.clone();
    post.request_method = "POST".into();
    post.body_path = "request-body.bin".into();
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &post, &plain
    ));

    let mut stored_credential = plain.clone();
    stored_credential.credential_ref = Some("sealed-old-context".into());
    assert!(!CoreCoordinator::duplicate_request_identity_matches(
        &same,
        &stored_credential
    ));
}

#[test]
fn native_transfers_use_vaulted_proxy_auth_and_do_not_retry_direct() {
    for (scheme, kind) in [("ftp", ResourceKind::Ftp), ("sftp", ResourceKind::Sftp)] {
        let root = lifecycle_test_root(&format!("proxy-rejected-{scheme}"));
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut core = PersistentCore::in_memory().unwrap();
        let spec = TaskSpec {
            url: format!("{scheme}://u@{}/file.bin", target.local_addr().unwrap()),
            filename: "file.bin".into(),
            proxy: format!("http://user:pass@{address}"),
            download_dir: root.to_string_lossy().into_owned(),
            resource_kind: kind,
            ..Default::default()
        };
        core.handle(CoreCommand::CreateTask { spec }).unwrap();
        assert_eq!(
            core.task_spec("task-1").unwrap().proxy,
            format!("http://user@{address}")
        );
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(&s);
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                lines.push(line);
            }
            assert!(lines
                .iter()
                .any(|line| line == "Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"));
            s.write_all(b"HTTP/1.1 407 Proxy authentication required\r\n\r\n")
                .unwrap();
        });
        let err = run_task_with_progress(Arc::new(Mutex::new(core)), "task-1").unwrap_err();
        assert!(err.contains("407") && !err.contains("pass"), "{err}");
        server.join().unwrap();
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn password_bearing_ftp_duplicates_reuse_tasks_and_refresh_credentials() {
    for (scheme, kind) in [("ftp", ResourceKind::Ftp), ("sftp", ResourceKind::Sftp)] {
        for status in ["paused", "failed"] {
            let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
            coordinator
                .set_setting("legal_terms_accepted", serde_json::json!(true))
                .unwrap();
            let spec = TaskSpec {
                url: format!("{scheme}://alice:original@host/file.bin"),
                resource_kind: kind,
                filename: "file.bin".into(),
                scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                start_paused: true,
                ..Default::default()
            };
            coordinator.dispatch_created(spec.clone()).unwrap();
            let id = coordinator.tasks().unwrap()[0].task_id.clone();
            assert_eq!(
                CoreCoordinator::duplicate_of(&coordinator.lock().unwrap(), &spec)
                    .unwrap()
                    .0,
                id
            );
            for secret in ["original", "renewed"] {
                coordinator
                    .lock()
                    .unwrap()
                    .handle(CoreCommand::UpdateProgress {
                        task_id: id.clone(),
                        downloaded_bytes: 0,
                        total_bytes: None,
                        speed_bytes_per_sec: 0,
                        stage: "transfer".into(),
                        status: status.into(),
                    })
                    .unwrap();
                let incoming = TaskSpec {
                    start_paused: secret == "original",
                    url: format!("{scheme}://alice:{secret}@host/file.bin"),
                    ..spec.clone()
                };
                let events = coordinator.dispatch_created(incoming.clone()).unwrap();
                assert!(!events
                    .iter()
                    .any(|event| matches!(event.event, CoreEvent::TaskCreated { .. })));
                let tasks = coordinator.tasks().unwrap();
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].task_id, id);
                assert_eq!(tasks[0].url, format!("{scheme}://alice@host/file.bin"));
                assert_eq!(
                    tasks[0].status,
                    if incoming.start_paused {
                        status
                    } else {
                        "queued"
                    }
                );
                let core = coordinator.lock().unwrap();
                assert_eq!(
                    core.store().hydrate_url(&tasks[0].url).unwrap(),
                    incoming.url
                );
                assert!(
                    !serde_json::to_string(&core.store().load_task_specs().unwrap())
                        .unwrap()
                        .contains(secret)
                );
            }
        }
    }
}

#[test]
fn concurrent_stale_duplicate_lookups_commit_one_task_unless_explicitly_allowed() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let spec = TaskSpec {
        url: "https://example.test/same-resource.bin".into(),
        scheduled_start_at: "2999-01-01T00:00:00Z".into(),
        ..Default::default()
    };
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let shared = coordinator.clone();
            let spec = spec.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                // Both clients finish lookup before either can commit, as can
                // happen between the old dispatcher's separate critical sections.
                let missing =
                    CoreCoordinator::duplicate_of(&shared.lock().unwrap(), &spec).is_none();
                barrier.wait();
                assert!(missing);
                shared.dispatch_created(spec).unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(coordinator.tasks().unwrap().len(), 1);
    coordinator
        .dispatch_created(TaskSpec {
            allow_duplicate: true,
            ..spec
        })
        .unwrap();
    assert_eq!(coordinator.tasks().unwrap().len(), 2);
}

fn spec() -> TaskSpec {
    TaskSpec {
        url: "https://example.test/path/file.bin".into(),
        resource_kind: crate::ResourceKind::File,
        title: "File".into(),
        filename: "../bad:name?.bin".into(),
        // 必须隔离到本进程专属子目录：`build_job()` 会调用 `prepare()` 真实落盘，
        // 直接指向 `%TEMP%` 会在共享临时目录根部留下 `.hls-tasks` 残留，
        // 且多个测试进程/真实实例之间会互相撞同一个固定路径。
        download_dir: std::env::temp_dir()
            .join(format!("hls-v7-spec-{}", std::process::id()))
            .to_string_lossy()
            .into_owned(),
        request_method: "GET".into(),
        credential_ref: None,
        replay_context_ref: None,
        concurrency: 4,
        checksum: None,
        expected_size: Some(100),
        etag: String::new(),
        last_modified: String::new(),
        ..Default::default()
    }
}

fn serve_harvest_response(response: String) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    break;
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let _ = sender.send(request);
            let mut stream = reader.into_inner();
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{address}"), receiver)
}

#[test]
fn core_completion_probe_uses_draft_headers_and_saved_credentials() {
    let body = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720\nvideo.m3u8\n";
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let (origin, request) = serve_harvest_response(response);
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .store_credential(
            "probe-session",
            &serde_json::json!({"cookie": "session=valid"}).to_string(),
            "browser_replay",
        )
        .unwrap();
    let url = format!("{origin}/master.m3u8");
    let events = coordinator
        .dispatch(CoreCommand::ProbeUrl {
            url: url.clone(),
            spec: Some(TaskSpec {
                url: "https://ignored.test/old.m3u8".into(),
                proxy: crate::net_policy::DIRECT_PROXY_SENTINEL.into(),
                credential_ref: Some("probe-session".into()),
                headers: BTreeMap::from([
                    ("Referer".into(), "https://site.test/watch/42".into()),
                    ("Authorization".into(), "Bearer supplied-token".into()),
                    ("User-Agent".into(), "Probe regression".into()),
                ]),
                ..TaskSpec::default()
            }),
        })
        .unwrap();
    let variants = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::ProbeResult {
                resource_kind: ResourceKind::Hls,
                variants,
                ..
            } => Some(variants),
            _ => None,
        })
        .expect("authenticated probe result");
    assert_eq!(variants[0].height, 720);
    let request = request
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .to_ascii_lowercase();
    for expected in [
        "referer: https://site.test/watch/42\r\n",
        "authorization: bearer supplied-token\r\n",
        "cookie: session=valid\r\n",
        "user-agent: probe regression\r\n",
    ] {
        assert!(
            request.contains(expected),
            "missing {expected:?} in {request:?}"
        );
    }
    assert!(coordinator.tasks().unwrap().is_empty());
}

#[test]
fn harvest_page_sends_referer_and_returns_resource_metadata() {
    let body = r#"<!doctype html><a href="/media/movie.mp4">Movie</a>"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let (origin, request) = serve_harvest_response(response);
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let events = coordinator
        .dispatch(CoreCommand::HarvestPage {
            url: format!("{origin}/watch"),
            referer: "https://site.test/watch/42".into(),
            probe_urls: Vec::new(),
        })
        .unwrap();
    let links = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::HarvestResult { links, .. } => Some(links),
            _ => None,
        })
        .expect("harvest result");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].filename, "movie.mp4");
    assert_eq!(links[0].extension, "mp4");
    let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("referer: https://site.test/watch/42\r\n"),
        "request did not preserve Referer: {request:?}"
    );
}

#[test]
fn harvest_size_probe_is_bounded_and_skips_invalid_candidates() {
    let response = concat!(
        "HTTP/1.1 206 Partial Content\r\n",
        "Content-Range: bytes 0-0/4096\r\n",
        "Content-Length: 1\r\n",
        "Accept-Ranges: bytes\r\n",
        "Connection: close\r\n\r\n",
        "x"
    )
    .to_string();
    let (origin, request) = serve_harvest_response(response);
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let candidate = format!("{origin}/movie.mp4");
    let events = coordinator
        .dispatch(CoreCommand::HarvestPage {
            url: format!("{origin}/watch"),
            referer: format!("{origin}/watch"),
            probe_urls: vec![candidate.clone(), "javascript:alert(1)".into()],
        })
        .unwrap();
    let links = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::HarvestProbeResult { links, .. } => Some(links),
            _ => None,
        })
        .expect("harvest probe result");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].url, candidate);
    assert_eq!(links[0].size, 4096);
    let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("range: bytes=0-0\r\n"));
    assert!(lower.contains(&format!("referer: {origin}/watch\r\n").to_ascii_lowercase()));

    let error = coordinator
        .dispatch(CoreCommand::HarvestPage {
            url: format!("{origin}/watch"),
            referer: String::new(),
            probe_urls: (0..101)
                .map(|index| format!("https://cdn.test/{index}.bin"))
                .collect(),
        })
        .unwrap_err();
    assert!(error.contains("100"));
}

#[test]
fn adaptive_media_output_has_a_playable_filename() {
    for kind in [ResourceKind::Hls, ResourceKind::Dash, ResourceKind::Live] {
        for (filename, expected) in [
            ("网页标题", "网页标题.mp4"),
            ("episode.M3U8", "episode.mp4"),
            ("episode.mpd", "episode.mp4"),
            ("episode.mp4", "episode.mp4"),
            ("custom.mkv", "custom.mp4"),
        ] {
            let spec = TaskSpec {
                url: "https://cdn.test/stream".into(),
                filename: filename.into(),
                resource_kind: kind,
                ..Default::default()
            };
            let paths = TaskPaths::for_task("media-1", &spec).unwrap();
            assert_eq!(paths.final_output.file_name().unwrap(), expected);
            assert!(paths.output.ends_with("payload.downloading"));
        }
    }
    let file = TaskSpec {
        filename: "无扩展名文件".into(),
        ..Default::default()
    };
    assert_eq!(
        TaskPaths::for_task("file-1", &file)
            .unwrap()
            .final_output
            .file_name()
            .unwrap(),
        "无扩展名文件"
    );
}

#[test]
fn task_paths_keep_payload_and_final_output_separate() {
    let paths = TaskPaths::for_task("task-1", &spec()).unwrap();
    assert!(paths.output.ends_with("payload.downloading"));
    assert!(paths.final_output.ends_with("_bad_name_.bin"));
    assert_ne!(paths.output, paths.final_output);
    assert_eq!(
        safe_filename("report&calc.exe", "https://cdn.test/a.bin"),
        "report_calc.exe"
    );
    assert_eq!(
        safe_filename("a%PATH%.txt", "https://cdn.test/a.bin"),
        "a_PATH_.txt"
    );
    assert!(!safe_filename("evil\nnotepad.exe", "https://cdn.test/a.bin").contains('\n'));
    assert_eq!(safe_filename("CON", "https://cdn.test/a.bin"), "_CON");
    assert_eq!(
        safe_filename("con.txt", "https://cdn.test/a.bin"),
        "_con.txt"
    );
    assert_eq!(
        safe_filename("COM1.dat", "https://cdn.test/a.bin"),
        "_COM1.dat"
    );
    assert_eq!(safe_filename("NUL", "https://cdn.test/a.bin"), "_NUL");
}

#[test]
fn task_paths_prepare_preserves_an_active_pause_or_cancel_request() {
    let root = std::env::temp_dir().join(format!(
        "hls-v7-control-race-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut task = spec();
    task.download_dir = root.to_string_lossy().into_owned();
    let paths = TaskPaths::for_task("task-1", &task).unwrap();
    paths.prepare().unwrap();
    for control in ["pause", "cancel"] {
        paths.set_control(control).unwrap();
        paths.prepare().unwrap();
        assert_eq!(fs::read_to_string(&paths.control).unwrap(), control);
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn truncated_filenames_cannot_become_reserved_device_names() {
    for device in ["CON", "NUL", "AUX", "PRN", "COM1", "LPT9"] {
        for extension in [
            "x".repeat(199 - device.len()),
            format!("{}{}", "😀".repeat(125), "x".repeat(4 - device.len())),
        ] {
            let filename = format!("{device}suffix.{extension}");
            let sanitized = safe_filename(&filename, "");
            assert!(!reserved_dos_device_name(&sanitized), "{sanitized}");
            assert!(sanitized.starts_with('_'), "{sanitized}");
            assert!(sanitized.encode_utf16().count() <= 255);
            assert_eq!(
                Path::new(&sanitized).extension().unwrap(),
                extension.as_str()
            );
        }
    }
}

fn local_source_fixture(label: &str, contents: &[u8]) -> (PathBuf, PathBuf, TaskSpec) {
    let root = std::env::temp_dir().join(format!(
        "hls-local-source-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("browser-download.bin");
    fs::write(&source, contents).unwrap();
    let spec = TaskSpec {
        url: "blob:https://site.test/opaque".into(),
        local_source_path: Some(
            fs::canonicalize(&source)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ),
        filename: "saved.bin".into(),
        download_dir: root.join("downloads").to_string_lossy().into_owned(),
        work_dir: root.join("work").to_string_lossy().into_owned(),
        expected_size: Some(contents.len() as u64),
        ..Default::default()
    };
    (root, source, spec)
}

fn create_test_task(spec: &TaskSpec) -> (Arc<Mutex<PersistentCore>>, String) {
    let mut persistent = PersistentCore::in_memory().unwrap();
    let events = persistent
        .handle(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    let task_id = events
        .into_iter()
        .find_map(|event| match event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id),
            _ => None,
        })
        .unwrap();
    (Arc::new(Mutex::new(persistent)), task_id)
}

#[test]
fn local_source_is_copied_and_published_with_verified_bytes() {
    let contents = vec![0x5a; 2 * 1024 * 1024 + 13];
    let (root, source, spec) = local_source_fixture("success", &contents);
    let (core, task_id) = create_test_task(&spec);
    let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
    paths.prepare().unwrap();

    run_task_with_throttle(Arc::clone(&core), &task_id, spec, String::new()).unwrap();

    assert_eq!(fs::read(&paths.final_output).unwrap(), contents);
    assert!(
        source.is_file(),
        "Core leaves source cleanup to the browser extension"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn canceled_local_source_copy_does_not_publish_or_remove_source() {
    let contents = vec![0x31; 2 * 1024 * 1024 + 13];
    let (root, source, spec) = local_source_fixture("cancel", &contents);
    let (core, task_id) = create_test_task(&spec);
    let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
    paths.prepare().unwrap();
    paths.set_control("cancel").unwrap();

    let error = run_task_with_throttle(core, &task_id, spec, String::new()).unwrap_err();

    assert_eq!(error, "canceled");
    assert!(!paths.final_output.exists());
    assert!(source.is_file());
    assert_eq!(fs::read(source).unwrap(), contents);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn local_source_in_output_directory_uses_a_distinct_target_path() {
    let root = std::env::temp_dir().join(format!(
        "hls-local-collision-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output_dir = root.join("downloads");
    fs::create_dir_all(&output_dir).unwrap();
    let source = output_dir.join("saved.bin");
    fs::write(&source, b"already downloaded").unwrap();
    let spec = TaskSpec {
        url: "data:application/octet-stream,".into(),
        local_source_path: Some(
            fs::canonicalize(&source)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ),
        filename: "saved.bin".into(),
        download_dir: output_dir.to_string_lossy().into_owned(),
        work_dir: root.join("work").to_string_lossy().into_owned(),
        expected_size: Some(18),
        ..Default::default()
    };

    let paths = TaskPaths::for_task("task-collision", &spec).unwrap();

    assert_ne!(paths.final_output, fs::canonicalize(&source).unwrap());
    assert_eq!(paths.final_output.file_name().unwrap(), "saved (2).bin");
    assert_eq!(fs::read(&source).unwrap(), b"already downloaded");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn start_like_actions_wait_then_keep_future_tasks_queued() {
    for (terminal_status, action, old_control) in [
        ("paused", "resume", "pause"),
        ("canceled", "retry", "cancel"),
    ] {
        let root = std::env::temp_dir().join(format!(
            "hls-v7-worker-restart-{}-{}-{action}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(root.to_string_lossy().into_owned()),
            )
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "https://cdn.test/restart.bin".into(),
                    filename: "restart.bin".into(),
                    scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        coordinator
            .lock()
            .unwrap()
            .handle(CoreCommand::UpdateProgress {
                task_id: "task-1".into(),
                downloaded_bytes: 7,
                total_bytes: Some(10),
                speed_bytes_per_sec: 0,
                stage: "waiting".into(),
                status: terminal_status.into(),
            })
            .unwrap();
        let paths = TaskPaths::for_task(
            "task-1",
            coordinator.lock().unwrap().task_spec("task-1").unwrap(),
        )
        .unwrap();
        paths.prepare().unwrap();
        paths.set_control(old_control).unwrap();
        coordinator.active.lock().unwrap().insert("task-1".into());

        let (wait_tx, wait_rx) = mpsc::channel();
        *coordinator.worker_wait_started.lock().unwrap() = Some(wait_tx);
        let runner = coordinator.clone();
        let action = action.to_string();
        let dispatch = thread::spawn(move || {
            runner.dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action,
            })
        });
        wait_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        assert_eq!(fs::read_to_string(&paths.control).unwrap(), old_control);
        assert_eq!(coordinator.tasks().unwrap()[0].status, terminal_status);

        coordinator
            .lock()
            .unwrap()
            .handle(CoreCommand::UpdateProgress {
                task_id: "task-1".into(),
                downloaded_bytes: 7,
                total_bytes: Some(10),
                speed_bytes_per_sec: 0,
                stage: "waiting".into(),
                status: terminal_status.into(),
            })
            .unwrap();
        coordinator.active.lock().unwrap().remove("task-1");
        dispatch.join().unwrap().unwrap();

        assert_eq!(coordinator.tasks().unwrap()[0].status, "queued");
        assert_eq!(fs::read_to_string(&paths.control).unwrap(), old_control);
        assert!(!coordinator.worker_is_active("task-1").unwrap());
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn paused_or_canceled_control_blocks_publish() {
    let control =
        std::env::temp_dir().join(format!("hls-v7-publish-control-{}", std::process::id()));
    for (value, expected) in [("pause", "paused"), ("cancel", "canceled")] {
        fs::write(&control, value).unwrap();
        assert_eq!(ensure_publish_allowed(&control).unwrap_err(), expected);
    }
    let _ = fs::remove_file(control);
}

#[test]
fn illegal_start_like_action_does_not_reset_worker_control() {
    let root = std::env::temp_dir().join(format!(
        "hls-v7-illegal-worker-start-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/queued.bin".into(),
                filename: "queued.bin".into(),
                scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let paths = TaskPaths::for_task(
        "task-1",
        coordinator.lock().unwrap().task_spec("task-1").unwrap(),
    )
    .unwrap();
    paths.prepare().unwrap();
    paths.set_control("pause").unwrap();

    let events = coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "resume".into(),
        })
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        &event.event,
        CoreEvent::Error { code, .. } if code == "illegal_task_action"
    )));
    assert_eq!(coordinator.tasks().unwrap()[0].status, "queued");
    assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
    assert!(!coordinator.worker_is_active("task-1").unwrap());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn start_in_a_disabled_queue_stays_queued_without_resetting_control() {
    let root = std::env::temp_dir().join(format!(
        "hls-v7-disabled-queue-start-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([{
                "id": "default",
                "name": "默认队列",
                "enabled": false,
                "priority": 0,
                "max_active": 3,
                "speed_limit_kib": 0,
                "schedule_enabled": false,
                "start_time": "00:00",
                "stop_time": "23:59",
                "active_days": "1,2,3,4,5,6,7",
                "completion_action": "none"
            }]),
        )
        .unwrap();
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/disabled.bin".into(),
                filename: "disabled.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let paths = TaskPaths::for_task(
        "task-1",
        coordinator.lock().unwrap().task_spec("task-1").unwrap(),
    )
    .unwrap();
    paths.prepare().unwrap();
    paths.set_control("pause").unwrap();

    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();

    assert_eq!(coordinator.tasks().unwrap()[0].status, "queued");
    assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
    assert!(!coordinator.worker_is_active("task-1").unwrap());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn start_reuses_an_active_worker_without_waiting_for_it() {
    let root = std::env::temp_dir().join(format!(
        "hls-v7-active-start-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    let spec = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/active-start.bin".into(),
            filename: "active-start.bin".into(),
            ..Default::default()
        })
        .unwrap();
    coordinator.dispatch_created(spec).unwrap();
    coordinator.active.lock().unwrap().insert("task-1".into());

    let (result_tx, result_rx) = mpsc::channel();
    let runner = coordinator.clone();
    let dispatch = thread::spawn(move || {
        let _ = result_tx.send(runner.dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        }));
    });
    let result = match result_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(error) => {
            coordinator.active.lock().unwrap().remove("task-1");
            dispatch.join().unwrap();
            panic!("start waited for the existing worker: {error}");
        }
    };
    result.unwrap();
    dispatch.join().unwrap();

    assert_eq!(coordinator.tasks().unwrap()[0].status, "downloading");
    assert!(coordinator.worker_is_active("task-1").unwrap());
    coordinator.active.lock().unwrap().remove("task-1");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn proxy_route_identity_preserves_global_and_site_modes() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("proxy_mode".into(), serde_json::json!("system")),
            (
                "proxy_url".into(),
                serde_json::json!("http://127.0.0.1:9999"),
            ),
        ]))
        .unwrap();
    let global_system = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://other.test/system.bin".into(),
            filename: "system.bin".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        global_system.proxy,
        crate::net_policy::SYSTEM_PROXY_SENTINEL
    );

    let system_rule = crate::format_site_rules(&[crate::SiteRule {
        host: "cdn.test".into(),
        proxy_mode: "system".into(),
        ..Default::default()
    }]);
    coordinator
        .set_settings(BTreeMap::from([
            ("proxy_mode".into(), serde_json::json!("manual")),
            (
                "proxy_url".into(),
                serde_json::json!("http://127.0.0.1:9999"),
            ),
            ("site_rules".into(), serde_json::json!(system_rule)),
            ("proxy_bypass".into(), serde_json::json!("")),
        ]))
        .unwrap();
    let site_system = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/system.bin".into(),
            filename: "system.bin".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(site_system.proxy, crate::net_policy::SYSTEM_PROXY_SENTINEL);

    let direct_rule = crate::format_site_rules(&[crate::SiteRule {
        host: "cdn.test".into(),
        proxy_mode: "direct".into(),
        ..Default::default()
    }]);
    coordinator
        .set_setting("site_rules", serde_json::json!(direct_rule))
        .unwrap();
    let site_direct = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/direct.bin".into(),
            filename: "direct.bin".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(site_direct.proxy, crate::net_policy::DIRECT_PROXY_SENTINEL);

    let manual_rule = crate::format_site_rules(&[crate::SiteRule {
        host: "cdn.test".into(),
        proxy_mode: "manual".into(),
        proxy: "http://127.0.0.1:7777".into(),
        ..Default::default()
    }]);
    coordinator
        .set_settings(BTreeMap::from([
            ("proxy_mode".into(), serde_json::json!("direct")),
            ("site_rules".into(), serde_json::json!(manual_rule)),
        ]))
        .unwrap();
    let site_manual = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/manual.bin".into(),
            filename: "manual.bin".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(site_manual.proxy, "http://127.0.0.1:7777");

    coordinator
        .set_setting("proxy_bypass", serde_json::json!("cdn.test"))
        .unwrap();
    let bypassed = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/bypass.bin".into(),
            filename: "bypass.bin".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(bypassed.proxy, crate::net_policy::DIRECT_PROXY_SENTINEL);
}

#[test]
fn advanced_defaults_are_applied_and_host_scope_is_enforced() {
    // 隔离的临时工作目录：本用例会真正派发 CreateTask，若用硬编码绝对路径
    // （原为 D:\HLS\Cache）就会在本机写出版本库之外的目录，轮转测试时污染磁盘。
    let root =
        std::env::temp_dir().join(format!("hls-v7-advanced-defaults-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let work_dir = root.to_string_lossy().into_owned();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            ("temp_dir".into(), serde_json::json!(work_dir.clone())),
            (
                "default_origin".into(),
                serde_json::json!("https://player.example.test"),
            ),
            ("allowed_hosts".into(), serde_json::json!("*.example.test")),
        ]))
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/video.mp4".into(),
                filename: "video.mp4".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .cloned()
        .unwrap();
    assert_eq!(spec.work_dir, work_dir);
    assert_eq!(
        spec.headers.get("Origin").map(String::as_str),
        Some("https://player.example.test")
    );
    let error = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://outside.invalid/video.mp4".into(),
                filename: "blocked.mp4".into(),
                ..Default::default()
            },
        })
        .unwrap_err();
    assert!(error.contains("允许的域名"));
    // The worker can still write its control file after CreateTask returns.
    coordinator.cancel_and_wait(&task_id).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(not(windows))]
#[test]
fn default_cookie_fails_closed_without_dpapi() {
    // 非 Windows 平台没有 DPAPI：默认 Cookie 必须拒绝保存，而不是以明文落库。
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let error = coordinator
        .set_default_cookie("session=private")
        .unwrap_err();
    assert!(error.contains("DPAPI"), "{error}");
    assert!(!coordinator.default_cookie_configured().unwrap());
    assert!(coordinator
        .load_credential(DEFAULT_COOKIE_CREDENTIAL_REF)
        .unwrap()
        .is_none());
    let spec = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://example.test/file.bin".into(),
            filename: "file.bin".into(),
            ..TaskSpec::default()
        })
        .unwrap();
    assert!(spec.credential_ref.is_none());
    assert!(!spec.headers.values().any(|value| value.contains("private")));
}

#[cfg(windows)]
#[test]
fn default_cookie_is_protected_and_attached_by_reference_only() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator.set_default_cookie("session=private").unwrap();
    assert!(coordinator.default_cookie_configured().unwrap());
    let spec = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://example.test/file.bin".into(),
            filename: "file.bin".into(),
            ..TaskSpec::default()
        })
        .unwrap();
    assert_eq!(
        spec.credential_ref.as_deref(),
        Some(DEFAULT_COOKIE_CREDENTIAL_REF)
    );
    assert!(!serde_json::to_string(&spec)
        .unwrap()
        .contains("session=private"));
    let blob = coordinator
        .load_credential(DEFAULT_COOKIE_CREDENTIAL_REF)
        .unwrap()
        .unwrap();
    assert!(blob.starts_with("dpapi:"));
    assert!(!blob.contains("session=private"));
    coordinator.set_default_cookie("").unwrap();
    assert!(!coordinator.default_cookie_configured().unwrap());
    assert!(coordinator
        .set_default_cookie("a=b\r\nX-Test: yes")
        .is_err());
}

#[test]
fn new_task_paths_use_the_version_neutral_work_directory() {
    let root = std::env::temp_dir().join(format!(
        "hls-task-path-current-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut task = spec();
    task.download_dir = root.to_string_lossy().into_owned();
    let paths = TaskPaths::for_task("new-task", &task).unwrap();
    assert!(paths
        .task_dir()
        .ends_with(Path::new(".hls-tasks").join("new-task")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn get_job_without_size_is_not_forced_sequential() {
    let mut spec = spec();
    spec.expected_size = None;
    let (job, paths) = build_job("task-probe", &spec).unwrap();
    assert!(!job.sequential);
    assert_eq!(job.total, 0);
    spec.request_method = "POST".into();
    let (post_job, post_paths) = build_job("task-post", &spec).unwrap();
    assert!(post_job.sequential);
    // 连同隔离根一起删除，避免只删 `.hls-tasks/<task>` 而把空父目录留在 %TEMP%。
    let _ = fs::remove_dir_all(spec.download_dir);
    let _ = fs::remove_dir_all(paths.task_dir());
    let _ = fs::remove_dir_all(post_paths.task_dir());
}

fn serve_http_fixture(body: &'static [u8]) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut range = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    let (start, end) = value.trim().split_once('-').unwrap();
                    range = Some((
                        start.parse::<usize>().unwrap(),
                        end.parse::<usize>().unwrap(),
                    ));
                }
            }
            let (start, end) = range.unwrap_or((0, body.len() - 1));
            let end = end.min(body.len() - 1);
            let mut stream = reader.into_inner();
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                body.len(),
                end - start + 1
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body[start..=end]);
        }
    });
    address
}

/// The default `system` proxy is resolved by WinHTTP; other platforms only
/// have the plain direct transport. Windows keeps the default so these tests
/// still exercise the system-proxy route there.
fn direct_transport_off_windows(coordinator: &CoreCoordinator) {
    #[cfg(not(windows))]
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .unwrap();
    #[cfg(windows)]
    let _ = coordinator;
}

fn direct_http_coordinator(download_dir: &Path) -> CoreCoordinator {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    for (key, value) in [
        ("legal_terms_accepted", serde_json::json!(true)),
        (
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        ),
        // The default system proxy needs WinHTTP; direct runs the Linux transport.
        ("proxy_mode", serde_json::json!("direct")),
    ] {
        coordinator.set_setting(key, value).unwrap();
    }
    coordinator
}

fn wait_for_task_status(coordinator: &CoreCoordinator, status: &str) -> crate::TaskSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        if task.status == status {
            return task;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "task did not reach {status}: {task:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn direct_multi_connection_task_completes_through_in_memory_control_and_progress() {
    let body: &'static [u8] = Box::leak(
        (0..3 * 1024 * 1024u32)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let address = serve_http_fixture(body);
    let download_dir = std::env::temp_dir().join(format!(
        "hls-p0-direct-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&download_dir).unwrap();
    let coordinator = direct_http_coordinator(&download_dir);
    let spec = TaskSpec {
        url: format!("http://{address}/fixture.bin"),
        resource_kind: crate::ResourceKind::File,
        title: "Fixture".into(),
        filename: "fixture.bin".into(),
        download_dir: download_dir.to_string_lossy().into_owned(),
        request_method: "GET".into(),
        concurrency: 4,
        expected_size: Some(body.len() as u64),
        ..Default::default()
    };
    coordinator
        .dispatch(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();
    let task = wait_for_task_status(&coordinator, "completed");
    assert_eq!(task.downloaded_bytes, body.len() as u64);
    assert_eq!(fs::read(&task.output_path).unwrap(), body);
    // The token lives only while the task runs.
    let paths = TaskPaths::for_task("task-1", &spec).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::task_control::current(&paths.control).is_some() {
        assert!(std::time::Instant::now() < deadline, "control token leaked");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(download_dir);
}

#[test]
fn direct_task_pause_reaches_a_stalled_worker_through_the_control_token() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut held = Vec::new();
        listener.set_nonblocking(true).unwrap();
        loop {
            if release_rx.try_recv().is_ok() {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                    }
                    let mut stream = reader.into_inner();
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\nabc",
                    );
                    held.push(stream);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    });
    let download_dir = std::env::temp_dir().join(format!(
        "hls-p0-pause-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&download_dir).unwrap();
    let coordinator = direct_http_coordinator(&download_dir);
    let spec = TaskSpec {
        url: format!("http://{address}/stall.bin"),
        resource_kind: crate::ResourceKind::File,
        title: "Stall".into(),
        filename: "stall.bin".into(),
        download_dir: download_dir.to_string_lossy().into_owned(),
        request_method: "GET".into(),
        concurrency: 1,
        ..Default::default()
    };
    coordinator
        .dispatch(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();
    let paths = TaskPaths::for_task("task-1", &spec).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::task_control::current(&paths.control).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "task never registered"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    let paused_at = std::time::Instant::now();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "pause".into(),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::task_control::current(&paths.control).is_some() {
        assert!(std::time::Instant::now() < deadline, "worker ignored pause");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(paused_at.elapsed() < Duration::from_secs(2));
    assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
    wait_for_task_status(&coordinator, "paused");
    let _ = release_tx.send(());
    server.join().unwrap();
    let _ = fs::remove_dir_all(download_dir);
}

#[test]
fn unnamed_http_task_updates_name_before_payload_completion_and_persists_it() {
    check_unnamed_http_response_filename("resolved payload.bin");
}

#[test]
fn unnamed_http_task_shortens_oversized_server_filename_and_keeps_extension() {
    check_unnamed_http_response_filename(&format!("{}.bin", "😀".repeat(180)));
}

fn check_unnamed_http_response_filename(server_name: &str) {
    let expected_name = safe_filename(server_name, "");
    assert!(expected_name.encode_utf16().count() <= 255);
    assert!(expected_name.ends_with(".bin"));
    let encoded_name: String = server_name
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect();
    let root = lifecycle_test_root("http-task-name");
    fs::create_dir_all(&root).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let release = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let server_release = Arc::clone(&release);
    let server_stop = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::SeqCst) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            let release = Arc::clone(&server_release);
            let encoded_name = encoded_name.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Disposition: attachment; filename*=UTF-8''{encoded_name}\r\nConnection: close\r\n\r\n");
                if !request.starts_with("HEAD ") {
                    while !release.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    let _ = stream.write_all(b"abcde");
                }
            });
        }
    });
    let coordinator = CoreCoordinator::new(PersistentCore::open(root.join("tasks.db")).unwrap());
    for (key, value) in [
        ("legal_terms_accepted", serde_json::json!(true)),
        ("download_dir", serde_json::json!(root.to_string_lossy())),
        ("proxy_mode", serde_json::json!("direct")),
    ] {
        coordinator.set_setting(key, value).unwrap();
    }
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address}/slow.bin"),
                start_paused: true,
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let provisional = coordinator.tasks().unwrap().remove(0);
    assert_eq!(provisional.title, "slow.bin");
    assert!(provisional.filename.is_empty());
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "resume".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let task = coordinator.tasks().unwrap().remove(0);
        if task.filename == expected_name {
            assert_ne!(task.status, "completed");
            assert_eq!(task.title, expected_name);
            assert_ne!(Path::new(&task.output_path), root.join(&expected_name));
            break;
        }
        assert!(Instant::now() < deadline, "{task:?}");
        thread::sleep(Duration::from_millis(20));
    }
    let restored = PersistentCore::open(root.join("tasks.db")).unwrap();
    assert_eq!(restored.snapshot("task-1").unwrap().filename, expected_name);
    drop(restored);
    release.store(true, Ordering::SeqCst);
    let completed = wait_for_task_status(&coordinator, "completed");
    assert_eq!(Path::new(&completed.output_path), root.join(&expected_name));
    assert_eq!(fs::read(&completed.output_path).unwrap(), b"abcde");
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn response_filename_ignores_missing_and_corrupt_metadata() {
    let root = std::env::temp_dir().join(format!("hls-response-name-{}", std::process::id()));
    let spec = TaskSpec {
        download_dir: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let paths = TaskPaths::for_task("task-1", &spec).unwrap();
    paths.prepare().unwrap();
    let metadata = paths.progress.with_extension("response.json");
    assert_eq!(http_response_filename(&paths), None);
    for bytes in [
        b"not json".as_slice(),
        br#"{}"#,
        br#"{"content_disposition":"attachment; filename=\"\""}"#,
        br#"{"content_disposition":"attachment; filename=\"bad\nname.bin\""}"#,
    ] {
        fs::write(&metadata, bytes).unwrap();
        assert_eq!(http_response_filename(&paths), None);
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn derived_filenames_fit_utf16_components_and_preserve_extensions() {
    for filename in [
        format!("{}.bin", "😀".repeat(180)),
        format!("{}.mp4", "测".repeat(300)),
    ] {
        let expected_extension = Path::new(&filename).extension().unwrap();
        for derived in [
            safe_filename(&filename, ""),
            safe_filename("", &format!("https://cdn.test/{filename}")),
        ] {
            assert!(derived.encode_utf16().count() <= 255, "{derived}");
            assert_eq!(Path::new(&derived).extension().unwrap(), expected_extension);
        }
    }
    assert_eq!(safe_filename("", ""), "download");
}

#[test]
fn http_name_update_failure_cancels_and_joins_the_running_transfer() {
    let root = lifecycle_test_root("http-name-update-error");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let coordinator = direct_http_coordinator(&root);
    let spec = TaskSpec {
        url: format!("http://{address}/slow.bin"),
        download_dir: root.to_string_lossy().into_owned(),
        concurrency: 1,
        ..Default::default()
    };
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    let paths = TaskPaths::for_task("task-1", &spec).unwrap();
    paths.prepare().unwrap();
    paths.set_control("run").unwrap();
    let control = paths.control.clone();
    let server_core = coordinator.core();
    let server = thread::spawn(move || {
        for _ in 0..1 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = server_core.lock().unwrap();
                panic!("inject Core failure after HTTP worker starts");
            }));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Disposition: attachment; filename=resolved.bin\r\nConnection: close\r\n\r\n");
            if !request.starts_with("HEAD ") {
                let deadline = Instant::now() + Duration::from_secs(5);
                while fs::read_to_string(&control).unwrap_or_default() != "cancel"
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(5));
                }
                let _ = stream.write_all(b"abcde");
            }
        }
    });
    let error = run_http_file(coordinator.core(), "task-1", spec, String::new()).unwrap_err();
    assert_eq!(error, "v7 Core mutex poisoned");
    assert_eq!(fs::read_to_string(&paths.control).unwrap(), "cancel");
    assert!(
        crate::task_control::current(&paths.control).is_none(),
        "HTTP worker still registered after failure"
    );
    server.join().unwrap();
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn coordinator_runs_http_task_and_atomically_publishes_output() {
    let body: &'static [u8] = b"v6-core-http-fixture";
    let address = serve_http_fixture(body);

    let download_dir = std::env::temp_dir().join(format!(
        "hls-v6-worker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&download_dir).unwrap();
    fs::write(download_dir.join("fixture.bin"), b"existing").unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    direct_transport_off_windows(&coordinator);
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address}/fixture.bin"),
                resource_kind: crate::ResourceKind::File,
                title: "Fixture".into(),
                filename: "fixture.bin".into(),
                download_dir: download_dir.to_string_lossy().into_owned(),
                request_method: "GET".into(),
                credential_ref: None,
                replay_context_ref: None,
                concurrency: 1,
                checksum: None,
                expected_size: Some(body.len() as u64),
                etag: String::new(),
                last_modified: String::new(),
                ..Default::default()
            },
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let published = loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        if task.status == "completed" {
            break PathBuf::from(task.output_path);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "task did not complete: {task:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        fs::read(download_dir.join("fixture.bin")).unwrap(),
        b"existing"
    );
    assert_eq!(published.file_name().unwrap(), "fixture_1.bin");
    assert_eq!(fs::read(&published).unwrap(), body);
    let _ = fs::remove_dir_all(download_dir);
}

/// AV 门禁的 fail-closed 回归：扫描引擎根本起不来时（命令合法但 exe 不
/// 存在、引擎崩溃/超时同理），任务必须失败、不得发布。以前这种情况返回
/// skipped，发布路径只看 state == "threat"，于是"用户开了扫描"和"产品
/// 根本没扫"被当成同一件事。
/// 这里故意把 av_scan_fail_on_threat 关掉：扫描没跑起来不是扫描结果，
/// 不该受"发现威胁才阻止"这个开关豁免。
#[test]
fn a_scan_that_cannot_run_never_publishes_the_download() {
    let body: &'static [u8] = b"v6-core-http-fixture";
    let address = serve_http_fixture(body);

    let download_dir = std::env::temp_dir().join(format!(
        "hls-v6-av-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&download_dir).unwrap();
    fs::write(download_dir.join("fixture.bin"), b"existing").unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    direct_transport_off_windows(&coordinator);
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        )
        .unwrap();
    coordinator
        .set_setting("av_scan_enabled", serde_json::json!(true))
        .unwrap();
    // 命令本身合法（不在解释器黑名单、带 {file}），但 exe 不存在：
    // spawn 失败 = 扫描没跑起来。被解释器黑名单拒绝的命令根本存不进来
    // （set_setting 有 reject_scan_shell，见对抗用例），那条路在
    // av_scan.rs 的单测里单独钉住。
    coordinator
        .set_setting(
            "av_scan_command",
            serde_json::json!(r"C:\hls-av-scan-no-such-engine.exe {file}"),
        )
        .unwrap();
    coordinator
        .set_setting("av_scan_fail_on_threat", serde_json::json!(false))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address}/fixture.bin"),
                resource_kind: crate::ResourceKind::File,
                title: "Fixture".into(),
                filename: "fixture.bin".into(),
                download_dir: download_dir.to_string_lossy().into_owned(),
                request_method: "GET".into(),
                credential_ref: None,
                replay_context_ref: None,
                concurrency: 1,
                checksum: None,
                expected_size: Some(body.len() as u64),
                etag: String::new(),
                last_modified: String::new(),
                ..Default::default()
            },
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let failure = loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        if task.status == "failed" {
            break task;
        }
        assert_ne!(task.status, "completed", "扫描没跑起来却发布了 payload");
        assert!(
            std::time::Instant::now() < deadline,
            "task neither failed nor completed: {task:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    let message = failure.error_message.clone().unwrap_or_default();
    assert!(
        message.contains("av_scan_error"),
        "失败原因应点名扫描没跑起来，实际 {message:?}"
    );
    assert_eq!(
        fs::read(download_dir.join("fixture.bin")).unwrap(),
        b"existing",
        "payload 不得离开下载目录"
    );
    // 发布名不得出现。暂存 payload 与校验失败等其它失败路径一致留在下载
    // 目录里（那是用户数据，不是垃圾）。
    assert!(
        !download_dir.join("fixture_1.bin").exists(),
        "不得出现已发布的产物"
    );
    let _ = fs::remove_dir_all(download_dir);
}

#[test]
fn automatic_http_retry_respects_permanent_failure_and_keeps_queue_running() {
    // 后续任务完成证明前一任务已走完整个收尾/重排队过程，不能只观察短暂的 failed 状态。
    for (status, invalid_range, expected_attempts) in [
        (401, false, 1),
        (403, false, 1),
        (404, false, 1),
        (503, false, 3),
        (206, false, 1),
        (206, true, 1),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                stream = reader.into_inner();
                if request.contains("/next.bin ") {
                    stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/1\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx").unwrap();
                    break;
                }
                if invalid_range {
                    let _ = stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nETag: \"retry-fixture\"\r\nConnection: close\r\n\r\ny");
                } else if status == 206 {
                    let _ = stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/1\r\nContent-Length: 1\r\nETag: \"changed-entity\"\r\nConnection: close\r\n\r\ny");
                } else {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
        });
        let download_dir = std::env::temp_dir().join(format!(
            "hls-http-retry-{}-{status}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting("queue_max_active", serde_json::json!(1))
            .unwrap();
        coordinator
            .set_setting("auto_retry_failed_max", serde_json::json!(2))
            .unwrap();
        for filename in ["expired.bin", "next.bin"] {
            coordinator
                .dispatch_created(TaskSpec {
                    url: format!("http://{address}/{filename}"),
                    filename: filename.into(),
                    resource_kind: crate::ResourceKind::File,
                    download_dir: download_dir.to_string_lossy().into_owned(),
                    expected_size: Some(1),
                    etag: "\"retry-fixture\"".into(),
                    concurrency: 1,
                    ..Default::default()
                })
                .unwrap();
        }
        let notify = coordinator.event_notify().unwrap();
        let mut sequence = coordinator.latest_sequence().unwrap();
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let tasks = coordinator.tasks().unwrap();
            if tasks
                .iter()
                .any(|task| task.task_id == "task-2" && task.status == "completed")
            {
                let failed = tasks.iter().find(|task| task.task_id == "task-1").unwrap();
                assert_eq!(failed.status, "failed", "HTTP {status}");
                assert_eq!(failed.http_status, (status != 206).then_some(status));
                if invalid_range {
                    assert_eq!(failed.error_code.as_deref(), Some("RANGE_UNSUPPORTED"));
                    assert!(failed.error_hint.contains("新建任务"));
                    assert!(!download_dir.join("expired.bin").exists());
                } else if status == 206 {
                    assert_eq!(failed.error_code.as_deref(), Some("RESOURCE_CHANGED"));
                    assert!(failed.error_hint.contains("新建任务"));
                    assert!(!download_dir.join("expired.bin").exists());
                }
                assert_eq!(
                    failed.error_attempt, expected_attempts,
                    "HTTP {status} automatic attempts"
                );
                assert!(failed
                    .available_actions
                    .iter()
                    .any(|action| action == "retry"));
                assert_eq!(fs::read(download_dir.join("next.bin")).unwrap(), b"x");
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "HTTP {status}: queue did not finish: {tasks:?}"
            );
            let guard = notify.0.lock().unwrap();
            let (guard, _) = notify
                .1
                .wait_timeout_while(guard, remaining, |current| *current <= sequence)
                .unwrap();
            sequence = *guard;
        }
        server.join().unwrap();
        coordinator.wait_for_worker_stop("task-2").unwrap();
        fs::remove_dir_all(download_dir).unwrap();
    }
}

#[test]
fn refreshed_url_resumes_paused_task_and_publishes_new_response() {
    let body: &'static [u8] = b"refreshed-signed-url";
    let address = serve_http_fixture(body);

    let download_dir = std::env::temp_dir().join(format!(
        "hls-v7-refresh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        )
        .unwrap();
    coordinator
        .dispatch_created(TaskSpec {
            url: "https://expired.test/signed.bin".into(),
            resource_kind: crate::ResourceKind::File,
            filename: "refreshed.bin".into(),
            download_dir: download_dir.to_string_lossy().into_owned(),
            expected_size: Some(body.len() as u64),
            concurrency: 1,
            headers: BTreeMap::from([
                ("Cookie".into(), "session=original-site".into()),
                ("Authorization".into(), "Bearer original-site".into()),
            ]),
            ..Default::default()
        })
        .unwrap();
    let original = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .cloned()
        .unwrap();
    TaskPaths::for_task("task-1", &original)
        .unwrap()
        .prepare()
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "pause".into(),
        })
        .unwrap();

    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: format!("refresh:http://{address}/signed.bin"),
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        if task.status == "completed" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "refreshed task did not resume: {}",
            task.status
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let refreshed = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .cloned()
        .unwrap();
    let output = TaskPaths::for_task("task-1", &refreshed)
        .unwrap()
        .final_output;
    assert_eq!(fs::read(output).unwrap(), body);
    let _ = fs::remove_dir_all(download_dir);
    assert!(refreshed.credential_ref.is_none());
    assert!(!refreshed.headers.keys().any(|name| {
        name.eq_ignore_ascii_case("cookie") || name.eq_ignore_ascii_case("authorization")
    }));
}

#[test]
fn structured_request_refresh_vaults_cookie_and_never_puts_it_in_snapshot() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .dispatch_created(TaskSpec {
            url: "https://cdn.test/expired.bin".into(),
            resource_kind: crate::ResourceKind::File,
            filename: "signed.bin".into(),
            ..Default::default()
        })
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::RefreshTaskRequest {
            task_id: "task-1".into(),
            url: "https://cdn.test/refreshed.bin?signature=new".into(),
            cookie: "session=private".into(),
            auto_resume: false,
        })
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        CoreEvent::Toast { message, .. } if message.contains("站点凭据")
    )));
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .cloned()
        .unwrap();
    assert_eq!(spec.url, "https://cdn.test/refreshed.bin?signature=new");
    assert!(!spec
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("cookie")));
    let reference = spec.credential_ref.expect("cookie should be vaulted");
    let protected = coordinator.load_credential(&reference).unwrap().unwrap();
    let plain = CredentialVault.unprotect(&protected).unwrap_or(protected);
    assert!(plain.contains("session=private"));
    let snapshot = coordinator.tasks().unwrap().remove(0);
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("session=private"));
    for (url, keep_credential) in [
        ("https://cdn.test/refreshed.bin?signature=next", true),
        ("https://other.test/refreshed.bin", false),
    ] {
        coordinator
            .dispatch(CoreCommand::RefreshTaskRequest {
                task_id: "task-1".into(),
                url: url.into(),
                cookie: String::new(),
                auto_resume: false,
            })
            .unwrap();
        let reference_after_refresh = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .unwrap()
            .credential_ref
            .clone();
        assert_eq!(
            reference_after_refresh.as_deref(),
            keep_credential.then_some(reference.as_str())
        );
    }
}

#[test]
fn default_torrent_creation_publishes_files_without_refetching_metadata() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    for (case, info) in [
        ("single-custom", "d6:lengthi5e4:name4:demo"),
        ("multi-custom", "d5:filesld6:lengthi3e4:pathl7:one.bineed6:lengthi2e4:pathl3:dir7:two.bineee4:name4:demo"),
        ("multi", "d5:filesld6:lengthi3e4:pathl7:one.bineed6:lengthi2e4:pathl3:dir7:two.bineee4:name4:demo"),
        ("single", "d6:lengthi5e4:name4:demo"),
        ("single-selected", "d6:lengthi5e4:name4:demo"),
        ("one-entry-directory", "d5:filesld6:lengthi5e4:pathl7:one.bineee4:name4:demo"),
        ("single-http-url", "d6:lengthi5e4:name14:bt-payload.bin"),
        ("multi-http-url", "d5:filesld6:lengthi3e4:pathl7:one.bineed6:lengthi2e4:pathl3:dir7:two.bineee4:name4:demo"),
    ] {
        let root = std::env::temp_dir().join(format!("hls-default-bt-{case}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let seed = format!("http://{address}/payload");
        let payload = b"abcde";
        let mut torrent = format!("d8:url-list{}:{}4:info{info}12:piece lengthi4e6:pieces40:", seed.len(), seed).into_bytes();
        torrent.extend_from_slice(&crate::crypto_lite::sha1(&payload[..4]));
        torrent.extend_from_slice(&crate::crypto_lite::sha1(&payload[4..]));
        torrent.extend_from_slice(b"ee");
        let metadata_requests = Arc::new(AtomicUsize::new(0));
        let requests = Arc::clone(&metadata_requests);
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let body: &[u8] = if request.contains("/source.torrent") {
                    requests.fetch_add(1, Ordering::SeqCst);
                    &torrent
                } else {
                    payload
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                if !request.starts_with("HEAD ") {
                    let _ = stream.write_all(body);
                }
            }
        });
        let coordinator = CoreCoordinator::new(PersistentCore::open(root.join("tasks.db")).unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting("download_dir", serde_json::json!(root.to_string_lossy()))
            .unwrap();
        // Existing output must survive default policy, including its nested data.
        let url_classified = case.ends_with("http-url");
        let single = case.starts_with("single");
        let custom = case.ends_with("custom");
        if case == "single-custom" {
            fs::write(root.join("demo"), b"unrelated metadata-named file").unwrap();
            fs::write(root.join("chosen.bin"), b"old requested file").unwrap();
            coordinator.set_setting("existing_file_policy", serde_json::json!("overwrite")).unwrap();
        } else if !url_classified {
            fs::create_dir_all(root.join("demo")).unwrap();
            fs::write(root.join("demo/one.bin"), b"existing").unwrap();
        }
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: format!("http://{address}/source.torrent"),
                    resource_kind: if url_classified { ResourceKind::File } else { ResourceKind::Torrent },
                    filename: if custom { if single { "chosen.bin".into() } else { "chosen-dir".into() } } else if case == "single-http-url" { "source.torrent".into() } else if url_classified { String::new() } else { "demo".into() },
                    torrent_selection: if case == "single-selected" {
                        vec![crate::TorrentFileSelection { index: 0, path: "demo".into(), selected: true }]
                    } else {
                        Vec::new()
                    },
                    download_dir: root.to_string_lossy().into_owned(),
                    ..Default::default()
                },
            })
            .unwrap();
        let created = coordinator.tasks().unwrap().remove(0);
        assert_eq!(created.filename, if custom { if single { "chosen.bin" } else { "chosen-dir" } } else if case == "single-http-url" { "bt-payload.bin" } else { "demo" });
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let completed = loop {
            let task = coordinator.tasks().unwrap().remove(0);
            if matches!(task.status.as_str(), "completed" | "failed") {
                break task;
            }
            assert!(Instant::now() < deadline, "{task:?}");
            thread::sleep(Duration::from_millis(10));
        };
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert_eq!(completed.status, "completed", "{completed:?}");
        assert!(!completed.output_missing);
        assert_eq!(completed.playback_ready, single);
        let published = PathBuf::from(&completed.output_path);
        if custom {
            if single {
                assert_eq!(fs::read(root.join("demo")).unwrap(), b"unrelated metadata-named file");
            }
            assert_eq!(published, root.join(if single { "chosen.bin" } else { "chosen-dir" }));
            assert_eq!(completed.filename, if single { "chosen.bin" } else { "chosen-dir" });
        } else if url_classified {
            assert_eq!(published, root.join(if single { "bt-payload.bin" } else { "demo" }));
        } else {
            assert_ne!(published, root.join("demo"));
        }
        if single {
            assert_eq!(fs::read(&published).unwrap(), payload);
        } else {
            assert_eq!(fs::read(published.join("one.bin")).unwrap(), if case.starts_with("multi") { &payload[..3] } else { payload });
            if case.starts_with("multi") {
                assert_eq!(fs::read(published.join("dir/two.bin")).unwrap(), b"de");
            }
        }
        if !url_classified && case != "single-custom" {
            assert_eq!(fs::read(root.join("demo/one.bin")).unwrap(), b"existing");
        }
        // Admission + transfer; publication must reuse the transfer's metadata.
        assert_eq!(metadata_requests.load(Ordering::SeqCst), 2);
        let mut reopened = PersistentCore::open(root.join("tasks.db")).unwrap();
        assert_eq!(
            reopened.snapshot("task-1").unwrap().output_path,
            completed.output_path
        );
        assert_eq!(
            reopened
                .task_spec("task-1")
                .unwrap()
                .torrent_selection
                .len(),
            usize::from(case == "single-selected")
        );
        // Finalization has closed the selection update boundary.
        assert!(coordinator
            .dispatch(CoreCommand::SetTaskTorrentFiles {
                task_id: "task-1".into(),
                selections: Vec::new()
            })
            .is_err());
        reopened.handle(CoreCommand::ClearCompleted).unwrap();
        drop(reopened);
        // A worker may still own the SQLite connection after publishing completion.
        while Arc::strong_count(&coordinator.core) != 1 {
            assert!(Instant::now() < deadline, "torrent worker did not finish");
            thread::yield_now();
        }
        drop(coordinator);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn torrent_transfer_reports_bytes_total_and_rate_before_completion() {
    let root = lifecycle_test_root("bt-live-progress");
    fs::create_dir_all(&root).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seed = format!("http://{address}/payload");
    let payload = vec![b'a'; 512 * 1024];
    let piece_length = payload.len() / 2;
    let mut torrent = format!(
        "d8:url-list{}:{}4:infod6:lengthi{}e4:name8:demo.bin12:piece lengthi{piece_length}e6:pieces40:",
        seed.len(), seed, payload.len()
    ).into_bytes();
    for piece in payload.chunks(piece_length) {
        torrent.extend_from_slice(&crate::crypto_lite::sha1(piece));
    }
    torrent.extend_from_slice(b"ee");
    let source = root.join("source.torrent");
    fs::write(&source, &torrent).unwrap();
    let (release_tx, release_rx) = mpsc::channel();
    let server_payload = payload.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            server_payload.len()
        )
        .unwrap();
        for chunk in server_payload[..piece_length].chunks(64 * 1024) {
            stream.write_all(chunk).unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(250));
        }
        release_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        stream.write_all(&server_payload[piece_length..]).unwrap();
    });
    let coordinator = direct_http_coordinator(&root);
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: source.to_string_lossy().into_owned(),
                resource_kind: ResourceKind::Torrent,
                download_dir: root.to_string_lossy().into_owned(),
                ..Default::default()
            },
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let task = coordinator.tasks().unwrap().remove(0);
        if task.downloaded_bytes > 0 && task.speed_bytes_per_sec > 0 {
            assert_eq!(task.status, "downloading");
            assert_eq!(task.total_bytes, Some(payload.len() as u64));
            assert!(task.downloaded_bytes < payload.len() as u64);
            break;
        }
        assert!(Instant::now() < deadline, "no live BT progress: {task:?}");
        thread::sleep(Duration::from_millis(20));
    }
    release_tx.send(()).unwrap();
    let completed = wait_for_task_status(&coordinator, "completed");
    assert_eq!(fs::read(completed.output_path).unwrap(), payload);
    server.join().unwrap();
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn imported_torrent_preserves_saved_category_directory() {
    let root = std::env::temp_dir().join(format!(
        "hls-import-torrent-category-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let address = serve_http_fixture(b"d4:infod6:lengthi5e4:name9:movie.mkv12:piece lengthi4e6:pieces40:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaee");
    let base = root.join("downloads");
    let saved = base.join(crate::category::category_label("media"));
    let source = root.join("tasks.json");
    fs::write(
        &source,
        serde_json::json!({
            "schema": "hls-downloader.tasks.v1",
            "tasks": [{
                "url": format!("http://{address}/source.torrent"),
                "resource_kind": ResourceKind::Torrent,
                "filename": "movie.mkv",
                "download_dir": saved,
                "scheduled_start_at": "2999-01-01T00:00:00Z",
            }],
        })
        .to_string(),
    )
    .unwrap();
    let coordinator = direct_http_coordinator(&base);
    coordinator
        .set_setting("auto_category_dirs", serde_json::json!(true))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: source.to_string_lossy().into_owned(),
                ..Default::default()
            },
        })
        .unwrap();
    let task = coordinator.tasks().unwrap().remove(0);
    assert_eq!(Path::new(&task.download_dir), saved);
    assert_eq!(
        Path::new(
            &coordinator
                .lock()
                .unwrap()
                .task_spec("task-1")
                .unwrap()
                .download_dir
        ),
        saved
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn torrent_category_routing_uses_resolved_names_once() {
    let root = std::env::temp_dir().join(format!("hls-torrent-categories-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source.torrent");
    fs::write(&source, b"d4:infod6:lengthi5e4:name9:movie.mkv12:piece lengthi4e6:pieces40:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaee").unwrap();
    let base = root.join("downloads");
    for (auto, custom, overrides, expected) in [
        (
            true,
            "",
            false,
            base.join(crate::category::category_label("media")),
        ),
        (false, "", false, base.clone()),
        (true, "", true, base.join("videos")),
        (false, "", true, base.join("videos")),
        (
            true,
            "chosen.zip",
            false,
            base.join(crate::category::category_label("archive")),
        ),
    ] {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        coordinator
            .set_setting("download_dir", serde_json::json!(base.to_string_lossy()))
            .unwrap();
        coordinator
            .set_setting("auto_category_dirs", serde_json::json!(auto))
            .unwrap();
        if overrides {
            coordinator
                .set_setting(
                    "browser_category_dirs",
                    serde_json::json!(serde_json::json!({
                        "media": base.join("videos").to_string_lossy(),
                        "other": base.join("other").to_string_lossy(),
                    })
                    .to_string()),
                )
                .unwrap();
        }
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: source.to_string_lossy().into_owned(),
                    resource_kind: ResourceKind::Torrent,
                    filename: custom.into(),
                    start_paused: true,
                    ..Default::default()
                },
            })
            .unwrap();
        let task = coordinator.tasks().unwrap().remove(0);
        assert_eq!(
            task.filename,
            if custom.is_empty() {
                "movie.mkv"
            } else {
                custom
            }
        );
        assert_eq!(Path::new(&task.download_dir), expected);
        assert_eq!(
            Path::new(
                &coordinator
                    .lock()
                    .unwrap()
                    .task_spec("task-1")
                    .unwrap()
                    .download_dir
            ),
            expected
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_torrent_file_contract_reads_and_persists_selection_before_start() {
    let root = std::env::temp_dir().join(format!("hls-v7-task-torrent-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("multi.torrent");
    let bytes = b"d4:infod5:filesld6:lengthi3e4:pathl7:one.bineed6:lengthi2e4:pathl3:dir7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaee";
    let parsed = crate::torrent_engine::parse_torrent_file(bytes).unwrap();
    assert_eq!(parsed.files.len(), 2);
    fs::write(&source, bytes).unwrap();

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("download_dir", serde_json::json!(root.to_string_lossy()))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: source.to_string_lossy().into_owned(),
                resource_kind: ResourceKind::Torrent,
                start_paused: true,
                filename: String::new(),
                download_dir: root.to_string_lossy().into_owned(),
                ..Default::default()
            },
        })
        .unwrap();

    let listed = coordinator
        .dispatch(CoreCommand::GetTaskTorrentFiles {
            task_id: "task-1".into(),
        })
        .unwrap();
    assert!(listed.iter().any(|event| matches!(
        &event.event,
        CoreEvent::TaskTorrentFiles { files, selections, total_size, .. }
            if files.len() == 2 && selections.len() == 2 && *total_size == 5
    )));
    let selected = vec![
        crate::TorrentFileSelection {
            index: 0,
            path: "one.bin".into(),
            selected: false,
        },
        crate::TorrentFileSelection {
            index: 1,
            path: "dir/two.bin".into(),
            selected: true,
        },
    ];
    coordinator
        .dispatch(CoreCommand::SetTaskTorrentFiles {
            task_id: "task-1".into(),
            selections: selected.clone(),
        })
        .unwrap();
    let stored = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .torrent_selection
        .clone();
    assert_eq!(stored, selected);
    let stored_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .clone();
    let paths = TaskPaths::for_task("task-1", &stored_spec).unwrap();
    let sidecar: Vec<crate::TorrentFileSelection> =
        serde_json::from_slice(&fs::read(paths.torrent_selection).unwrap()).unwrap();
    assert_eq!(sidecar, selected);
    let snapshot = coordinator.tasks().unwrap().remove(0);
    assert_eq!(snapshot.filename, "demo");
    assert_eq!(snapshot.total_bytes, Some(5));
    assert_eq!(snapshot.total_ranges, 2);
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_torrent_selection_update_cancels_requested_file_and_publishes_remaining_file() {
    // 该用例验证的是「取消已选文件并发布剩余文件」的行为，不是延迟。
    // 下面三处等待（BT 对端收到首个请求 / 收到取消消息 / 任务完成）都是等待异步
    // 机制就绪的墙钟预算：在 462 个用例并行、共享 4 核 CPU 时，tracker HTTP +
    // BT 握手偶发超过 5s，导致与本次改动无关的假失败。放宽为充足预算即可消除
    // 调度抖动噪声；真正的挂起仍会在该预算内失败，回归防护语义不变。
    const MACHINERY_WAIT: Duration = Duration::from_secs(60);
    use std::io::Read;
    use std::net::TcpStream;
    use std::sync::mpsc;
    let payload = b"aaaabbbb".to_vec();
    let pieces: Vec<[u8; 20]> = payload.chunks(4).map(crate::crypto_lite::sha1).collect();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hls-v7-live-torrent-selection-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();

    let peer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer_addr = peer_listener.local_addr().unwrap();
    let tracker_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tracker_addr = tracker_listener.local_addr().unwrap();
    let announce = format!("http://{tracker_addr}/announce");

    let mut info = "d5:filesld6:lengthi4e4:pathl7:one.bineed6:lengthi4e4:pathl7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:".to_string()
    .into_bytes();
    for piece in &pieces {
        info.extend_from_slice(piece);
    }
    info.push(b'e');
    let mut torrent = format!("d8:announce{}:{}4:info", announce.len(), announce).into_bytes();
    torrent.extend_from_slice(&info);
    torrent.push(b'e');
    let source = root.join("demo.torrent");
    fs::write(&source, &torrent).unwrap();
    let parsed = crate::torrent_engine::parse_torrent_file(&torrent).unwrap();
    assert_eq!(parsed.files.len(), 2);
    assert_eq!(parsed.length, payload.len() as u64);

    let tracker = std::thread::spawn(move || {
        let (stream, _) = tracker_listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            request.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        assert!(request.starts_with("GET /announce?"));
        let mut stream = reader.into_inner();
        let mut body = b"d5:peers6:".to_vec();
        body.extend_from_slice(&[
            127,
            0,
            0,
            1,
            (peer_addr.port() >> 8) as u8,
            peer_addr.port() as u8,
        ]);
        body.push(b'e');
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    });

    let (requested_tx, requested_rx) = mpsc::channel();
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let peer_payload = payload.clone();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = peer_listener.accept().unwrap();
        let mut handshake = [0u8; 68];
        stream.read_exact(&mut handshake).unwrap();
        stream.write_all(&handshake).unwrap();
        stream.write_all(&1u32.to_be_bytes()).unwrap();
        stream.write_all(&[1u8]).unwrap();

        let read_message = |stream: &mut TcpStream| -> Option<Vec<u8>> {
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).ok()?;
            let length = u32::from_be_bytes(header) as usize;
            let mut message = vec![0u8; length];
            stream.read_exact(&mut message).ok()?;
            Some(message)
        };
        loop {
            let Some(message) = read_message(&mut stream) else {
                return;
            };
            if message.len() < 13 || message[0] != 6 {
                continue;
            }
            let index = u32::from_be_bytes(message[1..5].try_into().unwrap()) as usize;
            if index != 0 {
                continue;
            }
            requested_tx.send(()).unwrap();
            loop {
                let Some(message) = read_message(&mut stream) else {
                    return;
                };
                if message.first() == Some(&8) {
                    let canceled = (
                        u32::from_be_bytes(message[1..5].try_into().unwrap()),
                        u32::from_be_bytes(message[5..9].try_into().unwrap()),
                        u32::from_be_bytes(message[9..13].try_into().unwrap()),
                    );
                    cancel_tx.send(canceled).unwrap();
                    break;
                }
            }
            loop {
                let Some(message) = read_message(&mut stream) else {
                    return;
                };
                if message.len() < 13 || message[0] != 6 {
                    continue;
                }
                let index = u32::from_be_bytes(message[1..5].try_into().unwrap()) as usize;
                let begin = u32::from_be_bytes(message[5..9].try_into().unwrap()) as usize;
                let block = u32::from_be_bytes(message[9..13].try_into().unwrap()) as usize;
                if index != 1 {
                    continue;
                }
                let start = index * 4 + begin;
                let mut response = vec![7u8];
                response.extend_from_slice(&(index as u32).to_be_bytes());
                response.extend_from_slice(&(begin as u32).to_be_bytes());
                response.extend_from_slice(&peer_payload[start..start + block]);
                stream
                    .write_all(&(response.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&response).unwrap();
                return;
            }
        }
    });

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    direct_transport_off_windows(&coordinator);
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("download_dir", serde_json::json!(root.to_string_lossy()))
        .unwrap();
    coordinator
        .set_setting("bt_enable_dht", serde_json::json!(false))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: source.to_string_lossy().into_owned(),
                resource_kind: ResourceKind::Torrent,
                filename: String::new(),
                download_dir: root.to_string_lossy().into_owned(),
                ..Default::default()
            },
        })
        .unwrap();
    let initial = vec![
        crate::TorrentFileSelection {
            index: 0,
            path: "one.bin".into(),
            selected: true,
        },
        crate::TorrentFileSelection {
            index: 1,
            path: "two.bin".into(),
            selected: false,
        },
    ];
    coordinator
        .dispatch(CoreCommand::SetTaskTorrentFiles {
            task_id: "task-1".into(),
            selections: initial,
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "start".into(),
        })
        .unwrap();

    requested_rx.recv_timeout(MACHINERY_WAIT).unwrap();
    let in_flight = coordinator
        .tasks()
        .unwrap()
        .into_iter()
        .find(|task| task.task_id == "task-1")
        .unwrap();
    assert_eq!(in_flight.status, "downloading");
    let updated = vec![
        crate::TorrentFileSelection {
            index: 0,
            path: "one.bin".into(),
            selected: false,
        },
        crate::TorrentFileSelection {
            index: 1,
            path: "two.bin".into(),
            selected: true,
        },
    ];
    let events = coordinator
        .dispatch(CoreCommand::SetTaskTorrentFiles {
            task_id: "task-1".into(),
            selections: updated.clone(),
        })
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        CoreEvent::Toast { message, .. } if message.contains("BT 文件选择已更新")
    )));
    assert_eq!(cancel_rx.recv_timeout(MACHINERY_WAIT).unwrap(), (0, 0, 4));

    let deadline = std::time::Instant::now() + MACHINERY_WAIT;
    let completed = loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        if task.status == "completed" {
            break task;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live torrent task did not complete: {task:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(completed.downloaded_bytes, 4);
    assert_eq!(completed.total_bytes, Some(4));
    let stored_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .cloned()
        .unwrap();
    assert_eq!(stored_spec.torrent_selection, updated);
    let paths = TaskPaths::for_task("task-1", &stored_spec).unwrap();
    let published_dir = PathBuf::from(&completed.output_path);
    assert!(published_dir.starts_with(&root));
    assert_eq!(fs::read(published_dir.join("two.bin")).unwrap(), b"bbbb");
    assert!(!published_dir.join("one.bin").exists());

    let deadline = std::time::Instant::now() + MACHINERY_WAIT;
    while paths.task_dir().exists() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        !paths.task_dir().exists(),
        "completed torrent cache was retained"
    );

    peer.join().unwrap();
    tracker.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_torrent_selection_update_wins_over_task_initialization() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hls-v7-torrent-selection-race-{}-{stamp}",
        std::process::id()
    ));
    let path = root.join("torrent-selection.json");
    let initial = vec![crate::TorrentFileSelection {
        index: 0,
        path: "one.bin".into(),
        selected: false,
    }];
    let updated = vec![crate::TorrentFileSelection {
        index: 0,
        path: "one.bin".into(),
        selected: true,
    }];
    let barrier = Arc::new(std::sync::Barrier::new(3));

    let initialize_path = path.clone();
    let initialize_barrier = Arc::clone(&barrier);
    let initialize = std::thread::spawn(move || {
        initialize_barrier.wait();
        initialize_torrent_selection(&initialize_path, &initial)
    });
    let update_path = path.clone();
    let update_barrier = Arc::clone(&barrier);
    let expected = updated.clone();
    let update = std::thread::spawn(move || {
        update_barrier.wait();
        write_torrent_selection(&update_path, &expected)
    });

    barrier.wait();
    initialize.join().unwrap().unwrap();
    update.join().unwrap().unwrap();
    let stored: Vec<crate::TorrentFileSelection> =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored, updated);
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        1,
        "unique temporary files must be removed after publication"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn checksum_mismatch_does_not_publish_to_download_dir() {
    let body: &'static [u8] = b"v6-checksum-payload";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                    break;
                }
            }
            let mut stream = reader.into_inner();
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len() - 1,
                body.len(),
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    let download_dir = std::env::temp_dir().join(format!(
        "hls-v6-checksum-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address}/payload.bin"),
                filename: "payload.bin".into(),
                download_dir: download_dir.to_string_lossy().into_owned(),
                checksum: Some(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .into(),
                ),
                expected_size: Some(body.len() as u64),
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-1")
            .unwrap();
        if task.status == "failed" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "checksum mismatch did not fail: {task:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!download_dir.join("payload.bin").exists());
    let _ = fs::remove_dir_all(download_dir);
}

#[test]
fn spec_from_url_drops_cross_origin_cookies() {
    let mut template = spec();
    template.url = "https://site.test/watch".into();
    template.headers.insert("Cookie".into(), "sid=1".into());
    template
        .headers
        .insert("Authorization".into(), "Bearer x".into());
    template
        .headers
        .insert("Referer".into(), "https://site.test/watch".into());
    template.credential_ref = Some("cred-page".into());
    let dirs = crate::category::CategoryDirs::default();
    let same = spec_from_url(
        &template,
        "https://site.test/clip.mp4",
        "clip.mp4",
        false,
        &dirs,
    );
    assert_eq!(same.headers.get("Cookie").unwrap(), "sid=1");
    assert_eq!(same.credential_ref.as_deref(), Some("cred-page"));
    let other = spec_from_url(
        &template,
        "https://cdn.test/clip.mp4",
        "clip.mp4",
        false,
        &dirs,
    );
    assert!(!other.headers.contains_key("Cookie"));
    assert!(!other.headers.contains_key("Authorization"));
    assert_eq!(
        other.headers.get("Referer").unwrap(),
        "https://site.test/watch"
    );
    assert!(other.credential_ref.is_none());
}

#[test]
fn player_control_accepts_speed_and_pause_on_null_backend() {
    let _guard = crate::player::TEST_PLAYER_ENV.lock().unwrap();
    std::env::set_var("HLS_V7_PLAYER_NULL", "1");
    player_control("pause").unwrap();
    player_control("resume").unwrap();
    player_control("speed:1.5").unwrap();
    player_control("fullscreen").unwrap();
    player_control("pip").unwrap();
    player_control("unpip").unwrap();
    player_control("vol_up").unwrap();
    player_control("seek_back").unwrap();
    player_control("seek_to:42").unwrap();
    player_control("embed_host:0,48,640,200").unwrap();
    player_control("embed_hwnd:42:0,48,640,200").unwrap();
    player_control("stop").unwrap();
}

#[test]
#[ignore = "requires bundled FFmpeg; decodes a local WAV through Core's private media mount"]
fn player_subtitle_audio_reads_owned_bytes_and_rejects_replaced_sessions() {
    use base64::Engine;
    let _guard = crate::player::TEST_PLAYER_ENV.lock().unwrap();
    std::env::set_var("HLS_V7_PLAYER_NULL", "1");
    let root = lifecycle_test_root("subtitle-audio");
    std::fs::create_dir_all(&root).unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let task_id = lifecycle_task(&coordinator, &root, "completed");
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .cloned()
        .unwrap();
    let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
    let pcm: Vec<u8> = [0xd0u8, 0x07].into_iter().cycle().take(64_000).collect();
    let mut wav = b"RIFF".to_vec();
    wav.extend(64_036u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(16_000u32.to_le_bytes());
    wav.extend(32_000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(64_000u32.to_le_bytes());
    wav.extend(&pcm);
    std::fs::write(&paths.final_output, wav).unwrap();
    coordinator
        .dispatch(CoreCommand::PlayTask {
            task_id: task_id.clone(),
        })
        .unwrap();
    let generation = player_ui_state().lock().unwrap().session_generation;
    let playing = shared_player().unwrap().source_url().unwrap();
    let decoded = coordinator
        .player_subtitle_audio(generation, 0.5, 0.5, || false)
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(decoded["audio"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, pcm[..16_000]);
    assert_eq!(decoded["duration"], 0.5);
    assert_eq!(shared_player().unwrap().source_url().unwrap(), playing);
    assert!(coordinator
        .player_subtitle_audio(generation, 0.0, 1.0, || true)
        .is_err());
    coordinator
        .dispatch(CoreCommand::PlayerControl {
            action: "stop".into(),
        })
        .unwrap();
    assert!(coordinator.validate_subtitle_player(generation).is_err());
    assert!(coordinator
        .player_subtitle_audio(generation, 0.0, 1.0, || false)
        .is_err());
    std::env::remove_var("HLS_V7_PLAYER_NULL");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn player_session_reports_play_pause_speed_and_stop() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let _guard = crate::player::TEST_PLAYER_ENV.lock().unwrap();
    std::env::set_var("HLS_V7_PLAYER_NULL", "1");
    let dir = std::env::temp_dir().join(format!("hls-player-session-{}", std::process::id()));
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .dispatch_created(TaskSpec {
            url: "https://cdn.test/video.mp4".into(),
            resource_kind: crate::ResourceKind::File,
            title: "测试影片".into(),
            filename: "video.mp4".into(),
            download_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .cloned()
        .unwrap();
    // Parallel Core fixtures reuse task-1; restore a distinct identity so their
    // completion workers cannot replace this test's global null player source.
    let task_id = "player-lifecycle-fixture";
    let mut snapshot = coordinator
        .lock()
        .unwrap()
        .snapshot("task-1")
        .cloned()
        .unwrap();
    snapshot.task_id = task_id.into();
    let mut store = crate::CoreStore::in_memory().unwrap();
    store
        .apply_events_and_spec(
            &[EventEnvelope {
                sequence: 1,
                event: CoreEvent::TaskCreated { snapshot },
            }],
            Some(&spec),
        )
        .unwrap();
    drop(coordinator);
    let coordinator = CoreCoordinator::new(PersistentCore::from_store(store).unwrap());
    let paths = TaskPaths::for_task(task_id, &spec).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&paths.final_output, b"local-media").unwrap();
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::UpdateProgress {
            task_id: task_id.into(),
            downloaded_bytes: 11,
            total_bytes: Some(11),
            speed_bytes_per_sec: 0,
            stage: "finished".into(),
            status: "completed".into(),
        })
        .unwrap();

    let play = coordinator
        .dispatch(CoreCommand::PlayTask {
            task_id: task_id.into(),
        })
        .unwrap();
    let server = shared_local_media().unwrap();
    let request = |url: &str| {
        let token = media_token_from_url(url).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .write_all(
                format!("GET /media/{token} HTTP/1.1\r\nRange: bytes=0-3\r\n\r\n").as_bytes(),
            )
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let first_url = shared_player().unwrap().source_url().unwrap();
    let generation = match &play.last().unwrap().event {
        CoreEvent::PlayerSession {
            session_generation, ..
        } => *session_generation,
        _ => unreachable!(),
    };
    coordinator.validate_subtitle_player(generation).unwrap();
    assert!(coordinator
        .player_subtitle_audio(generation + 1, 0.0, 1.0, || false)
        .is_err());
    assert!(request(&first_url).starts_with("HTTP/1.1 206"));
    // A failed replacement must keep the current playback address usable.
    assert!(coordinator
        .dispatch(CoreCommand::PlayTask {
            task_id: "missing".into()
        })
        .is_err());
    assert!(request(&first_url).starts_with("HTTP/1.1 206"));
    assert!(matches!(
        play.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession { active: true, paused: false, title, .. }) if title == "测试影片"
    ));
    let pause = coordinator
        .dispatch(CoreCommand::PlayerControl {
            action: "pause".into(),
        })
        .unwrap();
    assert!(matches!(
        pause.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession { paused: true, status, .. }) if status == "PAUSED"
    ));
    let speed = coordinator
        .dispatch(CoreCommand::PlayerControl {
            action: "speed:1.5".into(),
        })
        .unwrap();
    assert!(matches!(
        speed.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession { speed, .. }) if (*speed - 1.5).abs() < f64::EPSILON
    ));
    let status = coordinator
        .dispatch(CoreCommand::PlayerControl {
            action: "status".into(),
        })
        .unwrap();
    assert!(matches!(
        status.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession { active: true, paused: true, speed, status, .. })
            if (*speed - 1.5).abs() < f64::EPSILON && status == "PAUSED"
    ));
    let generation = match play.last().map(|item| &item.event) {
        Some(CoreEvent::PlayerSession {
            session_generation, ..
        }) => *session_generation,
        _ => panic!("missing playback session"),
    };
    let mut embed = crate::PlayerEmbed {
        session_generation: generation,
        layout_generation: 2,
        parent_hwnd: Some(42),
        x: 0,
        y: 48,
        width: 640,
        height: 200,
    };
    coordinator.player_embed(embed.clone()).unwrap();
    embed.layout_generation = 1;
    assert!(coordinator
        .player_embed(embed.clone())
        .unwrap_err()
        .contains("过期"));
    embed.layout_generation = 3;
    embed.width = 0;
    assert!(coordinator
        .player_embed(embed.clone())
        .unwrap_err()
        .contains("区域"));
    embed.width = 640;
    embed.parent_hwnd = None;
    let detached = coordinator.player_embed(embed.clone()).unwrap();
    assert!(matches!(
        detached.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession {
            active: true,
            layout_generation: 3,
            paused: true,
            ..
        })
    ));
    coordinator
        .dispatch(CoreCommand::PlayTask {
            task_id: task_id.into(),
        })
        .unwrap();
    let second_url = shared_player().unwrap().source_url().unwrap();
    assert!(request(&first_url).starts_with("HTTP/1.1 404"));
    assert!(request(&second_url).starts_with("HTTP/1.1 206"));
    assert!(coordinator
        .player_control(generation, "stop")
        .unwrap_err()
        .contains("会话已改变"));
    assert!(request(&second_url).starts_with("HTTP/1.1 206"));
    embed.layout_generation = 4;
    assert!(coordinator
        .player_embed(embed.clone())
        .unwrap_err()
        .contains("会话已改变"));
    let stop = coordinator
        .dispatch(CoreCommand::PlayerControl {
            action: "stop".into(),
        })
        .unwrap();
    assert!(matches!(
        stop.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession { active: false, status, .. }) if status == "STOPPED"
    ));
    assert!(coordinator.player_embed(embed).is_err());
    assert!(request(&second_url).starts_with("HTTP/1.1 404"));
    assert!(player_ui_state().lock().unwrap().mount_token.is_empty());
    coordinator
        .dispatch(CoreCommand::PlayTask {
            task_id: task_id.into(),
        })
        .unwrap();
    let third_url = shared_player().unwrap().source_url().unwrap();
    finish_published_task(
        &coordinator.core,
        task_id,
        &paths,
        &paths.final_output,
        None,
        &spec,
        false,
    )
    .unwrap();
    assert!(request(&third_url).starts_with("HTTP/1.1 404"));
    assert_eq!(
        shared_player().unwrap().source_url().unwrap(),
        paths.final_output.to_string_lossy()
    );
    stop_task_player(&coordinator, task_id).unwrap();
    assert!(request(&third_url).starts_with("HTTP/1.1 404"));
    let page = coordinator.task_page(None, 128).unwrap();
    assert_eq!(
        page.tasks[0].output_path,
        paths.final_output.to_string_lossy()
    );
    assert!(page.tasks[0].playback_ready);
    assert!(page.tasks[0]
        .available_actions
        .iter()
        .any(|action| action == "open"));
    std::fs::remove_file(&paths.final_output).unwrap();
    let page = coordinator.task_page(None, 128).unwrap();
    assert!(page.tasks[0].output_missing);
    assert!(!page.tasks[0].playback_ready);
    assert!(!page.tasks[0]
        .available_actions
        .iter()
        .any(|action| action == "open"));
    drop(coordinator);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn metalink_body_expands_to_http_task_with_mirrors() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: r#"<metalink><file name="demo.bin"><url priority="1">https://cdn.example.test/demo.bin</url><url priority="2">https://mirror.example.test/demo.bin</url></file></metalink>"#.into(),
                resource_kind: crate::ResourceKind::File,
                filename: String::new(),
                ..Default::default()
            },
        })
        .unwrap();
    let snapshot = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.filename, "demo.bin");
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&snapshot.task_id)
        .cloned()
        .unwrap();
    assert_eq!(spec.url, "https://cdn.example.test/demo.bin");
    assert_eq!(spec.mirrors, vec!["https://mirror.example.test/demo.bin"]);
}

#[test]
fn metalink_query_string_is_not_treated_as_metalink_body() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/demo.bin?<metalink><file name=\"x.bin\"><url>https://evil.test/malware.exe</url></file></metalink>".into(),
                filename: "demo.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
    assert!(spec.url.starts_with("https://cdn.example.test/demo.bin"));
    assert_ne!(spec.url, "https://evil.test/malware.exe");
    assert_eq!(spec.filename, "demo.bin");
    assert!(spec.mirrors.is_empty());
}

#[test]
fn multiline_metalink_paste_still_expands() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "<metalink>\n<file name=\"demo.bin\">\n<url>https://cdn.example.test/demo.bin</url>\n</file>\n</metalink>".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let snapshot = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.filename, "demo.bin");
}

#[test]
fn site_rules_fill_unset_task_speed_and_proxy() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting(
            "site_rules",
            serde_json::json!(r#"[{"host":"example.test","speed_limit_kib":64,"concurrency":2,"proxy":"http://127.0.0.1:9"}]"#),
        )
        .unwrap();
    let spec = apply_site_rules_to_spec(
        &coordinator.core(),
        TaskSpec {
            url: "https://cdn.example.test/a.bin".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(spec.speed_limit_kib, 64);
    assert_eq!(spec.concurrency, 2);
    assert_eq!(spec.proxy, "http://127.0.0.1:9");
}

#[test]
fn created_tasks_preserve_explicit_limits_over_site_defaults() {
    for (rules, url, site_speed, site_workers) in [
        (
            r#"[{"host":"127.0.0.1","speed_limit_kib":10000,"concurrency":12}]"#,
            "http://127.0.0.1/a.bin",
            10000,
            12,
        ),
        (
            r#"[{"host":"127.0.0.1","speed_limit_kib":10000,"concurrency":12}]"#,
            "http://other.test/a.bin",
            0,
            7,
        ),
        ("", "http://127.0.0.1/a.bin", 0, 7),
        (
            r#"[{"host":"127.0.0.1","speed_limit_kib":0,"concurrency":0}]"#,
            "http://127.0.0.1/a.bin",
            0,
            7,
        ),
    ] {
        for (speed, workers) in [(0, 0), (64, 0), (0, 3), (64, 3), (10000, 12)] {
            let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
            coordinator
                .set_settings(BTreeMap::from([
                    ("legal_terms_accepted".into(), serde_json::json!(true)),
                    ("default_concurrency".into(), serde_json::json!(7)),
                    ("site_rules".into(), serde_json::json!(rules)),
                ]))
                .unwrap();
            coordinator
                .dispatch(CoreCommand::CreateTask {
                    spec: TaskSpec {
                        url: url.into(),
                        filename: "a.bin".into(),
                        speed_limit_kib: speed,
                        concurrency: workers,
                        start_paused: true,
                        ..Default::default()
                    },
                })
                .unwrap();
            let expected_speed = if speed == 0 { site_speed } else { speed };
            let expected_workers = if workers == 0 { site_workers } else { workers };
            let spec = coordinator
                .lock()
                .unwrap()
                .task_spec("task-1")
                .cloned()
                .unwrap();
            assert_eq!(spec.speed_limit_kib, expected_speed);
            assert_eq!(spec.concurrency, expected_workers);
            let snapshot = coordinator.tasks().unwrap().remove(0);
            assert_eq!(snapshot.speed_limit_kib, expected_speed);
            assert_eq!(snapshot.max_workers, expected_workers);
            let reapplied = apply_site_rules_to_spec(&coordinator.core(), spec).unwrap();
            assert_eq!(reapplied.speed_limit_kib, expected_speed);
            assert_eq!(reapplied.concurrency, expected_workers);
        }
    }
}

#[test]
fn restored_task_worker_keeps_speed_changed_in_details() {
    let root =
        std::env::temp_dir().join(format!("hls-v7-site-speed-resume-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let db = root.join("tasks.db");
    {
        let coordinator = CoreCoordinator::new(PersistentCore::open(&db).unwrap());
        coordinator
            .set_settings(BTreeMap::from([
                ("legal_terms_accepted".into(), serde_json::json!(true)),
                (
                    "download_dir".into(),
                    serde_json::json!(root.to_string_lossy()),
                ),
                (
                    "site_rules".into(),
                    serde_json::json!(
                        r#"[{"host":"127.0.0.1","speed_limit_kib":10000,"concurrency":12}]"#
                    ),
                ),
            ]))
            .unwrap();
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: "http://127.0.0.1/a.bin".into(),
                    filename: "a.bin".into(),
                    start_paused: true,
                    ..Default::default()
                },
            })
            .unwrap();
        assert_eq!(coordinator.tasks().unwrap()[0].speed_limit_kib, 10000);
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "speed:64".into(),
            })
            .unwrap();
    }
    {
        let coordinator = CoreCoordinator::new(PersistentCore::open(&db).unwrap());
        assert_eq!(coordinator.tasks().unwrap()[0].speed_limit_kib, 64);
        let core = coordinator.core();
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .cloned()
            .unwrap();
        let (spec, _) = hydrate_replay_headers(&core, spec).unwrap();
        let worker_spec = apply_site_rules_to_spec(&core, spec).unwrap();
        assert_eq!(worker_spec.speed_limit_kib, 64);
        assert_eq!(worker_spec.concurrency, 12);
        let throttle = task_throttle_context(&core, "task-1", &worker_spec).unwrap();
        assert_eq!(throttle.task_limit_kib, 64);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn site_rule_credentials_are_vaulted_applied_and_deleted() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting(
            "site_rules",
            serde_json::json!(r#"[{"host":"cdn.test","enabled":true}]"#),
        )
        .unwrap();
    let headers = BTreeMap::from([
        ("Authorization".into(), "Bearer private-token".into()),
        ("X-Site-Key".into(), "private-key".into()),
    ]);

    coordinator
        .set_site_rule_credential("cdn.test", "sid=private", &headers, false)
        .unwrap();
    let raw = coordinator
        .lock()
        .unwrap()
        .store()
        .setting_string("site_rules", "")
        .unwrap();
    assert!(!raw.contains("sid=private"));
    assert!(!raw.contains("private-token"));
    assert!(!raw.contains("private-key"));
    let rule = crate::parse_site_rules(&raw).remove(0);
    assert!(!rule.credential_ref.is_empty());
    let protected = coordinator
        .load_credential(&rule.credential_ref)
        .unwrap()
        .unwrap();
    // DPAPI 只在 Windows 上可用；其他平台的站点规则凭据以明文存入本地库（无系统级加密可用）。
    #[cfg(windows)]
    assert!(!protected.contains("sid=private"));
    #[cfg(not(windows))]
    assert!(protected.contains("sid=private"));

    let ruled = apply_site_rules_to_spec(
        &coordinator.core(),
        TaskSpec {
            url: "https://cdn.test/video.m3u8".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        ruled.credential_ref.as_deref(),
        Some(rule.credential_ref.as_str())
    );
    let (hydrated, _) = hydrate_replay_headers(&coordinator.core(), ruled).unwrap();
    assert_eq!(
        hydrated.headers.get("Cookie").map(String::as_str),
        Some("sid=private")
    );
    assert_eq!(
        hydrated.headers.get("Authorization").map(String::as_str),
        Some("Bearer private-token")
    );

    coordinator
        .set_site_rule_credential("cdn.test", "", &BTreeMap::new(), true)
        .unwrap();
    assert!(coordinator
        .load_credential(&rule.credential_ref)
        .unwrap()
        .is_none());
    let cleared = apply_site_rules_to_spec(
        &coordinator.core(),
        TaskSpec {
            url: "https://cdn.test/video.m3u8".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(cleared.credential_ref.is_none());
}

#[test]
fn core_completion_replay_credentials_fail_explicitly_instead_of_dropping_context() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let anonymous = TaskSpec {
        url: "https://cdn.test/file.bin".into(),
        headers: BTreeMap::from([("Referer".into(), "https://site.test/watch".into())]),
        ..Default::default()
    };
    let (unchanged, replay) =
        hydrate_replay_headers(&coordinator.core(), anonymous.clone()).unwrap();
    assert_eq!(unchanged.headers, anonymous.headers);
    assert!(replay.is_empty());
    assert!(seal_spec_secrets(&coordinator, anonymous)
        .unwrap()
        .credential_ref
        .is_none());
    for (name, blob, expected) in [
        ("missing-private-reference", None, "不存在"),
        (
            "dpapi-private-reference",
            Some("dpapi:invalid-private-secret"),
            "无法解密",
        ),
        (
            "json-private-reference",
            Some(r#"{"cookie":"private-secret""#),
            "上下文已损坏",
        ),
        (
            "array-private-reference",
            Some(r#"["private-secret"]"#),
            "上下文已损坏",
        ),
        (
            "headers-private-reference",
            Some(r#"{"request_headers":"private-secret"}"#),
            "请求头已损坏",
        ),
    ] {
        if let Some(blob) = blob {
            coordinator
                .store_credential(name, blob, "browser_replay")
                .unwrap();
        }
        let spec = TaskSpec {
            url: "https://cdn.test/file.bin".into(),
            credential_ref: Some(name.into()),
            headers: BTreeMap::from([("Cookie".into(), "new-private-cookie".into())]),
            ..Default::default()
        };
        let hydrate_error = hydrate_replay_headers(&coordinator.core(), spec.clone()).unwrap_err();
        let seal_error = seal_spec_secrets(&coordinator, spec).unwrap_err();
        for error in [hydrate_error, seal_error] {
            assert!(error.contains(expected), "{name}: {error}");
            assert!(!error.contains("private"));
        }
        assert_eq!(coordinator.load_credential(name).unwrap().as_deref(), blob);
    }
}

#[test]
fn core_completion_sealing_new_headers_preserves_the_existing_replay_context() {
    for encoded_headers in [false, true] {
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        let mut context = serde_json::json!({
            "cookie": "existing-private-cookie",
            "request_headers": {"Authorization": "Bearer existing-private-token"},
            "request_contexts": {"https://segments.test": {
                "path_contexts": {"/media/": {"request_headers": {"X-CDN-Token": "existing-private-cdn"}}}
            }},
            "request_body": "ZXhpc3RpbmctcHJpdmF0ZS1ib2R5"
        });
        if encoded_headers {
            context["request_headers"] = Value::String(context["request_headers"].to_string());
        }
        coordinator
            .store_credential("existing-context", &context.to_string(), "browser_replay")
            .unwrap();
        let spec = TaskSpec {
            url: "https://cdn.test/file.bin".into(),
            credential_ref: Some("existing-context".into()),
            headers: BTreeMap::from([("X-New-Token".into(), "new-private-token".into())]),
            ..Default::default()
        };
        let (hydrated, _) = hydrate_replay_headers(&coordinator.core(), spec.clone()).unwrap();
        assert_eq!(
            hydrated.headers["Authorization"],
            "Bearer existing-private-token"
        );
        assert_eq!(hydrated.headers["Cookie"], "existing-private-cookie");
        let spec = seal_spec_secrets(&coordinator, spec).unwrap();
        assert!(spec.headers.is_empty());
        let sealed = coordinator
            .load_credential(spec.credential_ref.as_deref().unwrap())
            .unwrap()
            .unwrap();
        let unsealed = CredentialVault.unprotect(&sealed).unwrap();
        let merged: Value = serde_json::from_str(&unsealed).unwrap();
        assert_eq!(merged["cookie"], context["cookie"]);
        assert_eq!(merged["request_body"], context["request_body"]);
        assert_eq!(merged["request_contexts"], context["request_contexts"]);
        assert_eq!(
            merged["request_headers"]["Authorization"],
            "Bearer existing-private-token"
        );
        assert_eq!(
            merged["request_headers"]["X-New-Token"],
            "new-private-token"
        );
        assert_eq!(
            coordinator
                .load_credential("existing-context")
                .unwrap()
                .unwrap(),
            context.to_string()
        );
    }
}

#[test]
fn hydrate_replay_headers_applies_cookie() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .store_credential("cred-1", r#"{"cookie":"a=b"}"#, "browser_replay")
        .unwrap();
    let (spec, json) = hydrate_replay_headers(
        &coordinator.core(),
        TaskSpec {
            url: "https://example.test/a.bin".into(),
            credential_ref: Some("cred-1".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(spec.headers.get("Cookie").unwrap(), "a=b");
    assert!(json.contains("a=b"));
}

#[test]
fn create_task_moves_cookie_header_into_credential_ref() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("Cookie".into(), "sid=9".into());
    headers.insert("Referer".into(), "https://cdn.test/page".into());
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/a.bin".into(),
                headers,
                ..Default::default()
            },
        })
        .unwrap();
    let snapshot = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&snapshot.task_id)
        .cloned()
        .unwrap();
    assert!(!spec.headers.contains_key("Cookie"));
    assert_eq!(
        spec.headers.get("Referer").unwrap(),
        "https://cdn.test/page"
    );
    let blob = coordinator
        .load_credential(spec.credential_ref.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let plain = crate::CredentialVault.unprotect(&blob).unwrap_or(blob);
    assert!(plain.contains("sid=9"));
}

#[test]
fn discover_cast_with_null_timeout_emits_devices_event() {
    std::env::set_var("HLS_V7_CAST_NULL", "1");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let events = coordinator
        .dispatch(CoreCommand::DiscoverCastDevices {
            mode: "cast".into(),
        })
        .unwrap();
    assert!(events.iter().any(|envelope| matches!(
        envelope.event,
        crate::CoreEvent::CastDevices { .. } | crate::CoreEvent::Error { .. }
    )));
}

#[test]
fn configured_tvbox_is_preferred_without_duplicating_scan_result() {
    let configured = "http://192.168.2.11:9978/";
    let mut devices = vec![crate::CastDeviceInfo {
        id: "tvbox:http://192.168.2.11:9978".into(),
        label: "TVBox / 影视盒子".into(),
        location: "http://192.168.2.11:9978".into(),
        control_url: "http://192.168.2.11:9978".into(),
        service_type: "tvbox".into(),
    }];
    prepend_configured_tvbox(&mut devices, configured);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].control_url, "http://192.168.2.11:9978");

    let mut empty = Vec::new();
    prepend_configured_tvbox(&mut empty, configured);
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].id, "tvbox:configured");
    assert_eq!(empty[0].control_url, "http://192.168.2.11:9978");
}

#[test]
fn browser_media_push_is_persisted_and_resolved_through_core() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let request = MediaPushRequest {
        id: "media-push-test".into(),
        push_kind: "tvbox".into(),
        url: "https://cdn.test/video.mp4".into(),
        title: "测试视频".into(),
        status: "pending".into(),
        message: "等待选择设备".into(),
        location: String::new(),
        credential_ref: None,
        mime_type: String::new(),
        created_at_ms: 42,
    };
    let events = coordinator
        .dispatch(CoreCommand::RequestMediaPush {
            request: request.clone(),
        })
        .unwrap();
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::MediaPushRequested { request } if request.id == "media-push-test"
    )));

    let resolved = coordinator
        .dispatch(CoreCommand::ResolveMediaPush {
            request_id: request.id.clone(),
            status: "done".into(),
            message: "已推送到客厅电视".into(),
            location: "http://192.168.1.8/media/video".into(),
        })
        .unwrap();
    assert!(resolved.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::MediaPushResolved { request }
            if request.status == "done" && request.message.contains("客厅电视")
    )));
    let record = coordinator
        .load_handoffs()
        .unwrap()
        .into_iter()
        .find_map(|encoded| serde_json::from_str::<MediaPushRequest>(&encoded).ok())
        .unwrap();
    assert_eq!(record.status, "done");
    assert!(record.location.starts_with("http://192.168.1.8/"));
    assert!(coordinator
        .dispatch(CoreCommand::ResolveMediaPush {
            request_id: request.id,
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
        })
        .is_err());
}

#[test]
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
        credential_ref: None,
        mime_type: String::new(),
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
        credential_ref: None,
        mime_type: String::new(),
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
        credential_ref: None,
        mime_type: String::new(),
        created_at_ms: 102,
    };
    assert!(coordinator
        .dispatch(CoreCommand::RequestMediaPush { request })
        .is_err());
}

#[test]
fn local_url_shortcut_expands_to_http_task() {
    let dir = std::env::temp_dir().join(format!("v6-url-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("clip.url");
    fs::write(&path, "[InternetShortcut]\nURL=https://cdn.test/clip.mp4\n").unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: path.to_string_lossy().into_owned(),
                filename: "from-shortcut".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let snapshot = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&snapshot.task_id)
        .cloned()
        .unwrap();
    assert_eq!(spec.url, "https://cdn.test/clip.mp4");
    assert_eq!(spec.resource_kind, ResourceKind::File);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn coordinator_accept_handoff_creates_task_and_rejects_escaped_dir() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.test/a.bin".into(),
                handoff_id: "handoff-coord".into(),
                filename: "a.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert!(coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-coord".into(),
            filename: "a.bin".into(),
            download_dir: "../escape".into(),
            allow_duplicate: false,
            trusted_ui: false,
            start_paused: false,
            queue_id: String::new(),
            spec_override: None,
        })
        .is_err());
    let events = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-coord".into(),
            filename: "a.bin".into(),
            download_dir: String::new(),
            allow_duplicate: false,
            trusted_ui: false,
            start_paused: false,
            queue_id: String::new(),
            spec_override: None,
        })
        .unwrap();
    assert!(events
        .iter()
        .any(|envelope| matches!(envelope.event, crate::CoreEvent::HandoffResolved { .. })));
    assert_eq!(coordinator.tasks().unwrap().len(), 1);
}

#[test]
fn only_explicit_desktop_copy_bypasses_handoff_deduplication() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let url = "https://cdn.test/same.bin";
    coordinator
        .dispatch_created(TaskSpec {
            url: url.into(),
            filename: "same.bin".into(),
            start_paused: true,
            ..Default::default()
        })
        .unwrap();

    for (id, trusted_ui) in [("browser", false), ("desktop-copy", true)] {
        coordinator
            .dispatch(CoreCommand::OfferResource {
                offer: crate::ResourceOffer {
                    handoff_id: id.into(),
                    url: url.into(),
                    filename: "same.bin".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let events = coordinator
            .dispatch(CoreCommand::AcceptHandoff {
                handoff_id: id.into(),
                filename: "same (2).bin".into(),
                download_dir: String::new(),
                allow_duplicate: true,
                trusted_ui,
                start_paused: false,
                queue_id: String::new(),
                spec_override: None,
            })
            .unwrap();
        let copy = events.iter().find_map(|event| match &event.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        });
        if trusted_ui {
            let copy = copy.expect("explicit desktop copy creates a new task");
            let guard = coordinator.lock().unwrap();
            let spec = guard.task_spec(&copy.task_id).unwrap();
            assert!(spec.preserve_existing_output);
            assert_eq!(spec.filename, "same (2).bin");
        } else {
            assert!(
                copy.is_none(),
                "browser cannot bypass ordinary deduplication"
            );
        }
    }
    assert_eq!(coordinator.tasks().unwrap().len(), 2);
}

#[test]
fn coordinator_accept_handoff_carries_local_source_into_task_spec() {
    let contents = b"accepted browser payload";
    let (root, source, spec) = local_source_fixture("handoff", contents);
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("download_dir", serde_json::json!(spec.download_dir.clone()))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: spec.url.clone(),
                resource_kind: crate::ResourceKind::File,
                local_source_path: spec.local_source_path.clone(),
                handoff_id: "handoff-local-source".into(),
                filename: spec.filename.clone(),
                size: contents.len() as u64,
                ..Default::default()
            },
        })
        .unwrap();

    let events = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-local-source".into(),
            filename: spec.filename.clone(),
            download_dir: String::new(),
            allow_duplicate: false,
            trusted_ui: false,
            start_paused: false,
            queue_id: String::new(),
            spec_override: None,
        })
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let task_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .cloned()
        .unwrap();
    coordinator.wait_for_worker_stop(&task_id).unwrap();

    assert_eq!(task_spec.local_source_path, spec.local_source_path);
    assert!(source.is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn expand_create_preserves_detected_adaptive_kind_without_extension() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    for resource_kind in [crate::ResourceKind::Hls, crate::ResourceKind::Dash] {
        let spec = TaskSpec {
            url: "https://cdn.test/opaque?token=media".into(),
            resource_kind,
            filename: "video.mp4".into(),
            ..Default::default()
        };
        let expanded = coordinator.expand_create(spec).unwrap();
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].resource_kind, resource_kind);
    }
}

#[test]
fn expired_presenter_lease_transfers_pending_row_to_fallback() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .lock()
        .unwrap()
        .store_mut()
        .save_handoff(
            "handoff-ui",
            &serde_json::json!({
                "id": "handoff-ui",
                "status": "pending",
                "presentation": "presented",
                "presentation_owner": "crashed-presenter",
                "presentation_lease_until_ms": 1,
                "created_at_ms": 1
            })
            .to_string(),
            "pending",
            None,
            1,
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.test/a.bin".into(),
                handoff_id: "handoff-ui".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert!(coordinator
        .lock()
        .unwrap()
        .pending_handoff("handoff-ui")
        .is_some());
    let events = coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-ui".into(),
            ok: false,
            presenter_id: String::new(),
        })
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.event,
        crate::CoreEvent::UiShow { ref surface } if surface == "main"
    )));
    let json = coordinator
        .lock()
        .unwrap()
        .store()
        .load_handoffs()
        .unwrap()
        .join("\n");
    assert!(json.contains("\"presentation\":\"fallback\""));
    assert!(json.contains("\"presentation_owner\":\"workbench\""));
    assert!(json.contains("\"status\":\"pending\""));
    assert!(coordinator
        .lock()
        .unwrap()
        .pending_handoff("handoff-ui")
        .is_some());
}

#[test]
fn presenter_lease_blocks_workbench_and_owner_can_release() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .lock()
        .unwrap()
        .store_mut()
        .save_handoff(
            "handoff-shown",
            &serde_json::json!({
                "id": "handoff-shown",
                "status": "pending",
                "presentation": "queued",
                "created_at_ms": 1
            })
            .to_string(),
            "pending",
            None,
            1,
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.test/b.bin".into(),
                handoff_id: "handoff-shown".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let before_claim = coordinator.lock().unwrap().store().load_handoffs().unwrap();
    for presenter_id in ["", " \t "] {
        assert!(coordinator
            .dispatch(CoreCommand::PresentHandoff {
                handoff_id: "handoff-shown".into(),
                ok: true,
                presenter_id: presenter_id.into(),
            })
            .is_err());
    }
    assert_eq!(
        coordinator.lock().unwrap().store().load_handoffs().unwrap(),
        before_claim
    );
    coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-shown".into(),
            ok: true,
            presenter_id: "presenter-test".into(),
        })
        .unwrap();
    let claimed = coordinator
        .lock()
        .unwrap()
        .store()
        .load_handoffs()
        .unwrap()
        .join("\n");
    assert!(claimed.contains("\"presentation\":\"presenting\""));
    assert!(claimed.contains("\"presentation_owner\":\"presenter-test\""));
    assert!(coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-shown".into(),
            ok: true,
            presenter_id: "presenter-other".into(),
        })
        .is_err());
    assert!(coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-shown".into(),
            ok: false,
            presenter_id: String::new(),
        })
        .is_err());
    coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-shown".into(),
            ok: true,
            presenter_id: "presenter-test".into(),
        })
        .unwrap();
    let json = coordinator
        .lock()
        .unwrap()
        .store()
        .load_handoffs()
        .unwrap()
        .join("\n");
    assert!(json.contains("\"presentation\":\"presented\"") || json.contains("presented"));
    assert!(json.contains("pending"));
    assert!(!json.contains("\"status\":\"failed\""));
    assert!(coordinator
        .lock()
        .unwrap()
        .pending_handoff("handoff-shown")
        .is_some());
    coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-shown".into(),
            ok: false,
            presenter_id: "presenter-test".into(),
        })
        .unwrap();
    let released = coordinator
        .lock()
        .unwrap()
        .store()
        .load_handoffs()
        .unwrap()
        .join("\n");
    assert!(released.contains("\"presentation\":\"fallback\""));
}

#[test]
fn rejected_handoff_can_persist_source_site_kind_suppression() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let offer = crate::ResourceOffer {
        url: "https://cdn.test/movie.m3u8".into(),
        resource_kind: ResourceKind::Hls,
        source_page_url: "https://Video.Example.Test/watch/42".into(),
        handoff_id: "handoff-suppress".into(),
        filename: "movie.m3u8".into(),
        ..Default::default()
    };
    let encoded = serde_json::json!({
        "id": "handoff-suppress",
        "offer": offer,
        "filename": "movie.m3u8",
        "title": "Movie",
        "mime_type": "application/vnd.apple.mpegurl",
        "size": 0,
        "status": "pending",
        "presentation": "presented",
        "task_id": null,
        "created_at_ms": 1,
        "request_id": ""
    })
    .to_string();
    coordinator
        .save_handoff("handoff-suppress", &encoded, "pending", None, 1)
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource { offer })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::RejectHandoff {
            handoff_id: "handoff-suppress".into(),
            suppress_site_kind: true,
        })
        .unwrap();
    let rows = coordinator.load_handoffs().unwrap();
    assert!(rows.iter().any(|row| {
        row.contains("\"host\":\"video.example.test\"") && row.contains("\"kind\":\"hls\"")
    }));
}

#[test]
fn deleting_task_files_preserves_other_same_named_outputs() {
    let dir = std::env::temp_dir().join(format!(
        "hls-delete-owned-{}-{}",
        std::process::id(),
        handoff_now_ms()
    ));
    fs::create_dir_all(&dir).unwrap();
    let spec = TaskSpec {
        url: "https://example.test/clip.bin".into(),
        filename: "clip.bin".into(),
        download_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut core = PersistentCore::in_memory().unwrap();
    core.handle(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    let task_id = core.tasks()[0].task_id.clone();
    let coordinator = CoreCoordinator::new(core);
    let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
    paths.prepare().unwrap();
    fs::write(&paths.final_output, b"existing user file").unwrap();
    coordinator.delete_task_files(&task_id).unwrap();
    assert_eq!(
        fs::read(&paths.final_output).unwrap(),
        b"existing user file"
    );
    paths.prepare().unwrap();
    fs::write(&paths.output, b"new download").unwrap();
    let renamed = paths.publish_with("rename", false).unwrap();
    remember_published(&paths, &renamed);
    assert_ne!(renamed, paths.final_output);
    assert_eq!(resolved_snapshot_output(None, &paths), renamed);
    coordinator.delete_task_files(&task_id).unwrap();
    assert!(!renamed.exists());
    assert_eq!(
        fs::read(&paths.final_output).unwrap(),
        b"existing user file"
    );
    paths.prepare().unwrap();
    remember_published(&paths, &renamed);
    assert_eq!(resolved_snapshot_output(None, &paths), renamed);
    coordinator.delete_task_files(&task_id).unwrap();
    assert_eq!(
        fs::read(&paths.final_output).unwrap(),
        b"existing user file"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn published_path_outside_download_root_is_ignored() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|item| item.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("hls-pub-root-{}-{stamp}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let spec = TaskSpec {
        filename: "clip.bin".into(),
        download_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let paths = TaskPaths::for_task("task-pub", &spec).unwrap();
    paths.prepare().unwrap();
    let outside =
        std::env::temp_dir().join(format!("hls-pub-escape-{}-{stamp}", std::process::id()));
    fs::write(&outside, b"secret").unwrap();
    fs::write(
        paths.task_dir().join("published.path"),
        outside.to_string_lossy().as_bytes(),
    )
    .unwrap();
    let resolved = resolved_snapshot_output(None, &paths);
    assert_ne!(resolved, outside);
    let outside_canon = logical_canonical(&outside);
    assert_ne!(logical_canonical(&resolved), outside_canon);
    let _ = fs::remove_file(&outside);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn completed_snapshot_keeps_its_saved_output_when_marker_is_unreadable_or_removed() {
    let (core, paths, published, root) = published_task_fixture("marker-cleanup-race");
    let renamed = root.join("movie_1.mp4");
    fs::rename(&published, &renamed).unwrap();
    let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
    let spec = task_spec_for(&core, &task_id);
    finish_published_task(&core, &task_id, &paths, &renamed, None, &spec, false).unwrap();
    let snapshot = core.lock().unwrap().snapshot(&task_id).unwrap().clone();
    let marker = paths.task_dir().join("published.path");
    // 无效 UTF-8 模拟读取 marker 失败，不依靠随机删除时序复现。
    fs::write(&marker, [0xff]).unwrap();
    assert_eq!(resolved_snapshot_output(Some(&snapshot), &paths), renamed);
    fs::remove_file(&marker).unwrap();
    fs::remove_file(&renamed).unwrap();
    fs::write(&paths.final_output, b"another file at the original name").unwrap();
    assert_eq!(resolved_snapshot_output(Some(&snapshot), &paths), renamed);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legal_gate_blocks_create_and_start_until_accepted() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    assert!(
        coordinator.settings().unwrap().legal_accepted,
        "fresh in-memory CoreSettings must default legal_accepted to true"
    );
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(false))
        .unwrap();
    let create = coordinator.dispatch(CoreCommand::CreateTask {
        spec: TaskSpec {
            url: "https://cdn.test/gated.bin".into(),
            filename: "gated.bin".into(),
            ..Default::default()
        },
    });
    assert!(
        create.unwrap_err().contains("legal"),
        "CreateTask must not run before the legal gate"
    );
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/gated.bin".into(),
                filename: "gated.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(false))
        .unwrap();
    let start = coordinator.dispatch(CoreCommand::TaskAction {
        task_id: "task-1".into(),
        action: "start".into(),
    });
    assert!(
        start.unwrap_err().contains("legal"),
        "start must not run after the legal flag is cleared"
    );
    let resume = coordinator.dispatch(CoreCommand::TaskAction {
        task_id: "task-1".into(),
        action: "resume".into(),
    });
    assert!(resume.unwrap_err().contains("legal"));
    let retry = coordinator.dispatch(CoreCommand::TaskAction {
        task_id: "task-1".into(),
        action: "retry".into(),
    });
    assert!(retry.unwrap_err().contains("legal"));
}

#[test]
fn create_task_rejects_javascript_url() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let error = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "javascript:alert(1)".into(),
                filename: "x.bin".into(),
                ..Default::default()
            },
        })
        .unwrap_err();
    assert!(error.contains("协议") || error.contains("换行") || error.contains("不受支持"));
    let file_url = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "file:///C:/Windows/win.ini".into(),
                filename: "x.bin".into(),
                ..Default::default()
            },
        })
        .unwrap_err();
    assert!(file_url.contains("协议") || file_url.contains("不受支持"));
    assert!(coordinator
        .set_setting("proxy_url", serde_json::json!("http://127.0.0.1\r\nX: 1"))
        .is_err());
    assert!(coordinator
        .set_setting("download_dir", serde_json::json!("../escape"))
        .is_err());
}

#[test]
fn create_task_coerces_unsafe_method_and_etag() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/a.bin".into(),
                filename: "a.bin".into(),
                request_method: "CONNECT\r\nHost: evil".into(),
                etag: "\"ok\"\r\nX: 1".into(),
                last_modified: "Wed, 01 Jan 2020 00:00:00 GMT\nInjected".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let snapshot = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&snapshot.task_id)
        .cloned()
        .unwrap();
    assert_eq!(spec.request_method, "GET");
    assert!(spec.etag.is_empty());
    assert!(spec.last_modified.is_empty());
    let post = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/b.bin".into(),
                filename: "b.bin".into(),
                request_method: "POST".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let post_id = post
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let post_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&post_id)
        .cloned()
        .unwrap();
    assert_eq!(post_spec.request_method, "POST");
}

#[test]
fn adversarial_rejects_script_data_blob_and_refresh() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    for url in [
        "  DATA:text/plain,hi",
        "blob:https://cdn.test/1",
        "vbscript:msgbox(1)",
        "http://cdn.test/a.bin\r\nX: 1",
    ] {
        let error = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: url.into(),
                    filename: "x.bin".into(),
                    start_paused: true,
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(
            error.contains("协议")
                || error.contains("不受支持")
                || error.contains("换行")
                || error.contains("控制"),
            "url {url:?} leaked through: {error}"
        );
    }
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/ok.bin".into(),
                filename: "ok.bin".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let refresh = coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: "task-1".into(),
            action: "refresh:javascript:alert(1)".into(),
        })
        .unwrap_err();
    assert!(
        refresh.contains("不受支持") || refresh.contains("协议"),
        "refresh leaked: {refresh}"
    );
}

#[test]
fn adversarial_rejects_helper_and_scan_interpreters() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    assert!(coordinator
        .set_setting(
            "av_scan_command",
            serde_json::json!("powershell -Command Start-Process calc")
        )
        .is_err());
    assert!(coordinator
        .set_setting("av_scan_command", serde_json::json!("cmd /c calc {file}"))
        .is_err());
    assert!(coordinator
        .set_setting(
            "av_scan_command",
            serde_json::json!(r"C:\Windows\System32\mshta.exe {file}")
        )
        .is_err());
    assert!(coordinator
        .set_setting(
            "av_scan_command",
            serde_json::json!(r"C:\Windows\explorer.exe {file}")
        )
        .is_err());
    assert!(coordinator
        .set_setting(
            "av_scan_command",
            serde_json::json!("MpCmdRun.exe -Scan -File {file}")
        )
        .is_ok());
    assert!(coordinator
        .set_setting(
            "ffmpeg_path",
            serde_json::json!(r"C:\Windows\System32\cmd.exe")
        )
        .is_err());
    assert!(coordinator
        .set_setting("ffmpeg_path", serde_json::json!(r"..\ffmpeg.exe"))
        .is_err());
    assert!(coordinator
        .set_setting(
            "ffmpeg_path",
            serde_json::json!(r"C:\tools\ffmpeg.exe & calc.exe")
        )
        .is_err());
    assert!(coordinator
        .set_setting("default_user_agent", serde_json::json!("UA\r\nCookie: x"))
        .is_err());
    assert!(coordinator
        .set_setting("torrent_watch_dir", serde_json::json!(r"..\Windows"))
        .is_err());
    assert!(coordinator
        .set_setting("proxy_url", serde_json::json!("javascript:alert(1)"))
        .is_err());
    assert!(coordinator
        .set_setting("tvbox_endpoint", serde_json::json!("javascript:alert(1)"))
        .is_err());
    assert!(coordinator
        .set_setting(
            "tvbox_endpoint",
            serde_json::json!("https://8.8.8.8/action")
        )
        .is_err());
}

#[test]
fn adversarial_rejects_bom_null_ms_schemes_and_reserved_names() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    for url in [
        "\u{feff}javascript:alert(1)",
        "https://cdn.test/a.bin\0.gif",
        "ms-msdt:foo",
        "shell:AppsFolder",
        "search-ms:query=x",
        "about:blank",
        "view-source:https://cdn.test/a.bin",
        "file:C:/Windows/win.ini",
        "\\\\.\\pipe\\HLSDownloader.v7",
    ] {
        let error = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: url.into(),
                    filename: "x.bin".into(),
                    start_paused: true,
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(
            error.contains("协议")
                || error.contains("不受支持")
                || error.contains("控制")
                || error.contains("链接"),
            "url {url:?} leaked through: {error}"
        );
    }
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/con.bin".into(),
                filename: "CON".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
    let paths = TaskPaths::for_task(&id, &spec).unwrap();
    assert_eq!(
        paths
            .final_output
            .file_name()
            .and_then(|name| name.to_str()),
        Some("_CON")
    );
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/conin.bin".into(),
                filename: "CONIN$.txt".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
    let paths = TaskPaths::for_task(&id, &spec).unwrap();
    assert_eq!(
        paths
            .final_output
            .file_name()
            .and_then(|name| name.to_str()),
        Some("_CONIN$.txt")
    );
}

#[test]
fn adversarial_drops_injected_headers_and_non_http_mirrors() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("X-Injected\r\nX".into(), "1".into());
    headers.insert("X-Ok".into(), "safe".into());
    headers.insert("X-Bad".into(), "a\nb".into());
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/a.bin".into(),
                filename: "a.bin".into(),
                headers,
                mirrors: vec![
                    "javascript:alert(1)".into(),
                    "file:///C:/Windows/win.ini".into(),
                    "https://mirror.test/a.bin".into(),
                ],
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
    assert!(!spec
        .headers
        .keys()
        .any(|key| key.contains('\r') || key.contains('\n')));
    assert_eq!(spec.headers.get("X-Ok").map(String::as_str), Some("safe"));
    assert!(!spec.headers.contains_key("X-Bad"));
    let normalized = crate::mirrors::normalize_mirror_urls(&spec.url, &spec.mirrors);
    assert_eq!(normalized, vec!["https://mirror.test/a.bin".to_string()]);
    assert_eq!(spec.mirrors, vec!["https://mirror.test/a.bin".to_string()]);
}

#[test]
fn create_task_rejects_client_dir_outside_download_root() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    coordinator
        .set_setting("download_dir", serde_json::json!("downloads"))
        .unwrap();
    // 当前平台上的绝对目录（盘符路径在非 Windows 平台只是一个相对目录名，会被合法地放进根目录）。
    #[cfg(windows)]
    let outside = r"C:\Windows";
    #[cfg(not(windows))]
    let outside = "/etc";
    let escaped = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/a.bin".into(),
                filename: "a.bin".into(),
                download_dir: outside.into(),
                ..Default::default()
            },
        })
        .unwrap_err();
    assert!(
        escaped.contains("下载目录") || escaped.contains("根目录"),
        "absolute escape leaked: {escaped}"
    );
    coordinator
        .set_setting(
            "site_rules",
            serde_json::json!(r#"[{"host":"cdn.test","download_dir":"site-cache"}]"#),
        )
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/b.bin".into(),
                filename: "b.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            crate::CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator.lock().unwrap().task_spec(&id).cloned().unwrap();
    assert_eq!(spec.download_dir, "site-cache");
}

#[test]
fn coordinator_exports_normalized_tasks_through_the_core_event_contract() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .dispatch_created(TaskSpec {
            url: "https://cdn.test/archive.zip".into(),
            resource_kind: crate::ResourceKind::File,
            title: "Archive".into(),
            filename: "archive.zip".into(),
            ..Default::default()
        })
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::ExportTasks {
            task_ids: vec!["task-1".into()],
            format: "json".into(),
        })
        .unwrap();
    let (data, count) = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskExport {
                format,
                data,
                task_count,
            } if format == "json" => Some((data, *task_count)),
            _ => None,
        })
        .expect("task export event");
    assert_eq!(count, 1);
    let document: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(document["schema"], "hls-downloader.tasks.v1");
    assert_eq!(document["tasks"][0]["id"], "task-1");
    assert_eq!(document["tasks"][0]["filename"], "archive.zip");
}

#[test]
fn coordinator_reimports_its_exported_json_as_new_tasks() {
    let source = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    source
        .dispatch_created(TaskSpec {
            url: "https://cdn.test/archive.zip".into(),
            resource_kind: crate::ResourceKind::File,
            title: "Archive".into(),
            filename: "archive.zip".into(),
            speed_limit_kib: 256,
            download_dir: std::env::temp_dir()
                .join("hls-import-old-computer")
                .to_string_lossy()
                .into_owned(),
            scheduled_start_at: "2999-01-01T00:00:00Z".into(),
            ..Default::default()
        })
        .unwrap();
    let events = source
        .dispatch(CoreCommand::ExportTasks {
            task_ids: Vec::new(),
            format: "json".into(),
        })
        .unwrap();
    let data = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskExport { data, .. } => Some(data),
            _ => None,
        })
        .unwrap();
    let path = std::env::temp_dir().join(format!("hls-v7-task-import-{}.json", std::process::id()));
    fs::write(&path, data.as_bytes()).unwrap();

    let target = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    target
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let download_dir = std::env::temp_dir().join("hls-import-current-computer");
    target
        .set_setting(
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        )
        .unwrap();
    let imported = target
        .dispatch(CoreCommand::ImportPaths {
            paths: vec![path.to_string_lossy().into_owned()],
        })
        .unwrap();
    let snapshot = imported
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.filename, "archive.zip");
    assert_eq!(snapshot.speed_limit_kib, 256);
    assert_eq!(snapshot.download_dir, download_dir.to_string_lossy());
    assert_eq!(snapshot.scheduled_start_at, "2999-01-01T00:00:00Z");
    let _ = fs::remove_file(path);
}

#[test]
fn import_paths_continues_after_a_bad_file_and_reports_partial_success() {
    let root = std::env::temp_dir().join(format!("hls-v7-batch-import-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let paths: Vec<_> = ["first.json", "invalid.json", "last.json"]
        .iter()
        .map(|name| root.join(name))
        .collect();
    for (index, path) in paths.iter().enumerate() {
        let data = if index == 1 {
            "invalid JSON".into()
        } else {
            serde_json::json!({"schema":"hls-downloader.tasks.v1", "tasks":[{
                "url":format!("https://cdn.test/batch-{index}.bin"),
                "scheduled_start_at":"2999-01-01T00:00:00Z"
            }]})
            .to_string()
        };
        fs::write(path, data).unwrap();
    }
    let target = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    target
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let events = target
        .dispatch(CoreCommand::ImportPaths {
            paths: paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        })
        .unwrap();
    assert_eq!(target.tasks().unwrap().len(), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, CoreEvent::TaskCreated { .. }))
            .count(),
        2
    );
    assert!(events.iter().any(|event| matches!(&event.event,
        CoreEvent::Error { code, message } if code == "import_partial_failure"
            && message.contains("2 项任务") && message.contains("第 2 项")
    )));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_batch_urls_are_classified_independently_by_core() {
    let root = std::env::temp_dir().join(format!("hls-auto-batch-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let target = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    target
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    target
        .set_setting("download_dir", serde_json::json!(root))
        .unwrap();
    target
        .set_setting("temp_dir", serde_json::json!(root.join("cache")))
        .unwrap();
    for (url, expected) in [
        ("https://cdn.test/file.zip", crate::ResourceKind::File),
        (
            "https://cdn.test/MOVIE.M3U8?token=signed#player",
            crate::ResourceKind::Hls,
        ),
        (
            "https://cdn.test/live/stream.m3u8?token=signed",
            crate::ResourceKind::Live,
        ),
        (
            "https://cdn.test/movie.mpd?token=signed",
            crate::ResourceKind::Dash,
        ),
        ("ftp://ftp.test/file.zip", crate::ResourceKind::Ftp),
        ("sftp://sftp.test/file.zip", crate::ResourceKind::Sftp),
    ] {
        target
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: url.into(),
                    resource_kind: crate::ResourceKind::File,
                    start_paused: true,
                    ..Default::default()
                },
            })
            .unwrap();
        let tasks = target.tasks().unwrap();
        let task = tasks.iter().find(|task| task.url == url).unwrap();
        assert_eq!(task.resource_kind, expected, "{url}");
        assert_eq!(task.status, "paused", "{url}");
    }
    assert_eq!(target.tasks().unwrap().len(), 6);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_list_import_uses_default_for_a_queue_missing_on_this_computer() {
    let path = std::env::temp_dir().join(format!("hls-import-queue-{}.json", std::process::id()));
    fs::write(
        &path,
        serde_json::json!({"schema":"hls-downloader.tasks.v1", "tasks":[{
            "url":"https://cdn.test/queued.bin", "queue_id":"another-computers-queue",
            "scheduled_start_at":"2999-01-01T00:00:00Z"
        }]})
        .to_string(),
    )
    .unwrap();
    let target = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    target
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let events = target
        .dispatch(CoreCommand::ImportPaths {
            paths: vec![path.to_string_lossy().into_owned()],
        })
        .unwrap();
    let _ = fs::remove_file(path);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.event, CoreEvent::Error { .. })),
        "{events:?}"
    );
    let tasks = target.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].queue_id, crate::DEFAULT_QUEUE_ID);
}

#[test]
fn queue_profile_contract_validates_schedule_and_identity() {
    let valid = serde_json::json!([
        {
            "id": "default",
            "name": "默认队列",
            "enabled": true,
            "priority": 0,
            "max_active": 3,
            "speed_limit_kib": 0,
            "schedule_enabled": false,
            "start_time": "00:00",
            "stop_time": "23:59",
            "active_days": "1,2,3,4,5,6,7",
            "completion_action": "none"
        },
        {
            "id": "night-media",
            "name": "夜间媒体",
            "enabled": true,
            "priority": 20,
            "max_active": 2,
            "speed_limit_kib": 4096,
            "schedule_enabled": true,
            "start_time": "23:00",
            "stop_time": "07:00",
            "active_days": "1,2,3,4,5",
            "completion_action": "sleep"
        }
    ]);
    assert!(validate_queue_profiles(&valid).is_ok());
    assert!(validate_queue_profiles(&serde_json::json!([])).is_err());
    assert!(validate_queue_profiles(&serde_json::json!([{
        "id": "default", "name": "默认队列", "max_active": 0
    }]))
    .is_err());
    assert!(validate_queue_profiles(&serde_json::json!([{
        "id": "other", "name": "其他队列"
    }]))
    .is_err());
    assert!(validate_queue_profiles(&serde_json::json!([
        { "id": "default", "name": "重复" },
        { "id": "other", "name": "重复" }
    ]))
    .is_err());
    assert!(validate_queue_profiles(&serde_json::json!([{
        "id": "default", "name": "默认队列", "active_days": "1,1"
    }]))
    .is_err());
    assert!(validate_queue_profiles(&serde_json::json!([{
        "id": "default", "name": "默认队列", "priority": 101
    }]))
    .is_err());
}

#[test]
fn task_schedule_and_queue_membership_fail_closed_before_worker_spawn() {
    let future = crate::TaskSnapshot {
        scheduled_start_at: "2999-01-01T00:00:00Z".into(),
        ..Default::default()
    };
    assert!(!task_schedule_allowed(&future));

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let error = coordinator
        .apply_defaults_to_spec(TaskSpec {
            queue_id: "missing-queue".into(),
            ..spec()
        })
        .unwrap_err();
    assert!(error.contains("任务所属队列不存在"));
}

#[test]
fn queue_completion_action_requires_every_task_to_succeed() {
    let profile = QueueProfile {
        id: "night-media".into(),
        name: "夜间媒体".into(),
        completion_action: "sleep".into(),
        ..QueueProfile::default()
    };
    let spec = TaskSpec {
        queue_id: profile.id.clone(),
        ..spec()
    };
    let completed = |id: &str| crate::TaskSnapshot {
        task_id: id.into(),
        queue_id: profile.id.clone(),
        status: "completed".into(),
        ..Default::default()
    };
    let unrelated = crate::TaskSnapshot {
        task_id: "other".into(),
        queue_id: "default".into(),
        status: "queued".into(),
        ..Default::default()
    };
    let tasks = vec![completed("one"), completed("two"), unrelated];
    assert_eq!(
        queue_completion_decision(&tasks, std::slice::from_ref(&profile), &spec),
        Some(("sleep".into(), "队列：夜间媒体".into()))
    );

    let mut failed = tasks;
    failed[1].status = "failed".into();
    assert!(queue_completion_decision(&failed, &[profile], &spec).is_none());
}

#[test]
fn replay_request_body_is_bounded_materialized_and_removed() {
    assert_eq!(
        decode_base64_bounded("cG9zdC1ib2R5", 128).unwrap(),
        b"post-body"
    );
    assert!(decode_base64_bounded("%%%", 128).is_err());
    assert!(decode_base64_bounded("QUJDRA==", 3).is_err());

    let dir = std::env::temp_dir().join(format!("hls-replay-body-{}", std::process::id()));
    let spec = TaskSpec {
        url: "https://cdn.test/post".into(),
        filename: "post.bin".into(),
        download_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let paths = TaskPaths::for_task("replay-body", &spec).unwrap();
    paths.prepare().unwrap();
    let guard =
        materialize_replay_request_body(r#"{"request_body":"cG9zdC1ib2R5"}"#, &paths).unwrap();
    let path = guard.0.as_ref().unwrap().clone();
    assert_eq!(std::fs::read(&path).unwrap(), b"post-body");
    drop(guard);
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn curl_import_creates_a_task_with_encrypted_replay_context() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "legal_terms_version",
            serde_json::json!(crate::LEGAL_TERMS_VERSION),
        )
        .unwrap();
    let mut option_headers = BTreeMap::new();
    option_headers.insert("Origin".into(), "https://override.test".into());
    let events = coordinator
        .dispatch(CoreCommand::ImportCurl {
            command: r#"curl -X POST -H "Authorization: Bearer abc" -H "Origin: https://site.test" -b "sid=secret" --data-raw "id=42" https://cdn.test/file.bin"#.into(),
            options: TaskSpec {
                filename: "saved.bin".into(),
                headers: option_headers,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    let spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .cloned()
        .unwrap();
    assert_eq!(spec.url, "https://cdn.test/file.bin");
    assert_eq!(spec.request_method, "POST");
    assert_eq!(spec.filename, "saved.bin");
    assert!(spec.headers.is_empty());
    assert!(spec.credential_ref.is_some());
    let (hydrated, replay) = hydrate_replay_headers(&coordinator.core(), spec).unwrap();
    assert_eq!(
        hydrated.headers.get("Cookie").map(String::as_str),
        Some("sid=secret")
    );
    assert_eq!(
        hydrated.headers.get("Origin").map(String::as_str),
        Some("https://override.test")
    );
    assert_eq!(
        hydrated.headers.get("Authorization").map(String::as_str),
        Some("Bearer abc")
    );
    assert_eq!(
        decode_base64_bounded(
            serde_json::from_str::<Value>(&replay).unwrap()["request_body"]
                .as_str()
                .unwrap(),
            MAX_REPLAY_BODY_BYTES,
        )
        .unwrap(),
        b"id=42",
    );
}

#[test]
fn retired_settings_are_rejected_and_sort_is_current() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    for key in ["dark_mode", "task_column_layout", "toolbar_actions"] {
        assert!(coordinator
            .set_setting(key, serde_json::json!("unused"))
            .is_err());
    }
    coordinator
        .set_setting("task_sort", serde_json::json!("name"))
        .unwrap();
}

#[test]
fn failure_diagnostics_preserve_http_status_stage_and_actionable_hint() {
    let failure = task_failure_from_error(
        "HTTP 404",
        "transfer",
        "https://cdn.test/missing.mp4?token=secret",
        3,
        false,
    );
    assert_eq!(failure.code, "HTTP_404");
    assert_eq!(failure.http_status, Some(404));
    assert_eq!(failure.stage, "transfer");
    assert_eq!(failure.attempt, 3);
    assert!(failure.hint.contains("重新识别"));
    assert_eq!(
        extract_http_status("request failed with status=503"),
        Some(503)
    );
    assert_eq!(failure_stage("checking", "checksum mismatch"), "checksum");
}

#[test]
fn failure_diagnostics_classify_local_output_failures_without_false_http_codes() {
    let failure = task_failure_from_error(
        "write failed: no space left on device",
        "transfer",
        r"C:\downloads\movie.mp4",
        1,
        false,
    );
    assert_eq!(failure.code, "DISK_FULL");
    assert_eq!(failure.http_status, None);
    assert!(failure.hint.contains("空间"));
}

#[test]
fn share_media_rejects_ambiguous_and_untrusted_sources() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    assert!(share_media(&coordinator, "", "", "", "", "").is_err());
    assert!(share_media(&coordinator, "relative-video.mp4", "", "video", "", "").is_err());
    assert!(share_media(&coordinator, "", "javascript:alert(1)", "video", "", "").is_err());
    assert!(share_media(
        &coordinator,
        r"C:\missing.mp4",
        "https://cdn.test/video.mp4",
        "video",
        "",
        ""
    )
    .is_err());
}

#[test]
fn browser_media_push_binds_only_the_pending_source_and_requires_its_credentials() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .dispatch(CoreCommand::RequestMediaPush {
            request: MediaPushRequest {
                id: "push-context".into(),
                push_kind: "cast".into(),
                url: "https://cdn.test/media".into(),
                title: String::new(),
                mime_type: "application/dash+xml".into(),
                status: "pending".into(),
                message: String::new(),
                location: String::new(),
                credential_ref: Some("missing-push-credential".into()),
                created_at_ms: 1,
            },
        })
        .unwrap();
    assert!(share_media(
        &coordinator,
        "",
        "https://cdn.test/other",
        "",
        "",
        "push-context"
    )
    .unwrap_err()
    .contains("不匹配"));
    assert!(share_media(
        &coordinator,
        "",
        "https://cdn.test/media",
        "",
        "",
        "push-context"
    )
    .unwrap_err()
    .contains("凭据不存在"));
    coordinator
        .dispatch(CoreCommand::ResolveMediaPush {
            request_id: "push-context".into(),
            status: "canceled".into(),
            message: String::new(),
            location: String::new(),
        })
        .unwrap();
    assert!(share_media(
        &coordinator,
        "",
        "https://cdn.test/media",
        "",
        "",
        "push-context"
    )
    .unwrap_err()
    .contains("已结束"));
}

#[test]
fn stopping_cast_revokes_the_active_media_mount() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let server = shared_media().unwrap();
    let token = crate::playback::random_mount_token();
    let dir = std::env::temp_dir().join(format!("hls-cast-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("video.bin");
    std::fs::write(&file, b"cast-lifecycle").unwrap();
    server.mount(&token, file);
    remember_cast_mount(&token, &server.url_for(&token), "cast-lifecycle", "");

    let request = |token: &str| {
        let mut stream = TcpStream::connect(("127.0.0.1", server.bound_port())).unwrap();
        stream
            .write_all(
                format!("GET /media/{token} HTTP/1.1\r\nRange: bytes=0-3\r\n\r\n").as_bytes(),
            )
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).into_owned()
    };
    assert!(request(&token).starts_with("HTTP/1.1 206"));
    clear_cast_mount();
    assert!(request(&token).starts_with("HTTP/1.1 404"));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
fn published_task_fixture(
    label: &str,
) -> (Arc<Mutex<PersistentCore>>, TaskPaths, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "hls-publish-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let published = root.join("movie.mp4");
    std::fs::write(&published, b"a complete payload").unwrap();
    let core = Arc::new(Mutex::new(PersistentCore::in_memory().unwrap()));
    let spec = TaskSpec {
        url: "https://cdn.test/movie.mp4".into(),
        resource_kind: crate::ResourceKind::File,
        filename: "movie.mp4".into(),
        expected_size: Some(17),
        ..Default::default()
    };
    core.lock()
        .unwrap()
        .handle(CoreCommand::CreateTask { spec })
        .unwrap();
    let _ = core.lock().unwrap().tasks()[0].task_id.clone();
    let paths = TaskPaths {
        output: root.join("payload.part"),
        final_output: published.clone(),
        control: root.join("control"),
        progress: root.join("progress.json"),
        torrent_selection: root.join("torrent-selection.json"),
    };
    let core = Arc::clone(&core);
    (core, paths, published, root)
}

#[test]
fn completed_cache_cleanup_releases_player_after_recording_the_output() {
    let (core, paths, published, root) = published_task_fixture("player-cleanup");
    let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
    let spec = task_spec_for(&core, &task_id);
    fs::create_dir_all(root.join("segments")).unwrap();
    fs::write(root.join("segments/one.ts"), b"temporary segment").unwrap();
    let saved_core = Arc::clone(&core);
    let saved_id = task_id.clone();
    let saved_output = published.clone();
    let reached_cleanup = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&reached_cleanup);
    BEFORE_COMPLETED_CACHE_CLEANUP.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let snapshot = saved_core
                .lock()
                .unwrap()
                .snapshot(&saved_id)
                .unwrap()
                .clone();
            assert_eq!(snapshot.status, "completed");
            assert_eq!(snapshot.output_path, saved_output.to_string_lossy());
            // 在清理入口阻住完成线程，另一个线程必须仍能进入播放器。
            let (sent, received) = std::sync::mpsc::channel();
            let player_request = std::thread::spawn(move || {
                let _player = player_ui_state().lock().unwrap();
                let _ = sent.send(());
            });
            let available = received.recv_timeout(Duration::from_secs(5));
            assert!(
                available.is_ok(),
                "cache cleanup held the global player lock"
            );
            player_request.join().unwrap();
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
    });
    finish_published_task(&core, &task_id, &paths, &published, None, &spec, false).unwrap();
    assert!(reached_cleanup.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!root.join("segments").exists());
    assert_eq!(fs::read(&published).unwrap(), b"a complete payload");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_published_payload_is_recorded_not_failed() {
    let (core, paths, published, root) = published_task_fixture("recorded");
    let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
    let spec = task_spec_for(&core, &task_id);
    finish_published_task(&core, &task_id, &paths, &published, None, &spec, false).unwrap();

    let snapshot = core
        .lock()
        .unwrap()
        .tasks()
        .iter()
        .find(|task| task.task_id == task_id)
        .cloned()
        .unwrap();
    assert_eq!(snapshot.status, "completed");
    assert_eq!(
        snapshot.output_path,
        published.to_string_lossy().into_owned()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn task_deletion_cleans_incomplete_cache_and_preserves_other_tasks_and_outputs() {
    let root = std::env::temp_dir().join(format!(
        "hls-delete-cache-{}-{}",
        std::process::id(),
        handoff_now_ms()
    ));
    let spec = TaskSpec {
        url: "https://example.test/movie.bin".into(),
        filename: "movie.bin".into(),
        download_dir: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut core = PersistentCore::in_memory().unwrap();
    core.handle(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    core.handle(CoreCommand::CreateTask {
        spec: TaskSpec {
            url: "https://example.test/other.bin".into(),
            ..spec.clone()
        },
    })
    .unwrap();
    let ids: Vec<_> = core.tasks().into_iter().map(|task| task.task_id).collect();
    let coordinator = CoreCoordinator::new(core);
    let first = TaskPaths::for_task(&ids[0], &spec).unwrap();
    let other = TaskPaths::for_task(&ids[1], &spec).unwrap();
    first.prepare().unwrap();
    other.prepare().unwrap();
    fs::create_dir_all(first.task_dir().join("segments")).unwrap();
    fs::write(
        first.task_dir().join("segments/000000.ts"),
        b"unfinished fragment",
    )
    .unwrap();
    fs::write(
        first.task_dir().join("unknown-engine-work.tmp"),
        b"task-owned work",
    )
    .unwrap();
    fs::write(&other.output, b"other task").unwrap();
    fs::write(&first.final_output, b"existing user output").unwrap();
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: ids[0].clone(),
            action: "delete".into(),
        })
        .unwrap();
    assert!(!first.task_dir().exists());
    assert_eq!(fs::read(&other.output).unwrap(), b"other task");
    assert_eq!(
        fs::read(&first.final_output).unwrap(),
        b"existing user output"
    );
    assert_eq!(coordinator.tasks().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_persists_renamed_output_before_removing_all_cache_metadata() {
    for keep_temp in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "hls-finish-cache-{}-{}-{keep_temp}",
            std::process::id(),
            handoff_now_ms()
        ));
        let spec = TaskSpec {
            url: "https://example.test/movie.bin".into(),
            filename: "movie.bin".into(),
            download_dir: root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: spec.clone() })
            .unwrap();
        core.store_mut()
            .set_setting("keep_temp_files", keep_temp)
            .unwrap();
        let id = core.tasks()[0].task_id.clone();
        let coordinator = CoreCoordinator::new(core);
        let paths = TaskPaths::for_task(&id, &spec).unwrap();
        paths.prepare().unwrap();
        fs::write(&paths.final_output, b"existing user output").unwrap();
        fs::write(&paths.output, b"new output").unwrap();
        fs::write(
            paths.task_dir().join("native-engine.ranges.json"),
            b"checkpoint",
        )
        .unwrap();
        fs::write(paths.task_dir().join("ffmpeg-merge.log"), b"log").unwrap();
        for name in [
            "seg-0001.m4s",
            "seg-0001.progress.json",
            "seg-0001.progress.response.json",
            "init.progress.json",
            "init.progress.response.json",
            "sftp-resume.json",
            "playback-priority",
            "playback-byte-priority",
        ] {
            fs::write(paths.task_dir().join(name), b"engine work").unwrap();
        }
        fs::create_dir_all(paths.task_dir().join("playback-ready/segments")).unwrap();
        fs::write(
            paths.task_dir().join("playback-ready/segments/000001.ts"),
            b"123",
        )
        .unwrap();
        let published = paths.publish_with("rename", keep_temp).unwrap();
        remember_published(&paths, &published);
        finish_published_task(
            &coordinator.core,
            &id,
            &paths,
            &published,
            None,
            &spec,
            false,
        )
        .unwrap();
        assert_eq!(paths.task_dir().exists(), keep_temp);
        assert_eq!(
            coordinator.tasks().unwrap()[0].output_path,
            published.to_string_lossy()
        );
        assert_eq!(
            fs::read(&paths.final_output).unwrap(),
            b"existing user output"
        );
        coordinator.dispatch(CoreCommand::ClearCompleted).unwrap();
        assert!(!paths.task_dir().exists());
        assert!(published.is_file());
        assert!(coordinator.tasks().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn locked_cache_prevents_task_record_deletion() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = std::env::temp_dir().join(format!(
        "hls-locked-cache-{}-{}",
        std::process::id(),
        handoff_now_ms()
    ));
    let spec = TaskSpec {
        url: "https://example.test/x.bin".into(),
        filename: "x.bin".into(),
        download_dir: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut core = PersistentCore::in_memory().unwrap();
    core.handle(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    let id = core.tasks()[0].task_id.clone();
    let coordinator = CoreCoordinator::new(core);
    let paths = TaskPaths::for_task(&id, &spec).unwrap();
    paths.prepare().unwrap();
    fs::write(&paths.output, b"unfinished").unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&paths.output)
        .unwrap();
    assert!(coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: id.clone(),
            action: "delete".into()
        })
        .is_err());
    assert_eq!(coordinator.tasks().unwrap().len(), 1);
    drop(locked);
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: id,
            action: "delete".into(),
        })
        .unwrap();
    assert!(!paths.task_dir().exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completed_media_cleans_segments_and_keeps_published_files() {
    for kind in [
        crate::ResourceKind::Hls,
        crate::ResourceKind::Dash,
        crate::ResourceKind::Live,
    ] {
        let (core, paths, original, root) = published_task_fixture("media-cleanup");
        let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
        let mut spec = task_spec_for(&core, &task_id);
        spec.resource_kind = kind;
        spec.expected_size = Some(b"temporary media".len() as u64);
        let published = root.join("movie_1.mp4");
        for dir in ["segments", "audio", "live-subtitles", "subs"] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join("fragment.bin"), b"temporary media").unwrap();
        }
        for name in [
            "merged.mp4",
            "muxed.mp4",
            "init.mp4",
            "seg-0000.m4s",
            "local.m3u8",
            "vod_segments.json",
            "dash_vod_segments.json",
        ] {
            fs::write(root.join(name), b"temporary media").unwrap();
        }
        fs::write(
            root.join("subs/zh.srt"),
            b"1\n00:00:00,000 --> 00:00:01,000\nsubtitle\n",
        )
        .unwrap();
        fs::write(
            &paths.progress,
            br#"{"completed_segments":2,"total_segments":2}"#,
        )
        .unwrap();
        fs::write(root.join("user-notes.txt"), b"keep unknown files").unwrap();

        complete_payload(&core, &task_id, &paths, &root.join("merged.mp4"), &spec).unwrap();

        assert_eq!(core.lock().unwrap().tasks()[0].status, "completed");
        assert_eq!(fs::read(&original).unwrap(), b"a complete payload");
        assert_eq!(fs::read(&published).unwrap(), b"temporary media");
        assert_eq!(resolved_snapshot_output(None, &paths), published);
        assert!(root.join("movie_1.zh.srt").is_file());
        assert!(!paths.progress.exists());
        assert!(root.join("user-notes.txt").is_file());
        for name in [
            "segments",
            "audio",
            "live-subtitles",
            "subs",
            "merged.mp4",
            "muxed.mp4",
            "init.mp4",
            "seg-0000.m4s",
            "local.m3u8",
            "vod_segments.json",
            "dash_vod_segments.json",
        ] {
            assert!(
                !root.join(name).exists(),
                "leftover media work file: {name}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn completed_media_retains_opted_in_temps_and_failed_subtitle_copies() {
    for keep_temp in [false, true] {
        let (core, paths, published, root) = published_task_fixture("media-preserve");
        let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
        let mut spec = task_spec_for(&core, &task_id);
        spec.resource_kind = crate::ResourceKind::Hls;
        core.lock()
            .unwrap()
            .store_mut()
            .set_setting("keep_temp_files", keep_temp)
            .unwrap();
        fs::create_dir_all(root.join("segments")).unwrap();
        fs::write(root.join("segments/000000.jpeg"), b"media segment").unwrap();
        fs::create_dir_all(root.join("subs")).unwrap();
        fs::write(root.join("subs/zh.srt"), b"subtitle to preserve").unwrap();
        // 目标是目录，模拟字幕复制失败；成品仍应完成，字幕源不能被清理。
        fs::create_dir_all(root.join("movie.zh.srt")).unwrap();

        finish_published_task(&core, &task_id, &paths, &published, None, &spec, true).unwrap();

        assert_eq!(core.lock().unwrap().tasks()[0].status, "completed");
        assert_eq!(
            fs::read(root.join("subs/zh.srt")).unwrap(),
            b"subtitle to preserve"
        );
        assert_eq!(root.join("segments/000000.jpeg").exists(), keep_temp);
        assert_eq!(fs::read(&published).unwrap(), b"a complete payload");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn incomplete_media_keeps_resumable_segments() {
    for control in ["pause", "cancel", "run"] {
        let (core, paths, published, root) = published_task_fixture("media-incomplete");
        let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
        let mut spec = task_spec_for(&core, &task_id);
        spec.resource_kind = crate::ResourceKind::Hls;
        spec.expected_size = Some(999);
        fs::write(&paths.control, control).unwrap();
        let merged = root.join("merged.mp4");
        fs::write(&merged, b"incomplete merge").unwrap();
        fs::create_dir_all(root.join("segments")).unwrap();
        fs::write(root.join("segments/000000.jpeg"), b"resumable segment").unwrap();

        let error = complete_payload(&core, &task_id, &paths, &merged, &spec).unwrap_err();
        match control {
            "pause" => assert_eq!(error, "paused"),
            "cancel" => assert_eq!(error, "canceled"),
            _ => assert!(error.contains("size mismatch")),
        }
        assert_eq!(
            fs::read(root.join("segments/000000.jpeg")).unwrap(),
            b"resumable segment"
        );
        assert!(merged.is_file());
        assert_eq!(fs::read(&published).unwrap(), b"a complete payload");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn media_cleanup_preserves_a_published_file_inside_the_work_directory() {
    let (_core, paths, _published, root) = published_task_fixture("media-nested-output");
    fs::create_dir_all(root.join("segments")).unwrap();
    let published = root.join("segments/movie.mp4");
    fs::write(&published, b"published media").unwrap();
    fs::create_dir_all(root.join("audio")).unwrap();
    fs::write(root.join("audio/fragment.bin"), b"temporary audio").unwrap();

    assert!(cleanup_completed_media(&paths, &root.join("missing.mp4"), false).is_err());
    assert!(root.join("audio/fragment.bin").is_file());
    cleanup_completed_media(&paths, &published, false).unwrap();
    assert_eq!(fs::read(&published).unwrap(), b"published media");
    assert!(!root.join("audio").exists());
    fs::remove_dir_all(root).unwrap();
}

// The regression this closes: `complete_payload` tail-called `maybe_schedule_power`,
// which fails on a store read or a power-action error. The payload was already
// published, but the error escaped and the caller marked the task failed and
// re-queued it, downloading a file the user already had.
#[test]
fn a_published_payload_never_fails_the_task() {
    let (core, paths, published, root) = published_task_fixture("poison");
    let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
    let spec = task_spec_for(&core, &task_id);

    // Poison the Core mutex the way a panicking worker would, so every post-publish
    // store and power-action step inside finish_published_task fails.
    let victim = Arc::clone(&core);
    let worker = std::thread::spawn(move || {
        let _guard = victim.lock().unwrap();
        panic!("poison the Core mutex on purpose");
    });
    let _ = worker.join();
    assert!(core.lock().is_err(), "the mutex must be poisoned");

    assert!(
        finish_published_task(&core, &task_id, &paths, &published, None, &spec, false).is_ok(),
        "a payload that is already on disk must not fail its task"
    );
    assert_eq!(
        std::fs::read(&published).unwrap(),
        b"a complete payload",
        "the published file must survive untouched"
    );
    let _ = std::fs::remove_dir_all(root);
}

// 同上：只被 #[test] 使用，必须随测试一起编译。
#[cfg(test)]
fn task_spec_for(core: &Arc<Mutex<PersistentCore>>, task_id: &str) -> TaskSpec {
    core.lock()
        .unwrap()
        .task_spec(task_id)
        .cloned()
        .unwrap_or_else(TaskSpec::default)
}

// A torrent selection publishes a *directory*, so the size cannot be measured
// from `published` and the caller passes the byte count it wrote. The torrent
// branch used to do that with mark_progress directly; routing it through
// finish_published_task must not lose the number.
#[test]
fn a_published_selection_directory_reports_the_bytes_it_wrote() {
    let (core, paths, published, root) = published_task_fixture("selection");
    let task_id = core.lock().unwrap().tasks()[0].task_id.clone();
    let spec = task_spec_for(&core, &task_id);
    let selection = root.join("selection");
    std::fs::create_dir_all(&selection).unwrap();

    finish_published_task(
        &core,
        &task_id,
        &paths,
        &selection,
        Some(4242),
        &spec,
        false,
    )
    .unwrap();

    let snapshot = core
        .lock()
        .unwrap()
        .tasks()
        .iter()
        .find(|task| task.task_id == task_id)
        .cloned()
        .unwrap();
    assert_eq!(snapshot.status, "completed");
    assert_eq!(snapshot.downloaded_bytes, 4242);
    assert_eq!(snapshot.total_bytes, Some(4242));
    assert_eq!(
        snapshot.output_path,
        selection.to_string_lossy().into_owned()
    );
    // The single-file payload beside it must be untouched by a directory publish.
    assert_eq!(std::fs::read(&published).unwrap(), b"a complete payload");
    let _ = std::fs::remove_dir_all(root);
}

fn lifecycle_test_root(name: &str) -> PathBuf {
    std::env::var_os("PI_SCRATCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "hls-lifecycle-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
}

fn lifecycle_task(coordinator: &CoreCoordinator, root: &Path, status: &str) -> String {
    let mut core = coordinator.lock().unwrap();
    let url = format!("https://cdn.test/lifecycle-{}.bin", core.tasks().len());
    let events = core
        .handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url,
                download_dir: root.to_string_lossy().into_owned(),
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    if status != "queued" {
        core.handle(CoreCommand::UpdateProgress {
            task_id: task_id.clone(),
            downloaded_bytes: 7,
            total_bytes: Some(100),
            speed_bytes_per_sec: 0,
            stage: "waiting".into(),
            status: status.into(),
        })
        .unwrap();
    }
    task_id
}

#[test]
fn queued_pause_and_cancel_without_directories_are_not_auto_started() {
    for (action, status) in [("pause", "paused"), ("cancel", "canceled")] {
        let root = lifecycle_test_root(action);
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .unwrap();
        let task_id = lifecycle_task(&coordinator, &root, "queued");
        assert!(!root.exists());
        let events = coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id,
                action: action.into(),
            })
            .unwrap();
        assert!(!events
            .iter()
            .any(|event| matches!(event.event, CoreEvent::Error { .. })));
        coordinator.start_next_queued(false).unwrap();
        assert_eq!(coordinator.tasks().unwrap()[0].status, status);
        assert!(coordinator.active.lock().unwrap().is_empty());
        assert!(!root.exists());
    }
}

#[test]
fn queued_or_exiting_workers_still_receive_pause_and_cancel_control() {
    for status in ["queued", "paused"] {
        for action in ["pause", "cancel"] {
            let root = lifecycle_test_root("exiting-control");
            let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
            let task_id = lifecycle_task(&coordinator, &root, status);
            let paths = TaskPaths::for_task(
                &task_id,
                coordinator.lock().unwrap().task_spec(&task_id).unwrap(),
            )
            .unwrap();
            paths.prepare().unwrap();
            coordinator.active.lock().unwrap().insert(task_id.clone());
            coordinator
                .dispatch(CoreCommand::TaskAction {
                    task_id: task_id.clone(),
                    action: action.into(),
                })
                .unwrap();
            assert_eq!(fs::read_to_string(&paths.control).unwrap(), action);
            coordinator.active.lock().unwrap().remove(&task_id);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn pause_active_tasks_also_pauses_queued_tasks_before_worker_exit() {
    let root = lifecycle_test_root("pause-all");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let active_id = lifecycle_task(&coordinator, &root, "downloading");
    lifecycle_task(&coordinator, &root, "queued");
    lifecycle_task(&coordinator, &root, "queued");
    let paths = TaskPaths::for_task(
        &active_id,
        coordinator.lock().unwrap().task_spec(&active_id).unwrap(),
    )
    .unwrap();
    paths.prepare().unwrap();
    coordinator.active.lock().unwrap().insert(active_id.clone());
    coordinator.pause_active_tasks().unwrap();
    assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
    coordinator.active.lock().unwrap().remove(&active_id);
    coordinator.start_next_queued(false).unwrap();
    assert!(coordinator
        .tasks()
        .unwrap()
        .iter()
        .all(|task| task.status == "paused"));
    assert!(coordinator.active.lock().unwrap().is_empty());
    assert!(!root.join(".hls-tasks/task-2").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn schedule_stop_applies_only_to_running_tasks_and_enabled_scheduled_queues() {
    let mut task = crate::TaskSnapshot {
        status: "downloading".into(),
        scheduled_stop_at: "2000-01-01T00:00:00Z".into(),
        ..Default::default()
    };
    assert!(task_schedule_stop_due(&task, &[]));
    task.scheduled_stop_at = "2999-01-01T00:00:00Z".into();
    assert!(!task_schedule_stop_due(&task, &[]));
    // 开始时间限制启动，不应单独中断已运行任务。
    task.scheduled_stop_at.clear();
    task.scheduled_start_at = "2999-01-01T00:00:00Z".into();
    assert!(!task_schedule_stop_due(&task, &[]));
    let mut profile = QueueProfile {
        id: task.queue_id.clone(),
        enabled: true,
        schedule_enabled: true,
        start_time: "00:00".into(),
        stop_time: "00:00".into(),
        ..Default::default()
    };
    for status in ["downloading", "recording", "merging", "checking", "parsing"] {
        task.status = status.into();
        assert!(task_schedule_stop_due(&task, &[profile.clone()]));
    }
    for status in ["queued", "paused", "canceled", "completed", "failed"] {
        task.status = status.into();
        assert!(!task_schedule_stop_due(&task, &[profile.clone()]));
    }
    task.status = "downloading".into();
    profile.enabled = false;
    assert!(!task_schedule_stop_due(&task, &[profile.clone()]));
    profile.enabled = true;
    profile.schedule_enabled = false;
    assert!(!task_schedule_stop_due(&task, &[profile.clone()]));
    profile.schedule_enabled = true;
    profile.id = "other".into();
    assert!(!task_schedule_stop_due(&task, &[profile]));
}

#[test]
fn schedule_stop_dispatches_pause_control_without_touching_queued_tasks() {
    let root = lifecycle_test_root("scheduled-stop");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let task_id = lifecycle_task(&coordinator, &root, "recording");
    lifecycle_task(&coordinator, &root, "queued");
    let profile = QueueProfile {
        schedule_enabled: true,
        start_time: "00:00".into(),
        stop_time: "00:00".into(),
        ..Default::default()
    };
    coordinator
        .lock()
        .unwrap()
        .store_mut()
        .set_setting(
            "queue_profiles",
            serde_json::to_string(&vec![profile]).unwrap(),
        )
        .unwrap();
    let paths = TaskPaths::for_task(
        &task_id,
        coordinator.lock().unwrap().task_spec(&task_id).unwrap(),
    )
    .unwrap();
    paths.prepare().unwrap();
    coordinator.active.lock().unwrap().insert(task_id.clone());
    coordinator.pause_tasks_outside_schedule().unwrap();
    assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&task_id)
            .unwrap()
            .status,
        "paused"
    );
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot("task-2")
            .unwrap()
            .status,
        "queued"
    );
    coordinator.active.lock().unwrap().remove(&task_id);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn startup_reconciles_durable_ranges_when_database_progress_lags() {
    for (case, ranges, output_len, expected) in [
        ("merged", vec![(0, 39), (20, 59), (80, 89)], 100, 70),
        ("truncated", vec![(0, 79)], 50, 7),
        ("outside-total", vec![(0, 100)], 101, 7),
        ("other-resource", vec![(0, 79)], 100, 7),
        ("old-format", vec![(0, 79)], 100, 7),
        ("empty", vec![], 100, 0),
    ] {
        let root = lifecycle_test_root(case);
        fs::create_dir_all(&root).unwrap();
        let db = root.join("core.sqlite3");
        let coordinator = CoreCoordinator::new(PersistentCore::open(&db).unwrap());
        let task_id = lifecycle_task(&coordinator, &root, "downloading");
        let spec = coordinator
            .lock()
            .unwrap()
            .task_spec(&task_id)
            .unwrap()
            .clone();
        let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
        paths.prepare().unwrap();
        fs::write(&paths.output, vec![1; output_len]).unwrap();
        fs::write(
            paths.progress.with_file_name(crate::range_cache::FILE_NAME),
            serde_json::json!({"version":if case == "old-format" {1} else {2}, "total":100,
                "resource_key":if case == "other-resource" {"https://other.test/file"} else {&spec.url},
                "etag":"\"fixture\"", "last_modified":"", "ranges":ranges})
            .to_string(),
        )
        .unwrap();
        drop(coordinator);
        let restarted = CoreCoordinator::new(PersistentCore::open(&db).unwrap());
        restarted.recover_startup().unwrap();
        {
            let core = restarted.lock().unwrap();
            let task = core.snapshot(&task_id).unwrap();
            assert_eq!(task.status, "paused", "{case}");
            assert_eq!(task.downloaded_bytes, expected, "{case}");
            assert_eq!(task.total_bytes, Some(100), "{case}");
            assert_eq!(
                core.store().load_tasks().unwrap()[0].downloaded_bytes,
                expected,
                "{case}"
            );
        }
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn shutdown_waits_for_checkpoints_and_recovers_only_interrupted_tasks() {
    for resume in [false, true] {
        let root = lifecycle_test_root("shutdown");
        fs::create_dir_all(&root).unwrap();
        let db = root.join("core.sqlite3");
        let coordinator = CoreCoordinator::new(PersistentCore::open(&db).unwrap());
        coordinator
            .lock()
            .unwrap()
            .store_mut()
            .set_setting("legal_terms_accepted", false)
            .unwrap();
        coordinator
            .lock()
            .unwrap()
            .store_mut()
            .set_setting("resume_interrupted_on_startup", resume)
            .unwrap();
        let active_id = lifecycle_task(&coordinator, &root, "downloading");
        let manual_id = lifecycle_task(&coordinator, &root, "paused");
        let queued_id = lifecycle_task(&coordinator, &root, "queued");
        let paths = TaskPaths::for_task(
            &active_id,
            coordinator.lock().unwrap().task_spec(&active_id).unwrap(),
        )
        .unwrap();
        paths.prepare().unwrap();
        coordinator.active.lock().unwrap().insert(active_id.clone());
        let (tx, rx) = mpsc::channel();
        *coordinator.worker_wait_started.lock().unwrap() = Some(tx);
        let runner = coordinator.clone();
        let shutdown = thread::spawn(move || runner.dispatch(CoreCommand::Shutdown));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(fs::read_to_string(&paths.control).unwrap(), "pause");
        assert_eq!(
            coordinator
                .lock()
                .unwrap()
                .snapshot(&active_id)
                .unwrap()
                .status,
            "paused"
        );
        // 模拟 worker 的断点落盘和最后一笔进度，之后才从 active 移除。
        fs::write(&paths.progress, b"saved-checkpoint").unwrap();
        mark_progress(
            &coordinator.core,
            &active_id,
            47,
            Some(100),
            "waiting",
            "paused",
        )
        .unwrap();
        coordinator.active.lock().unwrap().remove(&active_id);
        shutdown.join().unwrap().unwrap();
        let persisted = coordinator.lock().unwrap().store().load_tasks().unwrap();
        let interrupted = persisted
            .iter()
            .find(|task| task.task_id == active_id)
            .unwrap();
        assert_eq!(interrupted.status, "downloading");
        assert_eq!(interrupted.downloaded_bytes, 47);
        assert_eq!(fs::read(&paths.progress).unwrap(), b"saved-checkpoint");
        coordinator.recover_startup().unwrap();
        assert_eq!(
            coordinator
                .lock()
                .unwrap()
                .snapshot(&active_id)
                .unwrap()
                .status,
            "paused"
        );
        assert!(coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: manual_id.clone(),
                action: "resume".into(),
            })
            .is_err());
        assert!(coordinator.active.lock().unwrap().is_empty());
        drop(coordinator);

        let restarted = CoreCoordinator::new(PersistentCore::open(&db).unwrap());
        restarted.recover_startup().unwrap();
        // 未接受法律条款，恢复测试不启动网络 worker，但可验证恢复后的排队意图。
        let core = restarted.lock().unwrap();
        let interrupted = core.snapshot(&active_id).unwrap();
        assert_eq!(interrupted.status, if resume { "queued" } else { "paused" });
        assert_eq!(interrupted.downloaded_bytes, 47);
        assert_eq!(core.snapshot(&manual_id).unwrap().status, "paused");
        assert_eq!(core.snapshot(&queued_id).unwrap().status, "queued");
        drop(core);
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn shutdown_timeout_does_not_persist_completed_stop_or_resume_intent() {
    let root = lifecycle_test_root("shutdown-timeout");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let task_id = lifecycle_task(&coordinator, &root, "downloading");
    let paths = TaskPaths::for_task(
        &task_id,
        coordinator.lock().unwrap().task_spec(&task_id).unwrap(),
    )
    .unwrap();
    paths.prepare().unwrap();
    coordinator.active.lock().unwrap().insert(task_id.clone());
    assert!(coordinator.prepare_for_update(Duration::ZERO).is_err());
    assert!(coordinator.worker_is_active(&task_id).unwrap());
    assert!(!coordinator.update_shutdown.load(Ordering::SeqCst));
    assert_eq!(
        coordinator.lock().unwrap().store().load_tasks().unwrap()[0].status,
        "paused"
    );
    assert!(!paths.progress.exists());
    coordinator.active.lock().unwrap().remove(&task_id);
    fs::remove_dir_all(root).unwrap();
}

fn audit_test_root(label: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/audit-tests")
        .join(format!(
            "{label}-{}-{}",
            std::process::id(),
            handoff_now_ms()
        ));
    fs::create_dir_all(&root).unwrap();
    fs::canonicalize(root).unwrap()
}

fn audit_handoff_fixture(label: &str) -> (CoreCoordinator, PathBuf) {
    let root = audit_test_root(label);
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("download_dir", serde_json::json!(root.join("downloads")))
        .unwrap();
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([QueueProfile {
                enabled: false,
                ..Default::default()
            }]),
        )
        .unwrap();
    coordinator
        .set_setting(
            "site_rules",
            serde_json::json!(serde_json::json!([{
                "host":"cdn.test", "download_dir":root.join("site-rule")
            }])
            .to_string()),
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.test/a.bin".into(),
                filename: "a.bin".into(),
                handoff_id: "audit".into(),
                ..Default::default()
            },
        })
        .unwrap();
    (coordinator, root)
}

fn audit_accept_handoff(
    coordinator: &CoreCoordinator,
    download_dir: String,
    trusted_ui: bool,
) -> Result<Vec<EventEnvelope>, String> {
    coordinator.dispatch(CoreCommand::AcceptHandoff {
        handoff_id: "audit".into(),
        filename: "a.bin".into(),
        download_dir,
        trusted_ui,
        allow_duplicate: false,
        start_paused: false,
        queue_id: String::new(),
        spec_override: None,
    })
}

#[test]
fn accept_handoff_trusted_ui_preserves_custom_directory_outside_default_and_site_rule() {
    let (coordinator, root) = audit_handoff_fixture("trusted-directory");
    let selected = root.join("custom").to_string_lossy().into_owned();
    audit_accept_handoff(&coordinator, selected.clone(), true).unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .task_spec("task-1")
            .unwrap()
            .download_dir,
        selected
    );
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accept_handoff_trusted_ui_rejects_parent_traversal() {
    let (coordinator, root) = audit_handoff_fixture("trusted-traversal");
    assert!(
        audit_accept_handoff(&coordinator, format!("{}/../escape", root.display()), true)
            .unwrap_err()
            .contains("..")
    );
    assert!(coordinator.raw_tasks().unwrap().is_empty());
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accept_handoff_untrusted_ui_rejects_custom_directory_outside_default() {
    let (coordinator, root) = audit_handoff_fixture("untrusted-directory");
    assert!(audit_accept_handoff(
        &coordinator,
        root.join("custom").to_string_lossy().into_owned(),
        false
    )
    .is_err());
    assert!(coordinator.raw_tasks().unwrap().is_empty());
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accept_handoff_trusted_ui_empty_directory_uses_site_rule_outside_default() {
    let (coordinator, root) = audit_handoff_fixture("site-rule-directory");
    audit_accept_handoff(&coordinator, String::new(), true).unwrap();
    assert_eq!(
        Path::new(
            &coordinator
                .lock()
                .unwrap()
                .task_spec("task-1")
                .unwrap()
                .download_dir
        ),
        root.join("site-rule")
    );
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scheduler_ticks_do_not_refresh_missing_output_flags_or_emit_updates() {
    let root = audit_test_root("scheduler-snapshots");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let task_id = lifecycle_task(&coordinator, &root, "completed");
    let output = root.join("finished.bin");
    fs::write(&output, b"finished").unwrap();
    coordinator
        .lock()
        .unwrap()
        .set_output_path(&task_id, output.to_string_lossy().into_owned())
        .unwrap();
    assert!(!coordinator.tasks().unwrap()[0].output_missing);
    fs::remove_file(output).unwrap();
    let sequence = coordinator.latest_sequence().unwrap();
    coordinator.pause_tasks_outside_schedule().unwrap();
    coordinator.start_next_queued(false).unwrap();
    assert!(
        !coordinator
            .lock()
            .unwrap()
            .snapshot(&task_id)
            .unwrap()
            .output_missing
    );
    assert!(!coordinator
        .events_after(sequence, 100)
        .unwrap()
        .iter()
        .any(|event| matches!(event.event, CoreEvent::TaskUpdated { .. })));
    assert!(
        coordinator.tasks().unwrap()[0].output_missing,
        "external snapshots still refresh flags"
    );
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn settings_reject_non_string_values_and_invalid_schedule_or_policy() {
    let string_keys = [
        "ffmpeg_path",
        "av_scan_command",
        "download_dir",
        "temp_dir",
        "torrent_watch_dir",
        "proxy_url",
        "tvbox_endpoint",
        "default_referer",
        "default_origin",
        "allowed_hosts",
        "default_user_agent",
        "proxy_mode",
        "proxy_bypass",
        "queue_active_days",
        "queue_auto_start_time",
        "queue_auto_stop_time",
        "existing_file_policy",
        "completion_power_action",
        "legal_terms_version",
        "preferred_cast_device_id",
    ];
    for key in string_keys {
        for value in [
            serde_json::json!(1),
            serde_json::json!(true),
            serde_json::json!({}),
            Value::Null,
        ] {
            assert!(
                CoreCoordinator::validate_setting(key, &value).is_err(),
                "{key}: {value}"
            );
        }
    }
    for key in ["queue_auto_start_time", "queue_auto_stop_time"] {
        for clock in ["24:00", "12:60", "1:00", "noon"] {
            assert!(CoreCoordinator::validate_setting(key, &serde_json::json!(clock)).is_err());
        }
        assert!(CoreCoordinator::validate_setting(key, &serde_json::json!("23:59")).is_ok());
    }
    for days in ["", "0", "8", "1,1", "1,01", "1,", "Mon"] {
        assert!(
            CoreCoordinator::validate_setting("queue_active_days", &serde_json::json!(days))
                .is_err(),
            "{days}"
        );
    }
    assert!(
        CoreCoordinator::validate_setting("queue_active_days", &serde_json::json!("1,3,7")).is_ok()
    );
    for (key, valid) in [
        ("existing_file_policy", vec!["rename", "overwrite", "skip"]),
        (
            "completion_power_action",
            vec!["", "none", "shutdown", "sleep", "hibernate"],
        ),
    ] {
        assert!(CoreCoordinator::validate_setting(key, &serde_json::json!("invalid")).is_err());
        for value in valid {
            assert!(CoreCoordinator::validate_setting(key, &serde_json::json!(value)).is_ok());
        }
    }
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let sequence = coordinator.latest_sequence().unwrap();
    assert!(coordinator
        .set_settings(BTreeMap::from([
            ("proxy_mode".into(), serde_json::json!("manual")),
            ("queue_auto_start_time".into(), serde_json::json!("24:00"))
        ]))
        .is_err());
    assert_eq!(coordinator.settings().unwrap().proxy_mode, "system");
    assert_eq!(coordinator.latest_sequence().unwrap(), sequence);
}

#[test]
fn settings_ffmpeg_path_set_and_clear_take_effect_without_environment_mutation() {
    let root = audit_test_root("ffmpeg-settings");
    let path = root.join("ffmpeg.exe");
    fs::write(&path, b"configured helper fixture").unwrap();
    let env = std::env::var_os("HLS_FFMPEG");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("ffmpeg_path", serde_json::json!(path))
        .unwrap();
    assert_eq!(crate::media::locate_ffmpeg(), Some(path.clone()));
    coordinator
        .set_setting("ffmpeg_path", serde_json::json!(""))
        .unwrap();
    assert_ne!(crate::media::locate_ffmpeg(), Some(path));
    assert_eq!(std::env::var_os("HLS_FFMPEG"), env);
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn auxiliary_network_routes_honor_proxy_mode_and_per_target_bypass() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("proxy_mode".into(), serde_json::json!("manual")),
            (
                "proxy_url".into(),
                serde_json::json!("http://127.0.0.1:8888"),
            ),
            (
                "proxy_bypass".into(),
                serde_json::json!("api.github.com,cdn.test"),
            ),
        ]))
        .unwrap();
    let settings = coordinator.settings().unwrap();
    assert_eq!(
        settings_proxy(&settings, crate::updater::LATEST_API),
        crate::net_policy::DIRECT_PROXY_SENTINEL
    );
    assert_eq!(
        settings_proxy(&settings, "https://github.com/releases/installer.msi"),
        settings.proxy_url
    );
    assert_eq!(
        settings_proxy(&settings, "https://cdn.test/source.torrent"),
        crate::net_policy::DIRECT_PROXY_SENTINEL
    );
    assert_eq!(
        settings_proxy(&settings, "https://page.test/watch"),
        settings.proxy_url
    );
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .unwrap();
    assert_eq!(
        settings_proxy(&coordinator.settings().unwrap(), "https://page.test/watch"),
        crate::net_policy::DIRECT_PROXY_SENTINEL
    );
    coordinator
        .set_setting("proxy_mode", serde_json::json!("system"))
        .unwrap();
    assert_eq!(
        settings_proxy(&coordinator.settings().unwrap(), "https://page.test/watch"),
        crate::net_policy::SYSTEM_PROXY_SENTINEL
    );
}

#[test]
fn queue_drain_accepts_done_and_completed_but_rejects_pending_or_failed_tasks() {
    let mut tasks = vec![
        crate::TaskSnapshot {
            status: "done".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
        crate::TaskSnapshot {
            status: "completed".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
    ];
    assert!(queue_is_successfully_drained(&tasks, "q"));
    assert!(!queue_is_successfully_drained(&tasks, "missing"));
    for status in ["failed", "queued", "canceled"] {
        tasks[1].status = status.into();
        assert!(!queue_is_successfully_drained(&tasks, "q"));
    }
}

#[test]
fn task_torrent_file_queries_hydrate_redirect_replay_context() {
    let root = audit_test_root("torrent-replay");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let source = format!("http://{address}/source.torrent");
    let origin = format!("http://{address}");
    let metadata = b"d4:infod5:filesld6:lengthi3e4:pathl7:one.bineed6:lengthi2e4:pathl3:dir7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaee";
    let server = thread::spawn(move || {
        let mut private_requests = 0;
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            if request.contains("/source.torrent") {
                write!(stream, "HTTP/1.1 302 Found\r\nLocation: /private/multi.torrent\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("x-replay-token: redirect-secret"),
                    "{request}"
                );
                private_requests += 1;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    metadata.len()
                )
                .unwrap();
                stream.write_all(metadata).unwrap();
            }
        }
        assert_eq!(private_requests, 2);
    });
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator.store_credential("torrent-replay", &serde_json::json!({"request_contexts": {
        origin: {"path_contexts": {"/private/": {"request_headers": {"X-Replay-Token": "redirect-secret"}}}}
    }}).to_string(), "browser_replay").unwrap();
    coordinator
        .dispatch_created(TaskSpec {
            url: source,
            filename: "demo".into(),
            download_dir: root.to_string_lossy().into_owned(),
            resource_kind: ResourceKind::Torrent,
            credential_ref: Some("torrent-replay".into()),
            start_paused: true,
            ..Default::default()
        })
        .unwrap();
    let listed = task_torrent_files(&coordinator, "task-1").unwrap();
    let selections = listed
        .iter()
        .find_map(|event| match &event.event {
            CoreEvent::TaskTorrentFiles { selections, .. } => Some(selections.clone()),
            _ => None,
        })
        .unwrap();
    set_task_torrent_files(&coordinator, "task-1", &selections).unwrap();
    server.join().unwrap();
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_torrent_publication_uses_the_latest_output_policy_spec() {
    let root = audit_test_root("torrent-latest-spec");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let seed = format!("http://{}/payload", listener.local_addr().unwrap());
    let payload = b"abcde";
    let mut torrent = format!(
        "d8:url-list{}:{}4:infod6:lengthi5e4:name4:demo12:piece lengthi16e6:pieces20:",
        seed.len(),
        seed
    )
    .into_bytes();
    torrent.extend_from_slice(&crate::crypto_lite::sha1(payload));
    torrent.extend_from_slice(b"ee");
    let source = root.join("source.torrent");
    fs::write(&source, torrent).unwrap();
    fs::write(root.join("demo"), b"existing output").unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("existing_file_policy", serde_json::json!("overwrite"))
        .unwrap();
    coordinator
        .set_setting("bt_enable_dht", serde_json::json!(false))
        .unwrap();
    let spec = TaskSpec {
        url: source.to_string_lossy().into_owned(),
        filename: "demo".into(),
        download_dir: root.to_string_lossy().into_owned(),
        resource_kind: ResourceKind::Torrent,
        ..Default::default()
    };
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    let core = coordinator.core();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::SeqCst) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            // Change the persisted spec before the final payload arrives, without timing a worker race.
            let mut guard = core.lock().unwrap();
            let mut latest = guard.task_spec("task-1").unwrap().clone();
            latest.preserve_existing_output = true;
            guard.replace_spec("task-1", latest).unwrap();
            drop(guard);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            )
            .unwrap();
            if !first.starts_with("HEAD ") {
                stream.write_all(payload).unwrap();
            }
        }
    });
    let result = run_task_with_throttle(coordinator.core(), "task-1", spec, String::new());
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    result.unwrap();
    assert_eq!(fs::read(root.join("demo")).unwrap(), b"existing output");
    let output = coordinator
        .lock()
        .unwrap()
        .snapshot("task-1")
        .unwrap()
        .output_path
        .clone();
    assert_ne!(Path::new(&output), root.join("demo"));
    assert_eq!(fs::read(output).unwrap(), payload);
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn torrent_probe_and_selection_use_the_configured_http_proxy() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            assert!(
                request.starts_with("GET http://torrent.invalid/source.torrent "),
                "{request}"
            );
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            let bytes = b"d4:infod6:lengthi3e4:name8:file.bin12:piece lengthi16e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(bytes).unwrap();
        }
    });
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("proxy_mode".into(), serde_json::json!("manual")),
            ("proxy_url".into(), serde_json::json!(proxy)),
        ]))
        .unwrap();
    assert!(
        probe_torrent_command(&coordinator, "http://torrent.invalid/source.torrent")
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, CoreEvent::TorrentProbeResult { .. }))
    );
    assert!(select_torrent_files_command(
        &coordinator,
        "http://torrent.invalid/source.torrent",
        &[]
    )
    .unwrap()
    .iter()
    .any(|event| matches!(event.event, CoreEvent::TorrentSelectionResult { .. })));
    server.join().unwrap();
}

#[test]
fn untrusted_relative_download_dir_also_constrains_default_work_dir() {
    let (coordinator, root) = audit_handoff_fixture("relative-work-dir");
    let expected = root.join("downloads").join("sub");
    audit_accept_handoff(&coordinator, "sub".into(), false).unwrap();
    let handoff_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .clone();
    assert_eq!(Path::new(&handoff_spec.download_dir), expected);
    assert_eq!(Path::new(&handoff_spec.work_dir), expected);

    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://other.test/b.bin".into(),
                filename: "b.bin".into(),
                download_dir: "sub".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let create_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-2")
        .unwrap()
        .clone();
    assert_eq!(Path::new(&create_spec.download_dir), expected);
    assert_eq!(Path::new(&create_spec.work_dir), expected);
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn coordinator_startup_syncs_persisted_ffmpeg_and_task_defaults_do_not_clobber_it() {
    let root = audit_test_root("ffmpeg-sync");
    let persisted = root.join("a").join("ffmpeg.exe");
    let updated = root.join("b").join("ffmpeg.exe");
    fs::create_dir_all(persisted.parent().unwrap()).unwrap();
    fs::create_dir_all(updated.parent().unwrap()).unwrap();
    fs::write(&persisted, b"ffmpeg a").unwrap();
    fs::write(&updated, b"ffmpeg b").unwrap();

    let mut core = PersistentCore::in_memory().unwrap();
    core.store_mut()
        .set_setting("ffmpeg_path", persisted.to_string_lossy().as_ref())
        .unwrap();
    let coordinator = CoreCoordinator::new(core);
    assert_eq!(crate::media::locate_ffmpeg(), Some(persisted.clone()));

    coordinator
        .set_setting("ffmpeg_path", serde_json::json!(updated))
        .unwrap();
    assert_eq!(crate::media::locate_ffmpeg(), Some(updated.clone()));

    // Overriding the runtime cache directly verifies task preparation never writes it back.
    crate::media::set_configured_ffmpeg(Some(persisted.clone()));
    let _ = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/video.m3u8".into(),
            filename: "video.mp4".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(crate::media::locate_ffmpeg(), Some(persisted));

    coordinator
        .set_setting("ffmpeg_path", serde_json::json!(""))
        .unwrap();
    assert_ne!(crate::media::locate_ffmpeg(), Some(updated));
    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn apply_defaults_allows_work_dir_in_settings_download_dir_when_temp_dir_empty_but_rejects_escapes()
{
    let root = std::env::temp_dir().join(format!(
        "hls-v7-work-dir-fallback-{}-{}",
        std::process::id(),
        handoff_now_ms()
    ));
    let nested = root.join("sub").join("updates");
    fs::create_dir_all(&nested).unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("download_dir", serde_json::json!(root))
        .unwrap();
    coordinator
        .set_setting("temp_dir", serde_json::json!(""))
        .unwrap();

    let allowed = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/a.bin".into(),
            filename: "a.bin".into(),
            download_dir: nested.to_string_lossy().into_owned(),
            work_dir: root.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(Path::new(&allowed.work_dir), root);

    // Path traversal and outside-root work_dir must still be rejected strictly.
    assert!(coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/a.bin".into(),
            filename: "a.bin".into(),
            download_dir: nested.to_string_lossy().into_owned(),
            work_dir: format!("{}/../outside", root.display()),
            ..Default::default()
        })
        .is_err());
    let outside = root.parent().unwrap().join("hls-outside-work-dir");
    assert!(coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.test/a.bin".into(),
            filename: "a.bin".into(),
            download_dir: nested.to_string_lossy().into_owned(),
            work_dir: outside.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .is_err());

    drop(coordinator);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn queue_drain_ignores_paused_and_historical_failures_while_blocking_active_batch_failures() {
    let profile = QueueProfile {
        id: "q".into(),
        name: "测试队列".into(),
        completion_action: "sleep".into(),
        ..QueueProfile::default()
    };
    let task_spec = TaskSpec {
        queue_id: "q".into(),
        ..spec()
    };
    let tasks_with_paused = vec![
        crate::TaskSnapshot {
            task_id: "t-done".into(),
            status: "completed".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
        crate::TaskSnapshot {
            task_id: "t-paused".into(),
            status: "paused".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
    ];
    assert!(queue_is_successfully_drained(&tasks_with_paused, "q"));
    assert_eq!(
        queue_completion_decision(
            &tasks_with_paused,
            std::slice::from_ref(&profile),
            &task_spec
        ),
        Some(("sleep".into(), "队列：测试队列".into()))
    );
    // Paused-only queue (no completed task) must not trigger completion power action.
    assert!(!queue_is_successfully_drained(&tasks_with_paused[1..], "q"));

    // Historical unrelated failed/canceled/paused tasks must not block a fresh batch completion.
    let tasks_with_history = vec![
        crate::TaskSnapshot {
            task_id: "old-failed".into(),
            status: "failed".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
        crate::TaskSnapshot {
            task_id: "old-canceled".into(),
            status: "canceled".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
        crate::TaskSnapshot {
            task_id: "old-paused".into(),
            status: "paused".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
        crate::TaskSnapshot {
            task_id: "fresh-done".into(),
            status: "completed".into(),
            queue_id: "q".into(),
            ..Default::default()
        },
    ];
    assert_eq!(
        queue_completion_decision_for_batch(
            &tasks_with_history,
            std::slice::from_ref(&profile),
            &task_spec,
            None,
        ),
        Some(("sleep".into(), "队列：测试队列".into()))
    );

    // An active-batch failure MUST still block the completion power action.
    let disrupted = std::collections::BTreeSet::from(["old-failed".to_string()]);
    assert!(queue_completion_decision_for_batch(
        &tasks_with_history,
        std::slice::from_ref(&profile),
        &task_spec,
        Some(&disrupted),
    )
    .is_none());

    // End-to-end PersistentCore tracking: historical failed task loaded from disk does not block,
    // while a failure in the active batch does block until a fresh run starts from idle.
    let root = audit_test_root("batch-power-history");
    let db = root.join("core.db");
    {
        let mut core = PersistentCore::open(&db).unwrap();
        core.handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/old.bin".into(),
                filename: "old.bin".into(),
                queue_id: "default".into(),
                ..Default::default()
            },
        })
        .unwrap();
        core.report_failure(
            "task-1",
            crate::TaskFailure {
                code: "DOWNLOAD_FAILED".into(),
                message: "boom".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let mut reopened = PersistentCore::open(&db).unwrap();
    assert!(reopened.batch_disrupted_for_queue("default").is_none());
    reopened
        .handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/new1.bin".into(),
                filename: "new1.bin".into(),
                queue_id: "default".into(),
                ..Default::default()
            },
        })
        .unwrap();
    reopened
        .handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/new2.bin".into(),
                filename: "new2.bin".into(),
                queue_id: "default".into(),
                ..Default::default()
            },
        })
        .unwrap();
    // task-2 fails while task-3 is still queued -> active batch is disrupted.
    reopened
        .report_failure(
            "task-2",
            crate::TaskFailure {
                code: "DOWNLOAD_FAILED".into(),
                message: "batch fail".into(),
                ..Default::default()
            },
        )
        .unwrap();
    reopened
        .handle(CoreCommand::UpdateProgress {
            task_id: "task-3".into(),
            downloaded_bytes: 10,
            total_bytes: Some(10),
            speed_bytes_per_sec: 0,
            stage: "finished".into(),
            status: "completed".into(),
        })
        .unwrap();
    assert!(reopened
        .batch_disrupted_for_queue("default")
        .is_some_and(|set| set.contains("task-2")));
    let default_profile = QueueProfile {
        id: "default".into(),
        name: "默认队列".into(),
        completion_action: "sleep".into(),
        ..QueueProfile::default()
    };
    let default_spec = TaskSpec {
        queue_id: "default".into(),
        ..spec()
    };
    assert!(queue_completion_decision_for_batch(
        &reopened.tasks(),
        std::slice::from_ref(&default_profile),
        &default_spec,
        reopened.batch_disrupted_for_queue("default"),
    )
    .is_none());

    // Starting a new task from idle clears stale batch_disrupted so the new batch can trigger power action.
    reopened
        .handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/new3.bin".into(),
                filename: "new3.bin".into(),
                queue_id: "default".into(),
                ..Default::default()
            },
        })
        .unwrap();
    reopened
        .handle(CoreCommand::UpdateProgress {
            task_id: "task-4".into(),
            downloaded_bytes: 10,
            total_bytes: Some(10),
            speed_bytes_per_sec: 0,
            stage: "finished".into(),
            status: "completed".into(),
        })
        .unwrap();
    assert_eq!(
        queue_completion_decision_for_batch(
            &reopened.tasks(),
            std::slice::from_ref(&default_profile),
            &default_spec,
            reopened.batch_disrupted_for_queue("default"),
        ),
        Some(("sleep".into(), "队列：默认队列".into()))
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accept_handoff_honors_start_paused_and_custom_queue_id() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([
                QueueProfile {
                    id: "default".into(),
                    name: "默认队列".into(),
                    ..Default::default()
                },
                QueueProfile {
                    id: "night-media".into(),
                    name: "夜间媒体".into(),
                    ..Default::default()
                }
            ]),
        )
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.test/later.mp4".into(),
                handoff_id: "handoff-later".into(),
                filename: "later.mp4".into(),
                ..Default::default()
            },
        })
        .unwrap();

    // Unknown queue_id must be rejected without consuming the pending handoff.
    assert!(coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-later".into(),
            filename: "later.mp4".into(),
            download_dir: String::new(),
            allow_duplicate: false,
            trusted_ui: true,
            start_paused: true,
            queue_id: "missing-queue".into(),
            spec_override: None,
        })
        .is_err());

    let events = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-later".into(),
            filename: "later.mp4".into(),
            download_dir: String::new(),
            allow_duplicate: false,
            trusted_ui: true,
            start_paused: true,
            queue_id: "night-media".into(),
            spec_override: None,
        })
        .unwrap();
    let created = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.clone()),
            _ => None,
        })
        .expect("AcceptHandoff should create a task");
    assert_eq!(created.status, "paused");
    assert_eq!(created.queue_id, "night-media");
    assert!(
        !coordinator.worker_is_active(&created.task_id).unwrap(),
        "start_paused handoff must not spawn a transfer worker"
    );
    let stored_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&created.task_id)
        .cloned()
        .unwrap();
    assert!(stored_spec.start_paused);
    assert_eq!(stored_spec.queue_id, "night-media");
}

#[test]
fn handoff_accept_persists_source_page_url_into_spec_and_snapshot() {
    let page_url = "https://video.example.test/watch?v=42";
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.example.test/video.mp4".into(),
                source_page_url: page_url.into(),
                handoff_id: "handoff-source-page".into(),
                filename: "video.mp4".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let events = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-source-page".into(),
            filename: "video.mp4".into(),
            download_dir: String::new(),
            allow_duplicate: false,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: None,
        })
        .unwrap();
    let created = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.clone()),
            _ => None,
        })
        .expect("AcceptHandoff should emit TaskCreated");
    assert_eq!(created.source_page_url, page_url);
    let stored_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&created.task_id)
        .cloned()
        .unwrap();
    assert_eq!(stored_spec.source_page_url, page_url);
    let listed = coordinator.tasks().unwrap();
    assert_eq!(listed[0].source_page_url, page_url);

    // Direct CoreRuntime::accept_handoff also preserves source_page_url.
    let mut runtime = crate::CoreRuntime::new();
    runtime.handle(CoreCommand::OfferResource {
        offer: crate::ResourceOffer {
            url: "https://cdn.example.test/direct.mp4".into(),
            source_page_url: page_url.into(),
            handoff_id: "handoff-direct".into(),
            filename: "direct.mp4".into(),
            ..Default::default()
        },
    });
    let runtime_events = runtime.handle(CoreCommand::AcceptHandoff {
        handoff_id: "handoff-direct".into(),
        filename: "direct.mp4".into(),
        download_dir: String::new(),
        allow_duplicate: false,
        trusted_ui: true,
        start_paused: true,
        queue_id: String::new(),
        spec_override: None,
    });
    let runtime_created = runtime_events
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(runtime_created.source_page_url, page_url);
    assert_eq!(
        runtime
            .task_spec(&runtime_created.task_id)
            .unwrap()
            .source_page_url,
        page_url
    );
}

#[test]
fn arm_task_refresh_updates_existing_task_via_offer_resource_without_duplicate() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let root = std::env::temp_dir().join(format!(
        "hls-arm-refresh-offer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let download_dir = root.join("downloads");
    fs::create_dir_all(&download_dir).unwrap();
    let payload: &'static [u8] = b"refreshed-payload-0123456789abcdef";
    let seen_cookie = Arc::new(Mutex::new(String::new()));
    let server_cookie = Arc::clone(&seen_cookie);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                continue;
            }
            let mut cookie = String::new();
            let mut range = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("cookie:") {
                    cookie = value.trim().to_string();
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                    range = Some(value.trim().to_string());
                }
            }
            let mut stream = reader.into_inner();
            if request_line.starts_with("GET /fresh.bin?sig=new ") {
                if !cookie.is_empty() {
                    *server_cookie.lock().unwrap() = cookie;
                }
                if let Some(spec) = range.and_then(|v| v.strip_prefix("bytes=").map(str::to_string))
                {
                    let (start_s, end_s) = spec.split_once('-').unwrap();
                    let start: usize = start_s.parse().unwrap();
                    let end: usize = if end_s.is_empty() {
                        payload.len() - 1
                    } else {
                        end_s.parse::<usize>().unwrap().min(payload.len() - 1)
                    };
                    let slice = &payload[start..=end];
                    let _ = write!(
                        stream,
                        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nETag: \"fresh-v1\"\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len(),
                        slice.len()
                    );
                    let _ = stream.write_all(slice);
                } else {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nETag: \"fresh-v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = stream.write_all(payload);
                }
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 410 Gone\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        }
    });
    let _ = server;

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(download_dir.to_string_lossy()),
        )
        .unwrap();

    // Create a paused task and simulate a prior expired-URL failure on it.
    let expired_url = format!("http://{addr}/expired.bin?sig=old");
    let created_events = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: expired_url,
                filename: "fresh.bin".into(),
                download_dir: download_dir.to_string_lossy().into_owned(),
                start_paused: true,
                source_page_url: "https://origin.test/old-page".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = created_events
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    coordinator
        .lock()
        .unwrap()
        .report_failure(
            &task_id,
            crate::TaskFailure {
                code: "HTTP_410".into(),
                message: "HTTP 410".into(),
                http_status: Some(410),
                ..Default::default()
            },
        )
        .unwrap();

    // Store fresh browser credentials in vault.
    let cred_ref = "cred-fresh-refresh".to_string();
    let replay_plain = serde_json::json!({"cookie": "sid=fresh"}).to_string();
    let protected = if cfg!(windows) {
        crate::CredentialVault.protect(&replay_plain).unwrap()
    } else {
        replay_plain
    };
    coordinator
        .store_credential(&cred_ref, &protected, "browser_replay")
        .unwrap();

    // Arm task refresh.
    let arm_events = coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: task_id.clone(),
        })
        .unwrap();
    assert!(arm_events.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::Toast { level, message }
            if level == "info" && message == "已进入刷新地址模式：请在浏览器重新点击该下载链接"
    )));

    // Subsequent browser OfferResource must refresh this task in place without emitting HandoffOffered.
    let fresh_url = format!("http://{addr}/fresh.bin?sig=new");
    let offer_events = coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: fresh_url.clone(),
                source_page_url: "https://origin.test/new-page".into(),
                credential_ref: Some(cred_ref.clone()),
                replay_context_ref: Some("replay-ctx-2".into()),
                request_method: "GET".into(),
                handoff_id: "handoff-refresh-1".into(),
                filename: "fresh.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert!(
        !offer_events
            .iter()
            .any(|envelope| matches!(envelope.event, CoreEvent::HandoffOffered { .. })),
        "consumed refresh offer must not open Confirm dialog: {offer_events:?}"
    );
    assert!(offer_events.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::HandoffResolved { handoff_id, task_id: Some(resolved) }
            if handoff_id == "handoff-refresh-1" && resolved == &task_id
    )));
    assert!(offer_events.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::Toast { level, message }
            if level == "info" && message == "下载地址与凭据已刷新，正在断点续传"
    )));
    assert!(offer_events.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::TaskUpdated { snapshot }
            if snapshot.task_id == task_id
                && snapshot.url == fresh_url
                && snapshot.source_page_url == "https://origin.test/new-page"
                && snapshot.error_code.is_none()
                && snapshot.error_message.is_none()
                && snapshot.http_status.is_none()
    )));

    // Only one task exists; spec carries refreshed URL and credential/replay refs.
    assert_eq!(coordinator.tasks().unwrap().len(), 1);
    let refreshed_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .cloned()
        .unwrap();
    assert_eq!(refreshed_spec.url, fresh_url);
    assert_eq!(
        refreshed_spec.credential_ref.as_deref(),
        Some(cred_ref.as_str())
    );
    assert_eq!(
        refreshed_spec.replay_context_ref.as_deref(),
        Some("replay-ctx-2")
    );
    assert_eq!(
        refreshed_spec.source_page_url,
        "https://origin.test/new-page"
    );

    // Durable handoff row is recorded as accepted with the armed task_id.
    let handoff_row = coordinator
        .query_handoffs(Some("handoff-refresh-1"), None, false)
        .unwrap()
        .pop()
        .expect("durable handoff row must exist");
    let row_json: serde_json::Value = serde_json::from_str(&handoff_row).unwrap();
    assert_eq!(row_json["status"], "accepted");
    assert_eq!(row_json["task_id"], task_id);

    // Wait for the auto-resumed task to finish downloading from /fresh.bin.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let task = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|item| item.task_id == task_id)
            .unwrap();
        if matches!(task.status.as_str(), "completed" | "done") {
            break;
        }
        assert!(
            task.status != "failed",
            "refreshed task failed: {:?}",
            task.error_message
        );
        assert!(
            Instant::now() < deadline,
            "refreshed task did not finish: {task:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(fs::read(download_dir.join("fresh.bin")).unwrap(), payload);
    assert_eq!(seen_cookie.lock().unwrap().as_str(), "sid=fresh");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn arm_task_refresh_consumes_durable_admit_handoff_and_marks_row_accepted() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    let created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/old.bin".into(),
                filename: "old.bin".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = created
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();

    coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: task_id.clone(),
        })
        .unwrap();

    let handoff = crate::contract::BrowserHandoff {
        id: "handoff-admit-refresh".into(),
        offer: crate::ResourceOffer {
            url: "https://cdn.example.test/new.bin?sig=fresh".into(),
            source_page_url: "https://site.example.test/download".into(),
            credential_ref: Some("cred-browser-99".into()),
            replay_context_ref: Some("replay-browser-99".into()),
            request_method: "GET".into(),
            handoff_id: "handoff-admit-refresh".into(),
            filename: "old.bin".into(),
            ..Default::default()
        },
        filename: "old.bin".into(),
        title: "old.bin".into(),
        mime_type: "application/octet-stream".into(),
        size: 1024,
        status: "pending".into(),
        presentation: "queued".into(),
        task_id: None,
        created_at_ms: 1_700_000_000_000,
        request_id: "req-admit-refresh-1".into(),
        suppression: None,
    };
    let encoded = serde_json::to_string(&handoff).unwrap();
    let seq_before = coordinator.latest_sequence().unwrap();
    let receipt_json = coordinator.admit_handoff(&encoded).unwrap();
    let receipt: crate::contract::BrowserHandoff = serde_json::from_str(&receipt_json).unwrap();
    assert_eq!(receipt.status, "accepted");
    assert_eq!(receipt.task_id.as_deref(), Some(task_id.as_str()));
    assert_eq!(
        receipt.offer.credential_ref.as_deref(),
        Some("cred-browser-99")
    );
    // No pending handoff row was created for the Confirm dialog.
    assert!(coordinator
        .query_handoffs(None, None, true)
        .unwrap()
        .is_empty());
    let events = coordinator.events_after(seq_before, 64).unwrap();
    assert!(!events
        .iter()
        .any(|envelope| matches!(envelope.event, CoreEvent::HandoffOffered { .. })));
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        CoreEvent::HandoffResolved { handoff_id, task_id: Some(resolved) }
            if handoff_id == "handoff-admit-refresh" && resolved == &task_id
    )));
    // Idempotent retry returns the same accepted receipt.
    let retry_json = coordinator.admit_handoff(&encoded).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&retry_json).unwrap(),
        serde_json::from_str::<serde_json::Value>(&receipt_json).unwrap()
    );

    // Pause/cancel worker cleanly before dropping coordinator.
    let _ = coordinator.dispatch(CoreCommand::TaskAction {
        task_id: task_id.clone(),
        action: "cancel".into(),
    });
    let _ = coordinator.wait_for_worker_stop(&task_id);
}

#[test]
fn arm_task_refresh_expired_or_disarmed_falls_back_to_normal_handoff() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    assert!(coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: "task-missing".into(),
        })
        .is_err());

    let created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/original.bin".into(),
                filename: "original.bin".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = created
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();

    // Arm then explicitly disarm with empty task_id.
    coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: task_id.clone(),
        })
        .unwrap();
    assert!(coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: String::new(),
        })
        .unwrap()
        .is_empty());

    let disarmed_events = coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.example.test/other.bin".into(),
                handoff_id: "handoff-disarmed".into(),
                filename: "other.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert!(disarmed_events
        .iter()
        .any(|envelope| matches!(envelope.event, CoreEvent::HandoffOffered { .. })));
    assert_eq!(
        coordinator.lock().unwrap().task_spec(&task_id).unwrap().url,
        "https://cdn.example.test/original.bin"
    );

    // Arm then expire TTL -> OfferResource must fall back to normal HandoffOffered.
    coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: task_id.clone(),
        })
        .unwrap();
    coordinator.expire_armed_refresh_for_test();
    let expired_events = coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.example.test/expired-fallback.bin".into(),
                handoff_id: "handoff-expired".into(),
                filename: "expired-fallback.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert!(expired_events
        .iter()
        .any(|envelope| matches!(envelope.event, CoreEvent::HandoffOffered { .. })));
    assert_eq!(
        coordinator.lock().unwrap().task_spec(&task_id).unwrap().url,
        "https://cdn.example.test/original.bin"
    );
}

#[test]
fn task_throttle_context_respects_speed_limit_enabled_toggle() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let task_spec = spec();
    coordinator
        .set_setting("download_speed_limit_kib", serde_json::json!(640))
        .unwrap();
    let ctx_on = task_throttle_context(&coordinator.core(), "task-1", &task_spec).unwrap();
    assert_eq!(ctx_on.global_limit_kib, 640);

    coordinator
        .set_setting("download_speed_limit_enabled", serde_json::json!(false))
        .unwrap();
    let ctx_off = task_throttle_context(&coordinator.core(), "task-1", &task_spec).unwrap();
    assert_eq!(ctx_off.global_limit_kib, 0);
    assert_eq!(coordinator.settings().unwrap().speed_limit_kib, 640);

    coordinator
        .set_setting("download_speed_limit_enabled", serde_json::json!(true))
        .unwrap();
    let ctx_restored = task_throttle_context(&coordinator.core(), "task-1", &task_spec).unwrap();
    assert_eq!(ctx_restored.global_limit_kib, 640);

    coordinator
        .set_setting("download_speed_limit_kib", serde_json::json!(0))
        .unwrap();
}

#[test]
fn dual_track_audio_url_downloads_both_streams_and_muxes_to_mp4() {
    let Some(ffmpeg) = crate::media::locate_ffmpeg() else {
        eprintln!("skipped: no FFmpeg on PATH/HLS_FFMPEG");
        return;
    };
    let root = lifecycle_test_root("dual-track-audio-url");
    let gen_dir = root.join("fixtures");
    fs::create_dir_all(&gen_dir).unwrap();
    let video_fixture = gen_dir.join("video_src.m4s");
    let audio_fixture = gen_dir.join("audio_src.m4s");

    let gen_video = std::process::Command::new(&ffmpeg)
        .args([
            "-y",
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=64x48:rate=10:duration=1",
            "-c:v",
            "mpeg4",
            "-f",
            "mp4",
        ])
        .arg(&video_fixture)
        .output()
        .unwrap();
    assert!(
        gen_video.status.success(),
        "{}",
        String::from_utf8_lossy(&gen_video.stderr)
    );

    let gen_audio = std::process::Command::new(&ffmpeg)
        .args([
            "-y",
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "aac",
            "-f",
            "mp4",
        ])
        .arg(&audio_fixture)
        .output()
        .unwrap();
    assert!(
        gen_audio.status.success(),
        "{}",
        String::from_utf8_lossy(&gen_audio.stderr)
    );

    let video_bytes = Arc::new(fs::read(&video_fixture).unwrap());
    let audio_bytes = Arc::new(fs::read(&audio_fixture).unwrap());
    let audio_headers_seen = Arc::new(Mutex::new(Vec::<String>::new()));

    let spawn_range_server =
        |payload: Arc<Vec<u8>>, record_headers: Option<Arc<Mutex<Vec<String>>>>| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                while let Ok((stream, _)) = listener.accept() {
                    let payload = Arc::clone(&payload);
                    let record_headers = record_headers.clone();
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut headers_raw = String::new();
                        let mut range_hdr = None;
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0
                                || line.trim().is_empty()
                            {
                                break;
                            }
                            if line.to_ascii_lowercase().starts_with("range:") {
                                range_hdr =
                                    Some(line.split_once(':').unwrap().1.trim().to_string());
                            }
                            headers_raw.push_str(&line);
                        }
                        if headers_raw.is_empty() {
                            return;
                        }
                        if let Some(log) = record_headers {
                            log.lock().unwrap().push(headers_raw.clone());
                        }
                        let mut out = reader.into_inner();
                        if let Some((start, end)) = range_hdr.and_then(|r| {
                            let (s, e) = r.strip_prefix("bytes=")?.split_once('-')?;
                            let start = s.parse::<usize>().ok()?;
                            let end = if e.is_empty() {
                                payload.len() - 1
                            } else {
                                e.parse::<usize>().ok()?.min(payload.len() - 1)
                            };
                            Some((start, end))
                        }) {
                            let slice = &payload[start..=end];
                            let header = format!(
                                "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                payload.len(),
                                slice.len()
                            );
                            let _ = out.write_all(header.as_bytes());
                            let _ = out.write_all(slice);
                        } else {
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                payload.len()
                            );
                            let _ = out.write_all(header.as_bytes());
                            let _ = out.write_all(&payload);
                        }
                    });
                }
            });
            addr
        };

    let video_addr = spawn_range_server(Arc::clone(&video_bytes), None);
    let audio_addr = spawn_range_server(
        Arc::clone(&audio_bytes),
        Some(Arc::clone(&audio_headers_seen)),
    );

    let out_dir = root.join("downloads");
    fs::create_dir_all(&out_dir).unwrap();

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(out_dir.to_string_lossy().as_ref()),
        )
        .unwrap();
    coordinator
        .set_setting(
            "default_referer",
            serde_json::json!("https://www.bilibili.com/video/BV1test"),
        )
        .unwrap();
    coordinator
        .set_setting(
            "default_user_agent",
            serde_json::json!("DualTrackTestUA/1.0"),
        )
        .unwrap();

    // Unsafe audio_url scheme must be rejected before starting a task.
    let bad_err = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{video_addr}/30280.m4s"),
                audio_url: "file:///C:/Windows/win.ini".into(),
                filename: "bad.m4s".into(),
                download_dir: out_dir.to_string_lossy().into_owned(),
                ..Default::default()
            },
        })
        .unwrap_err();
    assert!(bad_err.contains("音轨"), "unexpected error: {bad_err}");

    // OfferResource + AcceptHandoff with `.m4s` filename and video-only size.
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "handoff-dual-1".into(),
                url: format!("http://{video_addr}/30280.m4s?upsig=v_sig"),
                audio_url: format!("http://{audio_addr}/30216.m4s?upsig=a_sig"),
                resource_kind: ResourceKind::File,
                filename: "30280.m4s".into(),
                title: "Dual Track Video".into(),
                source_page_url: "https://www.bilibili.com/video/BV1test".into(),
                size: video_bytes.len() as u64,
                ..Default::default()
            },
        })
        .unwrap();

    let accepted = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-dual-1".into(),
            filename: String::new(),
            download_dir: out_dir.to_string_lossy().into_owned(),
            allow_duplicate: true,
            trusted_ui: true,
            start_paused: false,
            queue_id: String::new(),
            spec_override: None,
        })
        .unwrap();

    let task_id = accepted
        .iter()
        .find_map(|env| match &env.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .expect("TaskCreated event");

    let completed = wait_for_task_status(&coordinator, "completed");
    assert_eq!(completed.task_id, task_id);
    assert_eq!(completed.filename, "30280.mp4");
    coordinator.wait_for_worker_stop(&task_id).unwrap();

    let final_mp4 = out_dir.join("30280.mp4");
    assert!(
        final_mp4.is_file(),
        "expected muxed output at {}",
        final_mp4.display()
    );

    // Verify task-scoped video.part.m4s and audio.part.m4s were cleaned up.
    let task_work_dir = out_dir.join(".hls-tasks").join(&task_id);
    assert!(!task_work_dir.join("video.part.m4s").exists());
    assert!(!task_work_dir.join("audio.part.m4s").exists());

    // Verify cross-origin audio server received Referer & User-Agent without leaking cookies.
    {
        let seen = audio_headers_seen.lock().unwrap();
        assert!(!seen.is_empty(), "audio server must be contacted");
        let first_req = seen[0].to_ascii_lowercase();
        assert!(
            first_req.contains("referer: https://www.bilibili.com/video/bv1test"),
            "audio request missing Referer: {}",
            seen[0]
        );
        assert!(
            first_req.contains("user-agent: dualtracktestua/1.0"),
            "audio request missing User-Agent: {}",
            seen[0]
        );
        assert!(!first_req.contains("cookie:"));
    }

    // Verify muxed MP4 contains both video (stream 0) and audio (stream 1) frames.
    let decoded = std::process::Command::new(&ffmpeg)
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(&final_mp4)
        .args(["-f", "framecrc", "-"])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    let stdout = String::from_utf8_lossy(&decoded.stdout);
    assert!(
        stdout.lines().any(|line| line.starts_with("0,")),
        "muxed MP4 missing video frames:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.starts_with("1,")),
        "muxed MP4 missing audio frames:\n{stdout}"
    );

    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn refresh_task_url_preserves_halfway_checkpoint_without_redownloading_completed_ranges() {
    let root = lifecycle_test_root("refresh-preserves-checkpoint");
    let out_dir = root.join("downloads");
    fs::create_dir_all(&out_dir).unwrap();

    let total_len: usize = 256 * 1024;
    let half_len: usize = total_len / 2;
    let payload: Arc<Vec<u8>> = Arc::new((0..total_len).map(|i| (i % 251) as u8).collect());
    let new_token_ranges = Arc::new(Mutex::new(Vec::<(usize, usize)>::new()));
    let new_token_ranges_srv = Arc::clone(&new_token_ranges);
    let payload_srv = Arc::clone(&payload);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let payload = Arc::clone(&payload_srv);
            let new_ranges = Arc::clone(&new_token_ranges_srv);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut req_line = String::new();
                if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
                    return;
                }
                let mut range_hdr = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if line.to_ascii_lowercase().starts_with("range:") {
                        range_hdr = Some(line.split_once(':').unwrap().1.trim().to_string());
                    }
                }
                let mut out = reader.into_inner();
                let is_old_token = req_line.contains("token=old_sig");
                let is_new_token = req_line.contains("token=new_sig");
                if !is_old_token && !is_new_token {
                    let _ = out.write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    return;
                }
                let Some((start, req_end)) = range_hdr.and_then(|r| {
                    let (s, e) = r.strip_prefix("bytes=")?.split_once('-')?;
                    let start = s.parse::<usize>().ok()?;
                    let end = if e.is_empty() {
                        payload.len() - 1
                    } else {
                        e.parse::<usize>().ok()?.min(payload.len() - 1)
                    };
                    Some((start, end))
                }) else {
                    let _ = out.write_all(
                        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    return;
                };

                if is_old_token {
                    if start >= half_len {
                        // Presigned URL expired right after delivering the first 50% window!
                        let _ = out.write_all(
                            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        return;
                    }
                    let end = if req_end == 0 {
                        0
                    } else {
                        req_end.min(half_len - 1)
                    };
                    let slice = &payload[start..=end];
                    // Intentionally omit both ETag and Last-Modified to simulate signed CDN streams.
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len(),
                        slice.len()
                    );
                    let _ = out.write_all(header.as_bytes());
                    let _ = out.write_all(slice);
                } else {
                    new_ranges.lock().unwrap().push((start, req_end));
                    let slice = &payload[start..=req_end];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{req_end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len(),
                        slice.len()
                    );
                    let _ = out.write_all(header.as_bytes());
                    let _ = out.write_all(slice);
                }
            });
        }
    });

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(out_dir.to_string_lossy().as_ref()),
        )
        .unwrap();

    let old_url = format!("http://{addr}/signed/archive.bin?id=77&token=old_sig&expires=100000");
    let created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: old_url,
                filename: "archive.bin".into(),
                download_dir: out_dir.to_string_lossy().into_owned(),
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = created
        .iter()
        .find_map(|env| match &env.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();

    let failed = wait_for_task_status(&coordinator, "failed");
    assert_eq!(failed.task_id, task_id);

    let saved_key_before = coordinator
        .lock()
        .unwrap()
        .task_spec(&task_id)
        .unwrap()
        .resource_key
        .clone();
    assert_eq!(
        saved_key_before,
        format!("http://{addr}/signed/archive.bin?id=77")
    );

    // Refresh task URL with rotated `?token=new_sig&expires=999999` via ArmTaskRefresh + OfferResource.
    coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: task_id.clone(),
        })
        .unwrap();
    let refreshed_url =
        format!("http://{addr}/signed/archive.bin?id=77&token=new_sig&expires=999999");
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "handoff-refresh-50pct".into(),
                url: refreshed_url.clone(),
                resource_kind: ResourceKind::File,
                filename: "archive.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();

    let completed = wait_for_task_status(&coordinator, "completed");
    assert_eq!(completed.task_id, task_id);
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .task_spec(&task_id)
            .unwrap()
            .resource_key,
        saved_key_before
    );
    assert_eq!(fs::read(out_dir.join("archive.bin")).unwrap(), *payload);

    let ranges = new_token_ranges.lock().unwrap().clone();
    // Under `new_sig`, only the 1-byte probe (`0..=0`) and the uncovered second half (`131072..=262143`)
    // may be fetched; the completed first half (`0..=131071`) must NOT be re-downloaded!
    assert!(
        ranges
            .iter()
            .all(|&(start, end)| (start == 0 && end == 0) || start >= half_len),
        "completed first-half range was re-downloaded after URL refresh: {ranges:?}"
    );
    assert!(
        ranges.iter().any(|&(start, _)| start == half_len),
        "expected resume starting at 50% offset ({half_len}), saw: {ranges:?}"
    );

    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn arm_task_refresh_ignores_unrelated_tab_downloads_and_validates_audio_url_on_both_paths() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "allowed_hosts",
            serde_json::json!("cdn.example.test,media.example.test"),
        )
        .unwrap();

    // 1. Single-track file task armed for refresh.
    let created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/releases/app-installer.zip?sig=expired".into(),
                filename: "app-installer.zip".into(),
                source_page_url: "https://site-a.example.test/downloads".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = created
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();

    coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: task_id.clone(),
        })
        .unwrap();

    // Unrelated OfferResource from another tab/file must NOT overwrite `task_id` and must keep armed state active.
    let unrelated_offer_events = coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                url: "https://cdn.example.test/docs/manual.pdf".into(),
                source_page_url: "https://other-site.org/docs".into(),
                handoff_id: "handoff-unrelated-1".into(),
                filename: "manual.pdf".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert!(
        unrelated_offer_events
            .iter()
            .any(|envelope| matches!(envelope.event, CoreEvent::HandoffOffered { .. })),
        "unrelated OfferResource must fall back to normal handoff: {unrelated_offer_events:?}"
    );
    assert_eq!(
        coordinator.lock().unwrap().task_spec(&task_id).unwrap().url,
        "https://cdn.example.test/releases/app-installer.zip?sig=expired"
    );

    // Unrelated durable `admit_handoff` on the same site but different filename must also fall back to normal handoff.
    let unrelated_handoff = crate::contract::BrowserHandoff {
        id: "handoff-unrelated-2".into(),
        offer: crate::ResourceOffer {
            url: "https://cdn.example.test/releases/other-tool.zip?sig=1".into(),
            source_page_url: "https://site-a.example.test/downloads".into(),
            handoff_id: "handoff-unrelated-2".into(),
            filename: "other-tool.zip".into(),
            ..Default::default()
        },
        filename: "other-tool.zip".into(),
        title: "other-tool.zip".into(),
        mime_type: "application/zip".into(),
        size: 2048,
        status: "pending".into(),
        presentation: "queued".into(),
        task_id: None,
        created_at_ms: 1_700_000_000_100,
        request_id: "req-unrelated-2".into(),
        suppression: None,
    };
    let unrelated_receipt: crate::contract::BrowserHandoff = serde_json::from_str(
        &coordinator
            .admit_handoff(&serde_json::to_string(&unrelated_handoff).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(unrelated_receipt.status, "pending");
    assert!(unrelated_receipt.task_id.is_none());
    assert_eq!(
        coordinator.lock().unwrap().task_spec(&task_id).unwrap().url,
        "https://cdn.example.test/releases/app-installer.zip?sig=expired"
    );

    // Armed refresh is STILL active -> a matching refresh offer now updates `task_id`!
    let matching_handoff = crate::contract::BrowserHandoff {
        id: "handoff-matching-refresh".into(),
        offer: crate::ResourceOffer {
            url: "https://cdn.example.test/releases/app-installer.zip?sig=fresh2".into(),
            source_page_url: "https://site-a.example.test/downloads".into(),
            handoff_id: "handoff-matching-refresh".into(),
            filename: "app-installer.zip".into(),
            ..Default::default()
        },
        filename: "app-installer.zip".into(),
        title: "app-installer.zip".into(),
        mime_type: "application/zip".into(),
        size: 4096,
        status: "pending".into(),
        presentation: "queued".into(),
        task_id: None,
        created_at_ms: 1_700_000_000_200,
        request_id: "req-matching-refresh".into(),
        suppression: None,
    };
    let matched_receipt: crate::contract::BrowserHandoff = serde_json::from_str(
        &coordinator
            .admit_handoff(&serde_json::to_string(&matching_handoff).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(matched_receipt.status, "accepted");
    assert_eq!(matched_receipt.task_id.as_deref(), Some(task_id.as_str()));
    assert_eq!(
        coordinator.lock().unwrap().task_spec(&task_id).unwrap().url,
        "https://cdn.example.test/releases/app-installer.zip?sig=fresh2"
    );
    let _ = coordinator.dispatch(CoreCommand::TaskAction {
        task_id: task_id.clone(),
        action: "cancel".into(),
    });
    let _ = coordinator.wait_for_worker_stop(&task_id);

    // 2. Dual-track task armed for refresh rejects unsafe / non-allowlisted `audio_url` in both paths.
    let dual_created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://media.example.test/v/1080p.m4s?sig=old".into(),
                audio_url: "https://media.example.test/a/audio.m4s?sig=old".into(),
                filename: "episode-01.mp4".into(),
                source_page_url: "https://video.example.test/watch/ep1".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let dual_task_id = dual_created
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::ArmTaskRefresh {
            task_id: dual_task_id.clone(),
        })
        .unwrap();

    for (idx, bad_audio_url) in [
        "file:///C:/Windows/win.ini",
        "https://evil.untrusted.org/audio.m4s",
    ]
    .into_iter()
    .enumerate()
    {
        let offer_err = coordinator
            .dispatch(CoreCommand::OfferResource {
                offer: crate::ResourceOffer {
                    url: "https://media.example.test/v/1080p.m4s?sig=new".into(),
                    audio_url: bad_audio_url.into(),
                    source_page_url: "https://video.example.test/watch/ep1".into(),
                    handoff_id: format!("offer-bad-audio-{idx}"),
                    filename: "episode-01.mp4".into(),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(offer_err.contains("音轨下载地址"), "{offer_err}");

        let bad_handoff = crate::contract::BrowserHandoff {
            id: format!("handoff-bad-audio-{idx}"),
            offer: crate::ResourceOffer {
                url: "https://media.example.test/v/1080p.m4s?sig=new".into(),
                audio_url: bad_audio_url.into(),
                source_page_url: "https://video.example.test/watch/ep1".into(),
                handoff_id: format!("handoff-bad-audio-{idx}"),
                filename: "episode-01.mp4".into(),
                ..Default::default()
            },
            filename: "episode-01.mp4".into(),
            title: "episode-01.mp4".into(),
            mime_type: "video/mp4".into(),
            size: 0,
            status: "pending".into(),
            presentation: "queued".into(),
            task_id: None,
            created_at_ms: 1_700_000_000_300 + idx as u64,
            request_id: format!("req-bad-audio-{idx}"),
            suppression: None,
        };
        let err = coordinator
            .admit_handoff(&serde_json::to_string(&bad_handoff).unwrap())
            .unwrap_err();
        assert!(
            err.contains("音轨下载地址"),
            "expected audio_url rejection error, got: {err}"
        );
        assert_eq!(
            coordinator
                .lock()
                .unwrap()
                .task_spec(&dual_task_id)
                .unwrap()
                .audio_url,
            "https://media.example.test/a/audio.m4s?sig=old"
        );
    }
}

#[test]
fn dual_track_single_stream_failure_preserves_completed_track_until_real_user_cancel() {
    let root = lifecycle_test_root("dual-track-preserve-on-error");
    let out_dir = root.join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let video_bytes = vec![0x56u8; 32 * 1024];
    let video_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stall_audio = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (stalled_tx, stalled_rx) = std::sync::mpsc::channel();
    let stalled_tx = Arc::new(Mutex::new(Some(stalled_tx)));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let v_hits_srv = Arc::clone(&video_hits);
    let stall_srv = Arc::clone(&stall_audio);
    let stalled_tx_srv = Arc::clone(&stalled_tx);
    let release_rx_srv = Arc::clone(&release_rx);
    let v_payload = video_bytes.clone();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let v_hits = Arc::clone(&v_hits_srv);
            let stall = Arc::clone(&stall_srv);
            let stalled_tx = Arc::clone(&stalled_tx_srv);
            let release_rx = Arc::clone(&release_rx_srv);
            let payload = v_payload.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&mut stream);
                let mut req_line = String::new();
                let _ = reader.read_line(&mut req_line);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                if req_line.contains("/video.m4s") {
                    v_hits.fetch_add(1, Ordering::SeqCst);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&payload);
                } else if req_line.contains("/audio.m4s") && stall.load(Ordering::SeqCst) {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nContent-Length: 65536\r\n\r\nabcd",
                    );
                    if let Some(tx) = stalled_tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    if let Some(rx) = release_rx.lock().unwrap().take() {
                        let _ = rx.recv_timeout(Duration::from_secs(5));
                    }
                } else if req_line.contains("/audio.m4s") {
                    // Delay slightly so video finishes first, then fail with 403 Forbidden.
                    std::thread::sleep(Duration::from_millis(80));
                    let _ = stream.write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            });
        }
    });

    let coordinator = direct_http_coordinator(&out_dir);
    let spec = TaskSpec {
        url: format!("http://{addr}/video.m4s"),
        audio_url: format!("http://{addr}/audio.m4s"),
        filename: "dual-preserve.mp4".into(),
        download_dir: out_dir.to_string_lossy().into_owned(),
        concurrency: 1,
        ..Default::default()
    };
    let created = coordinator
        .dispatch(CoreCommand::CreateTask { spec: spec.clone() })
        .unwrap();
    let task_id = created
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .unwrap();

    let failed = wait_for_task_status(&coordinator, "failed");
    assert_eq!(failed.task_id, task_id);

    let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
    let task_dir = paths.task_dir();
    let video_tmp = task_dir.join("video.part.m4s");
    let video_state_dir = task_dir.join("video_state");
    let audio_state_dir = task_dir.join("audio_state");

    assert!(
        video_tmp.is_file(),
        "completed video.part.m4s must be preserved when audio track fails"
    );
    assert_eq!(fs::read(&video_tmp).unwrap().len(), video_bytes.len());
    assert!(
        video_state_dir.is_dir() && audio_state_dir.is_dir(),
        "per-track state directories must remain intact on single-track failure"
    );
    assert!(
        !matches!(
            crate::http_engine::read_control(&paths.control),
            crate::http_engine::Control::Cancel
        ),
        "shared control file must not be polluted with cancel on track error"
    );
    assert!(
        dual_track_part_complete(&video_tmp, &video_state_dir.join("progress.json")),
        "completed video track must be recognized as reusable for retry/refresh"
    );
    let hits_after_first_attempt = video_hits.load(Ordering::SeqCst);
    assert!(hits_after_first_attempt >= 1);

    // Retry while stalling `/audio.m4s`: completed `video.part.m4s` is reused without re-fetching,
    // and an active user Cancel wipes the dual-track temp files and state directories.
    stall_audio.store(true, Ordering::SeqCst);
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: task_id.clone(),
            action: "retry".into(),
        })
        .unwrap();
    stalled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        video_hits.load(Ordering::SeqCst),
        hits_after_first_attempt,
        "completed video track must not be re-downloaded on retry"
    );

    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: task_id.clone(),
            action: "cancel".into(),
        })
        .unwrap();
    let _ = release_tx.send(());
    coordinator.wait_for_worker_stop(&task_id).unwrap();
    assert!(
        !video_tmp.exists() && !video_state_dir.exists() && !audio_state_dir.exists(),
        "explicit user Cancel must clean up dual-track temp payloads and state dirs"
    );

    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn armed_refresh_handles_utf8_leading_dots_multitenant_hosts_and_stream_mp4_filenames() {
    // 1. Malformed leading/consecutive dots + UTF-8 labels must never panic and extract clean domains.
    assert_eq!(
        super::refresh_url_host("https://..中.example.com./a").as_deref(),
        Some("中.example.com")
    );
    assert_eq!(
        super::refresh_registrable_domain("..中.example.com"),
        "example.com"
    );
    assert_eq!(
        super::refresh_registrable_domain("a..b.example.com"),
        "example.com"
    );
    assert!(super::refresh_same_site_or_host(
        "..中.example.com",
        "cdn.example.com"
    ));

    // 2. Multi-tenant cloud/CDN suffixes must NOT treat unrelated tenants as the same site.
    for (tenant_a, tenant_b) in [
        ("d111.cloudfront.net", "d222.cloudfront.net"),
        ("tenant-a.github.io", "tenant-b.github.io"),
        ("pub-aaa.r2.dev", "pub-bbb.r2.dev"),
        ("ep1.azureedge.net", "ep2.azureedge.net"),
        ("app1.pages.dev", "app2.pages.dev"),
        ("api.acct1.workers.dev", "api.acct2.workers.dev"),
        ("site1.vercel.app", "site2.vercel.app"),
        ("site1.netlify.app", "site2.netlify.app"),
        (
            "bucket-a.s3.us-east-1.amazonaws.com",
            "bucket-b.s3.us-east-1.amazonaws.com",
        ),
        ("s3.us-east-1.amazonaws.com", "s3.eu-west-1.amazonaws.com"),
    ] {
        assert!(
            !super::refresh_same_site_or_host(tenant_a, tenant_b),
            "unrelated multi-tenant hosts {tenant_a} and {tenant_b} must not match as same site"
        );
    }

    let cf_task = TaskSpec {
        url: "https://d111.cloudfront.net/releases/setup.zip?Expires=1".into(),
        filename: "setup.zip".into(),
        ..Default::default()
    };
    let cf_other_tenant_offer = crate::ResourceOffer {
        url: "https://d222.cloudfront.net/releases/setup.zip?Expires=2".into(),
        filename: "setup.zip".into(),
        ..Default::default()
    };
    assert!(
        !CoreCoordinator::offer_matches_armed_refresh_task(&cf_task, &cf_other_tenant_offer),
        "different cloudfront tenants without shared page must not match"
    );

    // 3. Stream / dual-track tasks with `current.filename = "episode.mp4"` and empty `offer.filename`
    // must match `.m3u8` / `.mpd` / `.m4s` refresh offers on the same `source_page_url`.
    let page = "https://video.example.test/watch/ep01";
    for (kind, old_url, new_url, audio_old, audio_new) in [
        (
            crate::ResourceKind::Hls,
            "https://d111.cloudfront.net/hls/old-hash/master.m3u8?sig=1",
            "https://d222.cloudfront.net/hls/new-hash/index.m3u8?sig=2",
            "",
            "",
        ),
        (
            crate::ResourceKind::Dash,
            "https://d111.cloudfront.net/dash/old-hash/manifest.mpd?sig=1",
            "https://d222.cloudfront.net/dash/new-hash/stream.mpd?sig=2",
            "",
            "",
        ),
        (
            crate::ResourceKind::File,
            "https://d111.cloudfront.net/vod/old-vid-30080.m4s?sig=1",
            "https://d222.cloudfront.net/vod/new-vid-30080.m4s?sig=2",
            "https://d111.cloudfront.net/vod/old-aud-30280.m4s?sig=1",
            "https://d222.cloudfront.net/vod/new-aud-30280.m4s?sig=2",
        ),
    ] {
        let current = TaskSpec {
            url: old_url.into(),
            audio_url: audio_old.into(),
            resource_kind: kind,
            filename: "episode.mp4".into(),
            source_page_url: page.into(),
            ..Default::default()
        };
        let offer = crate::ResourceOffer {
            url: new_url.into(),
            audio_url: audio_new.into(),
            resource_kind: kind,
            filename: String::new(),
            source_page_url: page.into(),
            ..Default::default()
        };
        assert!(
            CoreCoordinator::offer_matches_armed_refresh_task(&current, &offer),
            "expected stream/dual-track mp4 task to match empty-filename refresh offer for {new_url}"
        );
    }

    // 4. Single-track renamed `ResourceKind::File` with empty `offer.filename` on exact same page.
    let renamed_file_task = TaskSpec {
        url: "https://cdn.example.test/files/9f8e7d6c.mp4?token=expired".into(),
        filename: "My Renamed Lecture 01.mp4".into(),
        source_page_url: page.into(),
        resource_kind: crate::ResourceKind::File,
        ..Default::default()
    };
    let rotated_hash_offer = crate::ResourceOffer {
        url: "https://cdn.example.test/files/1a2b3c4d.mp4?token=fresh".into(),
        filename: String::new(),
        source_page_url: page.into(),
        resource_kind: crate::ResourceKind::File,
        ..Default::default()
    };
    assert!(CoreCoordinator::offer_matches_armed_refresh_task(
        &renamed_file_task,
        &rotated_hash_offer
    ));

    let conflicting_ext_offer = crate::ResourceOffer {
        url: "https://cdn.example.test/files/1a2b3c4d.pdf?token=fresh".into(),
        filename: String::new(),
        source_page_url: page.into(),
        resource_kind: crate::ResourceKind::File,
        ..Default::default()
    };
    assert!(!CoreCoordinator::offer_matches_armed_refresh_task(
        &renamed_file_task,
        &conflicting_ext_offer
    ));
}

#[test]
fn worker_exit_respects_pause_and_cancel_without_false_failure_or_retry() {
    for action in ["pause", "cancel"] {
        let entered = Arc::new(AtomicBool::new(false));
        let entered_srv = Arc::clone(&entered);
        let release = Arc::new(AtomicBool::new(false));
        let release_srv = Arc::clone(&release);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line.trim().is_empty() {
                        break;
                    }
                    line.clear();
                }
                stream = reader.into_inner();
                entered_srv.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(3);
                while !release_srv.load(Ordering::SeqCst) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });

        let root = lifecycle_test_root(&format!("worker-exit-{action}"));
        let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
        coordinator
            .set_setting("legal_terms_accepted", serde_json::json!(true))
            .expect("legal");
        coordinator
            .set_setting("proxy_mode", serde_json::json!("direct"))
            .expect("proxy");
        coordinator
            .set_setting("auto_retry_failed_max", serde_json::json!(3))
            .expect("auto_retry");
        coordinator
            .set_setting(
                "download_dir",
                serde_json::json!(root.to_string_lossy().into_owned()),
            )
            .expect("download_dir");

        let events = coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: format!("http://{addr}/slow-503.bin"),
                    filename: "slow-503.bin".into(),
                    concurrency: 1,
                    ..Default::default()
                },
            })
            .expect("create task");
        let task_id = events
            .iter()
            .find_map(|env| match &env.event {
                CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
                _ => None,
            })
            .expect("task_id");

        let deadline = Instant::now() + Duration::from_secs(3);
        while !entered.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(entered.load(Ordering::SeqCst), "worker should connect");

        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: task_id.clone(),
                action: action.into(),
            })
            .expect("dispatch pause/cancel");
        release.store(true, Ordering::SeqCst);
        coordinator
            .wait_for_worker_stop(&task_id)
            .expect("wait worker stop");

        let snap = coordinator
            .lock()
            .expect("lock")
            .snapshot(&task_id)
            .cloned()
            .expect("snapshot");
        let expected_status = if action == "pause" {
            "paused"
        } else {
            "canceled"
        };
        assert_eq!(snap.status, expected_status);
        assert!(
            snap.error_code.is_none(),
            "paused/canceled task must not record HTTP 503 failure: {snap:?}"
        );
        assert_eq!(snap.error_attempt, 0);
        let _ = fs::remove_dir_all(root);
    }

    // Terminal canceled/completed tasks in CoreRuntime ignore late failure or progress regression,
    // while direct report_failure on a paused task remains permitted for test/diagnostic callers.
    let mut runtime = crate::CoreRuntime::new();
    runtime.handle(CoreCommand::CreateTask {
        spec: TaskSpec {
            url: "https://cdn.example.test/a.bin".into(),
            filename: "a.bin".into(),
            start_paused: true,
            ..Default::default()
        },
    });
    runtime.report_failure(
        "task-1",
        crate::TaskFailure {
            code: "HTTP_410".into(),
            message: "gone".into(),
            http_status: Some(410),
            attempt: 1,
            ..Default::default()
        },
    );
    assert_eq!(runtime.snapshot("task-1").expect("snap").status, "failed");

    runtime.handle(CoreCommand::CreateTask {
        spec: TaskSpec {
            url: "https://cdn.example.test/b.bin".into(),
            filename: "b.bin".into(),
            ..Default::default()
        },
    });
    runtime.handle(CoreCommand::TaskAction {
        task_id: "task-2".into(),
        action: "cancel".into(),
    });
    runtime.report_failure(
        "task-2",
        crate::TaskFailure {
            code: "HTTP_503".into(),
            message: "late error".into(),
            attempt: 1,
            ..Default::default()
        },
    );
    runtime.handle(CoreCommand::UpdateProgress {
        task_id: "task-2".into(),
        downloaded_bytes: 50,
        total_bytes: Some(100),
        speed_bytes_per_sec: 0,
        stage: "waiting".into(),
        status: "paused".into(),
    });
    assert_eq!(runtime.snapshot("task-2").expect("snap").status, "canceled");
    assert!(runtime
        .snapshot("task-2")
        .expect("snap")
        .error_code
        .is_none());
}

#[test]
fn dual_track_sibling_failure_preserves_partial_files_and_pause_returns_paused() {
    let video_payload = Arc::new(vec![b'V'; 64 * 1024]);
    let video_for_fail = Arc::clone(&video_payload);

    // Case 1: Audio returns 403 while video streams -> sibling_abort stops video,
    // task must end `failed` (NOT `canceled`) and preserve partial `.part.m4s` + state dirs.
    let v_listener = TcpListener::bind("127.0.0.1:0").expect("bind video");
    let v_addr = v_listener.local_addr().expect("v_addr");
    thread::spawn(move || {
        while let Ok((mut stream, _)) = v_listener.accept() {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line.trim().is_empty() {
                    break;
                }
                line.clear();
            }
            stream = reader.into_inner();
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                video_for_fail.len() - 1,
                video_for_fail.len(),
                video_for_fail.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&video_for_fail[..8192]);
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(400));
            let _ = stream.write_all(&video_for_fail[8192..]);
        }
    });

    let a_listener = TcpListener::bind("127.0.0.1:0").expect("bind audio");
    let a_addr = a_listener.local_addr().expect("a_addr");
    thread::spawn(move || {
        while let Ok((mut stream, _)) = a_listener.accept() {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line.trim().is_empty() {
                    break;
                }
                line.clear();
            }
            stream = reader.into_inner();
            thread::sleep(Duration::from_millis(80));
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });

    let root = lifecycle_test_root("dual-track-sibling-fail");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .expect("legal");
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .expect("proxy");
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .expect("download_dir");

    let created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{v_addr}/video.m4s"),
                audio_url: format!("http://{a_addr}/audio.m4s"),
                filename: "episode.mp4".into(),
                download_dir: root.to_string_lossy().into_owned(),
                concurrency: 1,
                ..Default::default()
            },
        })
        .expect("create dual-track task");
    let task_id = created
        .iter()
        .find_map(|env| match &env.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .expect("task_id");

    let failed = wait_for_task_status(&coordinator, "failed");
    assert_eq!(failed.task_id, task_id);
    assert_eq!(failed.http_status, Some(403));
    let task_dir = root.join(".hls-tasks").join(&task_id);
    assert!(
        task_dir.join("video_state").is_dir() || task_dir.join("video.part.m4s").exists(),
        "sibling failure must preserve dual-track partial files/checkpoints"
    );

    // Case 2: Pause during dual-track transfer returns `paused` and preserves state dirs.
    let v_entered = Arc::new(AtomicBool::new(false));
    let v_entered_srv = Arc::clone(&v_entered);
    let vp_listener = TcpListener::bind("127.0.0.1:0").expect("bind vp");
    let vp_addr = vp_listener.local_addr().expect("vp_addr");
    let video_for_pause = Arc::clone(&video_payload);
    thread::spawn(move || {
        while let Ok((mut stream, _)) = vp_listener.accept() {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line.trim().is_empty() {
                    break;
                }
                line.clear();
            }
            stream = reader.into_inner();
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                video_for_pause.len() - 1,
                video_for_pause.len(),
                video_for_pause.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&video_for_pause[..4096]);
            let _ = stream.flush();
            v_entered_srv.store(true, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(800));
        }
    });

    let created2 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{vp_addr}/video2.m4s"),
                audio_url: format!("http://{vp_addr}/audio2.m4s"),
                filename: "episode2.mp4".into(),
                download_dir: root.to_string_lossy().into_owned(),
                concurrency: 1,
                ..Default::default()
            },
        })
        .expect("create dual-track pause task");
    let task2_id = created2
        .iter()
        .find_map(|env| match &env.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .expect("task2_id");

    let deadline = Instant::now() + Duration::from_secs(3);
    while !v_entered.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: task2_id.clone(),
            action: "pause".into(),
        })
        .expect("pause dual-track");
    coordinator
        .wait_for_worker_stop(&task2_id)
        .expect("wait dual-track stop");

    let snap2 = coordinator
        .lock()
        .expect("lock")
        .snapshot(&task2_id)
        .cloned()
        .expect("snap2");
    assert_eq!(snap2.status, "paused");
    let task2_dir = root.join(".hls-tasks").join(&task2_id);
    assert!(
        task2_dir.join("video_state").is_dir(),
        "paused dual-track task must preserve video_state dir"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn quick_handoff_policy_blocks_fallback_on_coordinator_rejection_and_enriches_store_fallback() {
    // 1. Coordinator rejection (e.g. missing credential_ref in seal_spec_secrets when headers exist,
    // or invalid ffmpeg_path) must map to QuickHandoffPolicy::Disabled, NOT StoreFallback!
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .expect("legal");
    coordinator
        .set_setting("browser_quick_download_enabled", serde_json::json!(true))
        .expect("quick");
    coordinator
        .lock()
        .expect("lock")
        .store_mut()
        .set_setting("ffmpeg_path", serde_json::json!("not_ffmpeg.exe"))
        .expect("bad ffmpeg in store");
    coordinator.invalidate_settings_cache();
    let handoff = serde_json::json!({
        "id": "h-quick-reject",
        "request_id": "req-quick-reject",
        "offer": crate::ResourceOffer {
            handoff_id: "h-quick-reject".into(),
            url: "https://cdn.example.test/setup.zip".into(),
            filename: "setup.zip".into(),
            ..Default::default()
        },
        "filename": "setup.zip",
        "title": "",
        "mime_type": "",
        "size": 1024,
        "status": "pending",
        "presentation": "queued",
        "task_id": null,
        "created_at_ms": 1
    })
    .to_string();
    let admitted = coordinator.admit_handoff(&handoff).expect("admit");
    let admitted_val: Value = serde_json::from_str(&admitted).expect("parse admitted");
    assert_eq!(
        admitted_val["status"], "pending",
        "rejected coordinator quick handoff must stay pending instead of falling back to store"
    );
    assert!(coordinator.tasks().expect("tasks").is_empty());

    // 2. Direct PersistentCore::admit_handoff (StoreFallback) honors site_rules, effective_proxy,
    // default headers, and DEFAULT_COOKIE_CREDENTIAL_REF.
    let root = lifecycle_test_root("quick-store-fallback");
    let rule_dir = root.join("rule-out").to_string_lossy().into_owned();
    let mut core = PersistentCore::in_memory().expect("in_memory");
    core.store_mut()
        .set_settings(&BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "legal_terms_version".into(),
                serde_json::json!(crate::LEGAL_TERMS_VERSION),
            ),
            (
                "browser_quick_download_enabled".into(),
                serde_json::json!(true),
            ),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy()),
            ),
            (
                "default_user_agent".into(),
                serde_json::json!("GlobalUA/1.0"),
            ),
            (
                "default_referer".into(),
                serde_json::json!("https://global.example/ref"),
            ),
            (
                "default_origin".into(),
                serde_json::json!("https://global.example"),
            ),
            ("proxy_mode".into(), serde_json::json!("manual")),
            (
                "proxy_url".into(),
                serde_json::json!("http://127.0.0.1:8888"),
            ),
            (
                "site_rules".into(),
                serde_json::json!(serde_json::json!([{
                    "host": "vip.example.test",
                    "download_dir": rule_dir,
                    "concurrency": 6,
                    "speed_limit_kib": 256,
                    "user_agent": "RuleUA/2.0",
                    "proxy_mode": "direct"
                }])
                .to_string()),
            ),
        ]))
        .expect("set settings");
    #[cfg(windows)]
    {
        let blob = CredentialVault
            .protect(r#"{"cookie":"default=1"}"#)
            .expect("protect default cookie");
        core.store_mut()
            .store_credential(DEFAULT_COOKIE_CREDENTIAL_REF, &blob, "default_cookie")
            .expect("store default cookie");
    }

    let direct_handoff = serde_json::json!({
        "id": "h-store-fallback",
        "request_id": "req-store-fallback",
        "offer": crate::ResourceOffer {
            handoff_id: "h-store-fallback".into(),
            url: "https://vip.example.test/pkg.bin".into(),
            filename: "pkg.bin".into(),
            ..Default::default()
        },
        "filename": "pkg.bin",
        "title": "",
        "mime_type": "",
        "size": 2048,
        "status": "pending",
        "presentation": "queued",
        "task_id": null,
        "created_at_ms": 2
    })
    .to_string();
    let fallback_json = core.admit_handoff(&direct_handoff).expect("direct admit");
    let fallback_val: Value = serde_json::from_str(&fallback_json).expect("parse fallback");
    assert_eq!(fallback_val["status"], "accepted");
    let created_spec = core.task_spec("task-1").expect("task-1 spec");
    assert_eq!(created_spec.download_dir, rule_dir);
    assert_eq!(created_spec.concurrency, 6);
    assert_eq!(created_spec.speed_limit_kib, 256);
    assert_eq!(created_spec.proxy, crate::net_policy::DIRECT_PROXY_SENTINEL);
    assert_eq!(
        created_spec.headers.get("User-Agent").map(String::as_str),
        Some("RuleUA/2.0")
    );
    assert_eq!(
        created_spec.headers.get("Referer").map(String::as_str),
        Some("https://global.example/ref")
    );
    assert_eq!(
        created_spec.headers.get("Origin").map(String::as_str),
        Some("https://global.example")
    );
    #[cfg(windows)]
    assert_eq!(
        created_spec.credential_ref.as_deref(),
        Some(DEFAULT_COOKIE_CREDENTIAL_REF)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn refresh_task_request_stops_active_worker_overwrites_scoped_cookie_and_resumes() {
    let seen_cookies = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_srv = Arc::clone(&seen_cookies);
    let first_connected = Arc::new(AtomicBool::new(false));
    let first_srv = Arc::clone(&first_connected);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream);
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line);
            let mut cookie_seen = String::new();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("cookie") {
                        cookie_seen = v.trim().to_string();
                    }
                }
                line.clear();
            }
            stream = reader.into_inner();
            if req_line.contains("sig=old") {
                first_srv.store(true, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(350));
                let _ = stream.write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            } else {
                if let Ok(mut list) = seen_srv.lock() {
                    list.push(cookie_seen);
                }
                let payload = b"refreshed-ok";
                let resp = format!(
                    "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len() - 1,
                    payload.len(),
                    payload.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(payload);
            }
        }
    });

    let root = lifecycle_test_root("refresh-active-worker");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .expect("legal");
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .expect("proxy");
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .expect("download_dir");

    let origin = format!("http://{addr}");
    let initial_replay = serde_json::json!({
        "cookie": "sid=old-top",
        "cookies": [{"name": "sid", "value": "old-arr"}],
        "request_contexts": {
            origin: {
                "cookie": "sid=old-scoped",
                "cookies": ["sid=old-scoped-arr"],
                "request_headers": { "Cookie": "sid=old-hdr" }
            }
        }
    })
    .to_string();
    let blob = if cfg!(windows) {
        CredentialVault
            .protect(&initial_replay)
            .expect("protect replay")
    } else {
        initial_replay
    };
    coordinator
        .store_credential("cred-old-scoped", &blob, "browser_replay")
        .expect("store cred");

    let created = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{addr}/video.bin?sig=old"),
                filename: "video.bin".into(),
                download_dir: root.to_string_lossy().into_owned(),
                credential_ref: Some("cred-old-scoped".into()),
                concurrency: 1,
                ..Default::default()
            },
        })
        .expect("create");
    let task_id = created
        .iter()
        .find_map(|env| match &env.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .expect("task_id");

    let deadline = Instant::now() + Duration::from_secs(3);
    while !first_connected.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        coordinator.worker_is_active(&task_id).expect("active"),
        "worker should still be active when RefreshTaskRequest arrives"
    );

    coordinator
        .dispatch(CoreCommand::RefreshTaskRequest {
            task_id: task_id.clone(),
            url: format!("http://{addr}/video.bin?sig=fresh"),
            cookie: "sid=fresh-cookie".into(),
            auto_resume: true,
        })
        .expect("refresh active task");

    let completed = wait_for_task_status(&coordinator, "completed");
    assert_eq!(completed.task_id, task_id);
    assert!(completed.error_code.is_none());
    let cookies = seen_cookies.lock().expect("lock cookies").clone();
    assert!(
        cookies.iter().any(|c| c == "sid=fresh-cookie"),
        "refreshed download must send overwritten Cookie instead of stale request_contexts cookie: {cookies:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn cleared_or_empty_default_cookie_deletes_row_and_allows_existing_tasks_to_start() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_default_cookie("sid=initial")
        .expect("set cookie");
    assert!(coordinator.default_cookie_configured().expect("configured"));

    let spec = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: "https://cdn.example.test/item.bin".into(),
            filename: "item.bin".into(),
            ..Default::default()
        })
        .expect("apply defaults");
    assert_eq!(
        spec.credential_ref.as_deref(),
        Some(DEFAULT_COOKIE_CREDENTIAL_REF)
    );

    // Clearing default cookie deletes the row rather than storing an empty string.
    coordinator
        .set_default_cookie("   ")
        .expect("clear default cookie");
    assert!(!coordinator.default_cookie_configured().expect("cleared"));
    assert!(
        coordinator
            .load_credential(DEFAULT_COOKIE_CREDENTIAL_REF)
            .expect("load")
            .is_none(),
        "set_default_cookie(\"\") should remove settings:default-cookie row"
    );

    // Existing tasks referencing settings:default-cookie gracefully degrade to credential_ref = None.
    let (hydrated, replay) =
        hydrate_replay_headers(&coordinator.core(), spec.clone()).expect("hydrate missing default");
    assert!(hydrated.credential_ref.is_none());
    assert!(replay.is_empty());

    let mut with_auth = spec.clone();
    with_auth
        .headers
        .insert("Authorization".into(), "Bearer token-1".into());
    let sealed = seal_spec_secrets(&coordinator, with_auth).expect("seal missing default");
    assert!(sealed.credential_ref.is_some());

    // Legacy empty string or blank JSON cookie row also degrades cleanly.
    coordinator
        .store_credential(DEFAULT_COOKIE_CREDENTIAL_REF, "", "default_cookie")
        .expect("store legacy empty");
    assert!(!coordinator.default_cookie_configured().expect("empty row"));
    let (hydrated_legacy, _) =
        hydrate_replay_headers(&coordinator.core(), spec).expect("hydrate legacy empty");
    assert!(hydrated_legacy.credential_ref.is_none());
}

#[test]
fn deleting_task_and_in_flight_handoff_guards_prevent_races_without_breaking_retry() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .expect("legal");
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/guard.bin".into(),
                filename: "guard.bin".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .expect("create paused");
    coordinator
        .lock()
        .expect("lock")
        .handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 0,
            total_bytes: Some(10),
            speed_bytes_per_sec: 0,
            stage: "waiting".into(),
            status: "queued".into(),
        })
        .expect("mark queued");

    {
        let _deleting = coordinator
            .enter_deleting_task("task-1")
            .expect("enter deleting");
        let err = coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "start".into(),
            })
            .expect_err("start while deleting must fail");
        assert!(err.contains("删除中"), "{err}");
        coordinator
            .start_next_queued(false)
            .expect("start_next_queued skips deleting task");
        assert!(!coordinator.worker_is_active("task-1").expect("inactive"));
    }
    assert!(!coordinator
        .is_task_deleting("task-1")
        .expect("guard dropped"));

    // Handoff concurrency guard rejects overlapping AcceptHandoff / RejectHandoff,
    // and drops cleanly so subsequent retry succeeds.
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "h-guard".into(),
                url: "https://cdn.example.test/h.bin".into(),
                filename: "h.bin".into(),
                ..Default::default()
            },
        })
        .expect("offer");
    {
        let _in_flight = coordinator
            .enter_in_flight_handoff("h-guard")
            .expect("enter handoff");
        let dup_accept = coordinator
            .dispatch(CoreCommand::AcceptHandoff {
                handoff_id: "h-guard".into(),
                filename: "h.bin".into(),
                download_dir: String::new(),
                allow_duplicate: false,
                trusted_ui: true,
                start_paused: true,
                queue_id: String::new(),
                spec_override: None,
            })
            .expect_err("concurrent accept must fail");
        assert!(dup_accept.contains("正在处理中"), "{dup_accept}");
        let dup_reject = coordinator
            .dispatch(CoreCommand::RejectHandoff {
                handoff_id: "h-guard".into(),
                suppress_site_kind: false,
            })
            .expect_err("concurrent reject must fail");
        assert!(dup_reject.contains("正在处理中"), "{dup_reject}");
    }
    coordinator
        .dispatch(CoreCommand::RejectHandoff {
            handoff_id: "h-guard".into(),
            suppress_site_kind: false,
        })
        .expect("reject after guard drop succeeds");
}

#[test]
fn progress_clears_stale_error_fields_preserving_attempt_cap_until_manual_retry_or_completion() {
    let mut runtime = crate::CoreRuntime::new();
    runtime.handle(CoreCommand::CreateTask {
        spec: TaskSpec {
            url: "https://cdn.example.test/retry.bin".into(),
            filename: "retry.bin".into(),
            scheduled_start_at: "2999-01-01T00:00:00Z".into(),
            ..Default::default()
        },
    });
    runtime.report_failure(
        "task-1",
        crate::TaskFailure {
            code: "HTTP_503".into(),
            message: "HTTP 503".into(),
            stage: "transfer".into(),
            url: "https://cdn.example.test/retry.bin".into(),
            hint: "retry later".into(),
            http_status: Some(503),
            attempt: 2,
        },
    );

    // Auto-retry queues and restarts transfer: error banner fields clear, but error_attempt stays 2!
    runtime.handle(CoreCommand::UpdateProgress {
        task_id: "task-1".into(),
        downloaded_bytes: 0,
        total_bytes: Some(100),
        speed_bytes_per_sec: 0,
        stage: "waiting".into(),
        status: "queued".into(),
    });
    let queued = runtime.snapshot("task-1").expect("queued snap");
    assert!(queued.error_code.is_none());
    assert!(queued.error_message.is_none());
    assert!(queued.http_status.is_none());
    assert_eq!(
        queued.error_attempt, 2,
        "queued progress must preserve error_attempt so auto-retry remains bounded"
    );

    runtime.handle(CoreCommand::UpdateProgress {
        task_id: "task-1".into(),
        downloaded_bytes: 10,
        total_bytes: Some(100),
        speed_bytes_per_sec: 10,
        stage: "transfer".into(),
        status: "downloading".into(),
    });
    assert_eq!(
        runtime.snapshot("task-1").expect("dl snap").error_attempt,
        2
    );

    // Failure again -> manual deferred retry resets error_attempt to 0!
    runtime.report_failure(
        "task-1",
        crate::TaskFailure {
            code: "HTTP_503".into(),
            message: "HTTP 503".into(),
            http_status: Some(503),
            attempt: 3,
            ..Default::default()
        },
    );
    runtime.defer_start_action("task-1", "retry");
    let deferred_snap = runtime.snapshot("task-1").expect("deferred snap");
    assert_eq!(deferred_snap.status, "queued");
    assert_eq!(deferred_snap.error_attempt, 0);
    assert!(deferred_snap.error_code.is_none());

    // Completion also resets error_attempt to 0.
    runtime.report_failure(
        "task-1",
        crate::TaskFailure {
            code: "HTTP_503".into(),
            message: "HTTP 503".into(),
            attempt: 1,
            ..Default::default()
        },
    );
    runtime.handle(CoreCommand::UpdateProgress {
        task_id: "task-1".into(),
        downloaded_bytes: 100,
        total_bytes: Some(100),
        speed_bytes_per_sec: 0,
        stage: "finished".into(),
        status: "completed".into(),
    });
    assert_eq!(
        runtime
            .snapshot("task-1")
            .expect("completed snap")
            .error_attempt,
        0
    );
}

#[test]
fn batch_disrupted_tracks_only_fresh_transitions_and_power_action_toasts_use_cleared_level() {
    let _power_guard = TEST_POWER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _ = crate::power_action::cancel();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .expect("legal");
    coordinator
        .set_setting("completion_power_action", serde_json::json!("sleep"))
        .expect("power action");

    // Create historical failed task in batch 1.
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/old-fail.bin".into(),
                filename: "old-fail.bin".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .expect("create task-1");
    coordinator
        .lock()
        .expect("lock")
        .report_failure(
            "task-1",
            crate::TaskFailure {
                code: "HTTP_404".into(),
                message: "not found".into(),
                attempt: 1,
                ..Default::default()
            },
        )
        .expect("fail task-1");

    // Start a new batch with task-2 (clears batch_disrupted for default queue),
    // then touch task-1 metadata while it stays "failed": must NOT re-mark task-1 in batch_disrupted!
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.example.test/new-ok.bin".into(),
                filename: "new-ok.bin".into(),
                ..Default::default()
            },
        })
        .expect("create task-2");
    coordinator
        .lock()
        .expect("lock")
        .set_task_filename("task-1", "old-fail-renamed.bin".into())
        .expect("rename failed task");
    assert!(
        coordinator
            .lock()
            .expect("lock")
            .batch_disrupted_for_queue(crate::DEFAULT_QUEUE_ID)
            .is_none_or(|set| !set.contains("task-1")),
        "metadata update on already-failed task must not mark current batch disrupted"
    );

    // Attach a non-UI subscriber (`enter_subscriber(false)`), complete task-2, and verify
    // `maybe_schedule_power` still wakes `BackgroundToast`!
    let woke = Arc::new(AtomicBool::new(false));
    let bridge = coordinator.lock().expect("lock").presenter_bridge();
    {
        let woke = Arc::clone(&woke);
        bridge.set_waker(Some(Arc::new(move |mode| {
            if mode == crate::core_service::PresenterWakeMode::BackgroundToast {
                woke.store(true, Ordering::SeqCst);
            }
        })));
    }
    let _non_ui_sub = bridge.enter_subscriber(false);
    coordinator
        .lock()
        .expect("lock")
        .handle(CoreCommand::UpdateProgress {
            task_id: "task-2".into(),
            downloaded_bytes: 10,
            total_bytes: Some(10),
            speed_bytes_per_sec: 0,
            stage: "finished".into(),
            status: "completed".into(),
        })
        .expect("complete task-2");
    let spec2 = coordinator
        .lock()
        .expect("lock")
        .task_spec("task-2")
        .cloned()
        .expect("spec2");
    maybe_schedule_power(&coordinator.core(), "task-2", &spec2).expect("maybe_schedule_power");
    assert!(
        woke.load(Ordering::SeqCst),
        "maybe_schedule_power must wake BackgroundToast when only non-UI subscriber is attached"
    );

    let cancel_events = coordinator
        .dispatch(CoreCommand::CancelPowerAction)
        .expect("cancel power action");
    assert!(
        cancel_events.iter().any(|env| matches!(
            &env.event,
            CoreEvent::Toast { level, .. } if level == "power_action_cleared"
        )),
        "CancelPowerAction must emit power_action_cleared toast level: {cancel_events:?}"
    );
    let confirm_events = coordinator
        .dispatch(CoreCommand::ConfirmPowerAction)
        .expect("confirm idle power action");
    assert!(
        confirm_events.iter().any(|env| matches!(
            &env.event,
            CoreEvent::Toast { level, .. } if level == "power_action_cleared"
        )),
        "ConfirmPowerAction must emit power_action_cleared toast level: {confirm_events:?}"
    );
}

#[test]
fn accept_handoff_spec_override_merges_fields_and_enforces_origin_and_url_boundaries() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .expect("legal");
    let replay_plain = r#"{"cookie":"sid=origin-a"}"#;
    let replay_blob = if cfg!(windows) {
        CredentialVault.protect(replay_plain).expect("protect")
    } else {
        replay_plain.to_string()
    };
    coordinator
        .store_credential("cred-origin-a", &replay_blob, "browser_replay")
        .expect("store cred");

    // 1. Same URL override keeps credential_ref, replay_context_ref, audio_url, and merges options.
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "h-ovr-same-url".into(),
                url: "https://a.example.test/video.m4s".into(),
                audio_url: "https://a.example.test/audio.m4s".into(),
                credential_ref: Some("cred-origin-a".into()),
                replay_context_ref: Some("replay-ctx-a".into()),
                size: 4096,
                filename: "video.m4s".into(),
                ..Default::default()
            },
        })
        .expect("offer 1");
    coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "h-ovr-same-url".into(),
            filename: "episode.mp4".into(),
            download_dir: String::new(),
            allow_duplicate: true,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: Some(Box::new(TaskSpec {
                url: "https://a.example.test/video.m4s".into(),
                concurrency: 12,
                speed_limit_kib: 512,
                checksum: Some(
                    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                        .into(),
                ),
                scheduled_start_at: "23:30".into(),
                headers: BTreeMap::from([("X-Custom".into(), "yes".into())]),
                ..Default::default()
            })),
        })
        .expect("accept 1");
    let spec1 = coordinator
        .lock()
        .expect("lock")
        .task_spec("task-1")
        .cloned()
        .expect("spec1");
    assert_eq!(spec1.credential_ref.as_deref(), Some("cred-origin-a"));
    assert_eq!(spec1.replay_context_ref.as_deref(), Some("replay-ctx-a"));
    assert_eq!(spec1.audio_url, "https://a.example.test/audio.m4s");
    assert_eq!(spec1.concurrency, 12);
    assert_eq!(spec1.speed_limit_kib, 512);
    assert_eq!(spec1.scheduled_start_at, "23:30");
    assert_eq!(
        spec1.headers.get("X-Custom").map(String::as_str),
        Some("yes")
    );

    // 2. Same-origin different URL keeps credential_ref/replay_context_ref, clears audio_url/expected_size.
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "h-ovr-same-origin".into(),
                url: "https://a.example.test/v1.mp4".into(),
                audio_url: "https://a.example.test/a1.m4s".into(),
                credential_ref: Some("cred-origin-a".into()),
                replay_context_ref: Some("replay-ctx-a".into()),
                size: 8192,
                filename: "v1.mp4".into(),
                ..Default::default()
            },
        })
        .expect("offer 2");
    coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "h-ovr-same-origin".into(),
            filename: "v2.mp4".into(),
            download_dir: String::new(),
            allow_duplicate: true,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: Some(Box::new(TaskSpec {
                url: "https://a.example.test/v2-alt.mp4".into(),
                ..Default::default()
            })),
        })
        .expect("accept 2");
    let spec2 = coordinator
        .lock()
        .expect("lock")
        .task_spec("task-2")
        .cloned()
        .expect("spec2");
    assert_eq!(spec2.url, "https://a.example.test/v2-alt.mp4");
    assert_eq!(spec2.credential_ref.as_deref(), Some("cred-origin-a"));
    assert_eq!(spec2.replay_context_ref.as_deref(), Some("replay-ctx-a"));
    assert!(spec2.audio_url.is_empty());
    assert!(spec2.expected_size.is_none());

    // 3. Cross-origin URL drops offer.credential_ref & replay_context_ref so origin-a secrets never leak.
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "h-ovr-cross-origin".into(),
                url: "https://a.example.test/v1.mp4".into(),
                credential_ref: Some("cred-origin-a".into()),
                replay_context_ref: Some("replay-ctx-a".into()),
                size: 8192,
                filename: "v1.mp4".into(),
                ..Default::default()
            },
        })
        .expect("offer 3");
    coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "h-ovr-cross-origin".into(),
            filename: "other.mp4".into(),
            download_dir: String::new(),
            allow_duplicate: true,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: Some(Box::new(TaskSpec {
                url: "https://b.example.test/other.mp4".into(),
                ..Default::default()
            })),
        })
        .expect("accept 3");
    let spec3 = coordinator
        .lock()
        .expect("lock")
        .task_spec("task-3")
        .cloned()
        .expect("spec3");
    assert_eq!(spec3.url, "https://b.example.test/other.mp4");
    assert!(spec3.credential_ref.is_none());
    assert!(spec3.replay_context_ref.is_none());
    assert!(spec3.expected_size.is_none());
}

#[test]
fn accept_handoff_override_and_probe_preserve_explicit_headers_custom_dir_and_duplicate_retry() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local probe server");
    let addr = listener.local_addr().expect("local addr");
    let seen_requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_clone = Arc::clone(&seen_requests);
    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let count = stream.read(&mut buf).unwrap_or(0);
            seen_clone
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..count]).into_owned());
            let body =
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=854x480\n480p.m3u8\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let root = std::env::temp_dir().join(format!(
        "hls-t1-t5-probe-override-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let default_dir = root.join("default-downloads");
    let outside_dir = root.join("custom-outside-videos");
    fs::create_dir_all(&default_dir).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().expect("in_memory"));
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(default_dir.to_string_lossy()),
        )
        .unwrap();
    coordinator
        .set_setting("proxy_mode", serde_json::json!("direct"))
        .unwrap();

    let origin = format!("http://127.0.0.1:{}", addr.port());
    let target_url = format!("{origin}/vod/master.m3u8");
    let replay_json = serde_json::json!({
        "cookie": "sid=old-cookie",
        "request_contexts": {
            &origin: {
                "cookie": "sid=old-scoped-cookie",
                "request_headers": {
                    "Authorization": "Bearer old-token",
                    "X-Custom-Static": "from-browser"
                },
                "url_contexts": {
                    &target_url: {
                        "request_headers": {
                            "Authorization": "Bearer old-url-token"
                        }
                    }
                }
            }
        }
    })
    .to_string();
    let blob = if cfg!(windows) {
        crate::CredentialVault.protect(&replay_json).unwrap()
    } else {
        replay_json.clone()
    };
    coordinator
        .store_credential("cred-browser-scoped", &blob, "browser_replay")
        .unwrap();

    // 1. BUG-T5-5: ProbeUrl with custom outside `download_dir` + explicit Cookie & Authorization succeeds and sends explicit headers!
    let mut probe_headers = std::collections::BTreeMap::new();
    probe_headers.insert("Cookie".into(), "sid=editor-fresh".into());
    probe_headers.insert("Authorization".into(), "Bearer editor-fresh".into());
    probe_headers.insert("X-Editor-Header".into(), "yes".into());
    let probe_events = coordinator
        .dispatch(CoreCommand::ProbeUrl {
            url: target_url.clone(),
            spec: Some(TaskSpec {
                url: target_url.clone(),
                download_dir: outside_dir.to_string_lossy().into_owned(),
                filename: "movie.mp4".into(),
                credential_ref: Some("cred-browser-scoped".into()),
                headers: probe_headers.clone(),
                ..Default::default()
            }),
        })
        .expect("probe_command must not reject custom download_dir");
    server.join().unwrap();
    assert!(
        probe_events
            .iter()
            .any(|ev| matches!(ev.event, CoreEvent::ProbeResult { .. })),
        "probe_events={probe_events:?}"
    );
    let raw_req = seen_requests
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap_or_default();
    let lower_req = raw_req.to_ascii_lowercase();
    assert!(
        lower_req.contains("cookie: sid=editor-fresh"),
        "req={raw_req}"
    );
    assert!(
        lower_req.contains("authorization: bearer editor-fresh"),
        "req={raw_req}"
    );
    assert!(lower_req.contains("x-editor-header: yes"), "req={raw_req}");

    // 2a. BUG-T1-1 & BUG-T5-1: Credential-free HLS handoff hitting a completed duplicate with editor `spec_override`
    //     emits `DuplicateOffered` while keeping `pending_handoff` alive; retrying with `allow_duplicate: true` creates the task
    //     and clears stale `offer.preferred_variant_id` in favor of the probed 480p variant!
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: target_url.clone(),
                resource_kind: crate::ResourceKind::Hls,
                preferred_height: 480,
                preferred_bandwidth: 800_000,
                filename: "existing.mp4".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    {
        let mut core = coordinator.lock().unwrap();
        let mut snap = core.snapshot("task-1").cloned().unwrap();
        snap.status = "completed".into();
        snap.output_path = default_dir
            .join("existing.mp4")
            .to_string_lossy()
            .into_owned();
        fs::write(&snap.output_path, b"done").unwrap();
        let _ = core
            .emit(CoreEvent::TaskUpdated { snapshot: snap })
            .unwrap();
    }

    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "h-dup-then-allow".into(),
                url: target_url.clone(),
                resource_kind: crate::ResourceKind::Hls,
                preferred_variant_id: "1080p.m3u8".into(),
                preferred_height: 1080,
                filename: "existing.mp4".into(),
                ..Default::default()
            },
        })
        .unwrap();

    let first_try = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "h-dup-then-allow".into(),
            filename: "existing.mp4".into(),
            download_dir: outside_dir.to_string_lossy().into_owned(),
            allow_duplicate: false,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: Some(Box::new(TaskSpec {
                url: target_url.clone(),
                resource_kind: crate::ResourceKind::Hls,
                preferred_height: 480,
                preferred_bandwidth: 800_000,
                ..Default::default()
            })),
        })
        .unwrap();
    assert!(first_try
        .iter()
        .any(|ev| matches!(ev.event, CoreEvent::DuplicateOffered { .. })));
    assert!(!first_try
        .iter()
        .any(|ev| matches!(ev.event, CoreEvent::HandoffResolved { .. })));
    assert!(coordinator
        .lock()
        .unwrap()
        .pending_handoff("h-dup-then-allow")
        .is_some());

    let second_try = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "h-dup-then-allow".into(),
            filename: "existing-copy.mp4".into(),
            download_dir: outside_dir.to_string_lossy().into_owned(),
            allow_duplicate: true,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: Some(Box::new(TaskSpec {
                url: target_url.clone(),
                resource_kind: crate::ResourceKind::Hls,
                preferred_height: 480,
                preferred_bandwidth: 800_000,
                ..Default::default()
            })),
        })
        .unwrap();
    assert!(second_try
        .iter()
        .any(|ev| matches!(ev.event, CoreEvent::TaskCreated { .. })));
    assert!(second_try
        .iter()
        .any(|ev| matches!(ev.event, CoreEvent::HandoffResolved { .. })));

    let dup_copy_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-2")
        .cloned()
        .unwrap();
    assert!(dup_copy_spec.preferred_variant_id.is_empty());
    assert_eq!(dup_copy_spec.preferred_height, 480);

    // 2b. BUG-T1-2: Authenticated browser handoff with `request_contexts[origin]` overrides both Cookie
    //     and non-Cookie sensitive headers (`Authorization`) when sealed via `spec_override`!
    coordinator
        .dispatch(CoreCommand::OfferResource {
            offer: crate::ResourceOffer {
                handoff_id: "h-auth-override".into(),
                url: target_url.clone(),
                resource_kind: crate::ResourceKind::Hls,
                credential_ref: Some("cred-browser-scoped".into()),
                filename: "authed.mp4".into(),
                ..Default::default()
            },
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "h-auth-override".into(),
            filename: "authed.mp4".into(),
            download_dir: outside_dir.to_string_lossy().into_owned(),
            allow_duplicate: true,
            trusted_ui: true,
            start_paused: true,
            queue_id: String::new(),
            spec_override: Some(Box::new(TaskSpec {
                url: target_url,
                resource_kind: crate::ResourceKind::Hls,
                headers: probe_headers,
                ..Default::default()
            })),
        })
        .unwrap();
    let created_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-3")
        .cloned()
        .unwrap();
    let (hydrated_spec, _) = hydrate_replay_headers(&coordinator.core, created_spec).unwrap();
    assert_eq!(
        hydrated_spec
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer editor-fresh")
    );
    assert_eq!(
        hydrated_spec.headers.get("Cookie").map(String::as_str),
        Some("sid=editor-fresh")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn save_site_profile_maps_proxy_sentinels_isolates_credentials_and_rejects_non_http_hosts() {
    let root = lifecycle_test_root("save-site-profile");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    coordinator.set_default_cookie("global_sid=keep").unwrap();

    // 1. Task using SYSTEM_PROXY_SENTINEL + DEFAULT_COOKIE_CREDENTIAL_REF + opaque "null" Origin:
    //    must save proxy_mode="system", proxy="", origin="", and NOT alias settings:default-cookie!
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://media.example.test/video.mp4".into(),
                download_dir: root.to_string_lossy().into_owned(),
                proxy: crate::net_policy::SYSTEM_PROXY_SENTINEL.into(),
                headers: BTreeMap::from([
                    ("Origin".into(), "null".into()),
                    ("Referer".into(), "https://media.example.test/watch".into()),
                ]),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::SaveSiteProfile {
            task_id: "task-1".into(),
        })
        .unwrap();
    let rules = crate::parse_site_rules(&coordinator.settings().unwrap().site_rules);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].host, "media.example.test");
    assert_eq!(rules[0].proxy_mode, "system");
    assert!(rules[0].proxy.is_empty());
    assert!(rules[0].origin.is_empty());
    assert!(
        rules[0].credential_ref.is_empty(),
        "global default cookie ref must never be aliased into site rule"
    );

    // 2. Task with private Cookie credential: save_site_profile copies into dedicated site-rule ref,
    //    so clearing the site rule later never deletes the original task's credential or default cookie!
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://auth.example.test/private.mp4".into(),
                download_dir: root.to_string_lossy().into_owned(),
                proxy: crate::net_policy::DIRECT_PROXY_SENTINEL.into(),
                headers: BTreeMap::from([
                    ("Cookie".into(), "task_sid=secret".into()),
                    ("Origin".into(), "HTTPS://auth.example.test".into()),
                ]),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let task2_cred = coordinator
        .lock()
        .unwrap()
        .task_spec("task-2")
        .unwrap()
        .credential_ref
        .clone()
        .unwrap();
    coordinator
        .dispatch(CoreCommand::SaveSiteProfile {
            task_id: "task-2".into(),
        })
        .unwrap();
    let rules = crate::parse_site_rules(&coordinator.settings().unwrap().site_rules);
    let auth_rule = rules
        .iter()
        .find(|r| r.host == "auth.example.test")
        .unwrap();
    assert_eq!(auth_rule.proxy_mode, "direct");
    assert!(auth_rule.proxy.is_empty());
    assert_eq!(auth_rule.origin, "https://auth.example.test");
    assert!(auth_rule.credential_ref.starts_with("settings:site-rule:"));
    assert_ne!(auth_rule.credential_ref, task2_cred);

    coordinator
        .set_site_rule_credential("*.AUTH.example.test.", "", &BTreeMap::new(), true)
        .unwrap();
    assert!(
        coordinator.load_credential(&task2_cred).unwrap().is_some(),
        "clearing site rule credential must preserve original task credential"
    );
    assert!(coordinator.default_cookie_configured().unwrap());

    // 3. Magnet task without HTTP host rejects SaveSiteProfile cleanly.
    let magnet_id = lifecycle_task(&coordinator, &root, "paused");
    coordinator
        .lock()
        .unwrap()
        .replace_spec(
            &magnet_id,
            TaskSpec {
                url: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into(),
                resource_kind: crate::ResourceKind::Torrent,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(coordinator
        .dispatch(CoreCommand::SaveSiteProfile { task_id: magnet_id })
        .is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn single_ended_global_auto_start_does_not_fabricate_stop_window_and_stop_start_resumes_paused() {
    let root = lifecycle_test_root("global-auto-start-stop");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    coordinator
        .set_setting("queue_auto_start_enabled", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting("queue_auto_stop_enabled", serde_json::json!(false))
        .unwrap();
    let profiles = coordinator.settings().unwrap().queue_profiles;
    assert!(
        !profiles[0].schedule_enabled,
        "enabling only queue_auto_start_enabled must not synthesize a continuous stop window"
    );

    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://media.example.test/deferred.bin".into(),
                download_dir: root.to_string_lossy().into_owned(),
                scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = "task-1".to_string();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&task_id)
            .unwrap()
            .status,
        "queued"
    );
    coordinator.pause_active_tasks().unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&task_id)
            .unwrap()
            .status,
        "paused"
    );

    coordinator.resume_paused_tasks().unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&task_id)
            .unwrap()
            .status,
        "queued"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn local_link_file_expands_under_allowed_hosts_and_recomputes_child_resource_key_proxy_and_site_rules(
) {
    let root = lifecycle_test_root("expand-allowlist-proxy-rules");
    fs::create_dir_all(&root).unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    coordinator
        .set_setting(
            "allowed_hosts",
            serde_json::json!("intranet.allowed.test, cdn.allowed.test"),
        )
        .unwrap();
    coordinator
        .set_setting("proxy_url", serde_json::json!("http://127.0.0.1:8888"))
        .unwrap();
    coordinator
        .set_setting("proxy_mode", serde_json::json!("manual"))
        .unwrap();
    coordinator
        .set_setting("proxy_bypass", serde_json::json!("intranet.allowed.test"))
        .unwrap();
    let rules = crate::format_site_rules(&[crate::SiteRule {
        host: "cdn.allowed.test".into(),
        concurrency: 6,
        referer: "https://portal.allowed.test/".into(),
        ..Default::default()
    }]);
    coordinator
        .set_setting("site_rules", serde_json::json!(rules))
        .unwrap();

    let allowed_list = root.join("allowed.txt");
    fs::write(
        &allowed_list,
        "https://intranet.allowed.test/internal.zip\nhttps://cdn.allowed.test/video.mp4\n",
    )
    .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: allowed_list.to_string_lossy().into_owned(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();

    let specs: Vec<TaskSpec> = {
        let core = coordinator.lock().unwrap();
        core.tasks()
            .into_iter()
            .map(|t| core.task_spec(&t.task_id).unwrap().clone())
            .collect()
    };
    assert_eq!(specs.len(), 2);
    let intranet = specs
        .iter()
        .find(|s| s.url.contains("intranet.allowed.test"))
        .unwrap();
    let cdn = specs
        .iter()
        .find(|s| s.url.contains("cdn.allowed.test"))
        .unwrap();
    assert_eq!(
        intranet.resource_key,
        "https://intranet.allowed.test/internal.zip"
    );
    assert_eq!(intranet.proxy, crate::net_policy::DIRECT_PROXY_SENTINEL);
    assert_eq!(cdn.resource_key, "https://cdn.allowed.test/video.mp4");
    assert_eq!(cdn.proxy, "http://127.0.0.1:8888");
    assert_eq!(cdn.concurrency, 6);
    assert_eq!(
        cdn.headers.get("Referer").map(String::as_str),
        Some("https://portal.allowed.test/")
    );

    let blocked_list = root.join("blocked.txt");
    fs::write(&blocked_list, "https://evil.blocked.test/payload.exe\n").unwrap();
    let err = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: blocked_list.to_string_lossy().into_owned(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap_err();
    assert!(err.contains("白名单") || err.contains("域名范围"), "{err}");

    let harvest_err = coordinator
        .dispatch(CoreCommand::HarvestPage {
            url: "https://evil.blocked.test/index.html".into(),
            referer: String::new(),
            probe_urls: Vec::new(),
        })
        .unwrap_err();
    assert!(harvest_err.contains("域名范围"), "{harvest_err}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn handoff_editor_non_sensitive_header_edits_override_replay_context_and_workbench_presentation_is_distinct(
) {
    let root = lifecycle_test_root("handoff-cred-override-workbench");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();
    coordinator
        .set_setting(
            "default_referer",
            serde_json::json!("https://global-default.example/"),
        )
        .unwrap();

    let replay_json = serde_json::json!({
        "cookie": "sid=keep_me",
        "referer": "https://browser-page.example/watch",
        "user_agent": "BrowserUA/1.0",
        "request_contexts": {
            "https://cdn.example.test": {
                "cookie": "sid=keep_me",
                "referer": "https://browser-page.example/watch",
                "request_headers": {
                    "X-Custom": "old-value",
                    "User-Agent": "BrowserUA/1.0"
                }
            }
        }
    })
    .to_string();
    let blob = if cfg!(windows) {
        CredentialVault.protect(&replay_json).unwrap()
    } else {
        replay_json.clone()
    };
    coordinator
        .store_credential("browser-cred-orig", &blob, "browser_replay")
        .unwrap();

    let handoff = crate::contract::BrowserHandoff {
        id: "handoff-wb-1".into(),
        request_id: "req-wb-1".into(),
        status: "pending".into(),
        presentation: "queued".into(),
        created_at_ms: 1_000,
        task_id: None,
        filename: "clip.mp4".into(),
        title: "Clip".into(),
        mime_type: "video/mp4".into(),
        size: 0,
        suppression: None,
        offer: crate::ResourceOffer {
            handoff_id: "handoff-wb-1".into(),
            url: "https://cdn.example.test/clip.mp4".into(),
            filename: "clip.mp4".into(),
            title: "Clip".into(),
            mime_type: "video/mp4".into(),
            resource_kind: crate::ResourceKind::File,
            credential_ref: Some("browser-cred-orig".into()),
            source_page_url: "https://browser-page.example/watch".into(),
            ..Default::default()
        },
    };
    coordinator
        .admit_handoff(&serde_json::to_string(&handoff).unwrap())
        .unwrap();
    coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-wb-1".into(),
            ok: true,
            presenter_id: "confirm-1".into(),
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::PresentHandoff {
            handoff_id: "handoff-wb-1".into(),
            ok: true,
            presenter_id: "workbench:confirm-1".into(),
        })
        .unwrap();
    let persisted = coordinator
        .query_handoffs(Some("handoff-wb-1"), None, false)
        .unwrap();
    let state: Value = serde_json::from_str(&persisted[0]).unwrap();
    assert_eq!(state["presentation"], "workbench");
    assert_eq!(state["presentation_owner"], "workbench");

    let mut edited_headers = BTreeMap::new();
    edited_headers.insert("Referer".into(), "https://edited.example/custom".into());
    edited_headers.insert("User-Agent".into(), "EditedUA/2.0".into());
    edited_headers.insert("X-Custom".into(), "edited-value".into());
    coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-wb-1".into(),
            filename: "clip.mp4".into(),
            download_dir: root.to_string_lossy().into_owned(),
            allow_duplicate: false,
            trusted_ui: true,
            start_paused: true,
            queue_id: crate::DEFAULT_QUEUE_ID.into(),
            spec_override: Some(Box::new(TaskSpec {
                url: "https://cdn.example.test/clip.mp4".into(),
                filename: "clip.mp4".into(),
                headers: edited_headers,
                ..Default::default()
            })),
        })
        .unwrap();

    let stored_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .clone();
    assert_ne!(
        stored_spec.credential_ref.as_deref(),
        Some("browser-cred-orig")
    );
    let (hydrated, replay) = hydrate_replay_headers(&coordinator.core(), stored_spec).unwrap();
    assert_eq!(
        hydrated.headers.get("Cookie").map(String::as_str),
        Some("sid=keep_me")
    );
    assert_eq!(
        hydrated.headers.get("Referer").map(String::as_str),
        Some("https://edited.example/custom")
    );
    assert_eq!(
        hydrated.headers.get("User-Agent").map(String::as_str),
        Some("EditedUA/2.0")
    );
    assert_eq!(
        hydrated.headers.get("X-Custom").map(String::as_str),
        Some("edited-value")
    );
    let mut wire_headers = BTreeMap::new();
    apply_replay_json_for(
        &mut wire_headers,
        &replay,
        "https://cdn.example.test/clip.mp4",
    );
    assert_eq!(
        wire_headers.get("Referer").map(String::as_str),
        Some("https://edited.example/custom")
    );
    assert_eq!(
        wire_headers.get("User-Agent").map(String::as_str),
        Some("EditedUA/2.0")
    );
    assert_eq!(
        wire_headers.get("X-Custom").map(String::as_str),
        Some("edited-value")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn g1_1_spawn_reverts_early_exits_and_start_waits_for_retiring_worker_and_auto_retry_retires_before_requeue(
) {
    let root = lifecycle_test_root("g1-1-zombie-downloading");
    fs::create_dir_all(&root).unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting("legal_terms_accepted", serde_json::json!(true))
        .unwrap();
    coordinator
        .set_setting(
            "download_dir",
            serde_json::json!(root.to_string_lossy().into_owned()),
        )
        .unwrap();

    // 1a. Early return on `update_shutdown` while task is `"downloading"` with 0 active workers reverts to `"queued"`.
    let t_shutdown = lifecycle_task(&coordinator, &root, "downloading");
    coordinator.update_shutdown.store(true, Ordering::SeqCst);
    assert!(coordinator.spawn(t_shutdown.clone()).is_err());
    coordinator.update_shutdown.store(false, Ordering::SeqCst);
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&t_shutdown)
            .unwrap()
            .status,
        "queued"
    );

    // 1b. Early return on future `scheduled_start_at` reverts `"downloading"` -> `"queued"`.
    let t_sched = lifecycle_task(&coordinator, &root, "downloading");
    let mut sched_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&t_sched)
        .unwrap()
        .clone();
    sched_spec.scheduled_start_at = "2999-01-01T00:00:00Z".into();
    coordinator
        .lock()
        .unwrap()
        .replace_spec(&t_sched, sched_spec)
        .unwrap();
    coordinator.spawn(t_sched.clone()).unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&t_sched)
            .unwrap()
            .status,
        "queued"
    );

    // 1c. Early return on disabled queue reverts `"downloading"` -> `"queued"`.
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([
                QueueProfile {
                    id: "default".into(),
                    name: "默认队列".into(),
                    ..QueueProfile::default()
                },
                QueueProfile {
                    id: "disabled-q".into(),
                    name: "禁用队列".into(),
                    enabled: false,
                    ..QueueProfile::default()
                }
            ]),
        )
        .unwrap();
    let t_disabled = lifecycle_task(&coordinator, &root, "downloading");
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::AssignQueue {
            task_ids: vec![t_disabled.clone()],
            queue_id: "disabled-q".into(),
        })
        .unwrap();
    coordinator.spawn(t_disabled.clone()).unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&t_disabled)
            .unwrap()
            .status,
        "queued"
    );

    // 1d. Live worker in `active` must NOT be reverted, and `"paused"` / `"canceled"` / `deleting` are untouched.
    let t_live = lifecycle_task(&coordinator, &root, "downloading");
    coordinator.active.lock().unwrap().insert(t_live.clone());
    coordinator.spawn(t_live.clone()).unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&t_live)
            .unwrap()
            .status,
        "downloading"
    );
    coordinator.active.lock().unwrap().remove(&t_live);

    let t_deleting = lifecycle_task(&coordinator, &root, "downloading");
    let guard = coordinator.enter_deleting_task(&t_deleting).unwrap();
    coordinator.spawn(t_deleting.clone()).unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot(&t_deleting)
            .unwrap()
            .status,
        "downloading"
    );
    drop(guard);
    coordinator
        .dispatch(CoreCommand::TaskAction {
            task_id: t_deleting,
            action: "delete".into(),
        })
        .unwrap();
    for id in [t_shutdown, t_sched, t_disabled, t_live] {
        coordinator
            .dispatch(CoreCommand::TaskAction {
                task_id: id,
                action: "delete".into(),
            })
            .unwrap();
    }

    // 2. `"start"` waits for a retiring worker in `active` instead of stranding `"downloading"` with 0 workers.
    let address = serve_http_fixture(b"start-after-retire");
    let run_coord = direct_http_coordinator(&root);
    run_coord
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address}/retire.bin"),
                filename: "retire.bin".into(),
                scheduled_start_at: "2999-01-01T00:00:00Z".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let task_id = "task-1".to_string();
    // Clear future schedule directly on spec & snapshot while keeping `"queued"` status.
    {
        let mut core = run_coord.lock().unwrap();
        let mut clean_spec = core.task_spec(&task_id).unwrap().clone();
        clean_spec.scheduled_start_at.clear();
        core.replace_spec(&task_id, clean_spec).unwrap();
    }
    // Simulate retiring worker (control != "run") still registered in `active` when `"start"` is dispatched.
    {
        let spec = run_coord
            .lock()
            .unwrap()
            .task_spec(&task_id)
            .unwrap()
            .clone();
        let paths = TaskPaths::for_task(&task_id, &spec).unwrap();
        paths.prepare().unwrap();
        paths.set_control("pause").unwrap();
    }
    run_coord.active.lock().unwrap().insert(task_id.clone());
    let (wait_tx, wait_rx) = mpsc::channel();
    *run_coord.worker_wait_started.lock().unwrap() = Some(wait_tx);
    let active_clone = Arc::clone(&run_coord.active);
    let retire_id = task_id.clone();
    let releaser = thread::spawn(move || {
        wait_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        active_clone.lock().unwrap().remove(&retire_id);
    });
    run_coord
        .dispatch(CoreCommand::TaskAction {
            task_id: task_id.clone(),
            action: "start".into(),
        })
        .unwrap();
    releaser.join().unwrap();
    // Clear snapshot scheduled_start_at if still set or verify wait_for_worker_stop ran cleanly:
    // Note `replace_spec` preserves `snapshot.scheduled_start_at`, so test auto-retry on a fresh task below!
    assert_eq!(
        run_coord.lock().unwrap().snapshot(&task_id).unwrap().status,
        "queued"
    );

    // Also test `"start"` on an unscheduled queued task while retiring worker releases:
    let address2 = serve_http_fixture(b"start-completes-payload");
    run_coord
        .set_setting(
            "queue_profiles",
            serde_json::json!([QueueProfile {
                id: "default".into(),
                name: "默认队列".into(),
                enabled: false,
                ..QueueProfile::default()
            }]),
        )
        .unwrap();
    run_coord
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{address2}/started.bin"),
                filename: "started.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let task2_id = "task-2".to_string();
    {
        let spec2 = run_coord
            .lock()
            .unwrap()
            .task_spec(&task2_id)
            .unwrap()
            .clone();
        let paths2 = TaskPaths::for_task(&task2_id, &spec2).unwrap();
        paths2.prepare().unwrap();
        paths2.set_control("pause").unwrap();
    }
    run_coord.active.lock().unwrap().insert(task2_id.clone());
    // Re-enable queue (task2 is skipped by start_next_queued while still in `active`),
    // then dispatch `"start"` while releaser removes retiring `active` entry!
    run_coord
        .set_setting(
            "queue_profiles",
            serde_json::json!([QueueProfile {
                id: "default".into(),
                name: "默认队列".into(),
                enabled: true,
                ..QueueProfile::default()
            }]),
        )
        .unwrap();
    let (wait_tx2, wait_rx2) = mpsc::channel();
    *run_coord.worker_wait_started.lock().unwrap() = Some(wait_tx2);
    let active_clone2 = Arc::clone(&run_coord.active);
    let retire_id2 = task2_id.clone();
    let releaser2 = thread::spawn(move || {
        wait_rx2.recv_timeout(Duration::from_secs(3)).unwrap();
        active_clone2.lock().unwrap().remove(&retire_id2);
    });
    run_coord
        .dispatch(CoreCommand::TaskAction {
            task_id: task2_id.clone(),
            action: "start".into(),
        })
        .unwrap();
    releaser2.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snap = run_coord
            .lock()
            .unwrap()
            .snapshot(&task2_id)
            .cloned()
            .unwrap();
        if snap.status == "completed" {
            break;
        }
        assert!(Instant::now() < deadline, "task2 stuck: {snap:?}");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read(root.join("started.bin")).unwrap(),
        b"start-completes-payload"
    );

    // 3. Auto-retry removes retiring worker before/with `"queued"` transition so scheduler retries cleanly.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let retry_addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicU64::new(0));
    let server_hits = Arc::clone(&hits);
    let retry_server = thread::spawn(move || {
        // HTTP engine may retry 503 internally; fail enough requests to trigger whole-task failure #1,
        // then succeed with 200 OK on the auto-retried task run!
        while let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let n = server_hits.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // Non-503/non-401 error (e.g. truncated Content-Length) fails HTTP transfer immediately on attempt 1!
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort"
                );
                let _ = stream.flush();
            } else {
                let body = b"auto-retried-ok";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
                let _ = stream.flush();
                if n >= 1 {
                    break;
                }
            }
        }
    });
    let retry_coord = direct_http_coordinator(&root);
    retry_coord
        .set_setting("auto_retry_failed_max", serde_json::json!(2))
        .unwrap();
    retry_coord
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{retry_addr}/flaky.bin"),
                filename: "flaky.bin".into(),
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let completed = wait_for_task_status(&retry_coord, "completed");
    assert_eq!(completed.filename, "flaky.bin");
    assert_eq!(
        fs::read(root.join("flaky.bin")).unwrap(),
        b"auto-retried-ok"
    );
    let _ = retry_server.join();
    drop(retry_coord);
    drop(run_coord);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn g1_2_expand_create_avoids_double_category_nesting_and_untrusted_dir_false_rejection_and_syncs_work_dir(
) {
    let root = lifecycle_test_root("g1-2-category-expand");
    let default_dl = root.join("downloads");
    let external_media = root.join("external-media");
    let external_rule = root.join("external-rule");
    fs::create_dir_all(&default_dl).unwrap();
    fs::create_dir_all(&external_media).unwrap();
    fs::create_dir_all(&external_rule).unwrap();

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(default_dl.to_string_lossy().into_owned()),
            ),
            ("temp_dir".into(), serde_json::json!("")),
            ("auto_category_dirs".into(), serde_json::json!(true)),
        ]))
        .unwrap();

    // 1. Non-empty explicit relative `download_dir` ("batch") + `auto_category_dirs = true`
    // when expanding a local link file (`child.url != parent_url`) must NOT double-nest `媒体\媒体`.
    let link_list = root.join("links.txt");
    fs::write(&link_list, "https://cdn.example.test/episode1.mp4\n").unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: link_list.to_string_lossy().into_owned(),
                download_dir: "batch".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let spec1 = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .clone();
    let expected_cat_dir = default_dl
        .join("batch")
        .join(crate::category::category_label("media"));
    assert_eq!(
        Path::new(&spec1.download_dir),
        expected_cat_dir,
        "must resolve category subdir once without double nesting: {}",
        spec1.download_dir
    );
    assert_eq!(
        Path::new(&spec1.work_dir),
        expected_cat_dir,
        "work_dir must sync to resolved child download_dir when temp_dir is empty"
    );

    // 2. `browser_category_dirs` outside `settings.download_dir` AND parent site rule `download_dir`
    // outside `settings.download_dir` with `trust_download_dir = false` (both empty and non-empty explicit dir).
    let rules = crate::format_site_rules(&[crate::SiteRule {
        host: "portal.example.test".into(),
        download_dir: external_rule.to_string_lossy().into_owned(),
        ..Default::default()
    }]);
    coordinator
        .set_settings(BTreeMap::from([
            ("site_rules".into(), serde_json::json!(rules)),
            (
                "browser_category_dirs".into(),
                serde_json::json!(serde_json::json!({
                    "media": external_media.to_string_lossy(),
                })
                .to_string()),
            ),
        ]))
        .unwrap();

    let inline_metalink = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="movie.mkv">
    <url>https://cdn.other.test/movie.mkv</url>
  </file>
  <file name="notes.txt">
    <url>https://cdn.other.test/notes.txt</url>
  </file>
</metalink>"#;
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: inline_metalink.into(),
                download_dir: "custom-sub".into(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let spec2 = coordinator
        .lock()
        .unwrap()
        .task_spec("task-2")
        .unwrap()
        .clone();
    let spec3 = coordinator
        .lock()
        .unwrap()
        .task_spec("task-3")
        .unwrap()
        .clone();
    assert_eq!(Path::new(&spec2.download_dir), external_media);
    assert_eq!(Path::new(&spec2.work_dir), external_media);
    let expected_other_dir = default_dl
        .join("custom-sub")
        .join(crate::category::category_label("other"));
    assert_eq!(Path::new(&spec3.download_dir), expected_other_dir);
    assert_eq!(Path::new(&spec3.work_dir), expected_other_dir);

    // Also test parent HTTP `.meta4` matching `portal.example.test` site rule (`external_rule`)
    // expanding to `cdn.other.test` with empty `explicit.download_dir` (`trust_download_dir = false`).
    let meta_addr = serve_http_fixture(inline_metalink.as_bytes());
    let host_rules = crate::format_site_rules(&[crate::SiteRule {
        host: "127.0.0.1".into(),
        download_dir: external_rule.to_string_lossy().into_owned(),
        proxy_mode: "direct".into(),
        ..Default::default()
    }]);
    coordinator
        .set_setting("site_rules", serde_json::json!(host_rules))
        .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{meta_addr}/feed.meta4"),
                allow_duplicate: true,
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let spec5 = coordinator
        .lock()
        .unwrap()
        .task_spec("task-5")
        .unwrap()
        .clone();
    // `cdn.other.test` does NOT match `127.0.0.1` site rule, so its base is `default_dl` (not stale `external_rule`!).
    assert_eq!(
        Path::new(&spec5.download_dir),
        default_dl.join(crate::category::category_label("other"))
    );
    assert_eq!(Path::new(&spec5.work_dir), Path::new(&spec5.download_dir));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn g1_3_and_g7_5_authenticated_http_torrent_hydrates_replay_headers_and_preserves_manual_swarm_proxy_on_bypass(
) {
    let root = lifecycle_test_root("g1-3-g7-5-torrent-auth-proxy");
    fs::create_dir_all(&root).unwrap();

    // Start an authenticated HTTP server serving both `/auth.torrent` and `/seed.bin`,
    // requiring BOTH `Cookie: bt_auth=1` and `Authorization: Bearer bt-secret`.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = b"authenticated-torrent-payload-bytes".to_vec();
    let seed_url = format!("http://{addr}/seed.bin");
    let mut torrent_bytes = format!(
        "d8:url-list{}:{}4:infod6:lengthi{}e4:name12:auth-item.ts12:piece lengthi64e6:pieces20:",
        seed_url.len(),
        seed_url,
        payload.len()
    )
    .into_bytes();
    torrent_bytes.extend_from_slice(&crate::crypto_lite::sha1(&payload));
    torrent_bytes.extend_from_slice(b"ee");

    let stop_server = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop_server);
    let server_torrent = torrent_bytes.clone();
    let server_payload = payload.clone();
    let server = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while !server_stop.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut has_cookie = false;
            let mut has_auth = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("cookie:") && line.contains("bt_auth=1") {
                    has_cookie = true;
                }
                if lower.starts_with("authorization:") && line.contains("Bearer bt-secret") {
                    has_auth = true;
                }
            }
            if !has_cookie || !has_auth {
                let _ = write!(
                    stream,
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\nConnection: close\r\n\r\nunauthorized"
                );
                let _ = stream.flush();
                continue;
            }
            let body = if request_line.contains("/seed.bin") {
                &server_payload
            } else {
                &server_torrent
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });

    // 1. Configure `proxy_mode = "manual"` with a dead proxy `http://127.0.0.1:1`,
    // AND `proxy_bypass = "127.0.0.1"`!
    // Import via cURL (`G1-3` + `G7-5`):
    // - `spec.proxy` must retain `"http://127.0.0.1:1"` for the swarm (NOT `DIRECT_PROXY_SENTINEL`!).
    // - Admission (`validate_torrent_spec`) AND BT `.torrent` fetch must evaluate `effective_proxy`
    //   against `spec.url` (`127.0.0.1` bypassed -> direct!) AND send hydrated Cookie + Authorization!
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            ),
            ("proxy_mode".into(), serde_json::json!("manual")),
            ("proxy_url".into(), serde_json::json!("http://127.0.0.1:1")),
            ("proxy_bypass".into(), serde_json::json!("127.0.0.1")),
            ("bt_enable_dht".into(), serde_json::json!(false)),
        ]))
        .unwrap();

    let torrent_url = format!("http://{addr}/auth.torrent");
    let curl_cmd =
        format!("curl -b 'bt_auth=1' -H 'Authorization: Bearer bt-secret' '{torrent_url}'");
    coordinator
        .dispatch(CoreCommand::ImportCurl {
            command: curl_cmd,
            options: TaskSpec::default(),
        })
        .unwrap();

    let spec1 = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .clone();
    assert_eq!(
        spec1.proxy, "http://127.0.0.1:1",
        "Torrent spec.proxy must keep manual proxy_url for external swarm instead of baking DIRECT_PROXY_SENTINEL"
    );
    assert_eq!(spec1.filename, "auth-item.ts");
    let completed = wait_for_task_status(&coordinator, "completed");
    assert_eq!(
        fs::read(&completed.output_path).unwrap(),
        payload.as_slice()
    );

    // 2. Explicit `"__direct__"` on a Torrent spec must still honor `DIRECT_PROXY_SENTINEL`.
    let explicit_direct = coordinator
        .apply_defaults_to_spec(TaskSpec {
            url: torrent_url,
            resource_kind: ResourceKind::Torrent,
            proxy: "__direct__".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        explicit_direct.proxy,
        crate::net_policy::DIRECT_PROXY_SENTINEL
    );

    stop_server.store(true, Ordering::SeqCst);
    let _ = std::net::TcpStream::connect(addr);
    let _ = server.join();
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn g1_4_enabling_queue_deleting_queue_and_raising_max_active_immediately_start_queued_tasks() {
    let root = lifecycle_test_root("g1-4-queue-kick");
    fs::create_dir_all(&root).unwrap();
    let coordinator = direct_http_coordinator(&root);

    // 1. Queue starts disabled; non-scheduled task is queued; enabling queue starts it immediately.
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([
                QueueProfile {
                    id: "default".into(),
                    name: "默认队列".into(),
                    max_active: 1,
                    ..QueueProfile::default()
                },
                QueueProfile {
                    id: "batch-q".into(),
                    name: "批量队列".into(),
                    enabled: false,
                    ..QueueProfile::default()
                },
                QueueProfile {
                    id: "temp-q".into(),
                    name: "临时队列".into(),
                    enabled: false,
                    ..QueueProfile::default()
                }
            ]),
        )
        .unwrap();

    let addr1 = serve_http_fixture(b"enabled-queue-ok");
    let addr2 = serve_http_fixture(b"deleted-queue-migrated-ok");
    let addr3 = serve_http_fixture(b"max-active-raised-ok");

    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{addr1}/q1.bin"),
                filename: "q1.bin".into(),
                queue_id: "batch-q".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot("task-1")
            .unwrap()
            .status,
        "queued"
    );

    // Enabling `batch-q` via `set_setting("queue_profiles", ...)` must trigger `start_next_queued(false)`.
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([
                QueueProfile {
                    id: "default".into(),
                    name: "默认队列".into(),
                    max_active: 1,
                    ..QueueProfile::default()
                },
                QueueProfile {
                    id: "batch-q".into(),
                    name: "批量队列".into(),
                    enabled: true,
                    ..QueueProfile::default()
                },
                QueueProfile {
                    id: "temp-q".into(),
                    name: "临时队列".into(),
                    enabled: false,
                    ..QueueProfile::default()
                }
            ]),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if coordinator
            .lock()
            .unwrap()
            .snapshot("task-1")
            .unwrap()
            .status
            == "completed"
        {
            break;
        }
        assert!(Instant::now() < deadline, "task-1 stuck in batch-q");
        thread::sleep(Duration::from_millis(20));
    }

    // 2. Deleting disabled `temp-q` migrates its orphan queued task to `"default"` AND starts it!
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{addr2}/q2.bin"),
                filename: "q2.bin".into(),
                queue_id: "temp-q".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot("task-2")
            .unwrap()
            .status,
        "queued"
    );
    coordinator
        .set_setting(
            "queue_profiles",
            serde_json::json!([QueueProfile {
                id: "default".into(),
                name: "默认队列".into(),
                max_active: 1,
                ..QueueProfile::default()
            }]),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snap = coordinator
            .lock()
            .unwrap()
            .snapshot("task-2")
            .cloned()
            .unwrap();
        if snap.status == "completed" {
            assert_eq!(snap.queue_id, "default");
            break;
        }
        assert!(Instant::now() < deadline, "migrated task-2 stuck: {snap:?}");
        thread::sleep(Duration::from_millis(20));
    }

    // 3. Occupy slot 1 of `"default"` (`max_active = 1`), queue `task-4`, then raise `queue_max_active` to 2!
    let blocker_id = lifecycle_task(&coordinator, &root, "downloading");
    coordinator
        .active
        .lock()
        .unwrap()
        .insert(blocker_id.clone());
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{addr3}/q3.bin"),
                filename: "q3.bin".into(),
                queue_id: "default".into(),
                ..Default::default()
            },
        })
        .unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .snapshot("task-4")
            .unwrap()
            .status,
        "queued"
    );
    coordinator
        .set_setting("queue_max_active", serde_json::json!(2))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if coordinator
            .lock()
            .unwrap()
            .snapshot("task-4")
            .unwrap()
            .status
            == "completed"
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "task-4 stuck after max_active raise"
        );
        thread::sleep(Duration::from_millis(20));
    }
    coordinator.active.lock().unwrap().remove(&blocker_id);
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn g1_5_g1_7_g1_8_and_g4_7_deleting_guard_curl_headers_http_error_control_and_dash_live_cleanup() {
    let root = lifecycle_test_root("g1-misc-fixes");
    fs::create_dir_all(&root).unwrap();
    let coordinator = direct_http_coordinator(&root);

    // 1. G1-5: `dispatch_inner` rejects start actions when `is_task_deleting` without creating `.hls-tasks/{id}/control`.
    let del_id = lifecycle_task(&coordinator, &root, "queued");
    let del_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&del_id)
        .unwrap()
        .clone();
    let del_paths = TaskPaths::for_task(&del_id, &del_spec).unwrap();
    let _ = fs::remove_dir_all(del_paths.task_dir());
    {
        let _del_guard = coordinator.enter_deleting_task(&del_id).unwrap();
        let err = coordinator
            .dispatch_inner(CoreCommand::TaskAction {
                task_id: del_id.clone(),
                action: "start".into(),
            })
            .unwrap_err();
        assert!(err.contains("正在删除"), "{err}");
        assert!(
            !del_paths.task_dir().exists(),
            "deleting task must never recreate .hls-tasks directory"
        );
    }

    // 2. G1-7: `import_curl_command` merges `options.headers` case-insensitively over lowercase cURL headers.
    let mut explicit_headers = BTreeMap::new();
    explicit_headers.insert("Authorization".into(), "Bearer explicit-new".into());
    explicit_headers.insert("X-Custom-Key".into(), "explicit-val".into());
    explicit_headers.insert("Referer".into(), "https://explicit.example/page".into());
    coordinator
        .dispatch(CoreCommand::ImportCurl {
            command: "curl -H 'authorization: Bearer curl-old' -H 'x-custom-key: curl-old' -H 'referer: https://curl.example/old' 'https://cdn.example.test/item.bin'".into(),
            options: TaskSpec {
                headers: explicit_headers,
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let curl_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-2")
        .unwrap()
        .clone();
    let (hydrated_curl, replay_curl) =
        hydrate_replay_headers(&coordinator.core(), curl_spec).unwrap();
    assert_eq!(
        hydrated_curl
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        vec!["Bearer explicit-new"]
    );
    assert_eq!(
        hydrated_curl
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("x-custom-key"))
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        vec!["explicit-val"]
    );
    assert_eq!(
        hydrated_curl
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("referer"))
            .map(|(_, v)| v.as_str()),
        Some("https://explicit.example/page")
    );
    assert!(!replay_curl.contains("curl-old"), "{replay_curl}");

    // 3. G1-8: `run_http_file` does not overwrite `paths.control` with `"cancel"` on error/disconnect,
    // so genuine failures remain `"failed"` instead of `"canceled"`.
    let fail_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fail_addr = fail_listener.local_addr().unwrap();
    let fail_server = thread::spawn(move || {
        if let Ok((mut stream, _)) = fail_listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
            );
            let _ = stream.flush();
        }
    });
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{fail_addr}/missing.bin"),
                filename: "missing.bin".into(),
                ..Default::default()
            },
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let failed = loop {
        if let Some(task) = coordinator
            .tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-3" && task.status == "failed")
        {
            break task;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for task-3 failed: {:?}",
            coordinator.tasks().unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(failed.task_id, "task-3");
    assert_eq!(failed.http_status, Some(404));
    let fail_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-3")
        .unwrap()
        .clone();
    let fail_paths = TaskPaths::for_task("task-3", &fail_spec).unwrap();
    assert_eq!(
        crate::http_engine::read_control(&fail_paths.control),
        crate::http_engine::Control::Run
    );
    let _ = fail_server.join();

    // 4. G4-7: `cleanup_completed_media` removes `"live_dash_state.json"` and `".live_dash_state.json.tmp"`.
    let dash_spec = TaskSpec {
        url: "https://cdn.example.test/live.mpd".into(),
        resource_kind: ResourceKind::Dash,
        filename: "live-dash.mp4".into(),
        download_dir: root.to_string_lossy().into_owned(),
        work_dir: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let dash_paths = TaskPaths::for_task("task-dash-live", &dash_spec).unwrap();
    dash_paths.prepare().unwrap();
    let published = root.join("live-dash.mp4");
    fs::write(&published, b"published-dash-mp4").unwrap();
    fs::write(dash_paths.task_dir().join("live_dash_state.json"), b"{}").unwrap();
    fs::write(
        dash_paths.task_dir().join(".live_dash_state.json.tmp"),
        b"{}",
    )
    .unwrap();
    cleanup_completed_media(&dash_paths, &published, false).unwrap();
    cleanup_task_metadata(&dash_paths).unwrap();
    assert!(
        !dash_paths.task_dir().exists(),
        "live_dash_state.json artifacts must be cleaned so task directory is removed"
    );

    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn g1_6_maybe_schedule_power_uses_live_snapshot_queue_id_after_cross_queue_assign() {
    let _power_guard = TEST_POWER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _ = crate::power_action::cancel();
    let root = lifecycle_test_root("g1-6-power-queue-id");
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            ),
            (
                "queue_profiles".into(),
                serde_json::json!([
                    QueueProfile {
                        id: "default".into(),
                        name: "默认队列".into(),
                        completion_action: "none".into(),
                        ..QueueProfile::default()
                    },
                    QueueProfile {
                        id: "night".into(),
                        name: "夜间队列".into(),
                        completion_action: "sleep".into(),
                        ..QueueProfile::default()
                    }
                ]),
            ),
        ]))
        .unwrap();

    // 1. Seed a historical failure in `night` so `batch_disrupted["night"]` is non-empty while `night` is idle.
    let fail_night_id = lifecycle_task(&coordinator, &root, "queued");
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::AssignQueue {
            task_ids: vec![fail_night_id.clone()],
            queue_id: "night".into(),
        })
        .unwrap();
    coordinator
        .lock()
        .unwrap()
        .report_failure(
            &fail_night_id,
            crate::TaskFailure {
                code: "HTTP_500".into(),
                message: "boom".into(),
                stage: "transfer".into(),
                url: "https://cdn.example.test/fail.bin".into(),
                hint: String::new(),
                http_status: Some(500),
                attempt: 1,
            },
        )
        .unwrap();
    assert!(coordinator
        .lock()
        .unwrap()
        .batch_disrupted_for_queue("night")
        .is_some());

    // 2. Start a task in `"default"`, capture its stale worker-launch `spec` (`queue_id == "default"`),
    // then move it while live to `"night"` via `AssignQueue` and complete it!
    let live_id = lifecycle_task(&coordinator, &root, "downloading");
    let stale_launch_spec = coordinator
        .lock()
        .unwrap()
        .task_spec(&live_id)
        .unwrap()
        .clone();
    assert_eq!(stale_launch_spec.queue_id, "default");

    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::AssignQueue {
            task_ids: vec![live_id.clone()],
            queue_id: "night".into(),
        })
        .unwrap();
    assert!(
        coordinator
            .lock()
            .unwrap()
            .batch_disrupted_for_queue("night")
            .is_none(),
        "AssignQueue of live task into idle night queue must clear stale batch_disrupted"
    );

    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::UpdateProgress {
            task_id: live_id.clone(),
            downloaded_bytes: 64,
            total_bytes: Some(64),
            speed_bytes_per_sec: 0,
            stage: "finished".into(),
            status: "completed".into(),
        })
        .unwrap();

    maybe_schedule_power(&coordinator.core(), &live_id, &stale_launch_spec)
        .expect("maybe_schedule_power");
    let (action, title, _) =
        crate::power_action::pending_details().expect("night queue completion must arm sleep");
    assert_eq!(action, "sleep");
    assert_eq!(title, "队列：夜间队列");
    let _ = crate::power_action::cancel();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn g5_4_and_g7_8_json_task_import_enforces_allowed_hosts_and_cast_stop_clears_mount_on_offline_receiver(
) {
    let root = lifecycle_test_root("g5-4-g7-8-import-and-cast");
    fs::create_dir_all(&root).unwrap();
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            ),
            (
                "allowed_hosts".into(),
                serde_json::json!("allowed.example.test"),
            ),
        ]))
        .unwrap();

    // 1. G5-4: JSON task import rejects disallowed primary URL (including `.torrent` container URLs)
    // with `"地址不在允许的域名白名单中"` and strips disallowed mirrors!
    let bad_json = root.join("bad-tasks.json");
    fs::write(
        &bad_json,
        serde_json::json!({
            "schema": "hls-downloader.tasks.v1",
            "tasks": [{
                "url": "https://blocked.example.test/evil.torrent",
                "filename": "evil.torrent",
                "start_paused": true
            }]
        })
        .to_string(),
    )
    .unwrap();
    let err = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: bad_json.to_string_lossy().into_owned(),
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap_err();
    assert_eq!(err, "地址不在允许的域名白名单中");

    let mixed_mirrors_json = root.join("mirrors-tasks.json");
    fs::write(
        &mixed_mirrors_json,
        serde_json::json!({
            "schema": "hls-downloader.tasks.v1",
            "tasks": [{
                "url": "https://allowed.example.test/pkg.zip",
                "filename": "pkg.zip",
                "mirrors": [
                    "https://allowed.example.test/mirror1.zip",
                    "https://blocked.example.test/mirror2.zip"
                ],
                "scheduled_start_at": "2999-01-01T00:00:00Z"
            }]
        })
        .to_string(),
    )
    .unwrap();
    coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: mixed_mirrors_json.to_string_lossy().into_owned(),
                ..Default::default()
            },
        })
        .unwrap();
    let imported_spec = coordinator
        .lock()
        .unwrap()
        .task_spec("task-1")
        .unwrap()
        .clone();
    assert_eq!(
        imported_spec.mirrors,
        vec!["https://allowed.example.test/mirror1.zip".to_string()]
    );

    // 2. G7-8: `cast_control_events` on `"stop"` ALWAYS calls `clear_cast_mount()` and emits
    // `CoreEvent::CastSession { active: false, status: "STOPPED", .. }` even when `control_session("stop")` fails!
    {
        let _cast_guard = TEST_CAST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let media_file = root.join("cast-secret.mp4");
        fs::write(&media_file, b"secret-cast-stream").unwrap();
        let server = shared_media().unwrap();
        let token = crate::playback::random_mount_token();
        server.mount(&token, media_file);
        let loopback_url = server.url_for(&token);

        // Verify mounted token is currently reachable on loopback before stop.
        let (_, before_bytes) = crate::http_engine::fetch_bytes_limited(
            &loopback_url,
            &std::collections::HashMap::new(),
            crate::net_policy::DIRECT_PROXY_SENTINEL,
            1024,
        )
        .unwrap();
        assert_eq!(before_bytes, b"secret-cast-stream");

        // Ensure `crate::cast::control_session("stop")` returns `Err(...)` (no active cast device in `last_cast`!).
        let _ = crate::cast::control_session("stop");
        let gen = crate::cast::session_generation();
        assert!(
            crate::cast::control_session("stop").is_err(),
            "precondition: control_session(\"stop\") must return Err when receiver/session is unavailable"
        );
        remember_cast_mount(&token, &loopback_url, "Secret Cast", "task-1");
        assert!(active_cast_mount().lock().unwrap().is_some());

        let stop_events = if gen > 0 {
            coordinator.cast_control(gen, "stop").unwrap()
        } else {
            coordinator
                .dispatch(CoreCommand::ControlCast {
                    action: "stop".into(),
                })
                .unwrap()
        };
        assert!(
            active_cast_mount().lock().unwrap().is_none(),
            "cast stop must always clear active_cast_mount even on receiver error"
        );
        assert!(
            matches!(
                &stop_events.last().unwrap().event,
                CoreEvent::CastSession {
                    active: false,
                    status,
                    media_url,
                    ..
                } if status == "STOPPED" && media_url.is_empty()
            ),
            "expected CastSession STOPPED event, got: {stop_events:?}"
        );

        // Verify the MediaServer mount was unmounted so the token is no longer served!
        let (status_after, _) = crate::http_engine::fetch_bytes_limited(
            &loopback_url,
            &std::collections::HashMap::new(),
            crate::net_policy::DIRECT_PROXY_SENTINEL,
            1024,
        )
        .unwrap();
        assert_eq!(status_after, 404);
    }

    let _ = fs::remove_dir_all(root);
}

fn created_task_id(events: &[EventEnvelope]) -> String {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            CoreEvent::TaskCreated { snapshot } => Some(snapshot.task_id.clone()),
            _ => None,
        })
        .expect("expected TaskCreated event")
}

#[test]
fn t2_r1_end_to_end_coordinator_enforces_redirect_allowed_hosts_across_all_workers_and_surfaces() {
    let root = lifecycle_test_root("t2-r1-allowed-hosts-e2e");
    fs::create_dir_all(&root).unwrap();

    // Forbidden server on `127.0.0.2`: MUST NEVER receive a single request when `allowed_hosts = "127.0.0.1"`.
    let forbidden_listener = TcpListener::bind("127.0.0.2:0").unwrap();
    let forbidden_addr = forbidden_listener.local_addr().unwrap();
    let forbidden_hits = Arc::new(AtomicU64::new(0));
    let forbidden_hits_srv = Arc::clone(&forbidden_hits);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_forbidden = Arc::clone(&stop);
    let forbidden_srv = thread::spawn(move || {
        forbidden_listener.set_nonblocking(true).unwrap();
        while !stop_forbidden.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = forbidden_listener.accept() {
                forbidden_hits_srv.fetch_add(1, Ordering::SeqCst);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nforbidden"
                );
                let _ = stream.flush();
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });

    // Allowed server on `127.0.0.1` serving manifests/videos and 302 redirects to `127.0.0.2`.
    let allowed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let allowed_addr = allowed_listener.local_addr().unwrap();
    let live_audio_playlist_hits = Arc::new(AtomicU64::new(0));
    let live_audio_hits_srv = Arc::clone(&live_audio_playlist_hits);
    let stop_allowed = Arc::clone(&stop);
    let allowed_srv = thread::spawn(move || {
        allowed_listener.set_nonblocking(true).unwrap();
        while !stop_allowed.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = allowed_listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            match path {
                "/ok-video.mp4" | "/ok-seg.ts" => {
                    let body = b"valid-media-bytes-payload";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
                "/vod.m3u8" => {
                    let body = format!(
                        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nhttp://{allowed_addr}/redir-seg.ts\n#EXT-X-ENDLIST\n"
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
                "/live-master.m3u8" => {
                    let body = format!(
                        "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Main\",DEFAULT=YES,URI=\"http://{allowed_addr}/live-audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"aud\"\nhttp://{allowed_addr}/live-video.m3u8\n"
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
                "/live-video.m3u8" => {
                    let body = format!(
                        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nhttp://{allowed_addr}/ok-seg.ts\n#EXT-X-ENDLIST\n"
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
                "/live-audio.m3u8" => {
                    live_audio_hits_srv.fetch_add(1, Ordering::SeqCst);
                    let loc = format!("http://{forbidden_addr}/evil-audio.m3u8");
                    let _ = write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
                "/vod.mpd" => {
                    let body = format!(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
  <Period>
    <AdaptationSet mimeType="video/mp4" contentType="video">
      <Representation id="v1" bandwidth="500000" width="640" height="360">
        <SegmentList duration="4">
          <SegmentURL media="http://{allowed_addr}/redir-dash.m4s"/>
        </SegmentList>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/dash+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
                _ => {
                    let loc = format!("http://{forbidden_addr}/evil-target.bin");
                    let _ = write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
            let _ = stream.flush();
        }
    });

    // Real CoreCoordinator WITHOUT manual `with_allowed_hosts` / `with_proxy_policy`!
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            ),
            ("proxy_mode".into(), serde_json::json!("direct")),
            ("allowed_hosts".into(), serde_json::json!("127.0.0.1")),
            ("auto_retry_failed_max".into(), serde_json::json!(0)),
        ]))
        .unwrap();

    let wait_task_terminal = |coord: &CoreCoordinator, id: &str| -> crate::TaskSnapshot {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snap = coord.lock().unwrap().snapshot(id).cloned().unwrap();
            if matches!(snap.status.as_str(), "failed" | "completed" | "canceled") {
                return snap;
            }
            assert!(Instant::now() < deadline, "task {id} timed out: {snap:?}");
            thread::sleep(Duration::from_millis(20));
        }
    };

    // 1. Single-track HTTP (`run_http_file` worker thread)
    let ev1 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{allowed_addr}/redir-file.bin"),
                filename: "single.bin".into(),
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let t1 = created_task_id(&ev1);
    let snap1 = wait_task_terminal(&coordinator, &t1);
    assert_eq!(
        snap1.status, "failed",
        "single-track redirect must fail: {snap1:?}"
    );

    // 2. Dual-track HTTP (`run_dual_track_http_file` audio worker thread)
    let ev2 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{allowed_addr}/ok-video.mp4"),
                audio_url: format!("http://{allowed_addr}/redir-audio.m4a"),
                filename: "dual.mp4".into(),
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let t2 = created_task_id(&ev2);
    let snap2 = wait_task_terminal(&coordinator, &t2);
    assert_eq!(
        snap2.status, "failed",
        "dual-track audio redirect must fail: {snap2:?}"
    );

    // 3. VOD HLS (`poll_media_progress` + `download_batch` segment worker thread)
    let ev3 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{allowed_addr}/vod.m3u8"),
                resource_kind: crate::ResourceKind::Hls,
                filename: "vod-hls.mp4".into(),
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let t3 = created_task_id(&ev3);
    let snap3 = wait_task_terminal(&coordinator, &t3);
    assert_eq!(
        snap3.status, "failed",
        "HLS segment redirect must fail: {snap3:?}"
    );

    // 4. Live HLS with `#EXT-X-MEDIA:TYPE=AUDIO` (`LiveAudioRecorder` worker thread!)
    let ev4 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{allowed_addr}/live-master.m3u8"),
                resource_kind: crate::ResourceKind::Live,
                filename: "live-hls.ts".into(),
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let t4 = created_task_id(&ev4);
    let snap4 = wait_task_terminal(&coordinator, &t4);
    assert_eq!(
        snap4.status, "failed",
        "LiveAudioRecorder redirect must fail: {snap4:?}"
    );
    assert!(
        live_audio_playlist_hits.load(Ordering::SeqCst) >= 1,
        "LiveAudioRecorder must have fetched /live-audio.m3u8 on its spawned thread"
    );

    // 5. VOD DASH (`download_vod_dash` segment worker thread)
    let ev5 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{allowed_addr}/vod.mpd"),
                resource_kind: crate::ResourceKind::Dash,
                filename: "vod-dash.mp4".into(),
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let t5 = created_task_id(&ev5);
    let snap5 = wait_task_terminal(&coordinator, &t5);
    assert_eq!(
        snap5.status, "failed",
        "DASH segment redirect must fail: {snap5:?}"
    );

    // 6. ProbeUrl & HarvestPage
    let probe_ev = coordinator
        .dispatch(CoreCommand::ProbeUrl {
            url: format!("http://{allowed_addr}/probe-redir"),
            spec: None,
        })
        .unwrap();
    assert!(
        probe_ev
            .iter()
            .any(|e| matches!(&e.event, CoreEvent::Error { code, .. } if code == "probe_failed")),
        "ProbeUrl redirect to non-whitelisted host must emit probe_failed: {probe_ev:?}"
    );
    assert!(
        coordinator
            .dispatch(CoreCommand::HarvestPage {
                url: format!("http://{allowed_addr}/harvest-redir"),
                referer: String::new(),
                probe_urls: Vec::new(),
            })
            .is_err(),
        "HarvestPage redirect to non-whitelisted host must fail"
    );
    let harvest_probe_ev = coordinator
        .dispatch(CoreCommand::HarvestPage {
            url: format!("http://{allowed_addr}/ok-video.mp4"),
            referer: String::new(),
            probe_urls: vec![format!("http://{allowed_addr}/harvest-size-redir")],
        })
        .unwrap();
    assert!(
        matches!(
            &harvest_probe_ev[0].event,
            CoreEvent::HarvestProbeResult { links, .. } if links.len() == 1 && links[0].size == 0
        ),
        "HarvestPage probe_urls redirect must be blocked (size == 0): {harvest_probe_ev:?}"
    );

    // 7. Crawl (`crawl::dispatch` + `crawl::spawn` background worker + `scope.spawn` page workers)
    let created_crawl = crate::crawl::dispatch(
        &coordinator,
        crate::CrawlRequest::Create {
            spec: crate::CrawlSpec {
                entry_url: format!("http://{allowed_addr}/crawl-redir"),
                same_origin: false,
                max_pages: 5,
                page_concurrency: 1,
                ..Default::default()
            },
        },
    )
    .unwrap();
    let crawl_id = created_crawl[0].id.clone();
    let crawl_stop = Arc::new(AtomicBool::new(false));
    crate::crawl::spawn(coordinator.clone(), Arc::clone(&crawl_stop));
    let crawl_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let listed = crate::crawl::dispatch(&coordinator, crate::CrawlRequest::List).unwrap();
        let proj = listed.iter().find(|p| p.id == crawl_id).unwrap();
        if proj.status == "paused" {
            assert!(
                proj.reason.contains("allowed_hosts") || !proj.reason.is_empty(),
                "crawl project must pause with error on disallowed redirect: {proj:?}"
            );
            break;
        }
        assert!(
            Instant::now() < crawl_deadline,
            "crawl did not fail/pause: {proj:?}"
        );
        thread::sleep(Duration::from_millis(30));
    }
    crawl_stop.store(true, Ordering::SeqCst);

    // 8. ShareMedia / MediaProxy (`MediaServer` worker thread serving proxied stream)
    {
        let _cast_guard = TEST_CAST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let share_ev = coordinator
            .dispatch(CoreCommand::ShareMedia {
                path: String::new(),
                url: format!("http://{allowed_addr}/cast-redir.mp4"),
                title: "Cast Redirect Test".into(),
                device_id: String::new(),
                request_id: String::new(),
            })
            .unwrap();
        let published_url = share_ev
            .iter()
            .find_map(|e| match &e.event {
                CoreEvent::CastSession { media_url, .. } => Some(media_url.clone()),
                _ => None,
            })
            .expect("ShareMedia should emit CastSession");
        let token = media_token_from_url(&published_url).unwrap();
        let loopback_proxy_url = shared_media().unwrap().url_for(&token);
        let (proxy_status, _) = crate::http_engine::fetch_bytes_limited(
            &loopback_proxy_url,
            &std::collections::HashMap::new(),
            crate::net_policy::DIRECT_PROXY_SENTINEL,
            1024,
        )
        .unwrap();
        assert_eq!(
            proxy_status, 502,
            "MediaProxy must block redirect to disallowed host with 502"
        );
        clear_cast_mount();
    }

    assert_eq!(
        forbidden_hits.load(Ordering::SeqCst),
        0,
        "non-whitelisted redirect target 127.0.0.2 must NEVER be contacted across any path!"
    );

    stop.store(true, Ordering::SeqCst);
    let _ = allowed_srv.join();
    let _ = forbidden_srv.join();
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn t2_r1_end_to_end_coordinator_switches_manual_proxy_and_bypass_across_redirects_and_mirrors() {
    let root = lifecycle_test_root("t2-r1-proxy-bypass-e2e");
    fs::create_dir_all(&root).unwrap();

    // `127.0.0.1` acts as "public" (NOT in `proxy_bypass` -> MUST use authenticated manual proxy!).
    // `127.0.0.2` acts as "bypassed" (`proxy_bypass = "127.0.0.2"` -> MUST connect DIRECT!).
    let public_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let public_addr = public_listener.local_addr().unwrap();
    let bypass_listener = TcpListener::bind("127.0.0.2:0").unwrap();
    let bypass_addr = bypass_listener.local_addr().unwrap();
    let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let proxied_targets: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bypass_direct_hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Authenticated HTTP forward proxy (`puser:psecret`).
    let expected_proxy_auth = format!("Basic {}", STANDARD.encode("puser:psecret"));
    let stop_proxy = Arc::clone(&stop);
    let proxied_log = Arc::clone(&proxied_targets);
    let proxy_srv = thread::spawn(move || {
        proxy_listener.set_nonblocking(true).unwrap();
        while !stop_proxy.load(Ordering::SeqCst) {
            let Ok((mut client, _)) = proxy_listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            client.set_nonblocking(false).unwrap();
            let _ = client.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let mut req_line = String::new();
            if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
                continue;
            }
            let mut authed = false;
            let mut extra_headers = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if line
                    .to_ascii_lowercase()
                    .starts_with("proxy-authorization:")
                {
                    if line.contains(&expected_proxy_auth) {
                        authed = true;
                    }
                } else if !line.to_ascii_lowercase().starts_with("proxy-connection:")
                    && !line.to_ascii_lowercase().starts_with("connection:")
                    && !line.to_ascii_lowercase().starts_with("host:")
                {
                    extra_headers.push(line);
                }
            }
            if !authed {
                let _ = write!(
                    client,
                    "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = client.flush();
                continue;
            }
            let parts: Vec<&str> = req_line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let method = parts[0];
            let target_url = parts[1].to_string();
            proxied_log.lock().unwrap().push(target_url.clone());

            let without_scheme = target_url.strip_prefix("http://").unwrap_or(&target_url);
            let (authority, path_part) = match without_scheme.split_once('/') {
                Some((h, p)) => (h, format!("/{p}")),
                None => (without_scheme, "/".to_string()),
            };
            if let Ok(mut upstream) = std::net::TcpStream::connect(authority) {
                let _ = upstream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = write!(
                    upstream,
                    "{method} {path_part} HTTP/1.1\r\nHost: {authority}\r\nX-Via-Manual-Proxy: 1\r\nConnection: close\r\n"
                );
                for hdr in &extra_headers {
                    let _ = upstream.write_all(hdr.as_bytes());
                }
                let _ = upstream.write_all(b"\r\n");
                let _ = upstream.flush();
                let mut resp = Vec::new();
                let _ = upstream.read_to_end(&mut resp);
                let _ = client.write_all(&resp);
                let _ = client.flush();
            }
        }
    });

    // Public server (`127.0.0.1`): REJECTS direct connections missing `X-Via-Manual-Proxy: 1`!
    let stop_public = Arc::clone(&stop);
    let public_srv = thread::spawn(move || {
        public_listener.set_nonblocking(true).unwrap();
        while !stop_public.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = public_listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line);
            let mut via_proxy = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("x-via-manual-proxy:") {
                    via_proxy = true;
                }
            }
            if !via_proxy {
                // Direct leak to public server! Fail immediately with 403!
                let _ = write!(
                    stream,
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 11\r\nConnection: close\r\n\r\ndirect-leak"
                );
                let _ = stream.flush();
                continue;
            }
            let path = req_line.split_whitespace().nth(1).unwrap_or("/");
            match path {
                "/redir-to-bypass" => {
                    let loc = format!("http://{bypass_addr}/from-public-redir.bin");
                    let _ = write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
                "/fail-primary" => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
                "/public-target.bin" | "/public-mirror.bin" => {
                    let body = b"served-via-authenticated-manual-proxy";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
                _ => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
            let _ = stream.flush();
        }
    });

    // Bypassed server (`127.0.0.2`): REJECTS proxied connections carrying `X-Via-Manual-Proxy: 1`!
    let stop_bypass = Arc::clone(&stop);
    let bypass_log = Arc::clone(&bypass_direct_hits);
    let bypass_srv = thread::spawn(move || {
        bypass_listener.set_nonblocking(true).unwrap();
        while !stop_bypass.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = bypass_listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line);
            let mut via_proxy = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("x-via-manual-proxy:") {
                    via_proxy = true;
                }
            }
            if via_proxy {
                let _ = write!(
                    stream,
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 10\r\nConnection: close\r\n\r\nproxy-leak"
                );
                let _ = stream.flush();
                continue;
            }
            let path = req_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            bypass_log.lock().unwrap().push(path.clone());
            match path.as_str() {
                "/redir-to-public" => {
                    let loc = format!("http://{public_addr}/public-target.bin");
                    let _ = write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
                "/fail-bypass" => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
                "/from-public-redir.bin" | "/bypass-mirror.bin" => {
                    let body = b"served-directly-via-bypass";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
                _ => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
            let _ = stream.flush();
        }
    });

    // Use a real disk-backed or in-memory PersistentCore with DPAPI-sealed proxy password!
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            ),
            ("proxy_mode".into(), serde_json::json!("manual")),
            (
                "proxy_url".into(),
                serde_json::json!(format!("http://puser:psecret@{proxy_addr}")),
            ),
            ("proxy_bypass".into(), serde_json::json!("127.0.0.2")),
            ("auto_retry_failed_max".into(), serde_json::json!(0)),
        ]))
        .unwrap();

    let wait_done = |coord: &CoreCoordinator, id: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snap = coord.lock().unwrap().snapshot(id).cloned().unwrap();
            if snap.status == "completed" {
                return;
            }
            assert!(
                snap.status != "failed",
                "task {id} unexpectedly failed: {snap:?}"
            );
            assert!(Instant::now() < deadline, "task {id} timed out: {snap:?}");
            thread::sleep(Duration::from_millis(20));
        }
    };

    // Case 1: Public (`127.0.0.1`, manual proxy) -> 302 redirect -> Bypassed (`127.0.0.2`, direct)
    let ev1 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{public_addr}/redir-to-bypass"),
                filename: "pub-to-bypass-redir.bin".into(),
                concurrency: 1,
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id1 = created_task_id(&ev1);
    wait_done(&coordinator, &id1);
    assert_eq!(
        fs::read(root.join("pub-to-bypass-redir.bin")).unwrap(),
        b"served-directly-via-bypass"
    );

    // Case 2: Public (`127.0.0.1`, manual proxy) -> mirror failover -> Bypassed (`127.0.0.2`, direct)
    let ev2 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{public_addr}/fail-primary"),
                mirrors: vec![format!("http://{bypass_addr}/bypass-mirror.bin")],
                filename: "pub-to-bypass-mirror.bin".into(),
                concurrency: 1,
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id2 = created_task_id(&ev2);
    wait_done(&coordinator, &id2);
    assert_eq!(
        fs::read(root.join("pub-to-bypass-mirror.bin")).unwrap(),
        b"served-directly-via-bypass"
    );

    // Case 3: Bypassed (`127.0.0.2`, pre-collapsed `spec.proxy == DIRECT_PROXY_SENTINEL`!)
    // -> 302 redirect -> Public (`127.0.0.1`, MUST switch to authenticated manual proxy!)
    let ev3 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{bypass_addr}/redir-to-public"),
                filename: "bypass-to-pub-redir.bin".into(),
                concurrency: 1,
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id3 = created_task_id(&ev3);
    assert_eq!(
        coordinator.lock().unwrap().task_spec(&id3).unwrap().proxy,
        crate::net_policy::DIRECT_PROXY_SENTINEL,
        "admission still records DIRECT_PROXY_SENTINEL for bypassed entry URL"
    );
    wait_done(&coordinator, &id3);
    assert_eq!(
        fs::read(root.join("bypass-to-pub-redir.bin")).unwrap(),
        b"served-via-authenticated-manual-proxy"
    );

    // Case 4: Bypassed (`127.0.0.2`, direct) -> mirror failover -> Public (`127.0.0.1`, manual proxy!)
    let ev4 = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{bypass_addr}/fail-bypass"),
                mirrors: vec![format!("http://{public_addr}/public-mirror.bin")],
                filename: "bypass-to-pub-mirror.bin".into(),
                concurrency: 1,
                allow_duplicate: true,
                ..Default::default()
            },
        })
        .unwrap();
    let id4 = created_task_id(&ev4);
    wait_done(&coordinator, &id4);
    assert_eq!(
        fs::read(root.join("bypass-to-pub-mirror.bin")).unwrap(),
        b"served-via-authenticated-manual-proxy"
    );

    // Verify zero `127.0.0.2` URLs ever touched the manual proxy, and all `127.0.0.1` URLs DID!
    let proxied = proxied_targets.lock().unwrap().clone();
    assert!(
        proxied.iter().all(|u| !u.contains("127.0.0.2")),
        "bypassed 127.0.0.2 must NEVER go through manual proxy: {proxied:?}"
    );
    assert!(
        proxied.iter().any(|u| u.contains("/public-target.bin")),
        "redirect from bypassed 127.0.0.2 to public 127.0.0.1 must use manual proxy: {proxied:?}"
    );
    assert!(
        proxied.iter().any(|u| u.contains("/public-mirror.bin")),
        "mirror failover from bypassed 127.0.0.2 to public 127.0.0.1 must use manual proxy: {proxied:?}"
    );
    let direct_hits = bypass_direct_hits.lock().unwrap().clone();
    assert!(direct_hits.contains(&"/redir-to-public".to_string()));
    assert!(direct_hits.contains(&"/from-public-redir.bin".to_string()));
    assert!(direct_hits.contains(&"/bypass-mirror.bin".to_string()));

    stop.store(true, Ordering::SeqCst);
    let _ = proxy_srv.join();
    let _ = public_srv.join();
    let _ = bypass_srv.join();
    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn t2_r8_magnet_metadata_proxy_preserves_global_manual_and_system_and_fails_closed_on_empty_manual()
{
    let root = lifecycle_test_root("t2-r8-magnet-proxy");
    fs::create_dir_all(&root).unwrap();
    let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=ubuntu";

    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_settings(BTreeMap::from([
            ("legal_terms_accepted".into(), serde_json::json!(true)),
            (
                "download_dir".into(),
                serde_json::json!(root.to_string_lossy().into_owned()),
            ),
            ("bt_enable_dht".into(), serde_json::json!(false)),
            ("proxy_bypass".into(), serde_json::json!("127.0.0.1")),
        ]))
        .unwrap();
    coordinator
        .lock()
        .unwrap()
        .store_mut()
        .set_setting("proxy_mode", "manual")
        .unwrap();
    coordinator.invalidate_settings_cache();

    // 1. Empty `manual` proxy MUST fail closed (`INVALID_MANUAL_PROXY`) on `magnet:` admission & file selection!
    let empty_settings = coordinator.settings().unwrap();
    assert_eq!(
        resolve_torrent_metadata_proxy(&empty_settings, magnet, ""),
        crate::net_policy::INVALID_MANUAL_PROXY
    );
    assert!(
        coordinator
            .dispatch(CoreCommand::CreateTask {
                spec: TaskSpec {
                    url: magnet.into(),
                    resource_kind: crate::ResourceKind::Torrent,
                    start_paused: true,
                    ..Default::default()
                },
            })
            .is_err(),
        "magnet CreateTask with empty manual proxy must fail closed instead of falling back to direct"
    );

    // 2. Global `manual` proxy is preserved when `explicit_proxy` is empty:
    //    `magnet:...&xs=http://torrent.invalid/ubuntu.torrent` MUST fetch `&xs=` via the manual proxy!
    let payload = b"magnet-xs-via-manual-proxy";
    let info_body = format!(
        "d6:lengthi{}e4:name13:via-proxy.bin12:piece lengthi64e6:pieces20:",
        payload.len()
    );
    let mut info_bytes = info_body.into_bytes();
    info_bytes.extend_from_slice(&crate::crypto_lite::sha1(payload));
    info_bytes.push(b'e');
    let info_hash_hex = crate::crypto_lite::sha1(&info_bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut torrent_bytes = b"d4:info".to_vec();
    torrent_bytes.extend_from_slice(&info_bytes);
    torrent_bytes.push(b'e');

    let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let proxy_hits = Arc::new(AtomicU64::new(0));
    let proxy_hits_srv = Arc::clone(&proxy_hits);
    let proxy_srv = thread::spawn(move || {
        if let Ok((mut stream, _)) = proxy_listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            proxy_hits_srv.fetch_add(1, Ordering::SeqCst);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                torrent_bytes.len()
            );
            let _ = stream.write_all(&torrent_bytes);
            let _ = stream.flush();
        }
    });

    coordinator
        .set_setting(
            "proxy_url",
            serde_json::json!(format!("http://{proxy_addr}")),
        )
        .unwrap();
    let http_manual_settings = coordinator.settings().unwrap();
    assert_eq!(
        resolve_torrent_metadata_proxy(&http_manual_settings, magnet, ""),
        format!("http://{proxy_addr}")
    );
    let magnet_with_xs =
        format!("magnet:?xt=urn:btih:{info_hash_hex}&xs=http://torrent.invalid/ubuntu.torrent");
    let ev = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: magnet_with_xs,
                resource_kind: crate::ResourceKind::Torrent,
                start_paused: true,
                ..Default::default()
            },
        })
        .unwrap();
    let tid = created_task_id(&ev);
    proxy_srv.join().unwrap();
    assert_eq!(
        proxy_hits.load(Ordering::SeqCst),
        1,
        "magnet &xs= metadata probe must route through global manual proxy"
    );
    assert_eq!(
        coordinator.lock().unwrap().snapshot(&tid).unwrap().filename,
        "via-proxy.bin"
    );

    coordinator
        .set_setting("proxy_url", serde_json::json!("socks5://127.0.0.1:9050"))
        .unwrap();
    let socks_settings = coordinator.settings().unwrap();
    assert_eq!(
        resolve_torrent_metadata_proxy(&socks_settings, magnet, ""),
        "socks5://127.0.0.1:9050"
    );

    coordinator
        .set_setting("proxy_mode", serde_json::json!("system"))
        .unwrap();
    let system_settings = coordinator.settings().unwrap();
    assert_eq!(
        resolve_torrent_metadata_proxy(&system_settings, magnet, ""),
        crate::net_policy::SYSTEM_PROXY_SENTINEL
    );

    drop(coordinator);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn auto_retry_governance_skips_permanent_failures_while_retrying_transient_errors() {
    // 1. Direct classifier & retry-gate coverage across permanent and transient errors.
    for (error, expected_code) in [
        ("write failed: no space left on device", "DISK_FULL"),
        ("磁盘空间不足", "DISK_FULL"),
        ("ffmpeg not found", "MEDIA_MERGE_FAILED"),
        (
            "外部工具路径必须指向 ffmpeg 可执行文件",
            "MEDIA_MERGE_FAILED",
        ),
        (
            "检测到受 DRM 加密保护的流媒体（Widevine/PlayReady），不支持直接解密下载",
            "DRM_PROTECTED",
        ),
        ("不支持的 HLS 加密方法: SAMPLE-AES", "DRM_PROTECTED"),
        (
            "不支持 KEYFORMAT=com.apple.streamingkeydelivery / DRM 加密",
            "DRM_PROTECTED",
        ),
        ("下载地址不在允许的域名范围内", "NETWORK_POLICY"),
        (
            "public URL cannot target a private network",
            "NETWORK_POLICY",
        ),
        ("手动代理模式未配置代理服务器地址", "NETWORK_POLICY"),
        ("proxy not configured", "NETWORK_POLICY"),
        ("HTTP 403", "HTTP_403"),
        ("HTTP 404", "HTTP_404"),
    ] {
        let failure = task_failure_from_error(error, "transfer", "https://cdn.test/x", 1, false);
        assert_eq!(failure.code, expected_code, "error={error}");
        assert!(
            !is_auto_retryable_task_failure(&failure.code, error),
            "permanent error must not auto-retry: {error} ({})",
            failure.code
        );
    }

    for (error, expected_code) in [
        ("connection reset by peer", "DOWNLOAD_FAILED"),
        (
            "response body length mismatch: expected 100, got 5",
            "SIZE_MISMATCH",
        ),
        ("socket read timed out", "NETWORK_TIMEOUT"),
        ("代理连接或握手超时", "NETWORK_TIMEOUT"),
        ("HTTP 503", "HTTP_503"),
        ("HTTP 429", "HTTP_429"),
    ] {
        let failure = task_failure_from_error(error, "transfer", "https://cdn.test/x", 1, false);
        assert_eq!(failure.code, expected_code, "error={error}");
        assert!(
            is_auto_retryable_task_failure(&failure.code, error),
            "transient error must remain auto-retryable: {error} ({})",
            failure.code
        );
    }

    // 2. End-to-end worker test: DRM stream, `allowed_hosts`, and invalid manual proxy policy
    // fail immediately on attempt 1 even when `auto_retry_failed_max = 3`, while a transient 503
    // auto-retries to completion.
    use std::sync::atomic::AtomicUsize;
    let root = lifecycle_test_root("smart-retry-governance");
    let drm_hits = Arc::new(AtomicUsize::new(0));
    let drm_srv_hits = Arc::clone(&drm_hits);
    let drm_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let drm_addr = drm_listener.local_addr().unwrap();
    let drm_server = thread::spawn(move || {
        if let Ok((mut stream, _)) = drm_listener.accept() {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            drm_srv_hits.fetch_add(1, Ordering::SeqCst);
            let body = b"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://drm\"\n#EXTINF:4.0,\nseg0.ts\n#EXT-X-ENDLIST\n";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        }
    });

    let flaky_hits = Arc::new(AtomicUsize::new(0));
    let flaky_srv_hits = Arc::clone(&flaky_hits);
    let flaky_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let flaky_addr = flaky_listener.local_addr().unwrap();
    let flaky_server = thread::spawn(move || {
        while let Ok((mut stream, _)) = flaky_listener.accept() {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let n = flaky_srv_hits.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= 3 {
                // Exhaust HTTP engine's single-run transport retries so task-level auto-retry triggers.
                let _ = stream.write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nrecovered-ok",
                );
                break;
            }
        }
    });

    let coord = direct_http_coordinator(&root);
    coord
        .set_setting("auto_retry_failed_max", serde_json::json!(3))
        .unwrap();
    coord
        .set_setting("queue_max_active", serde_json::json!(1))
        .unwrap();

    // Task 1: DRM-protected HLS must fail once (`error_attempt == 1`, `DRM_PROTECTED`) and unblock queue.
    let ev_drm = coord
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{drm_addr}/drm.m3u8"),
                resource_kind: crate::ResourceKind::Hls,
                filename: "drm.mp4".into(),
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let drm_id = created_task_id(&ev_drm);

    // Task 2: Transient 503 -> auto-retries via `auto_retry_failed_max` and completes.
    let ev_flaky = coord
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{flaky_addr}/flaky.bin"),
                resource_kind: crate::ResourceKind::File,
                filename: "flaky.bin".into(),
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let flaky_id = created_task_id(&ev_flaky);

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let tasks = coord.tasks().unwrap();
        let flaky_done = tasks
            .iter()
            .any(|t| t.task_id == flaky_id && t.status == "completed");
        if flaky_done {
            let drm_snap = tasks.iter().find(|t| t.task_id == drm_id).unwrap();
            assert_eq!(drm_snap.status, "failed");
            assert_eq!(drm_snap.error_code.as_deref(), Some("DRM_PROTECTED"));
            assert_eq!(drm_snap.error_attempt, 1);
            assert_eq!(drm_hits.load(Ordering::SeqCst), 1);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for auto-retry governance tasks: {tasks:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(fs::read(root.join("flaky.bin")).unwrap(), b"recovered-ok");

    // Task 3: `allowed_hosts` policy rejection after task creation fails immediately on attempt 1.
    let ev_allow = coord
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: format!("http://{flaky_addr}/disallowed.bin"),
                resource_kind: crate::ResourceKind::File,
                filename: "disallowed.bin".into(),
                start_paused: true,
                concurrency: 1,
                ..Default::default()
            },
        })
        .unwrap();
    let allow_id = created_task_id(&ev_allow);
    coord
        .set_setting("allowed_hosts", serde_json::json!("allowed.example.test"))
        .unwrap();
    coord
        .dispatch(CoreCommand::TaskAction {
            task_id: allow_id.clone(),
            action: "resume".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snap = coord.lock().unwrap().snapshot(&allow_id).cloned().unwrap();
        if snap.status == "failed" {
            assert_eq!(snap.error_code.as_deref(), Some("NETWORK_POLICY"));
            assert_eq!(snap.error_attempt, 1);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "allowed_hosts failure did not settle to failed: {snap:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let _ = drm_server.join();
    let _ = flaky_server.join();
    drop(coord);
    let _ = fs::remove_dir_all(root);
}
