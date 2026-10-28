//! 时间工具：纪元毫秒、公历→日序、本地时间。
//!
//! 相对参考项目的变化：原来用 `windows` crate 的 `GetLocalTime()` 取本地时间，
//! 服务端要跑在 Linux 上，改成 `chrono::Local`。`windows` 依赖因此整个去掉。

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, Timelike};

/// 当前纪元毫秒（UTC）。全仓统一用它，避免各处自己 `SystemTime::now()` 后比较口径不同。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 本地日期与小时 `(年, 月, 日, 时)`。
///
/// 每日动作要按**本地**日期判断"今天做没做"，所以不能用 UTC。
pub fn local_date_hour() -> (i32, u32, u32, u32) {
    let now = Local::now();
    (
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
    )
}

/// Howard Hinnant 的 days_from_civil：1970-01-01 起的天数（可为负）。
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// 把「某时区的 Y-M-D H:M:S」换算成纪元毫秒。
pub fn civil_to_epoch_ms(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    offset_secs: i64,
) -> u64 {
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    secs.max(0) as u64 * 1000
}

/// 下一个「本机时区 04:00」的纪元毫秒（余额停放用：避开上游凌晨的结算点）。
///
/// 用 `chrono::Local` 而不是手算时区偏移：手算在半小时时区（+5:30）与
/// 夏令时切换当天会算错。
pub fn next_local_4am_ms(now_ms: u64) -> u64 {
    let now = Local::now();
    let today_4am = now
        .date_naive()
        .and_hms_opt(4, 0, 0)
        .expect("04:00 总是合法时间");

    let today_4am_ms = today_4am
        .and_local_timezone(Local)
        .single()
        .map(|dt| dt.timestamp_millis() as u64)
        .unwrap_or(now_ms);

    let target = if today_4am_ms > now_ms {
        today_4am_ms
    } else {
        // 已经过了今天 04:00，取明天。
        let tomorrow = today_4am + chrono::Duration::days(1);
        tomorrow
            .and_local_timezone(Local)
            .single()
            .map(|dt| dt.timestamp_millis() as u64)
            .unwrap_or(now_ms + 24 * 3600 * 1000)
    };

    // 时钟回拨或时区跳变的极端情况下，至少停放一分钟，避免立刻重试打成死循环。
    target.max(now_ms + 60_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_match_unix_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        // 2036-01-01 = 1970 起 66 年 + 16 个闰日
        assert_eq!(days_from_civil(2036, 1, 1), 66 * 365 + 16);
    }

    #[test]
    fn epoch_conversion_honours_timezone_offset() {
        assert_eq!(civil_to_epoch_ms(1970, 1, 1, 8, 0, 0, 8 * 3600), 0);
        assert_eq!(civil_to_epoch_ms(1970, 1, 1, 0, 0, 0, 0), 0);
        assert_eq!(civil_to_epoch_ms(1970, 1, 2, 0, 0, 0, 0), 86_400_000);
    }

    #[test]
    fn balance_parking_is_always_in_the_future() {
        let now = now_ms();
        let target = next_local_4am_ms(now);
        assert!(target > now, "停放时刻必须在将来：{target} <= {now}");
        assert!(target <= now + 25 * 3600 * 1000, "停放不应超过一天");
    }
}
