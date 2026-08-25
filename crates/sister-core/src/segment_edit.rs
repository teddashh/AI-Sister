//! 使用者對段落的合併／切開。
//!
//! 演算法每次打開時間軸都會把 `segment` 砍掉重算，所以編輯不能寫進那張表。
//! 這裡是 append-only 的動作紀錄：重算之後再依時間順序套上去。
//! 同一份紀錄也是 SPEC §4.3 的訓練訊號——記的是邊界與結構，不是畫面文字。

use crate::model::Millis;
use crate::segment::{CutKind, EventRefs, OVERLAP_MARGIN_MS, Segment};

/// 使用者做過的那一種動作。字串是存進資料庫的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Merge,
    Split,
}

impl EditKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Split => "split",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        match s {
            "merge" => Some(Self::Merge),
            "split" => Some(Self::Split),
            _ => None,
        }
    }
}

/// 套在一段上、指出它此刻的形狀從哪一次編輯來。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedEdit {
    pub id: i64,
    pub kind: EditKind,
}

/// 資料庫裡的一列，含撤銷。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEdit {
    pub id: i64,
    pub ts: Millis,
    /// `"merge"` / `"split"` / `"undo"`。
    pub kind: String,
    /// 合併：被拿掉的那道邊界（右段的 `core_started_at`）。切開：切點。
    pub at_ms: Option<Millis>,
    /// 這次動作碰到的核心範圍。forget 用它判斷重疊。
    pub from_ms: Option<Millis>,
    pub to_ms: Option<Millis>,
    /// 當時演算法在 `at_ms` 有沒有切。沒有就是 `None`，不是空陣列。
    pub algo_cut_kinds: Option<Vec<CutKind>>,
    pub algo_confidence: Option<f32>,
    /// 撤銷：指向被撤的那一列。
    pub target_id: Option<i64>,
}

/// 還沒被撤銷的合併／切開，依 id 舊到新。
pub fn active_edits(edits: &[StoredEdit]) -> Vec<&StoredEdit> {
    let undone: std::collections::HashSet<i64> = edits
        .iter()
        .filter(|e| e.kind == "undo")
        .filter_map(|e| e.target_id)
        .collect();
    edits
        .iter()
        .filter(|e| e.kind != "undo" && !undone.contains(&e.id))
        .collect()
}

/// 把編輯依序套到演算法剛切好的段落上。套不上的那一筆就跳過，不發明一段。
pub fn apply_edits(mut segs: Vec<Segment>, edits: &[StoredEdit]) -> Vec<Segment> {
    for e in active_edits(edits) {
        match e.kind.as_str() {
            "merge" => {
                if let Some(at) = e.at_ms {
                    segs = apply_merge(segs, at, e.id);
                }
            }
            "split" => {
                if let Some(at) = e.at_ms {
                    segs = apply_split(segs, at, e.id);
                }
            }
            _ => {}
        }
    }
    segs
}

fn apply_merge(mut segs: Vec<Segment>, at: Millis, edit_id: i64) -> Vec<Segment> {
    let Some(i) = segs.iter().position(|s| s.core_started_at == at) else {
        return segs;
    };
    if i == 0 {
        return segs;
    }
    let right = segs.remove(i);
    let left = segs.remove(i - 1);
    segs.insert(
        i - 1,
        merge_two(
            &left,
            &right,
            AppliedEdit {
                id: edit_id,
                kind: EditKind::Merge,
            },
        ),
    );
    segs
}

fn apply_split(mut segs: Vec<Segment>, at: Millis, edit_id: i64) -> Vec<Segment> {
    let Some(i) = segs
        .iter()
        .position(|s| s.core_started_at < at && at < s.core_ended_at)
    else {
        return segs;
    };
    let orig = segs.remove(i);
    let edit = AppliedEdit {
        id: edit_id,
        kind: EditKind::Split,
    };
    let (left, right) = split_one(&orig, at, edit);
    segs.insert(i, left);
    segs.insert(i + 1, right);
    segs
}

fn merge_two(left: &Segment, right: &Segment, edit: AppliedEdit) -> Segment {
    let left_dur = left.core_ended_at.saturating_sub(left.core_started_at);
    let right_dur = right.core_ended_at.saturating_sub(right.core_started_at);
    let take_right = right_dur > left_dur;
    Segment {
        started_at: left.started_at,
        ended_at: right.ended_at,
        core_started_at: left.core_started_at,
        core_ended_at: right.core_ended_at,
        app: if take_right {
            right.app.clone()
        } else {
            left.app.clone()
        },
        title: if take_right {
            right.title.clone()
        } else {
            left.title.clone()
        },
        host: if take_right {
            right.host.clone()
        } else {
            left.host.clone()
        },
        cut_kinds: left.cut_kinds.clone(),
        confidence: left.confidence,
        event_ids: merge_refs(&left.event_ids, &right.event_ids),
        last_edit: Some(edit),
    }
}

