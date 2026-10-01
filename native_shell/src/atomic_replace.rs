//! 原子替换一个已存在文件的共享助手。
//!
//! 此前 `http_engine.rs::replace_checkpoint_file` 与
//! `download_worker.rs::replace_torrent_selection_file` 是同一对
//! `#[cfg(windows)] MoveFileExW` / `#[cfg(not(windows))] fs::rename` 助手跨文件各写
//! 一遍，而且 Windows 侧已经漂移：前者带 20 次退避重试，后者是单发。
//!
//! 丢失重试是真问题：BT 文件选择表就写在 BT 客户端自己持有的目录旁边，
//! 单发 MoveFileExW 一旦撞上 ERROR_SHARING_VIOLATION 就直接失败。
//!
//! 这里把**重试策略**与**平台机制**分开：策略是与平台无关的闭包助手，在所有平台上
//! 编译并有单测（否则非 Windows 构建只能测到 fs::rename，碰不到最需要测的那段）；
//! 平台机制仍是各平台各一行。合并后两处都拿到带退避的那份语义。

use std::io;
use std::thread;
use std::time::Duration;

/// 退避重试的总次数。20 次 × 10ms 约 200ms，足够杀软的一次扫描让出句柄。
const MAX_ATTEMPTS: u32 = 20;
/// 每次重试间隔。
const RETRY_BACKOFF: Duration = Duration::from_millis(10);

/// 值得重试的 Windows 错误码：拒绝访问 / 共享冲突。二者都表示"文件正被占用"，
/// 是瞬时状态而非调用方错误。
fn is_transiently_locked(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32))
}

/// 对"瞬时占用"退避重试的原子替换。
///
/// `move_once` 是平台各自的一次移动尝试（Windows 的 MoveFileExW、其它平台的 rename）。
/// 非瞬时错误立即返回，不做无谓等待；瞬时错误退避到 [`MAX_ATTEMPTS`] 次为止。
pub(crate) fn replace_with_retry(mut move_once: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    for attempt in 0..MAX_ATTEMPTS {
        match move_once() {
            Ok(()) => return Ok(()),
            Err(error) => {
                let last = attempt + 1 == MAX_ATTEMPTS;
                if last || !is_transiently_locked(&error) {
                    return Err(error);
                }
                thread::sleep(RETRY_BACKOFF);
            }
        }
    }
    unreachable!("循环在最后一次尝试必然 return")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn succeeds_on_the_first_attempt_without_sleeping() {
        let mut calls = 0;
        replace_with_retry(|| {
            calls += 1;
            Ok(())
        })
        .expect("一次成功不该失败");
        assert_eq!(calls, 1, "成功路径不该重试");
    }

    #[test]
    fn retries_transient_lock_and_then_succeeds() {
        // 复现真实场景：前两次撞上"文件被占用"，第三次成功。
        let mut calls = 0;
        let started = std::time::Instant::now();
        replace_with_retry(|| {
            calls += 1;
            if calls < 3 {
                return Err(io::Error::from_raw_os_error(32)); // ERROR_SHARING_VIOLATION
            }
            Ok(())
        })
        .expect("瞬时占用重试后应成功");
        assert_eq!(calls, 3, "应该在第三次成功");
        // 两次退避 = 20ms，留出下限以防有人把退避删了测试却不红。
        assert!(
            started.elapsed() >= Duration::from_millis(18),
            "退避被跳过了？"
        );
    }

    #[test]
    fn gives_up_after_the_attempt_budget() {
        let mut calls = 0;
        let error = replace_with_retry(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(5)) // ERROR_ACCESS_DENIED
        })
        .expect_err("一直占用最终必须失败，不能无限重试");
        assert_eq!(calls, MAX_ATTEMPTS as usize, "必须用完整个预算");
        assert_eq!(
            error.raw_os_error(),
            Some(5),
            "要把最后一次的系统错误抛出去"
        );
    }

    #[test]
    fn a_permanent_error_fails_immediately_without_burning_the_budget() {
        // 路径不存在这类错误重试 20 次毫无意义，只是让用户多等 200ms。
        let mut calls = 0;
        let error = replace_with_retry(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(3)) // ERROR_PATH_NOT_FOUND
        })
        .expect_err("非瞬时错误必须失败");
        assert_eq!(calls, 1, "非瞬时错误不该重试");
        assert_eq!(error.raw_os_error(), Some(3));
    }

    #[test]
    fn both_lock_error_codes_are_treated_as_transient() {
        for code in [5u32, 32] {
            assert!(
                is_transiently_locked(&io::Error::from_raw_os_error(code as i32)),
                "错误码 {code} 应视为瞬时占用"
            );
        }
        for code in [2u32, 3, 87] {
            assert!(
                !is_transiently_locked(&io::Error::from_raw_os_error(code as i32)),
                "错误码 {code} 是永久错误，不应重试"
            );
        }
    }
}
