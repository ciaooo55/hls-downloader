//! Same-URL reuse: resume / retry / focus instead of a second task.

pub fn canonicalize_url(url: &str) -> String {
    url.trim()
        .split('#')
        .next()
        .unwrap_or(url)
        .trim()
        .to_string()
}

pub fn suggest_duplicate_action(status: &str, output_missing: bool) -> &'static str {
    match status {
        "paused" | "pausing" => "resume",
        "failed" | "canceled" | "unsupported" => "retry",
        "queued" | "awaiting_selection" | "awaiting_confirmation" => "start",
        "completed" | "done" if output_missing => "retry",
        "completed" | "done" => "open",
        "downloading" | "recording" | "merging" | "checking" | "parsing" => "focus",
        _ => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_same_url_resumes() {
        assert_eq!(suggest_duplicate_action("paused", false), "resume");
        assert_eq!(suggest_duplicate_action("downloading", false), "focus");
        assert_eq!(suggest_duplicate_action("completed", true), "retry");
        assert_eq!(suggest_duplicate_action("completed", false), "open");
    }
}