fn split_one(seg: &Segment, at: Millis, edit: AppliedEdit) -> (Segment, Segment) {
    let mut left = seg.clone();
    left.core_ended_at = at;
    left.ended_at = at.saturating_add(OVERLAP_MARGIN_MS);
    left.last_edit = Some(edit);

    let mut right = seg.clone();
    right.core_started_at = at;
    right.started_at = at.saturating_sub(OVERLAP_MARGIN_MS);
    right.cut_kinds = Vec::new();
    right.confidence = None;
    right.last_edit = Some(edit);
    (left, right)
}

fn merge_refs(a: &EventRefs, b: &EventRefs) -> EventRefs {
    EventRefs {
        focus: union_ids(&a.focus, &b.focus),
        system: union_ids(&a.system, &b.system),
        clipboard: union_ids(&a.clipboard, &b.clipboard),
        input: union_ids(&a.input, &b.input),
    }
}

fn union_ids(a: &[i64], b: &[i64]) -> Vec<i64> {
    let mut out = a.to_vec();
    out.extend_from_slice(b);
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::CutKind;

    fn seg(core_start: Millis, core_end: Millis, app: &str) -> Segment {
        Segment {
            started_at: core_start.saturating_sub(OVERLAP_MARGIN_MS),
            ended_at: core_end.saturating_add(OVERLAP_MARGIN_MS),
            core_started_at: core_start,
            core_ended_at: core_end,
            app: Some(app.into()),
            title: Some(app.into()),
            host: None,
            cut_kinds: if core_start == 0 {
                Vec::new()
            } else {
                vec![CutKind::AppChange]
            },
            confidence: if core_start == 0 { None } else { Some(0.5) },
            event_ids: EventRefs::default(),
            last_edit: None,
        }
    }

    fn merge_at(id: i64, at: Millis, from: Millis, to: Millis) -> StoredEdit {
        StoredEdit {
            id,
            ts: 1_000 + id,
            kind: "merge".into(),
            at_ms: Some(at),
            from_ms: Some(from),
            to_ms: Some(to),
            algo_cut_kinds: Some(vec![CutKind::AppChange]),
            algo_confidence: Some(0.5),
            target_id: None,
        }
    }

    fn split_at(id: i64, at: Millis, from: Millis, to: Millis) -> StoredEdit {
        StoredEdit {
            id,
            ts: 1_000 + id,
            kind: "split".into(),
            at_ms: Some(at),
            from_ms: Some(from),
            to_ms: Some(to),
            algo_cut_kinds: None,
            algo_confidence: None,
            target_id: None,
        }
    }

    fn undo(id: i64, target: i64) -> StoredEdit {
        StoredEdit {
            id,
            ts: 1_000 + id,
            kind: "undo".into(),
            at_ms: None,
            from_ms: None,
            to_ms: None,
            algo_cut_kinds: None,
            algo_confidence: None,
            target_id: Some(target),
        }
    }

    fn apps(segs: &[Segment]) -> Vec<Option<&str>> {
        segs.iter().map(|s| s.app.as_deref()).collect()
    }

    fn cores(segs: &[Segment]) -> Vec<(Millis, Millis)> {
        segs.iter()
            .map(|s| (s.core_started_at, s.core_ended_at))
            .collect()
    }

    #[test]
    fn merge_joins_two_adjacent_segments() {
        let segs = vec![seg(0, 60_000, "a"), seg(60_000, 120_000, "b")];
        let out = apply_edits(segs, &[merge_at(1, 60_000, 0, 120_000)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].core_started_at, 0);
        assert_eq!(out[0].core_ended_at, 120_000);
        assert_eq!(out[0].last_edit.unwrap().kind, EditKind::Merge);
        // 左邊比較長（並列時取左）：兩段一樣長，取左。
        assert_eq!(out[0].app.as_deref(), Some("a"));
    }

    #[test]
    fn split_in_the_middle_makes_two() {
        let segs = vec![seg(0, 120_000, "a")];
        let out = apply_edits(segs, &[split_at(1, 50_000, 0, 120_000)]);
        assert_eq!(cores(&out), vec![(0, 50_000), (50_000, 120_000)]);
        assert!(out[0].last_edit.is_some());
        assert_eq!(out[1].cut_kinds, Vec::new());
        assert!(
            out[1].confidence.is_none(),
            "人切開的邊界不是演算法的切刀，不該有演算法的信心值"
        );
    }

    #[test]
    fn merge_then_split_in_the_merged_range() {
        let segs = vec![
            seg(0, 60_000, "a"),
            seg(60_000, 120_000, "b"),
            seg(120_000, 180_000, "c"),
        ];
        let edits = [
            merge_at(1, 60_000, 0, 120_000),
            split_at(2, 90_000, 0, 120_000),
        ];
        let out = apply_edits(segs, &edits);
        assert_eq!(
            cores(&out),
            vec![(0, 90_000), (90_000, 120_000), (120_000, 180_000)]
        );
        assert_eq!(out[0].last_edit.unwrap().kind, EditKind::Split);
        assert_eq!(out[1].last_edit.unwrap().kind, EditKind::Split);
        assert!(out[2].last_edit.is_none(), "沒碰到的那一段不該被標成改過");
    }

    #[test]
    fn split_inside_already_merged_range() {
        let segs = vec![seg(0, 60_000, "a"), seg(60_000, 180_000, "b")];
        let edits = [
            merge_at(1, 60_000, 0, 180_000),
            split_at(2, 100_000, 0, 180_000),
        ];
        let out = apply_edits(segs, &edits);
        assert_eq!(cores(&out), vec![(0, 100_000), (100_000, 180_000)]);
    }

    #[test]
    fn split_then_merge_back_restores_one() {
        let segs = vec![seg(0, 120_000, "a")];
        let edits = [
            split_at(1, 40_000, 0, 120_000),
            merge_at(2, 40_000, 0, 120_000),
        ];
        let out = apply_edits(segs, &edits);
        assert_eq!(cores(&out), vec![(0, 120_000)]);
        assert_eq!(out[0].last_edit.unwrap().kind, EditKind::Merge);
    }

    #[test]
    fn repeated_edits_apply_in_order() {
        let segs = vec![
            seg(0, 60_000, "a"),
            seg(60_000, 120_000, "b"),
            seg(120_000, 180_000, "c"),
        ];
        let edits = [
            merge_at(1, 60_000, 0, 120_000),
            merge_at(2, 120_000, 0, 180_000),
        ];
        let out = apply_edits(segs, &edits);
        assert_eq!(out.len(), 1);
        assert_eq!(cores(&out), vec![(0, 180_000)]);
        assert_eq!(out[0].last_edit.unwrap().id, 2);
    }

    #[test]
    fn undo_skips_that_edit_on_replay() {
        let segs = vec![seg(0, 60_000, "a"), seg(60_000, 120_000, "b")];
        let edits = [merge_at(1, 60_000, 0, 120_000), undo(2, 1)];
        let out = apply_edits(segs, &edits);
        assert_eq!(apps(&out), vec![Some("a"), Some("b")]);
        assert!(out.iter().all(|s| s.last_edit.is_none()));
    }

    #[test]
    fn merge_at_a_missing_boundary_is_a_noop() {
        let segs = vec![seg(0, 120_000, "a")];
        let out = apply_edits(segs.clone(), &[merge_at(1, 60_000, 0, 120_000)]);
        assert_eq!(cores(&out), cores(&segs));
        assert!(out[0].last_edit.is_none());
    }

    #[test]
    fn split_on_an_existing_boundary_is_a_noop() {
        let segs = vec![seg(0, 60_000, "a"), seg(60_000, 120_000, "b")];
        let out = apply_edits(segs.clone(), &[split_at(1, 60_000, 0, 120_000)]);
        assert_eq!(cores(&out), cores(&segs));
    }

    #[test]
    fn split_at_the_start_or_end_is_a_noop() {
        let segs = vec![seg(0, 120_000, "a")];
        let at_start = apply_edits(segs.clone(), &[split_at(1, 0, 0, 120_000)]);
        let at_end = apply_edits(segs.clone(), &[split_at(2, 120_000, 0, 120_000)]);
        assert_eq!(cores(&at_start), cores(&segs));
        assert_eq!(cores(&at_end), cores(&segs));
    }

    #[test]
    fn training_fields_remember_the_algorithm_cut() {
        let e = merge_at(1, 60_000, 0, 120_000);
        assert_eq!(e.kind, "merge");
        assert_eq!(e.at_ms, Some(60_000));
        assert_eq!(e.algo_cut_kinds.as_deref(), Some(&[CutKind::AppChange][..]));
        assert_eq!(e.algo_confidence, Some(0.5));
        assert!(e.ts > 0);
    }
}
