//! The UI pipe carries temporary display intent only; Core owns all task state.
#[cfg(windows)]
use hls_client::{CoreCommand, CorePipeRequest, CorePipeResponse};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
#[cfg(windows)]
use std::time::{Duration, Instant};

pub struct ActivationServer {
    pub requests: mpsc::Receiver<Option<String>>,
    stop: Arc<AtomicBool>,
}

#[cfg(windows)]
fn pipe_name() -> String {
    format!("{}.ui", hls_client::v7_pipe_name())
}

impl ActivationServer {
    pub fn start() -> Result<Self, String> {
        let (tx, requests) = mpsc::sync_channel(32);
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        {
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let server = hls_client::NamedPipeServer::new(pipe_name());
            let stopping = Arc::clone(&stop);
            std::thread::spawn(move || {
                let handler = Arc::new(move |request| match request {
                    CorePipeRequest::Command {
                        request_id,
                        command: CoreCommand::OpenMain { launch_id },
                    } => {
                        if launch_id.as_deref().is_some_and(|id| {
                            id.is_empty() || id.len() > 80 || id.chars().any(char::is_control)
                        }) {
                            return CorePipeResponse::Error {
                                request_id: Some(request_id),
                                code: "invalid_launch_id".into(),
                                message: "启动标识无效".into(),
                            };
                        }
                        // Preserve each browser launch receipt. Legacy untracked opens may coalesce.
                        match tx.try_send(launch_id) {
                            Ok(()) | Err(mpsc::TrySendError::Full(None)) => {
                                CorePipeResponse::Events {
                                    request_id,
                                    events: Vec::new(),
                                }
                            }
                            Err(mpsc::TrySendError::Full(Some(_))) => CorePipeResponse::Error {
                                request_id: Some(request_id),
                                code: "ui_busy".into(),
                                message: "界面正在处理打开请求，请重试".into(),
                            },
                            Err(mpsc::TrySendError::Disconnected(_)) => CorePipeResponse::Error {
                                request_id: Some(request_id),
                                code: "ui_stopping".into(),
                                message: "界面正在退出，请重试".into(),
                            },
                        }
                    }
                    _ => CorePipeResponse::Error {
                        request_id: None,
                        code: "ui_intent_only".into(),
                        message: "UI 激活管道只接受打开工作台".into(),
                    },
                });
                let _ = server.serve_loop_with_ready(stopping, handler, ready_tx);
            });
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => {}
                result => {
                    stop.store(true, Ordering::SeqCst);
                    return Err(format!("UI 激活管道启动失败：{result:?}"));
                }
            }
        }
        #[cfg(not(windows))]
        let _ = tx;
        Ok(Self { requests, stop })
    }
}

impl Drop for ActivationServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn forward_workbench(launch_id: Option<String>) -> Result<(), String> {
    #[cfg(windows)]
    {
        forward_workbench_on_pipe(&pipe_name(), launch_id, Duration::from_secs(10))
    }
    #[cfg(not(windows))]
    {
        let _ = launch_id;
        Err("UI 激活需要 Windows".into())
    }
}

#[cfg(windows)]
fn forward_workbench_on_pipe(
    pipe: &str,
    launch_id: Option<String>,
    timeout: Duration,
) -> Result<(), String> {
    // The instance lock is claimed before Core/font initialization. Wait for
    // its pipe rather than silently dropping a concurrent user launch, while
    // bounding connect, write, read, and retry backoff under one total deadline.
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let connect_deadline = Instant::now() + remaining.min(Duration::from_millis(200));
        match hls_client::NamedPipeClient::connect_with_deadline(pipe, connect_deadline).and_then(
            |mut client| {
                client.request_with_deadline(
                    &CorePipeRequest::Command {
                        request_id: 1,
                        command: CoreCommand::OpenMain {
                            launch_id: launch_id.clone(),
                        },
                    },
                    deadline,
                )
            },
        ) {
            Ok(CorePipeResponse::Events { request_id: 1, .. }) => return Ok(()),
            Ok(CorePipeResponse::Error { code, message, .. })
                if code != "ui_busy" && code != "ui_stopping" =>
            {
                return Err(message);
            }
            Ok(CorePipeResponse::Error { message, .. }) => last_error = message,
            Ok(_) => last_error = "UI 激活响应不兼容".into(),
            Err(error) => last_error = error,
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(Duration::from_millis(40).min(remaining));
    }
    Err(format!("无法唤醒已有界面：{last_error}"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn forward_workbench_bounds_total_deadline_when_peer_accepts_and_never_replies() {
        let pipe = format!(
            r"\\.\pipe\hls-downloader-ui-activation-deadline-test-{}",
            std::process::id()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let server = hls_client::NamedPipeServer::new(pipe.clone());
        let stop_handle = server.stop_handle();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (received_tx, received_rx) = mpsc::sync_channel::<()>(1);
        let handler: Arc<dyn Fn(CorePipeRequest) -> CorePipeResponse + Send + Sync> =
            Arc::new(move |request| {
                let _ = received_tx.try_send(());
                let _ = release_rx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recv_timeout(Duration::from_secs(5));
                match request {
                    CorePipeRequest::Command { request_id, .. } => CorePipeResponse::Events {
                        request_id,
                        events: Vec::new(),
                    },
                    _ => CorePipeResponse::Error {
                        request_id: None,
                        code: "unexpected".into(),
                        message: "unexpected".into(),
                    },
                }
            });
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let server_stop = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            server.serve_loop_with_ready(server_stop, handler, ready_tx)
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("activation test pipe ready timeout")
            .expect("activation test pipe start failed");

        let started = Instant::now();
        let err = forward_workbench_on_pipe(
            &pipe,
            Some("launch-hang".into()),
            Duration::from_millis(250),
        )
        .unwrap_err();
        let elapsed = started.elapsed();

        assert!(err.contains("无法唤醒已有界面"), "err={err}");
        assert!(err.contains("timed out"), "err={err}");
        assert!(
            elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(1),
            "forward_workbench exceeded total deadline budget: {elapsed:?}"
        );
        received_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("hanging peer did not receive OpenMain request");

        let _ = release_tx.send(());
        stop.store(true, Ordering::SeqCst);
        stop_handle.signal();
        worker.join().unwrap().unwrap();
    }
}
