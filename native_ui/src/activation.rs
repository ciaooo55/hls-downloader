//! The UI pipe carries temporary display intent only; Core owns all task state.
use hls_client::{CoreCommand, CorePipeRequest, CorePipeResponse};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub struct ActivationServer {
    pub requests: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
}

fn pipe_name() -> String {
    format!("{}.ui", hls_client::v7_pipe_name())
}

impl ActivationServer {
    pub fn start() -> Result<Self, String> {
        let (tx, requests) = mpsc::sync_channel(1);
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
                        command: CoreCommand::OpenMain,
                    } => {
                        // Coalesce repeated launches while the first window is loading.
                        match tx.try_send(()) {
                            Ok(()) | Err(mpsc::TrySendError::Full(())) => {
                                CorePipeResponse::Events {
                                    request_id,
                                    events: Vec::new(),
                                }
                            }
                            Err(mpsc::TrySendError::Disconnected(())) => CorePipeResponse::Error {
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

pub fn forward_workbench() -> Result<(), String> {
    #[cfg(windows)]
    {
        // The instance lock is claimed before Core/font initialization. Wait for
        // its pipe rather than silently dropping a concurrent user launch.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last_error = String::new();
        while Instant::now() < deadline {
            match hls_client::NamedPipeClient::connect(&pipe_name(), 200).and_then(|mut pipe| {
                pipe.request(&CorePipeRequest::Command {
                    request_id: 1,
                    command: CoreCommand::OpenMain,
                })
            }) {
                Ok(CorePipeResponse::Events { .. }) => return Ok(()),
                Ok(CorePipeResponse::Error { message, .. }) => last_error = message,
                Ok(_) => last_error = "UI 激活响应不兼容".into(),
                Err(error) => last_error = error,
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        Err(format!("无法唤醒已有界面：{last_error}"))
    }
    #[cfg(not(windows))]
    {
        Err("UI 激活需要 Windows".into())
    }
}
