//! Identity-safe HTTP mirrors. An empty list is a no-op.

const MAX_MIRRORS: usize = 16;
const MAX_MIRROR_URL_LENGTH: usize = 8192;

pub fn canonical_http_url(value: &str) -> String {
    let raw = value.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return String::new();
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return String::new();
    }
    let rest = rest.split_once('#').map(|(keep, _)| keep).unwrap_or(rest);
    let boundary = rest
        .char_indices()
        .find(|(_, ch)| matches!(ch, '/' | '?'))
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    let (authority, path_query) = rest.split_at(boundary);
    let authority = authority.trim_end_matches('.').to_ascii_lowercase();
    if authority.is_empty() {
        return String::new();
    }
    let suffix = if path_query.starts_with('?') {
        format!("/{path_query}")
    } else {
        format!("/{}", path_query.trim_start_matches('/'))
    };
    format!("{scheme}://{authority}{suffix}")
}

pub fn normalize_mirror_urls(primary: &str, mirrors: &[String]) -> Vec<String> {
    let primary_key = canonical_http_url(primary);
    let mut seen = std::collections::BTreeSet::from([primary_key]);
    let mut result = Vec::new();
    for raw in mirrors {
        let url = raw.trim();
        if url.is_empty()
            || url.starts_with('#')
            || url.len() > MAX_MIRROR_URL_LENGTH
            || url.contains('\r')
            || url.contains('\n')
            || url.contains('\0')
        {
            continue;
        }
        let key = canonical_http_url(url);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        result.push(url.to_string());
        if result.len() >= MAX_MIRRORS {
            break;
        }
    }
    result
}

pub fn mirror_identity_compatible(
    primary_len: Option<u64>,
    primary_etag: &str,
    candidate_len: Option<u64>,
    candidate_etag: &str,
) -> bool {
    let lengths_match = match (primary_len, candidate_len) {
        (Some(left), Some(right)) if left != right => return false,
        (Some(_), Some(_)) => true,
        _ => false,
    };
    let left = strong_etag(primary_etag);
    let right = strong_etag(candidate_etag);
    if !left.is_empty() && !right.is_empty() {
        return left == right;
    }
    // Equal known lengths are sufficient when a CDN omits strong ETags. If
    // either length is unknown, require matching strong ETags instead of
    // admitting a mirror whose resource identity cannot be verified.
    lengths_match
}

fn strong_etag(value: &str) -> String {
    let etag = value.trim();
    if etag.to_ascii_lowercase().starts_with("w/") {
        String::new()
    } else {
        etag.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_mirrors_are_noop() {
        assert!(normalize_mirror_urls("https://cdn.example/a.bin", &[]).is_empty());
    }

    #[test]
    fn mirrors_drop_primary_and_duplicates() {
        let mirrors = normalize_mirror_urls(
            "https://CDN.example/a.bin",
            &[
                "https://cdn.example/a.bin".into(),
                "https://mirror.example/a.bin".into(),
                "https://mirror.example/a.bin".into(),
            ],
        );
        assert_eq!(mirrors, vec!["https://mirror.example/a.bin"]);
    }

    #[test]
    fn root_query_preserves_signed_value_case() {
        assert_eq!(
            canonical_http_url("https://CDN.example?token=ABC#fragment"),
            "https://cdn.example/?token=ABC"
        );
        let mirrors = normalize_mirror_urls(
            "https://cdn.example?token=ABC",
            &[
                "https://CDN.example/?token=ABC#fragment".into(),
                "https://cdn.example?token=abc".into(),
            ],
        );
        assert_eq!(mirrors, vec!["https://cdn.example?token=abc"]);
    }

    #[test]
    fn mirrors_drop_crlf_urls() {
        let mirrors = normalize_mirror_urls(
            "https://cdn.example/a.bin",
            &["https://mirror.example/a.bin\r\nX: 1".into()],
        );
        assert!(mirrors.is_empty());
    }

    #[test]
    fn weak_etag_does_not_block_length_match() {
        assert!(mirror_identity_compatible(
            Some(10),
            "W/\"1\"",
            Some(10),
            "W/\"2\""
        ));
        assert!(!mirror_identity_compatible(
            Some(10),
            "\"abc\"",
            Some(10),
            "\"xyz\""
        ));
        assert!(mirror_identity_compatible(
            Some(10),
            "\"abc\"",
            Some(10),
            ""
        ));
    }

    #[test]
    fn unknown_length_requires_matching_strong_etag() {
        assert!(!mirror_identity_compatible(Some(10), "", None, ""));
        assert!(!mirror_identity_compatible(None, "", None, ""));
        assert!(mirror_identity_compatible(
            None, "\"same\"", None, "\"same\""
        ));
        assert!(!mirror_identity_compatible(
            None,
            "W/\"same\"",
            None,
            "W/\"same\""
        ));
    }
}
