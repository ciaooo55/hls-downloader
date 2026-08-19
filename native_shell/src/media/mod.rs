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
    let reference = reference.trim();
    if reference.is_empty()
        || reference.contains('\r')
        || reference.contains('\n')
        || reference.contains('\0')
    {
        return String::new();
    }
    let lower = reference.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return reference.to_string();
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
        return format!("{scheme}://{rest}");
    }
    if let Some(scheme_end) = base.find("://") {
        let after = &base[scheme_end + 3..];
        let host_end = after.find('/').map(|index| scheme_end + 3 + index);
        if reference.starts_with('/') {
            let origin = host_end.map(|index| &base[..index]).unwrap_or(base);
            return format!("{origin}{reference}");
        }
        let dir_end = base.rfind('/').unwrap_or(base.len());
        return format!("{}/{reference}", &base[..dir_end]);
    }
    String::new()
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
