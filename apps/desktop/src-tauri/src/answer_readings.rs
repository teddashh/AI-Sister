use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Serialize)]
pub(crate) struct BrainAnswer {
    pub(crate) state: &'static str,
    pub(crate) provider: Option<String>,
}

impl BrainAnswer {
    pub(crate) fn new(state: &'static str, provider: Option<String>) -> Self {
        Self { state, provider }
    }

    pub(crate) fn not_configured() -> Self {
        Self::new("not_configured", None)
    }
}

// 只決定是否要算盲點；畫面先開口的措辭由 renderHits 自己決定。
// 收切片與同一顆 brain，避免另傳一組容易接反的布林。
pub(crate) fn needs_answer_blind_spots<F, H>(
    facts: &[F],
    hits: &[H],
    readings: &[sister_core::grounded_answer::Reading],
    brain: &BrainAnswer,
) -> bool {
    facts.is_empty()
        && hits.is_empty()
        && !readings.iter().any(|r| r.matched)
        && brain.state != "thinking"
}

// 題庫計的是畫面給了幾筆答案，時間背景不算一次命中。
pub(crate) fn answer_hit_count<F, H>(
    facts: &[F],
    hits: &[H],
    readings: &[sister_core::grounded_answer::Reading],
) -> usize {
    facts.len() + hits.len() + readings.iter().filter(|r| r.matched).count()
}

#[cfg(test)]
mod answer_blind_count_tests {
    use super::*;

    #[test]
    fn blind_counts_only_run_for_empty_terminal_answers() {
        for state in ["thinking", "not_configured", "search_failed", "no_sources"] {
            for (facts, hits) in [
                (vec![], vec![]),
                (vec![1], vec![]),
                (vec![], vec![1]),
                (vec![1], vec![1]),
            ] {
                let brain = BrainAnswer::new(state, None);
                let mut count_calls = 0;
                if needs_answer_blind_spots(&facts, &hits, &[], &brain) {
                    count_calls += 1;
                }
                let expected = match (state, facts.len(), hits.len()) {
                    ("thinking", _, _) => 0,
                    (_, 0, 0) => 1,
                    _ => 0,
                };
                assert_eq!(
                    count_calls,
                    expected,
                    "COUNT 呼叫次數：state={state}, facts={}, hits={}",
                    facts.len(),
                    hits.len()
                );
            }
        }
    }
}

/// 時間背景只供出處回查；只有被問題打中的卡片帶文字。
/// 本機快答還沒有模型正文，因此這些文字本身就是要呈現的答案。
#[derive(Debug, Serialize)]
pub(crate) struct Reading {
    pub(crate) card_id: i64,
    pub(crate) frame_id: Option<i64>,
    /// 只有內容命中才有值，與 core matched 恰好相等。
    pub(crate) activity: Option<String>,
    /// 卡片所屬段落開始時間，用來標示直接呈現的判讀。
    pub(crate) at: i64,
}

impl Reading {
    pub(crate) fn from_core(
        r: &sister_core::grounded_answer::Reading,
        openable: &HashSet<i64>,
    ) -> Self {
        Self {
            card_id: r.card_id,
            frame_id: r.frame_id.filter(|id| openable.contains(id)),
            activity: r.matched.then(|| r.activity.clone()),
            at: r.at,
        }
    }
}

#[cfg(test)]
mod matched_tests {
    use super::*;
    fn core_reading(matched: bool, activity: &str) -> sister_core::grounded_answer::Reading {
        sister_core::grounded_answer::Reading {
            card_id: 7,
            matched,
            at: 100,
            activity: activity.into(),
            frame_id: Some(42),
        }
    }

    #[test]
    fn query_log_counts_visible_card_answers_only() {
        let background = core_reading(false, "背景");
        let matched = core_reading(true, "退款");
        assert_eq!(answer_hit_count::<i32, i32>(&[], &[], &[]), 0);
        assert_eq!(
            answer_hit_count::<i32, i32>(&[], &[], std::slice::from_ref(&background)),
            0
        );
        assert_eq!(
            answer_hit_count::<i32, i32>(&[], &[], std::slice::from_ref(&matched)),
            1
        );
        assert_eq!(answer_hit_count(&[1, 2], &[3], &[background, matched]), 4);
    }

    #[test]
    fn dto_activity_is_some_exactly_when_matched() {
        for matched in [false, true] {
            for activity in ["退款申請", ""] {
                for openable in [HashSet::new(), HashSet::from([42])] {
                    let core = core_reading(matched, activity);
                    let dto = Reading::from_core(&core, &openable);
                    assert_eq!(dto.activity.is_some(), core.matched);
                    assert_eq!(dto.activity.as_deref(), matched.then_some(activity));
                    assert_eq!(dto.card_id, core.card_id);
                    assert_eq!(dto.at, core.at);
                    assert_eq!(dto.frame_id, openable.contains(&42).then_some(42));
                }
            }
        }
    }

    #[test]
    fn blind_counts_include_content_dimension_exhaustively() {
        for state in ["thinking", "not_configured", "search_failed", "no_sources"] {
            for facts in [vec![], vec![1]] {
                for hits in [vec![], vec![1]] {
                    for readings in [
                        vec![],
                        vec![core_reading(false, "背景")],
                        vec![core_reading(true, "退款")],
                    ] {
                        let actual = needs_answer_blind_spots(
                            &facts,
                            &hits,
                            &readings,
                            &BrainAnswer::new(state, None),
                        );
                        let expected = match (
                            state,
                            facts.len(),
                            hits.len(),
                            readings.first().map(|r| r.matched),
                        ) {
                            ("thinking", _, _, _) | (_, _, _, Some(true)) => false,
                            (_, 0, 0, _) => true,
                            _ => false,
                        };
                        assert_eq!(
                            actual, expected,
                            "{state} facts={facts:?} hits={hits:?} readings={readings:?}"
                        );
                    }
                }
            }
        }
    }
}
