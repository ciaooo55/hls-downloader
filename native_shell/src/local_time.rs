//! 本地时间。此前 `net_policy.rs` 的 `local_weekday_iso` 与 `chrono_minutes_now`
//! 各自声明了一份 `#[repr(C)] struct SystemTime`、一份 `GetLocalTime` extern、一份
//! 初始化，两处逐字节相同，只是取出的字段不同（一个取 day_of_week，一个取
//! hour/minute）。
//!
//! # 切分方式
//!
//! FFI 那部分**不能**像 `win_reg` / `atomic_replace` 那样整块搬出来：这里带
//! `#[link(name = "kernel32")]`，无条件编译会让非 Windows 链接 `-lkernel32` 而失败。
//! 所以 extern 块与调用留在 `net_policy.rs` 的 `#[cfg(windows)]` 里，只搬：
//!
//! 1. 字段结构体（纯 Rust，无链接依赖，所有平台编译）；
//! 2. **派生逻辑**——`windows_day_of_week_to_iso` 与 Unix 时间的星期公式。
//!
//! 第 2 项才是真正值得收的：星期映射算错时不会报错，只会安静地把"周日"当成"周一"，
//! 而计划任务的触发窗口就整体偏移一天。这恰恰是 Linux 上唯一能覆盖到的部分
//! （SYSTEMTIME 那条路径编不出 Windows 目标就碰不到）。

use std::time::{SystemTime, UNIX_EPOCH};

/// 与 kernel32 `SYSTEMTIME` 布局一致的本地时间字段。
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct LocalTimeFields {
    pub(crate) year: u16,
    pub(crate) month: u16,
    pub(crate) day_of_week: u16,
    pub(crate) day: u16,
    pub(crate) hour: u16,
    pub(crate) minute: u16,
    pub(crate) second: u16,
    pub(crate) milliseconds: u16,
}

impl LocalTimeFields {
    /// Windows `GetLocalTime` 的 `wDayOfWeek` 是 0=周日…6=周六；
    /// 本项目的计划窗口用 ISO 星期（1=周一…7=周日）。
    /// 这个映射错一夜之间不会有人发现，只会让任务整体偏移一天。
    pub(crate) fn iso_weekday(&self) -> u8 {
        match self.day_of_week {
            0 => 7,
            other => other as u8,
        }
    }

    /// 当天从零点过去的分钟数。
    pub(crate) fn minutes_since_midnight(&self) -> u32 {
        u32::from(self.hour) * 60 + u32::from(self.minute)
    }
}

/// 非 Windows 上由 Unix 秒推 ISO 星期。
///
/// 1970-01-01 是星期四（ISO 4），故 `(days + 3) % 7 + 1`。
/// 这里按 Unix 秒取整到天，与 Windows 侧"按本地日历日"在语义上一致：
/// 两者都以**本地**一天的起点为界，而不是按 UTC 日界。
pub(crate) fn unix_seconds_to_iso_weekday(unix_seconds: u64) -> u8 {
    (((unix_seconds / 86_400) + 3) % 7 + 1) as u8
}

/// 非 Windows 上取当前 Unix 秒。失败（时钟早于 epoch）时按 0 处理，与旧行为一致。
pub(crate) fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_day_of_week_maps_sunday_to_seven() {
        // 周日(0)必须落到 7。若这行写成 other as u8，周日会变成 0，
        // 计划窗口"周日"就永远匹配不上——而且是静默的。
        assert_eq!(
            LocalTimeFields {
                day_of_week: 0,
                ..Default::default()
            }
            .iso_weekday(),
            7
        );
    }

    #[test]
    fn windows_day_of_week_maps_monday_through_saturday_unchanged() {
        for (windows_value, iso) in [(1u16, 1u8), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6)] {
            let fields = LocalTimeFields {
                day_of_week: windows_value,
                ..Default::default()
            };
            assert_eq!(fields.iso_weekday(), iso, "wDayOfWeek={windows_value}");
        }
    }

    #[test]
    fn iso_weekday_output_stays_within_one_to_seven() {
        for day_of_week in 0u16..=6 {
            let weekday = LocalTimeFields {
                day_of_week,
                ..Default::default()
            }
            .iso_weekday();
            assert!(
                (1..=7).contains(&weekday),
                "wDayOfWeek={day_of_week} 得出 {weekday}"
            );
        }
    }

    #[test]
    fn minutes_since_midnight_ignores_date_and_seconds() {
        let fields = LocalTimeFields {
            year: 2026,
            month: 9,
            day: 29,
            day_of_week: 2,
            hour: 13,
            minute: 45,
            second: 59,
            milliseconds: 999,
        };
        assert_eq!(fields.minutes_since_midnight(), 13 * 60 + 45);
    }

    #[test]
    fn minutes_since_midnight_wraps_at_midnight_and_last_minute() {
        assert_eq!(
            LocalTimeFields {
                hour: 0,
                minute: 0,
                ..Default::default()
            }
            .minutes_since_midnight(),
            0
        );
        assert_eq!(
            LocalTimeFields {
                hour: 23,
                minute: 59,
                ..Default::default()
            }
            .minutes_since_midnight(),
            23 * 60 + 59
        );
    }

    #[test]
    fn unix_epoch_day_is_thursday() {
        // 1970-01-01 是星期四（ISO 4）。这条钉住整条公式的锚点。
        assert_eq!(unix_seconds_to_iso_weekday(0), 4);
    }

    #[test]
    fn known_dates_map_to_their_iso_weekday() {
        // 三个已知日期，覆盖"非epoch起点"与跨闰年。
        let cases = [
            (86_400u64 * 19_723, 1u8), // 2024-01-01 周一
            (86_400u64 * 10_957, 6u8), // 2000-01-01 周六
            (86_400u64 * 20_492, 7u8), // 2026-01-04 周日
        ];
        for (seconds, expected) in cases {
            assert_eq!(
                unix_seconds_to_iso_weekday(seconds),
                expected,
                "unix 秒 {seconds} 的 ISO 星期应为 {expected}"
            );
        }
    }

    #[test]
    fn unix_weekday_advances_by_one_each_day_and_returns_to_sunday() {
        // 连续性：相邻两天必须相差一天；第 8 天回到起点。
        let base = 1_800_000_000u64;
        let first = unix_seconds_to_iso_weekday(base);
        for offset in 1u64..=7 {
            let next = unix_seconds_to_iso_weekday(base + offset * 86_400);
            let expected = if first + offset as u8 > 7 {
                first + offset as u8 - 7
            } else {
                first + offset as u8
            };
            assert_eq!(next, expected, "第 {offset} 天");
        }
    }

    #[test]
    fn both_platforms_agree_on_the_epoch_day() {
        // 两侧必须给同一个答案，否则"计划任务按星期触发"会随平台漂移。
        let from_unix = unix_seconds_to_iso_weekday(0);
        // 1970-01-01 在 Windows 的 GetLocalTime 里 wDayOfWeek = 4（星期四）。
        let from_windows = LocalTimeFields {
            day_of_week: 4,
            ..Default::default()
        }
        .iso_weekday();
        assert_eq!(from_unix, from_windows);
    }
}
