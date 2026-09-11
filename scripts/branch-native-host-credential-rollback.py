from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"{label} anchor not found")
    return text.replace(old, new, 1)

# Core IPC protocol + client.
path = Path("native_shell/src/core_ipc.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    LoadCredential {
        request_id: u64,
        credential_ref: String,
    },
    SaveHandoff {''',
    '''    LoadCredential {
        request_id: u64,
        credential_ref: String,
    },
    DeleteCredential {
        request_id: u64,
        credential_ref: String,
    },
    SaveHandoff {''',
    "CorePipeRequest delete credential",
)
text = replace_once(
    text,
    '''    pub fn load_credential(&mut self, credential_ref: &str) -> Result<Option<String>, String> {
        match self.request(&CorePipeRequest::LoadCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
        })? {
            CorePipeResponse::Credential { protected_blob, .. } => Ok(protected_blob),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected credential response: {other:?}")),
        }
    }

    pub fn save_handoff(''',
    '''    pub fn load_credential(&mut self, credential_ref: &str) -> Result<Option<String>, String> {
        match self.request(&CorePipeRequest::LoadCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
        })? {
            CorePipeResponse::Credential { protected_blob, .. } => Ok(protected_blob),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected credential response: {other:?}")),
        }
    }

    pub fn delete_credential(&mut self, credential_ref: &str) -> Result<(), String> {
        match self.request(&CorePipeRequest::DeleteCredential {
            request_id: 1,
            credential_ref: credential_ref.into(),
        })? {
            CorePipeResponse::Credential {
                protected_blob: None,
                ..
            } => Ok(()),
            CorePipeResponse::Error { message, .. } => Err(message),
            other => Err(format!("unexpected credential delete response: {other:?}")),
        }
    }

    pub fn save_handoff(''',
    "CoreIpcClient delete credential",
)
text = replace_once(
    text,
    '''    #[test]
    fn hello_uses_the_v7_protocol_identity() {''',
    '''    #[test]
    fn delete_credential_request_roundtrip_preserves_identity() {
        let request = CorePipeRequest::DeleteCredential {
            request_id: 11,
            credential_ref: "cred-owned".into(),
        };
        let decoded: CorePipeRequest = decode_message(&encode_message(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn hello_uses_the_v7_protocol_identity() {''',
    "Core IPC delete serialization test",
)
path.write_text(text, encoding="utf-8")

# Coordinator exposes the existing CoreStore delete primitive to IPC.
path = Path("native_shell/src/download_worker.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    pub fn load_credential(&self, credential_ref: &str) -> Result<Option<String>, String> {
        self.lock()?.store().load_credential(credential_ref)
    }

    pub fn default_cookie_configured''',
    '''    pub fn load_credential(&self, credential_ref: &str) -> Result<Option<String>, String> {
        self.lock()?.store().load_credential(credential_ref)
    }

    pub fn delete_credential(&self, credential_ref: &str) -> Result<(), String> {
        self.lock()?.store_mut().delete_credential(credential_ref)
    }

    pub fn default_cookie_configured''',
    "CoreCoordinator delete credential",
)
path.write_text(text, encoding="utf-8")

# Resident Core server handles credential deletion over the same durable IPC boundary.
path = Path("native_shell/src/core_server.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''        CorePipeRequest::LoadCredential {
            request_id,
            credential_ref,
        } => match coordinator.load_credential(&credential_ref) {
            Ok(protected_blob) => CorePipeResponse::Credential {
                request_id,
                protected_blob,
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::SaveHandoff {''',
    '''        CorePipeRequest::LoadCredential {
            request_id,
            credential_ref,
        } => match coordinator.load_credential(&credential_ref) {
            Ok(protected_blob) => CorePipeResponse::Credential {
                request_id,
                protected_blob,
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::DeleteCredential {
            request_id,
            credential_ref,
        } => match coordinator.delete_credential(&credential_ref) {
            Ok(()) => CorePipeResponse::Credential {
                request_id,
                protected_blob: None,
            },
            Err(error) => CorePipeResponse::Error {
                request_id: Some(request_id),
                code: "credential_failed".into(),
                message: error,
            },
        },
        CorePipeRequest::SaveHandoff {''',
    "Core server delete credential dispatch",
)
text = replace_once(
    text,
    '''    #[test]
    fn ui_and_native_host_share_one_core_over_ipc() {''',
    '''    #[test]
    fn credential_delete_roundtrips_over_ipc() {
        let server = CoreServer::in_memory().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let mut client = CoreIpcClient::connect_addr(addr).unwrap();
        client
            .store_credential("rollback-test", "protected", "browser_replay")
            .unwrap();
        assert_eq!(
            client.load_credential("rollback-test").unwrap().as_deref(),
            Some("protected")
        );
        client.delete_credential("rollback-test").unwrap();
        assert_eq!(client.load_credential("rollback-test").unwrap(), None);
        server.shutdown();
    }

    #[test]
    fn ui_and_native_host_share_one_core_over_ipc() {''',
    "Core server delete credential roundtrip test",
)
path.write_text(text, encoding="utf-8")

# Native Host owns only the browser replay credential it creates for this download.
path = Path("native_shell/src/native_host.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    fn store_credential(
        &mut self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => {
                core.store_mut()
                    .store_credential(credential_ref, protected_blob, kind)
            }
            Self::Remote(client) => client.store_credential(credential_ref, protected_blob, kind),
        }
    }

    fn save_handoff(''',
    '''    fn store_credential(
        &mut self,
        credential_ref: &str,
        protected_blob: &str,
        kind: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => {
                core.store_mut()
                    .store_credential(credential_ref, protected_blob, kind)
            }
            Self::Remote(client) => client.store_credential(credential_ref, protected_blob, kind),
        }
    }

    fn delete_credential(&mut self, credential_ref: &str) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Local(core) => core.store_mut().delete_credential(credential_ref),
            Self::Remote(client) => client.delete_credential(credential_ref),
        }
    }

    fn save_handoff(''',
    "Native Host delete credential backend",
)
old_download = '''    fn download(&mut self, message: &Value) -> Result<Value, String> {
        let payload = resource_payload(message)?;
        let offer = parse_offer(payload)?;
        let credential_ref = self.persist_browser_context(payload)?;
        let filename = filename(payload, &offer.url);
        let events = self.core.handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: offer.url,
                resource_kind: offer.resource_kind,
                title: field(payload, "title"),
                filename,
                download_dir: String::new(),
                request_method: offer.request_method,
                credential_ref: credential_ref.or(offer.credential_ref),
                replay_context_ref: offer.replay_context_ref,
                concurrency: 8,
                checksum: None,
                expected_size: payload
                    .get("size")
                    .and_then(Value::as_u64)
                    .filter(|size| *size > 0),
                etag: field(payload, "etag"),
                last_modified: field(payload, "last_modified"),
                ..Default::default()
            },
        })?;
        let snapshot = events
            .into_iter()
            .find_map(|event| match event.event {
                CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                _ => None,
            })
            .ok_or_else(|| "Rust Core 未返回新建任务快照".to_string())?;
        let _ = self.core.handle(CoreCommand::TaskAction {
            task_id: snapshot.task_id.clone(),
            action: "start".into(),
        });
        Ok(json!({"ok": true, "task": snapshot, "activated": true}))
    }
'''
new_download = '''    fn download(&mut self, message: &Value) -> Result<Value, String> {
        let payload = resource_payload(message)?;
        let offer = parse_offer(payload)?;
        let owned_credential_ref = self.persist_browser_context(payload)?;
        let filename = filename(payload, &offer.url);
        let snapshot = self.create_download_task(
            TaskSpec {
                url: offer.url,
                resource_kind: offer.resource_kind,
                title: field(payload, "title"),
                filename,
                download_dir: String::new(),
                request_method: offer.request_method,
                credential_ref: owned_credential_ref.clone().or(offer.credential_ref),
                replay_context_ref: offer.replay_context_ref,
                concurrency: 8,
                checksum: None,
                expected_size: payload
                    .get("size")
                    .and_then(Value::as_u64)
                    .filter(|size| *size > 0),
                etag: field(payload, "etag"),
                last_modified: field(payload, "last_modified"),
                ..Default::default()
            },
            owned_credential_ref,
        )?;
        let _ = self.core.handle(CoreCommand::TaskAction {
            task_id: snapshot.task_id.clone(),
            action: "start".into(),
        });
        Ok(json!({"ok": true, "task": snapshot, "activated": true}))
    }

    fn create_download_task(
        &mut self,
        spec: TaskSpec,
        owned_credential_ref: Option<String>,
    ) -> Result<TaskSnapshot, String> {
        let result = self
            .core
            .handle(CoreCommand::CreateTask { spec })
            .and_then(|events| {
                events
                    .into_iter()
                    .find_map(|event| match event.event {
                        CoreEvent::TaskCreated { snapshot } => Some(snapshot),
                        _ => None,
                    })
                    .ok_or_else(|| "Rust Core 未返回新建任务快照".to_string())
            });
        match result {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                if let Some(credential_ref) = owned_credential_ref {
                    if let Err(rollback_error) = self.core.delete_credential(&credential_ref) {
                        return Err(format!(
                            "{error}; browser replay credential rollback failed: {rollback_error}"
                        ));
                    }
                }
                Err(error)
            }
        }
    }
'''
text = replace_once(text, old_download, new_download, "Native Host transactional download")
text = replace_once(
    text,
    '''    #[test]
    fn browser_download_creates_a_durable_rust_task_without_http() {''',
    '''    #[test]
    fn download_credential_rollback_is_transactional() {
        let server = crate::CoreServer::in_memory().unwrap();
        server
            .coordinator()
            .set_setting("legal_terms_accepted", json!(false))
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _worker = server.serve(listener);
        let client = CoreIpcClient::connect_addr(addr).unwrap();
        let mut session = NativeHostSession::from_backend(HostCore::Remote(client)).unwrap();

        session
            .core
            .store_credential("owned-failure", "protected", "browser_replay")
            .unwrap();
        let error = session
            .create_download_task(
                TaskSpec {
                    url: "https://cdn.test/fail.bin".into(),
                    filename: "fail.bin".into(),
                    credential_ref: Some("owned-failure".into()),
                    ..Default::default()
                },
                Some("owned-failure".into()),
            )
            .unwrap_err();
        assert!(error.contains("legal terms"), "{error}");
        assert_eq!(
            server
                .coordinator()
                .load_credential("owned-failure")
                .unwrap(),
            None
        );

        server
            .coordinator()
            .set_setting("legal_terms_accepted", json!(true))
            .unwrap();
        session
            .core
            .store_credential("owned-success", "protected", "browser_replay")
            .unwrap();
        let snapshot = session
            .create_download_task(
                TaskSpec {
                    url: "https://cdn.test/success.bin".into(),
                    filename: "success.bin".into(),
                    credential_ref: Some("owned-success".into()),
                    ..Default::default()
                },
                Some("owned-success".into()),
            )
            .unwrap();
        assert_eq!(snapshot.filename, "success.bin");
        assert_eq!(
            server
                .coordinator()
                .load_credential("owned-success")
                .unwrap()
                .as_deref(),
            Some("protected")
        );
        server.shutdown();
    }

    #[test]
    fn browser_download_creates_a_durable_rust_task_without_http() {''',
    "Native Host rollback lifecycle test",
)
path.write_text(text, encoding="utf-8")
