//! Windows 注册表相关的小助手。
//!
//! `wide` / `registry_error` 此前在 `startup.rs` 和 `native_host_registration.rs`
//! 各有一份，逐字节相同。两个注册表模块各自持有同一份平台专用代码，改一处忘另一处
//! 就会漂移，故收到这里。
//!
//! **这两个函数在所有平台上都参与编译**，不加 `#[cfg(windows)]`。因为它们只在
//! Windows 上被调用，若按惯例给模块加 cfg，非 Windows 构建就再也碰不到它们——
//! 而那正是唯一能提前发现"改动破坏了它们"的机会（本项目 CI 的 clippy 只在
//! windows-latest 跑）。为此非 Windows 上给 `allow(dead_code)`：不是没人用，
//! 是故意让它仍然被编译和测试。

use std::io;

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn registry_error(action: &str, code: u32) -> String {
    format!("{action}: {}", io::Error::from_raw_os_error(code as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_appends_exactly_one_nul_terminator() {
        // 注册表 API 要的是 NUL 结尾的 UTF-16。多一个 NUL 会写出空壳值，
        // 少一个则 API 返回 ERROR_MORE_DATA——两者都极难在 UI 上看出来。
        let encoded = wide("HKCU\\Run");
        assert_eq!(encoded.last(), Some(&0), "必须以 NUL 结尾");
        assert_eq!(
            encoded.iter().filter(|&&u| u == 0).count(),
            1,
            "只能有一个 NUL"
        );
        assert_eq!(
            &encoded[..encoded.len() - 1],
            "HKCU\\Run".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn wide_of_empty_string_is_just_the_terminator() {
        assert_eq!(wide(""), vec![0]);
    }

    #[test]
    fn registry_error_carries_both_the_action_and_the_code_text() {
        // 文案必须同时告诉用户"哪一步失败"和"系统怎么说"，否则只剩一句
        // "拒绝访问"之类，无法定位是开机自启还是浏览器宿主注册。
        let message = registry_error("open HKCU\\Run", 5);
        assert!(
            message.starts_with("open HKCU\\Run: "),
            "缺少动作名：{message}"
        );
        assert!(!message.ends_with(": "), "缺少系统错误描述：{message}");
    }
}
