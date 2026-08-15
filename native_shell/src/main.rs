#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use hls_native_shell::{
    decode_frame, encode_frame, paint_snapshot, CoreClient, ResidentShell, PROTOCOL_NAME,
    PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if let Some(job_path) = flag_value(&args, "--job") {
        return run_job_file(&job_path);
    }
    if args.iter().any(|arg| arg == "--self-test") {
        return self_test();
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err == "native shell already running" => {
            notify_existing_instance(&args);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run_job_file(path: &str) -> ExitCode {
    match hls_native_shell::load_job(Path::new(path))
        .and_then(|job| hls_native_shell::finish_job(&job))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn self_test() -> ExitCode {
    let snapshot = paint_snapshot(&json!({
        "id": "self-test",
        "filename": "setup.exe",
        "url": "https://cdn.test/setup.exe",
        "size": 8
    }));
    let frame = match encode_frame(&json!({
        "protocol": PROTOCOL_NAME,
        "version": PROTOCOL_VERSION,
        "kind": "handoff",
        "presentable": true,
        "snapshot": snapshot
    })) {
        Ok(frame) => frame,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    match decode_frame(&frame) {
        Ok(message) if message["snapshot"]["filename"] == "setup.exe" => {
            let mut shell = ResidentShell::boot("headless");
            if let Err(err) = shell.offer(&message["snapshot"]) {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
            if !shell.windows.handoff.created || !shell.windows.handoff.visible || !shell.tray {
                eprintln!("pre-created window state is wrong");
                return ExitCode::from(1);
            }
            println!("{PROTOCOL_NAME}/{PROTOCOL_VERSION} ok");
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("self-test snapshot mismatch");
            ExitCode::from(1)
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let headless = args.iter().any(|arg| arg == "--headless") || cfg!(not(windows));
    let own_tray = !args.iter().any(|arg| arg == "--no-tray");
    let core_url = flag_value(args, "--core-url").unwrap_or_else(default_core_url);
    let token = flag_value(args, "--token").unwrap_or_else(default_token);
    let status_path = flag_value(args, "--status-path").map(PathBuf::from);
    let backend = if headless { "headless" } else { "win32" };
    let shell = Arc::new(Mutex::new(ResidentShell::boot(backend)));
    write_status(status_path.as_deref(), &shell);

    let client = if token.is_empty() {
        None
    } else {
        Some(CoreClient::parse(&core_url, &token)?)
    };
    if let Some(client) = &client {
        ensure_core(client)?;
        import_launch_paths(client, args);
    }

    let stop = Arc::new(AtomicBool::new(false));

    #[cfg(windows)]
    if !headless {
        // HWNDs and tray must exist before Python marks the presenter ready.
        let ui = hls_native_shell::win32::Win32Host::boot(Arc::clone(&shell), own_tray)?;
        if let Some(client) = client.clone() {
            *ui.core.lock().unwrap() = Some(client.clone());
            let ui_shell = Arc::clone(&shell);
            let status = status_path.clone();
            let stop_flag = Arc::clone(&stop);
            thread::spawn(move || {
                poll_core(client, ui_shell, status.as_deref(), &stop_flag);
            });
        }
        hls_native_shell::win32::run_loop();
        stop.store(true, Ordering::SeqCst);
        if let Some(client) = client {
            let _ = client.shutdown();
        }
        return Ok(());
    }
    let _ = own_tray;

    if let Some(client) = client.clone() {
        connect_core(&client, &shell, status_path.as_deref())?;
        let poll_shell = Arc::clone(&shell);
        let status = status_path.clone();
        let stop_flag = Arc::clone(&stop);
        thread::spawn(move || {
            poll_core(client, poll_shell, status.as_deref(), &stop_flag);
        });
    }

    if io::stdin().is_terminal() {
        eprintln!("{PROTOCOL_NAME} {backend} resident. Type {{\"op\":\"shutdown\"}} to exit.");
    }
    let stdin_shell = Arc::clone(&shell);
    let stdin_stop = Arc::clone(&stop);
    let stdin_status = status_path.clone();
    let stdin_client = client.clone();
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let command: Value =
                serde_json::from_str(&line).unwrap_or_else(|_| json!({"op": line.trim()}));
            let op = command.get("op").and_then(Value::as_str).unwrap_or("");
            if let Ok(mut state) = stdin_shell.lock() {
                match op {
                    "open_main" => {
                        let _ = state.open_main();
                    }
                    "hide_main" => state.hide_main(),
                    "accept" => state.accept(),
                    "reject" => state.reject(),
                    "shutdown" => {
                        state.shutdown();
                        stdin_stop.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
            if op == "open_main" {
                if let Some(core) = &stdin_client {
                    if let Ok(tasks) = core.list_tasks() {
                        if let Ok(mut state) = stdin_shell.lock() {
                            state.replace_tasks(tasks);
                        }
                    }
                }
            }
            write_status(stdin_status.as_deref(), &stdin_shell);
            if stdin_stop.load(Ordering::SeqCst) {
                break;
            }
        }
    });
    while !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }
    if let Some(client) = &client {
        let _ = client.shutdown();
    }
    Ok(())
}

fn connect_core(
    client: &CoreClient,
    shell: &Mutex<ResidentShell>,
    status_path: Option<&Path>,
) -> Result<(), String> {
    wait_for_core(client);
    let mut last = "native-shell boot: core not ready".to_string();
    for _ in 0..40 {
        match client.boot() {
            Ok(_) => {
                if let Ok(mut state) = shell.lock() {
                    state.core_running = true;
                }
                write_status(status_path, shell);
                return Ok(());
            }
            Err(err) => last = format!("native-shell boot: {err}"),
        }
        thread::sleep(Duration::from_millis(150));
    }
    Err(last)
}

fn poll_core(
    client: CoreClient,
    shell: Arc<Mutex<ResidentShell>>,
    status_path: Option<&Path>,
    stop: &AtomicBool,
) {
    let mut after = 0u64;
    let mut booted = shell
        .lock()
        .map(|state| state.core_running)
        .unwrap_or(false);
    while !stop.load(Ordering::SeqCst) {
        if !booted {
            match connect_core(&client, &shell, status_path) {
                Ok(()) => booted = true,
                Err(_) => {
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
            }
        }
        match client.wait_events(after, 4.0) {
            Ok(payload) => {
                let sequence = payload
                    .get("sequence")
                    .and_then(Value::as_u64)
                    .unwrap_or(after);
                let events = payload
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for event in events {
                    if let Some(seq) = event.get("sequence").and_then(Value::as_u64) {
                        after = after.max(seq);
                    }
                    if event.get("kind").and_then(Value::as_str) == Some("http_job") {
                        spawn_resident_http_job(&event);
                        continue;
                    }
                    #[cfg(windows)]
                    if let Some(host) = hls_native_shell::win32::host() {
                        host.enqueue(event);
                        continue;
                    }
                    if let Ok(mut state) = shell.lock() {
                        let _ = state.apply_event(&event);
                    }
                }
                let refresh = shell
                    .lock()
                    .map(|state| state.main_open && state.task_list.needs_refresh)
                    .unwrap_or(false);
                if refresh {
                    if let Ok(tasks) = client.list_tasks() {
                        if let Ok(mut state) = shell.lock() {
                            state.replace_tasks(tasks);
                        }
                    }
                }
                after = after.max(sequence);
                write_status(status_path, &shell);
            }
            Err(_) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(120));
            }
        }
    }
}

fn spawn_resident_http_job(event: &Value) {
    let job_path = event
        .get("job_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if job_path.is_empty() {
        return;
    }
    let progress_path = event
        .get("progress_path")
        .and_then(Value::as_str)
        .map(str::to_string);
    thread::spawn(move || {
        hls_native_shell::run_queued_job(
            Path::new(&job_path),
            progress_path.as_deref().map(Path::new),
        );
    });
}

fn write_status(path: Option<&Path>, shell: &Mutex<ResidentShell>) {
    let Some(path) = path else {
        return;
    };
    let Ok(state) = shell.lock() else {
        return;
    };
    let Ok(payload) = serde_json::to_vec_pretty(&*state) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, payload).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

fn ensure_core(client: &CoreClient) -> Result<(), String> {
    if client.health().is_ok() {
        return Ok(());
    }
    if let Some(root) = hls_native_shell::install_root() {
        if hls_native_shell::locate_core_executable(&root).is_some() {
            hls_native_shell::spawn_core(&root)?;
        }
    }
    wait_for_core(client);
    if client.health().is_ok() {
        Ok(())
    } else {
        Err(format!(
            "download core did not open http://127.0.0.1:{}",
            client.port
        ))
    }
}

fn import_launch_paths(client: &CoreClient, args: &[String]) {
    for arg in args.iter().skip(1) {
        if arg.starts_with("--") {
            continue;
        }
        if hls_native_shell::download_import_route(arg).is_some() {
            let _ = client.import_path(arg);
        }
    }
}

fn notify_existing_instance(args: &[String]) {
    // Tauri helper launches use --no-tray. Do not steal focus from an already
    // running supervisor when that helper process exits on the single-instance mutex.
    if args.iter().any(|arg| arg == "--no-tray") {
        return;
    }
    let core_url = flag_value(args, "--core-url").unwrap_or_else(default_core_url);
    let token = flag_value(args, "--token").unwrap_or_else(default_token);
    let Ok(client) = CoreClient::parse(&core_url, &token) else {
        return;
    };
    import_launch_paths(&client, args);
    let _ = client.open_main();
}

fn wait_for_core(client: &CoreClient) {
    for _ in 0..200 {
        if client.health().is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn default_core_url() -> String {
    env::var("HLS_CORE_URL").unwrap_or_else(|_| "http://127.0.0.1:8765/api".into())
}

fn default_token() -> String {
    if let Ok(token) = env::var("HLS_TOKEN") {
        return token;
    }
    read_token_from_config().unwrap_or_default()
}

fn read_token_from_config() -> Option<String> {
    let mut candidates = Vec::new();
    if let Ok(local) = env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("HLS Downloader")
                .join("config.json"),
        );
    }
    candidates.push(PathBuf::from("config.json"));
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("config.json"));
            candidates.push(dir.join("../config.json"));
        }
    }
    for path in candidates {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(token) = value.get("token").and_then(Value::as_str) {
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}
