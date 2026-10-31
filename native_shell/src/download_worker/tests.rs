use super::*;
use crate::{ResourceKind, TaskSpec};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

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

    // 合法形状照旧放行，而且四个目录一个都不能丢。
    for stored in [
        serde_json::json!({"media":"E:/Videos","program":"E:/Apps","archive":"E:/Archives","other":"E:/Other"}),
        serde_json::json!(null),
        serde_json::json!(""),
    ] {
        assert!(
            CoreCoordinator::validate_setting("browser_category_dirs", &stored).is_ok(),
            "合法形状被误拒: {stored}"
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
    let _ = std::fs::remove_dir_all(&root);
}

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
fn player_session_reports_play_pause_speed_and_stop() {
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
    let paths = TaskPaths::for_task("task-1", &spec).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&paths.final_output, b"local-media").unwrap();
    coordinator
        .lock()
        .unwrap()
        .handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 11,
            total_bytes: Some(11),
            speed_bytes_per_sec: 0,
            stage: "finished".into(),
            status: "completed".into(),
        })
        .unwrap();

    let play = coordinator
        .dispatch(CoreCommand::PlayTask {
            task_id: "task-1".into(),
        })
        .unwrap();
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
    let stop = coordinator
        .dispatch(CoreCommand::PlayerControl {
            action: "stop".into(),
        })
        .unwrap();
    assert!(matches!(
        stop.last().map(|item| &item.event),
        Some(CoreEvent::PlayerSession { active: false, status, .. }) if status == "STOPPED"
    ));
    let _ = std::fs::remove_dir_all(dir);
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
fn site_rules_override_task_speed_and_proxy() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    coordinator
        .set_setting(
            "site_rules",
            serde_json::json!("example.test=speed:64,conn:2,proxy:http://127.0.0.1:9"),
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
    assert!(!protected.contains("sid=private"));

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
            trusted_ui: false,
        })
        .is_err());
    let events = coordinator
        .dispatch(CoreCommand::AcceptHandoff {
            handoff_id: "handoff-coord".into(),
            filename: "a.bin".into(),
            download_dir: String::new(),
            trusted_ui: false,
        })
        .unwrap();
    assert!(events
        .iter()
        .any(|envelope| matches!(envelope.event, crate::CoreEvent::HandoffResolved { .. })));
    assert_eq!(coordinator.tasks().unwrap().len(), 1);
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
            trusted_ui: false,
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
    assert!(json.contains("\"status\":\"pending\""));
    assert!(coordinator
        .lock()
        .unwrap()
        .pending_handoff("handoff-ui")
        .is_some());
}

#[test]
fn presenter_lease_blocks_compose_and_owner_can_release() {
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
    assert_eq!(resolve_published(&paths), renamed);
    coordinator.delete_task_files(&task_id).unwrap();
    assert!(!renamed.exists());
    assert_eq!(
        fs::read(&paths.final_output).unwrap(),
        b"existing user file"
    );
    paths.prepare().unwrap();
    remember_published(&paths, &renamed);
    assert_eq!(resolve_published(&paths), renamed);
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
    let resolved = resolve_published(&paths);
    assert_ne!(resolved, outside);
    let outside_canon = logical_canonical(&outside);
    assert_ne!(logical_canonical(&resolved), outside_canon);
    let _ = fs::remove_file(&outside);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn legal_gate_blocks_create_and_start_until_accepted() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
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
    let escaped = coordinator
        .dispatch(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://cdn.test/a.bin".into(),
                filename: "a.bin".into(),
                download_dir: r"C:\Windows".into(),
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
fn ui_layout_settings_validate_required_entries_widths_and_persistence() {
    let coordinator = CoreCoordinator::new(PersistentCore::in_memory().unwrap());
    let columns = "name:320:1,progress:180:1,status:120:1,speed:120:0,size:120:1,actions:96:0";
    coordinator
        .set_setting("task_column_layout", serde_json::json!(columns))
        .unwrap();
    assert_eq!(
        coordinator
            .lock()
            .unwrap()
            .store()
            .setting_string("task_column_layout", "")
            .unwrap(),
        columns
    );

    assert!(coordinator
        .set_setting("task_column_layout", serde_json::json!("name:47:1"))
        .is_err());
    assert!(coordinator
        .set_setting(
            "task_column_layout",
            serde_json::json!("name:120:0,progress:120:1")
        )
        .is_err());
    assert!(coordinator
        .set_setting("toolbar_actions", serde_json::json!("new:0,paste:1"))
        .is_err());
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
    assert!(share_media(&coordinator, "", "", "", "").is_err());
    assert!(share_media(&coordinator, "relative-video.mp4", "", "video", "").is_err());
    assert!(share_media(&coordinator, "", "javascript:alert(1)", "video", "").is_err());
    assert!(share_media(
        &coordinator,
        r"C:\missing.mp4",
        "https://cdn.test/video.mp4",
        "video",
        ""
    )
    .is_err());
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
    remember_cast_mount(&token);

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
        assert_eq!(resolve_published(&paths), published);
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
