//! HLS / DASH playlist helpers and FFmpeg mux.

mod dash;
mod harness;
mod hls;
mod merge;
mod subtitles;

/// Resolve a playlist/manifest URI against `base`. Absolute schemes other than
/// http(s) (file, javascript, data, skd, …) are dropped so fetch/mux never
/// follow them. Relative paths keep the existing origin/directory join.
pub(crate) fn resolve_http_uri(base: &str, reference: &str) -> String {
    let reference = reference.trim().trim_start_matches('\u{feff}');
    if reference.is_empty() || reference.chars().any(|ch| ch.is_control()) {
        return String::new();
    }
    let lower = reference.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return inherit_access_query(base, reference);
    }
    if has_absolute_scheme(reference) {
        return String::new();
    }
    if let Some(rest) = reference.strip_prefix("//") {
        let scheme = if base.to_ascii_lowercase().starts_with("https://") {
            "https"
        } else {
            "http"
        };
        return inherit_access_query(base, &format!("{scheme}://{rest}"));
    }
    let clean_base = base.split(['?', '#']).next().unwrap_or(base);
    if let Some(scheme_end) = clean_base.find("://") {
        let authority_start = scheme_end + 3;
        let path_start = clean_base[authority_start..]
            .find('/')
            .map(|index| authority_start + index);
        let origin_end = path_start.unwrap_or(clean_base.len());
        let origin = &clean_base[..origin_end];
        if reference.starts_with('/') {
            return inherit_access_query(base, &format!("{origin}{reference}"));
        }
        let directory = path_start
            .map(|_| {
                let dir_end = clean_base.rfind('/').unwrap_or(origin_end);
                &clean_base[..dir_end]
            })
            .unwrap_or(clean_base);
        return inherit_access_query(base, &format!("{directory}/{reference}"));
    }
    String::new()
}

fn inherit_access_query(base: &str, resolved: &str) -> String {
    let base_origin = crate::credentials::request_origin(base);
    if base_origin.is_empty() || base_origin != crate::credentials::request_origin(resolved) {
        return resolved.to_string();
    }
    let base_query = url_query(base);
    if base_query.is_empty() {
        return resolved.to_string();
    }
    let child_query = url_query(resolved);
    let child_names: std::collections::BTreeSet<String> = child_query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(query_name)
        .collect();
    let base_names: std::collections::BTreeSet<String> = base_query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(query_name)
        .collect();
    let terse_signature = base_names.contains("s") && base_names.contains("e");
    let inherited: Vec<&str> = base_query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = query_name(pair);
            !child_names.contains(&name)
                && (hls_access_query_name(&name)
                    || (terse_signature && matches!(name.as_str(), "s" | "e" | "_t")))
        })
        .collect();
    if inherited.is_empty() {
        return resolved.to_string();
    }
    let (without_fragment, fragment) = resolved.split_once('#').unwrap_or((resolved, ""));
    let separator = if without_fragment.contains('?') {
        "&"
    } else {
        "?"
    };
    let mut next = format!("{without_fragment}{separator}{}", inherited.join("&"));
    if !fragment.is_empty() {
        next.push('#');
        next.push_str(fragment);
    }
    next
}

fn url_query(url: &str) -> &str {
    url.split_once('?')
        .map(|(_, tail)| tail.split('#').next().unwrap_or(""))
        .unwrap_or("")
}

fn query_name(pair: &str) -> String {
    pair.split('=')
        .next()
        .unwrap_or("")
        .replace('+', " ")
        .to_ascii_lowercase()
}

fn hls_access_query_name(name: &str) -> bool {
    name.starts_with("x-amz-")
        || matches!(
            name,
            "token"
                | "auth"
                | "authorization"
                | "signature"
                | "sig"
                | "expire"
                | "expires"
                | "expiry"
                | "policy"
                | "key-pair-id"
                | "hdnea"
                | "hmac"
                | "jwt"
                | "session"
                | "sessionid"
                | "access_key"
                | "access-key"
                | "pkey"
                | "psch"
                | "playlisttype"
                | "validfrom"
                | "validto"
                | "ipa"
                | "hdl"
                | "hash"
        )
}

fn has_absolute_scheme(reference: &str) -> bool {
    let bytes = reference.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut index = 1;
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'+' | b'.' | b'-'))
    {
        index += 1;
    }
    bytes.get(index) == Some(&b':')
}

#[allow(unused_imports)]
pub use dash::{
    audio_choices as dash_audio_choices, download_dash, download_dash_selected, parse_mpd,
    representation_choices,
};
#[allow(unused_imports)]
pub use hls::{
    audio_choices, download_hls, download_hls_selected, download_hls_with, parse_playlist,
    variant_choices, HlsDownloadOptions,
};
#[allow(unused_imports)]
pub use merge::{concat_files, merge_with_ffmpeg, mux_av};

#[cfg(test)]
mod tests {
    use super::resolve_http_uri;

    #[test]
    fn resolves_children_from_query_bearing_origin_base() {
        assert_eq!(
            resolve_http_uri("https://cdn.example?token=abc", "/seg.ts"),
            "https://cdn.example/seg.ts?token=abc"
        );
        assert_eq!(
            resolve_http_uri("https://cdn.example?token=abc", "seg.ts"),
            "https://cdn.example/seg.ts?token=abc"
        );
    }

    #[test]
    fn inherits_terse_signatures_only_as_a_pair() {
        assert_eq!(
            resolve_http_uri("https://cdn.example/master.m3u8?s=sort", "seg.ts"),
            "https://cdn.example/seg.ts"
        );
        assert_eq!(
            resolve_http_uri("https://cdn.example/master.m3u8?e=event", "seg.ts"),
            "https://cdn.example/seg.ts"
        );
        assert_eq!(
            resolve_http_uri(
                "https://cdn.example/master.m3u8?s=abc&e=123&_t=nonce",
                "seg.ts"
            ),
            "https://cdn.example/seg.ts?s=abc&e=123&_t=nonce"
        );
    }
}