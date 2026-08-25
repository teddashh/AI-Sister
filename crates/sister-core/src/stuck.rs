//! 卡住偵測 v0：停留 + 反覆切換 + error 事實共現。
//!
//! 只算、只記，不開口、不提醒。三個成分各自是 `Option`：沒量到就是
//! `None`，不用 0 冒充。三個都量到、而且都過門檻，才算卡住。
//!
//! 門檻是實作選擇，不是規格常數。SPEC §5.1 只說「同視窗長停留 + 反覆
//! 小幅切換」；PHASES.md Phase 3 加上 error 事實共現。

use crate::model::Millis;
use crate::segment::Segment;

/// 停留至少這麼久才叫「長停留」。3 分鐘是實作選擇。
pub const STUCK_DWELL_MS: Millis = 3 * 60 * 1_000;
/// 這段期間至少這麼多次切換才叫「反覆」。6 是實作選擇。
pub const STUCK_SWITCH_MIN: i64 = 6;

/// 一段有 input_metrics 覆蓋的時間。沒有列就不是這份輸入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputSpan {
    pub ts_start: Millis,
    pub ts_end: Millis,
    pub window_switches: i64,
}

/// 一筆 `FactKind::ErrorCode`。`app` 對不上就不算「同窗口」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorHit {
    pub ts: Millis,
    pub app: Option<String>,
}

/// 三個成分都量過、而且一起成立的一次卡住。
#[derive(Debug, Clone, PartialEq)]
pub struct StuckSignal {
    pub started_at: Millis,
    pub ended_at: Millis,
    pub app: Option<String>,
    pub title: Option<String>,
    pub dwell_ms: Millis,
    pub switch_count: i64,
    pub error_fact_count: i64,
}

/// 對一段活動的三次量測。缺的那種是 `None`，不是 0。
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub started_at: Millis,
    pub ended_at: Millis,
    pub app: Option<String>,
    pub title: Option<String>,
    pub dwell_ms: Option<Millis>,
    pub switch_count: Option<i64>,
    pub error_fact_count: Option<i64>,
}

impl Observation {
    pub fn is_stuck(&self) -> bool {
        match (self.dwell_ms, self.switch_count, self.error_fact_count) {
            (Some(d), Some(s), Some(e)) => d >= STUCK_DWELL_MS && s >= STUCK_SWITCH_MIN && e > 0,
            _ => false,
        }
    }
}

/// 以演算法切出來的段落當活動單位（未套用使用者編輯）。
///
/// 用段落而不是「連續同一 app 的焦點」：切換後 30 秒內折返會被黏合，
/// 那段「盯著同一件事、中間翻去別的窗口」才是卡住要抓的形狀。
pub fn observe_segments(
    segs: &[Segment],
    inputs: &[InputSpan],
    errors: &[ErrorHit],
) -> Vec<Observation> {
    segs.iter()
        .filter(|s| s.core_ended_at > s.core_started_at)
        .map(|s| observe_one(s, inputs, errors))
        .collect()
}

pub fn detect(segs: &[Segment], inputs: &[InputSpan], errors: &[ErrorHit]) -> Vec<StuckSignal> {
    observe_segments(segs, inputs, errors)
        .into_iter()
        .filter_map(|o| {
            let (Some(dwell_ms), Some(switch_count), Some(error_fact_count)) =
                (o.dwell_ms, o.switch_count, o.error_fact_count)
            else {
                return None;
            };
            if dwell_ms < STUCK_DWELL_MS || switch_count < STUCK_SWITCH_MIN || error_fact_count == 0
            {
                return None;
            }
            Some(StuckSignal {
                started_at: o.started_at,
                ended_at: o.ended_at,
                app: o.app,
                title: o.title,
                dwell_ms,
                switch_count,
                error_fact_count,
            })
        })
        .collect()
}

fn observe_one(seg: &Segment, inputs: &[InputSpan], errors: &[ErrorHit]) -> Observation {
    let from = seg.core_started_at;
    let to = seg.core_ended_at;
    Observation {
        started_at: from,
        ended_at: to,
        app: seg.app.clone(),
        title: seg.title.clone(),
        dwell_ms: Some(to.saturating_sub(from)),
        switch_count: switches_in(inputs, from, to),
        error_fact_count: errors_in(errors, from, to, seg.app.as_deref()),
    }
}

/// 沒有任何 input_metrics 列蓋到這段 → `None`。有列而加總是 0 → `Some(0)`。
fn switches_in(inputs: &[InputSpan], from: Millis, to: Millis) -> Option<i64> {
    let mut any = false;
    let mut sum: i64 = 0;
    for m in inputs {
        if m.ts_end > from && m.ts_start < to {
            any = true;
            sum = sum.saturating_add(m.window_switches.max(0));
        }
    }
    if any { Some(sum) } else { None }
}

