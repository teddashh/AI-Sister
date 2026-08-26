//! 本地日曆日。日期字串是 `%Y-%m-%d`，日界是本地午夜。
//!
//! 這幾支是純函式，放這裡是因為 `brain` 依賴 `db`、`wakeup` 依賴
//! `reviewer`，日期計算三邊都要用，不能抄三份。

use chrono::{Local, LocalResult, NaiveDate, TimeZone};

use crate::model::Millis;

pub fn local_day_key(ts: Millis) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ts)
        .map(|d| d.with_timezone(&Local).format("%Y-%m-%d").to_string())
}

pub fn previous_local_day_key(now: Millis) -> Option<String> {
    let dt = chrono::DateTime::from_timestamp_millis(now)?.with_timezone(&Local);
    let prev = dt
        .date_naive()
        .checked_sub_signed(chrono::TimeDelta::days(1))?;
    Some(prev.format("%Y-%m-%d").to_string())
}

/// 日曆上的下一天。`day_key` 是 `%Y-%m-%d`，加的是日曆日，不是時區換算。
pub fn next_local_day_key(day_key: &str) -> Option<String> {
    let date = NaiveDate::parse_from_str(day_key, "%Y-%m-%d").ok()?;
    Some(
        date.checked_add_signed(chrono::TimeDelta::days(1))?
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// 本地日界 `[start, end)`。`end` 是下一個本地午夜。
pub fn local_day_bounds(day_key: &str) -> Option<(Millis, Millis)> {
    let date = NaiveDate::parse_from_str(day_key, "%Y-%m-%d").ok()?;
    let start = local_midnight(date)?;
    let end = local_midnight(date.checked_add_signed(chrono::TimeDelta::days(1))?)?;
    Some((start, end))
}

fn local_midnight(date: NaiveDate) -> Option<Millis> {
    let naive = date.and_hms_opt(0, 0, 0)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt.timestamp_millis()),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_local_day_key_crosses_month_end() {
        assert_eq!(
            next_local_day_key("2026-08-31").as_deref(),
            Some("2026-09-01")
        );
        assert_eq!(
            next_local_day_key("not-a-date").as_deref(),
            None,
            "看不懂的日期不能發明下一天"
        );
    }

    #[test]
    fn previous_local_day_key_is_calendar_yesterday() {
        let tuesday = match Local.with_ymd_and_hms(2026, 8, 25, 0, 1, 0) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp_millis(),
            LocalResult::None => return,
        };
        assert_eq!(
            previous_local_day_key(tuesday).as_deref(),
            Some("2026-08-24")
        );
        assert_eq!(local_day_key(tuesday).as_deref(), Some("2026-08-25"));
    }

    #[test]
    fn local_day_bounds_are_half_open_at_local_midnight() {
        let Some((start, end)) = local_day_bounds("2026-08-24") else {
            return;
        };
        assert_eq!(local_day_key(start).as_deref(), Some("2026-08-24"));
        assert_eq!(local_day_key(end).as_deref(), Some("2026-08-25"));
        assert!(
            local_day_key(end.saturating_sub(1)).as_deref() == Some("2026-08-24"),
            "結束前一毫秒還在當天"
        );
    }
}
