from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PATH = ROOT / "native_shell/src/download_worker.rs"
text = PATH.read_text(encoding="utf-8")


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    text = text.replace(old, new, 1)


old = '''    fn duplicate_of(&self, spec: &TaskSpec) -> Result<Option<(String, String)>, String> {
        let want = crate::duplicate::canonicalize_url(&spec.url);
        if want.is_empty() {
            return Ok(None);
        }
        let core = self.lock()?;
        for task in core.tasks() {
            if let Some(stored) = core.task_spec(&task.task_id) {
                if crate::duplicate::canonicalize_url(&stored.url) == want {
                    return Ok(Some((task.task_id, task.status)));
                }
            }
        }
        Ok(None)
    }
'''
new = '''    fn duplicate_request_identity_matches(incoming: &TaskSpec, stored: &TaskSpec) -> bool {
        let method = |spec: &TaskSpec| {
            crate::http_engine::sanitize_http_method(&spec.request_method).to_ascii_uppercase()
        };
        let safe = |spec: &TaskSpec| {
            spec.credential_ref
                .as_deref()
                .is_none_or(|credential| credential.trim().is_empty())
                && spec.body_path.trim().is_empty()
                && method(spec) != "POST"
                && !spec.headers.keys().any(|name| sensitive_header_name(name))
        };
        let normalized_headers = |spec: &TaskSpec| {
            spec.headers
                .iter()
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        safe(incoming)
            && safe(stored)
            && method(incoming) == method(stored)
            && normalized_headers(incoming) == normalized_headers(stored)
    }

    fn duplicate_of(&self, spec: &TaskSpec) -> Result<Option<(String, String)>, String> {
        let want = crate::duplicate::canonicalize_url(&spec.url);
        if want.is_empty() {
            return Ok(None);
        }
        let core = self.lock()?;
        for task in core.tasks() {
            if let Some(stored) = core.task_spec(&task.task_id) {
                if crate::duplicate::canonicalize_url(&stored.url) == want
                    && Self::duplicate_request_identity_matches(spec, stored)
                {
                    return Ok(Some((task.task_id, task.status)));
                }
            }
        }
        Ok(None)
    }
'''
replace_once(old, new, "duplicate identity implementation")

anchor = '''    use std::net::TcpListener;
'''
tests = '''    use std::net::TcpListener;

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
        assert!(CoreCoordinator::duplicate_request_identity_matches(&same, &plain));

        let mut changed_method = same.clone();
        changed_method.request_method = "HEAD".into();
        assert!(!CoreCoordinator::duplicate_request_identity_matches(
            &changed_method,
            &plain
        ));

        let mut changed_header = same.clone();
        changed_header.headers.insert("Referer".into(), "https://page.test/".into());
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
            &sensitive,
            &plain
        ));

        let mut post = same.clone();
        post.request_method = "POST".into();
        post.body_path = "request-body.bin".into();
        assert!(!CoreCoordinator::duplicate_request_identity_matches(&post, &plain));

        let mut stored_credential = plain.clone();
        stored_credential.credential_ref = Some("sealed-old-context".into());
        assert!(!CoreCoordinator::duplicate_request_identity_matches(
            &same,
            &stored_credential
        ));
    }
'''
replace_once(anchor, tests, "duplicate identity regression test")

PATH.write_text(text, encoding="utf-8")
print("duplicate request identity patch applied")
