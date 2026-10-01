//! 扩展名分类表。Core 侧曾经有三处各自写字面量，已收成此处三个具名常量。
//!
//! **注意这是三张表，不是一张，且它们的成员有意不同**，不要合并：
//!
//! - [`MEDIA_FOLDER_EXTENSIONS`]：`category.rs::download_category` 用它决定**归哪个
//!   分类文件夹**。图片（jpg/png/gif/webp）和 mpd 算媒体——它们是媒体文件。
//! - [`MEDIA_PLAYABLE_EXTENSIONS`]：`core_runtime::completed_actions` 用它决定**要不
//!   要给"播放"动作**。图片没有"播放"语义，mpd 是 DASH 清单也不该直接播，所以不含。
//! - [`EXECUTABLE_EXTENSIONS`]："分类文件夹"与"启动"动作共用这一份，两处一致。
//!
//! `playable_is_subset_of_folder` 钉住了 playable ⊂ folder：将来只改一边，那条测试会
//! 提醒你是有意的还是漏了。

/// 归"媒体"分类文件夹的扩展名。与 `desktop_ui` 的 `catMEDIA_EXTENSIONS` 逐项相等，
/// 由 `desktop_ui` 的 `CategoryParityTest` 跨语言比对守卫。
pub const MEDIA_FOLDER_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "mp3", "m4a", "flac", "wav", "ac3", "aac",
    "3gp", "flv", "m2ts", "mka", "mpd", "mpeg", "mpg", "ogg", "opus", "wma", "wmv", "jpg", "png",
    "gif", "webp",
];

/// 可给"播放"动作的扩展名。是 [`MEDIA_FOLDER_EXTENSIONS`] 的真子集：去掉图片与 mpd。
pub const MEDIA_PLAYABLE_EXTENSIONS: &[&str] = &[
    "3gp", "aac", "ac3", "avi", "flac", "flv", "m2ts", "m4a", "m4v", "mka", "mkv", "mov", "mp3",
    "mp4", "mpeg", "mpg", "ogg", "opus", "ts", "wav", "webm", "wma", "wmv",
];

/// 可执行／安装包扩展名。"分类文件夹"与"启动"动作共用这一份。
pub const EXECUTABLE_EXTENSIONS: &[&str] =
    &["appx", "bat", "cmd", "com", "exe", "msi", "msix", "ps1"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playable_is_subset_of_folder() {
        // playable 必须整体落在 folder 里，否则会出现"可播放但不归媒体"的自相矛盾。
        let missing: Vec<&str> = MEDIA_PLAYABLE_EXTENSIONS
            .iter()
            .copied()
            .filter(|ext| !MEDIA_FOLDER_EXTENSIONS.contains(ext))
            .collect();
        assert!(
            missing.is_empty(),
            "playable 里有 folder 没有的扩展名：{missing:?}"
        );
    }

    #[test]
    fn folder_strictly_contains_images_which_are_not_playable() {
        // 语义要反过来钉住：图片在 folder 里、不在 playable 里。若哪天这条失败，
        // 说明有人无意中把两份表合并了——那会让"播放"出现在图片任务上。
        for image in ["jpg", "png", "gif", "webp"] {
            assert!(
                MEDIA_FOLDER_EXTENSIONS.contains(&image),
                "{image} 应归媒体文件夹"
            );
            assert!(
                !MEDIA_PLAYABLE_EXTENSIONS.contains(&image),
                "{image} 不该有播放动作"
            );
        }
    }

    #[test]
    fn no_duplicates_within_each_table() {
        for (name, table) in [
            ("folder", MEDIA_FOLDER_EXTENSIONS),
            ("playable", MEDIA_PLAYABLE_EXTENSIONS),
            ("executable", EXECUTABLE_EXTENSIONS),
        ] {
            let mut sorted = table.to_vec();
            sorted.sort_unstable();
            let before = sorted.len();
            sorted.dedup();
            assert_eq!(before, sorted.len(), "{name} 表里有重复项");
        }
    }
}
