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
    let ext = extension(name).to_ascii_lowercase();
    // 两张扩展名表都在 media_ext.rs：folder 表定"归哪个文件夹"，与 completed_actions
    // 的 playable 表**有意不同**（图片算媒体但没有播放语义），三张表由那里的守卫测试钉住。
    if crate::media_ext::MEDIA_FOLDER_EXTENSIONS.contains(&ext.as_str()) {
        "media"
    } else if crate::media_ext::EXECUTABLE_EXTENSIONS.contains(&ext.as_str()) {
        "program"
    } else if matches!(
        ext.as_str(),
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

/// 写入 `browser_category_dirs` 前必须先过这一关。
///
/// `parse_category_dirs` 故意宽容：历史脏数据照样读得出来（读路径不能炸）。
/// 但写路径必须严格——Compose 主界面曾经把这个键写成
/// `"E:/Videos|E:/Apps|..."`，Rust 解析失败后静默回退成四个空目录，
/// 用户在设置里填的分类目录整段消失，而且 download_worker 的越界校验也被跳过。
/// 现在格式不对就明确拒绝，坏数据再也进不到 Core 状态里。
pub fn validate_category_dirs(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| "分类目录格式无效".to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "分类目录格式无效".to_string())?;
    for key in ["media", "program", "archive", "other"] {
        if let Some(item) = object.get(key) {
            if !item.is_string() {
                return Err(format!("分类目录 {key} 必须是字符串"));
            }
        }
    }
    Ok(())
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
    }

    #[test]
    fn writing_category_dirs_rejects_the_pipe_joined_shape_the_ui_once_sent() {
        // Compose 主界面当年把四个目录用 "|" 拼成一个字符串发过来，parse_category_dirs
        // 解析失败后静默回退成空目录——用户填的目录整段消失。写路径必须把它挡掉。
        let pipe_joined = "E:/Videos|E:/Apps|E:/Archives|E:/Other";
        assert!(validate_category_dirs(pipe_joined).is_err());
        // 而且旧形状读出来必须是空的，证明这不是"两边格式不一致但都能用"。
        assert_eq!(parse_category_dirs(pipe_joined), CategoryDirs::default());

        // 空串是合法的（表示没有覆盖），Core 的 Settings 响应也会把它原样带回。
        assert!(validate_category_dirs("").is_ok());
        assert!(validate_category_dirs("  ").is_ok());

        // 合法形状：媒体/程序/压缩包/其他 四个键都是字符串。
        assert!(validate_category_dirs(
            r#"{"media":"E:/Videos","program":"","archive":"","other":""}"#
        )
        .is_ok());
        // 只要有一个键在，也算合法——presenter 会逐个键累积写入。
        assert!(validate_category_dirs(r#"{"other":"E:/Other"}"#).is_ok());

        // 不是对象、或者值是数字/数组，一律拒绝。
        assert!(validate_category_dirs(r#"["E:/Videos"]"#).is_err());
        assert!(validate_category_dirs(r#"{"media":12}"#).is_err());
        assert!(validate_category_dirs(r#"not json at all"#).is_err());
    }

    #[test]
    fn round_trip_keeps_all_four_directories() {
        let stored =
            r#"{"media":"E:/Videos","program":"E:/Apps","archive":"E:/Arch","other":"E:/Other"}"#;
        assert!(validate_category_dirs(stored).is_ok());
        let dirs = parse_category_dirs(stored);
        assert_eq!(dirs.media, "E:/Videos");
        assert_eq!(dirs.program, "E:/Apps");
        assert_eq!(dirs.archive, "E:/Arch");
        assert_eq!(dirs.other, "E:/Other");
    }

    #[test]
    fn category_extensions_are_case_insensitive() {
        assert_eq!(
            download_category("VIDEO.MP4", "", ResourceKind::File),
            "media"
        );
        assert_eq!(
            download_category("SETUP.EXE", "", ResourceKind::File),
            "program"
        );
        assert_eq!(
            download_category("ARCHIVE.ZIP", "", ResourceKind::File),
            "archive"
        );
    }
}
