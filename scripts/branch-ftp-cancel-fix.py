from pathlib import Path

path = Path("native_shell/src/ftp_engine.rs")
text = path.read_text(encoding="utf-8")

old_data = '''    let data_raw = TcpStream::connect(data_addr).map_err(|error| error.to_string())?;
    let mut data = if target.tls {
        wrap_tls(data_raw, &target.host)?
    } else {
        Conn::Plain(data_raw)
    };
'''
new_data = '''    let data_raw = TcpStream::connect(data_addr).map_err(|error| error.to_string())?;
    // Keep a raw duplicate of the socket outside Schannel so pause/cancel can
    // interrupt a blocked FTP or FTPS data read without relying on short TLS
    // read timeouts or transport-specific timeout semantics.
    let data_abort = data_raw.try_clone().map_err(|error| error.to_string())?;
    let mut data = if target.tls {
        wrap_tls(data_raw, &target.host)?
    } else {
        Conn::Plain(data_raw)
    };
'''
if old_data not in text:
    raise SystemExit("data connection anchor not found")
text = text.replace(old_data, new_data, 1)

old_loop = '''    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = resume_from;
    crate::http_engine::write_progress(&progress, downloaded, size, 0.0, "downloading");
    loop {
        let flag = std::fs::read_to_string(control).unwrap_or_else(|_| "run".into());
        if flag.trim() == "pause" {
            return Err("paused".into());
        }
        if flag.trim() == "cancel" {
            return Err("canceled".into());
        }
        let count = data.read(&mut buf).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        file.write_all(&buf[..count])
            .map_err(|error| error.to_string())?;
        downloaded += count as u64;
        crate::http_engine::write_progress(&progress, downloaded, size, 0.0, "downloading");
        crate::net_policy::consume(count);
    }
    drop(data);
    file.flush().map_err(|error| error.to_string())?;
'''
new_loop = '''    let stop_reader = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_reader_worker = std::sync::Arc::clone(&stop_reader);
    let progress_for_reader = progress.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let data_reader = std::thread::spawn(move || {
        let result = (|| -> Result<u64, String> {
            let mut buf = [0u8; 64 * 1024];
            let mut downloaded = resume_from;
            crate::http_engine::write_progress(
                &progress_for_reader,
                downloaded,
                size,
                0.0,
                "downloading",
            );
            loop {
                let count = data.read(&mut buf).map_err(|error| error.to_string())?;
                if stop_reader_worker.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("stopped".into());
                }
                if count == 0 {
                    break;
                }
                file.write_all(&buf[..count])
                    .map_err(|error| error.to_string())?;
                downloaded += count as u64;
                crate::http_engine::write_progress(
                    &progress_for_reader,
                    downloaded,
                    size,
                    0.0,
                    "downloading",
                );
                crate::net_policy::consume(count);
            }
            file.flush().map_err(|error| error.to_string())?;
            Ok(downloaded)
        })();
        let _ = result_tx.send(result);
    });

    let downloaded = loop {
        let flag = std::fs::read_to_string(control).unwrap_or_else(|_| "run".into());
        let action = flag.trim();
        if matches!(action, "pause" | "cancel") {
            stop_reader.store(true, std::sync::atomic::Ordering::Release);
            let _ = data_abort.shutdown(std::net::Shutdown::Both);
            // A normal TCP shutdown wakes both plain recv and Schannel's
            // underlying recv quickly. Bound the cleanup wait so task control
            // never regresses to the old unbounded data-channel stall.
            match result_rx.recv_timeout(Duration::from_millis(750)) {
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = data_reader.join();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Dropping JoinHandle detaches only as a last resort. The
                    // stop flag prevents any later read from being written.
                }
            }
            return Err(if action == "pause" { "paused" } else { "canceled" }.into());
        }
        match result_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => {
                let _ = data_reader.join();
                break result?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = data_reader.join();
                return Err("FTP data reader stopped unexpectedly".into());
            }
        }
    };
'''
if old_loop not in text:
    raise SystemExit("transfer loop anchor not found")
text = text.replace(old_loop, new_loop, 1)

if "stalled_data_channel_observes_pause_and_cancel_promptly" not in text:
    test_block = r'''

    fn spawn_stalled_ftp_server() -> (
        u16,
        std::sync::mpsc::Receiver<()>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (stalled_tx, stalled_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut ctrl, _) = listener.accept().unwrap();
            ctrl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            ctrl.write_all(b"220 Ready\r\n").unwrap();
            let mut reader = std::io::BufReader::new(ctrl.try_clone().unwrap());
            let mut data_listener = None;
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end().to_ascii_uppercase();
                if command.starts_with("USER ") {
                    ctrl.write_all(b"230 Logged in\r\n").unwrap();
                } else if command == "TYPE I" {
                    ctrl.write_all(b"200 Type set\r\n").unwrap();
                } else if command.starts_with("SIZE ") {
                    ctrl.write_all(b"213 4\r\n").unwrap();
                } else if command == "PASV" {
                    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                    let data_port = listener.local_addr().unwrap().port();
                    data_listener = Some(listener);
                    ctrl.write_all(
                        format!(
                            "227 Entering Passive Mode (127,0,0,1,{},{})\r\n",
                            data_port / 256,
                            data_port % 256
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                } else if command.starts_with("RETR ") {
                    ctrl.write_all(b"150 Opening data\r\n").unwrap();
                    let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                    data.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    stalled_tx.send(()).unwrap();
                    let mut byte = [0u8; 1];
                    let _ = std::io::Read::read(&mut data, &mut byte);
                    break;
                } else {
                    panic!("unexpected FTP command: {command}");
                }
            }
        });
        (port, stalled_rx, server)
    }

    fn assert_stalled_transfer_interrupts(action: &str) {
        let (port, stalled_rx, server) = spawn_stalled_ftp_server();
        let root = std::env::temp_dir().join(format!(
            "hls-v7-ftp-stall-{}-{}-{action}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("payload.downloading");
        let control = root.join("control");
        std::fs::write(&control, "run").unwrap();
        let worker_output = output.clone();
        let worker_control = control.clone();
        let url = format!("ftp://127.0.0.1:{port}/file.bin");
        let worker = std::thread::spawn(move || {
            download_ftp(&url, &worker_output, &worker_control, false)
        });
        stalled_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("FTP data connection did not enter stalled read");
        let started = std::time::Instant::now();
        std::fs::write(&control, action).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error, action);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{action} took too long to interrupt a stalled FTP data read: {:?}",
            started.elapsed()
        );
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stalled_data_channel_observes_pause_and_cancel_promptly() {
        assert_stalled_transfer_interrupts("pause");
        assert_stalled_transfer_interrupts("cancel");
    }
'''
    end = text.rfind("\n}")
    if end == -1:
        raise SystemExit("tests module closing brace not found")
    text = text[:end] + test_block + text[end:]

path.write_text(text, encoding="utf-8")