/// 窗口認不出來（沒有 app）→ `None`，因為「同窗口」沒量到。
/// 認得出來、一個 error 都沒有 → `Some(0)`。
fn errors_in(errors: &[ErrorHit], from: Millis, to: Millis, app: Option<&str>) -> Option<i64> {
    let app = app.filter(|s| !s.is_empty())?;
    let n = errors
        .iter()
        .filter(|e| e.ts >= from && e.ts < to)
        .filter(|e| {
            e.app
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(app))
        })
        .count();
    Some(n as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::EventRefs;

    fn seg(start: Millis, end: Millis, app: Option<&str>) -> Segment {
        Segment {
            started_at: start,
            ended_at: end,
            core_started_at: start,
            core_ended_at: end,
            app: app.map(|s| s.into()),
            title: app.map(|s| s.into()),
            host: None,
            cut_kinds: Vec::new(),
            confidence: None,
            event_ids: EventRefs::default(),
            last_edit: None,
        }
    }

    fn input(start: Millis, end: Millis, switches: i64) -> InputSpan {
        InputSpan {
            ts_start: start,
            ts_end: end,
            window_switches: switches,
        }
    }

    fn err_at(ts: Millis, app: &str) -> ErrorHit {
        ErrorHit {
            ts,
            app: Some(app.into()),
        }
    }

    #[test]
    fn all_three_measured_and_over_threshold_is_stuck() {
        let segs = [seg(0, STUCK_DWELL_MS + 1_000, Some("code.exe"))];
        let inputs = [input(0, STUCK_DWELL_MS, STUCK_SWITCH_MIN)];
        let errors = [err_at(30_000, "code.exe")];
        let found = detect(&segs, &inputs, &errors);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].switch_count, STUCK_SWITCH_MIN);
        assert_eq!(found[0].error_fact_count, 1);
    }

    #[test]
    fn missing_input_metrics_is_not_zero_and_not_stuck() {
        let segs = [seg(0, STUCK_DWELL_MS + 1_000, Some("code.exe"))];
        let errors = [err_at(30_000, "code.exe")];
        let obs = observe_segments(&segs, &[], &errors);
        assert_eq!(obs[0].switch_count, None, "沒量到切換，不可以寫 0");
        assert!(!obs[0].is_stuck());
        assert!(detect(&segs, &[], &errors).is_empty());
    }

    #[test]
    fn measured_zero_switches_is_zero_and_not_stuck() {
        let segs = [seg(0, STUCK_DWELL_MS + 1_000, Some("code.exe"))];
        let inputs = [input(0, 10_000, 0)];
        let errors = [err_at(30_000, "code.exe")];
        let obs = observe_segments(&segs, &inputs, &errors);
        assert_eq!(obs[0].switch_count, Some(0), "量到 0 次切換，那就是 0");
        assert!(!obs[0].is_stuck());
    }

    #[test]
    fn measured_zero_errors_is_zero_and_not_stuck() {
        let segs = [seg(0, STUCK_DWELL_MS + 1_000, Some("code.exe"))];
        let inputs = [input(0, 10_000, STUCK_SWITCH_MIN)];
        let obs = observe_segments(&segs, &inputs, &[]);
        assert_eq!(obs[0].error_fact_count, Some(0));
        assert!(!obs[0].is_stuck());
    }

    #[test]
    fn no_app_means_same_window_was_not_measured() {
        let segs = [seg(0, STUCK_DWELL_MS + 1_000, None)];
        let inputs = [input(0, 10_000, STUCK_SWITCH_MIN)];
        let errors = [err_at(30_000, "code.exe")];
        let obs = observe_segments(&segs, &inputs, &errors);
        assert_eq!(
            obs[0].error_fact_count, None,
            "窗口認不出來，同窗口的 error 沒量到"
        );
        assert!(!obs[0].is_stuck());
    }

    #[test]
    fn error_on_a_different_app_does_not_count() {
        let segs = [seg(0, STUCK_DWELL_MS + 1_000, Some("code.exe"))];
        let inputs = [input(0, 10_000, STUCK_SWITCH_MIN)];
        let errors = [err_at(30_000, "chrome.exe")];
        let obs = observe_segments(&segs, &inputs, &errors);
        assert_eq!(obs[0].error_fact_count, Some(0));
        assert!(!obs[0].is_stuck());
    }

    #[test]
    fn short_dwell_is_not_stuck_even_with_switches_and_errors() {
        let segs = [seg(0, 30_000, Some("code.exe"))];
        let inputs = [input(0, 10_000, STUCK_SWITCH_MIN)];
        let errors = [err_at(1_000, "code.exe")];
        assert!(detect(&segs, &inputs, &errors).is_empty());
    }
}
