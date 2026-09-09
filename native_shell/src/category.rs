//! IDM-style category folders for newly created tasks.

pub fn download_category(filename: &str, url: &str, kind: crate::ResourceKind) -> &'static str {
    if matches!(
        kind,
        crate::ResourceKind::Hls | crate::ResourceKind::Dash | crate::ResourceKind::Live
    ) {
        return "media";
    }
    let name = if filename.trim().is_empty() {
        url
    } else {
        filename
    };
    let ext = extension(name);
    if matches!(
        ext,
        "mp4"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "m4v"
            | "ts"
            | "mp3"
            | "m4a"
            | "flac"
            | "wav"
            | "jpg"
            | "png"
            | "gif"
            | "webp"
    ) {
        "media"
    } else if matches!(ext, "exe" | "msi" | "msix" | "appx" | "bat" | "cmd") {
        "program"
    } else if matches!(
        ext,
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "iso"
    ) {
        "archive"
    } else {
        "other"
    }
}

pub fn category_label(category: &str) -> &'static str {
    match category {
        "media" => "媒体",
        "program" => "程序",
        "archive" => "压缩包",
        _ => "其他",
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CategoryDirs {
    pub media: String,
    pub program: String,
    pub archive: String,
    pub other: String,
}

pub fn parse_category_dirs(raw: &str) -> CategoryDirs {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return CategoryDirs::default();
    };
    CategoryDirs {
        media: value
            .get("media")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        program: value
            .get("program")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        archive: value
            .get("archive")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        other: value
            .get("other")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

pub fn category_dirs_json(dirs: &CategoryDirs) -> String {
    serde_json::json!({
        "media": dirs.media,
        "program": dirs.program,
        "archive": dirs.archive,
        "other": dirs.other,
    })
    .to_string()
}

impl CategoryDirs {
    pub fn get(&self, category: &str) -> &str {
        match category {
            "media" => self.media.as_str(),
            "program" => self.program.as_str(),
            "archive" => self.archive.as_str(),
            _ => self.other.as_str(),
        }
    }
}

pub fn resolve_category_dir(
    download_dir: &str,
    filename: &str,
    url: &str,
    kind: crate::ResourceKind,
    auto_category: bool,
    overrides: &CategoryDirs,
) -> String {
    let chosen = download_category(filename, url, kind);
    let configured = overrides.get(chosen).trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    if !auto_category {
        return download_dir.to_string();
    }
    let root = if download_dir.trim().is_empty() {
        std::path::PathBuf::from("downloads")
    } else {
        std::path::PathBuf::from(download_dir)
    };
    root.join(category_label(chosen))
        .to_string_lossy()
        .into_owned()
}

fn extension(path: &str) -> &str {
    let name = path.split(['?', '#']).next().unwrap_or(path);
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceKind;

    #[test]
    fn places_media_under_chinese_subdir() {
        let dir = resolve_category_dir(
            "D:\\Downloads",
            "show.mkv",
            "",
            ResourceKind::File,
            true,
            &CategoryDirs::default(),
        );
        assert!(dir.ends_with("媒体") || dir.ends_with("媒体\\") || dir.contains("媒体"));
        assert_eq!(
            download_category("setup.exe", "", ResourceKind::File),
            "program"
        );
        assert_eq!(
            resolve_category_dir(
                "D:\\Downloads",
                "a.bin",
                "",
                ResourceKind::File,
                false,
                &CategoryDirs::default(),
            ),
            "D:\\Downloads"
        );
        let override_media = CategoryDirs {
            media: "E:\\Videos".into(),
            ..CategoryDirs::default()
        };
        assert_eq!(
            resolve_category_dir(
                "D:\\Downloads",
                "show.mkv",
                "",
                ResourceKind::File,
                false,
                &override_media,
            ),
            "E:\\Videos"
        );
        assert_eq!(
            parse_category_dirs(r#"{"media":" E:\\Videos ","program":""}"#).media,
            "E:\\Videos"
        );
        assert!(category_dirs_json(&override_media).contains("Videos"));
    }
}
