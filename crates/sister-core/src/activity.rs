//! 活動級章節：把分鐘級 `segment` 聚成「一件事」。
//!
//! 這是 SPEC §4.2 兩級結構的**程式版前身**。§4.2 把小時級叫做 `session`，
//! 由 §6 Reviewer 來做——而 §6 是 LLM 層（mid tier）。Phase 3 明寫「純程式，
//! 仍然零 LLM」，所以這一版不呼叫任何模型。
//!
//! **不能叫 `session`。** `db.rs` 的 `sessions` 表是一次錄製（`app_version`／
//! `platform`／`note`），跟 §4.2 的 session 是完全不同的東西。這裡的
//! [`Activity`] 對應 §4.2 那一級（跨 app 工作集、小時級的「你今天的一天」
//! 的章節），只是程式用分鐘級 segment 的切刀把它拼回來。
//!
//! 聚合規則：相鄰、且後一段只被 `time_cap` 打開 → 同一件事。
//! §4.1 的工作集黏合已經在 `segment` 裡做過（壓得掉 app／host 變更，壓不掉
//! 10 分鐘上限），所以這一刀把安全閥切碎的同質段——含 terminal + browser +
//! editor 的工作集——併回去，不必另寫一套工作集判斷。
//!
//! 底下的 `segment` 一列都不准動：它們是 §4.3 的訓練訊號，也是時間軸上
//! 手動合併／切開的對象。這裡只產出一個答案端看的視圖。

use crate::model::Millis;
use crate::segment::Segment;

/// 一件活動。時長看 [`Self::core_ms`]，不要拿 `started_at`／`ended_at` 相減。
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// 第一段的顯示起點（含 5 秒 margin）。答案裡的鐘面不看這個。
    pub started_at: Millis,
    /// 最後一段的顯示迄點（含 5 秒 margin）。
    pub ended_at: Millis,
    pub core_started_at: Millis,
    pub core_ended_at: Millis,
    /// 這件事裡核心時間最長的那一個視窗。就是 app／title，不是一句解釋。
    pub app: Option<String>,
    pub title: Option<String>,
    pub host: Option<String>,
    /// 由幾個分鐘級 `segment` 併成。1 = 沒併過。
    ///
    /// 答案若要講，講這個數字。不要把「5 個 10 分鐘的 segment」講成她判斷出
    /// 他專注了 50 分鐘——那 50 是安全閥切出來的份數乘上去的。
    pub segment_count: usize,
}

impl Activity {
    /// 核心時長。相鄰 segment 的 5 秒 margin 重疊不計入。
    pub fn core_ms(&self) -> Millis {
        self.core_ended_at.saturating_sub(self.core_started_at)
    }
}

/// 把一段連續的 `segment` 聚成活動。空輸入是空輸出，不是一段假的全天。
pub fn group(segments: &[Segment]) -> Vec<Activity> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (i, next) in segments.iter().enumerate() {
        if let Some((_, to)) = ranges.last_mut() {
            let prev = &segments[*to - 1];
            if continues(prev, next) {
                *to = i + 1;
                continue;
            }
        }
        ranges.push((i, i + 1));
    }
    ranges
        .into_iter()
        .filter_map(|(from, to)| from_segments(&segments[from..to]))
        .collect()
}

/// 後一段只被 10 分鐘上限打開、且核心時間跟前一段相接（或重疊）→ 同一件事。
///
/// 有缺口（中間被忘掉、或人手切開留下的空切刀）就不併。Idle／鎖定／app 變更
/// 都不是 `time_cap`，保持獨立。
fn continues(prev: &Segment, next: &Segment) -> bool {
    next.core_started_at <= prev.core_ended_at && next.opened_only_by_time_cap()
}

fn from_segments(segs: &[Segment]) -> Option<Activity> {
    let first = segs.first()?;
    let last = segs.last()?;
    let (app, title, host) = representative_of(segs);
    Some(Activity {
        started_at: first.started_at,
        ended_at: last.ended_at,
        core_started_at: first.core_started_at,
        core_ended_at: last.core_ended_at,
        app,
        title,
        host,
        segment_count: segs.len(),
    })
}

struct Dwell {
    app: Option<String>,
    title: Option<String>,
    host: Option<String>,
    ms: Millis,
}

/// 核心時間加總最長的那一個 (app, title, host)。平手取先出現的。
fn representative_of(segs: &[Segment]) -> (Option<String>, Option<String>, Option<String>) {
    let mut totals: Vec<Dwell> = Vec::new();
    for s in segs {
        let dur = s.core_ms();
        if let Some(slot) = totals
            .iter_mut()
            .find(|d| d.app == s.app && d.title == s.title && d.host == s.host)
        {
            slot.ms = slot.ms.saturating_add(dur);
        } else {
            totals.push(Dwell {
                app: s.app.clone(),
                title: s.title.clone(),
                host: s.host.clone(),
                ms: dur,
            });
        }
    }
    if totals.is_empty() {
        return (None, None, None);
    }
    let mut best = 0;
    for i in 1..totals.len() {
        if totals[i].ms > totals[best].ms {
            best = i;
        }
    }
    let Dwell {
        app, title, host, ..
    } = totals.swap_remove(best);
    (app, title, host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SystemKind;
    use crate::segment::{
        CutKind, EventRefs, EventStream, FocusPoint, InputPoint, OVERLAP_MARGIN_MS, SystemPoint,
        TIME_CAP_MS, segment,
    };

    fn focus(id: i64, ts: Millis, app: &str, title: &str, url: Option<&str>) -> FocusPoint {
        FocusPoint {
            id,
            ts,
            app_id: Some(app.into()),
            app_name: None,
            window_title: Some(title.into()),
            url: url.map(|u| u.into()),
        }
    }

    fn seg(
        core_start: Millis,
        core_end: Millis,
        app: &str,
        title: &str,
        kinds: Vec<CutKind>,
    ) -> Segment {
        Segment {
            started_at: core_start.saturating_sub(OVERLAP_MARGIN_MS),
            ended_at: core_end.saturating_add(OVERLAP_MARGIN_MS),
            core_started_at: core_start,
            core_ended_at: core_end,
            app: Some(app.into()),
            title: Some(title.into()),
            host: None,
            cut_kinds: kinds,
            confidence: None,
            event_ids: EventRefs::default(),
            last_edit: None,
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(group(&[]).is_empty());
    }

    #[test]
    fn time_capped_stay_groups_back_to_one_activity() {
        // 45 分鐘同一視窗：安全閥切成 5 段，答案要拼回一件事。
        let mut segs = Vec::new();
        for i in 0..4 {
            let start = i * TIME_CAP_MS;
            segs.push(seg(
                start,
                start + TIME_CAP_MS,
                "code.exe",
                "db.rs",
                if i == 0 {
                    Vec::new()
                } else {
                    vec![CutKind::TimeCap]
                },
            ));
        }
        segs.push(seg(
            4 * TIME_CAP_MS,
            4 * TIME_CAP_MS + 5 * 60_000,
            "code.exe",
            "db.rs",
            vec![CutKind::TimeCap],
        ));
        assert_eq!(segs.len(), 5);

        let acts = group(&segs);
        assert_eq!(acts.len(), 1, "time_cap 切碎的同質段該併回一件事");
        assert_eq!(acts[0].segment_count, 5);
        assert_eq!(acts[0].core_ms(), 45 * 60_000);
        assert_eq!(acts[0].app.as_deref(), Some("code.exe"));

        let naive: Millis = segs.iter().map(|s| s.ended_at - s.started_at).sum();
        assert_eq!(
            naive,
            45 * 60_000 + 5 * 2 * OVERLAP_MARGIN_MS,
            "把含 margin 的起迄相加會把邊界算五次"
        );
        assert!(
            naive > acts[0].core_ms(),
            "核心時長必須短過 margin 相加，否則測試沒釘到那條錯"
        );
        assert_ne!(naive, 50 * 60_000); // 也不是「5 × 10 分鐘」
    }

    #[test]
    fn afternoon_three_stays_are_three_activities_not_thirteen() {
        // 真語料的形狀：寫程式 45、查文件 25、寫週報 45。
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs — AI-Sister", None),
                focus(
                    2,
                    45 * 60_000,
                    "chrome.exe",
                    "SQLite user_version 文件",
                    Some("https://sqlite.org/pragma.html"),
                ),
                focus(3, 70 * 60_000, "notion.exe", "週報", None),
                focus(4, 115 * 60_000, "notion.exe", "週報", None),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(
            segs.len(),
            13,
            "分鐘級仍是 13 段，安全閥沒關掉：{}",
            segs.len()
        );
        let acts = group(&segs);
        assert_eq!(acts.len(), 3, "答案該是 3 件事，得到 {}", acts.len());
        assert_eq!(
            acts.iter().map(|a| a.segment_count).collect::<Vec<_>>(),
            vec![5, 3, 5]
        );
        assert_eq!(acts[0].core_ms(), 45 * 60_000);
        assert_eq!(acts[1].core_ms(), 25 * 60_000);
        assert_eq!(acts[2].core_ms(), 45 * 60_000);
        assert_eq!(acts[0].app.as_deref(), Some("code.exe"));
        assert_eq!(acts[1].app.as_deref(), Some("chrome.exe"));
        assert_eq!(acts[2].app.as_deref(), Some("notion.exe"));

        let naive: Millis = segs.iter().map(|s| s.ended_at - s.started_at).sum();
        let core: Millis = acts.iter().map(|a| a.core_ms()).sum();
        assert_eq!(core, 115 * 60_000);
        assert!(
            naive > core,
            "13 段的 margin 相加 ({naive}) 必須長過核心 ({core})"
        );
    }

    #[test]
    fn workset_time_caps_group_as_one_activity() {
        // 工作集黏合壓掉 app 變更，壓不掉 10 分鐘上限——聚合要把上限併回去。
        let mut focus_pts = Vec::new();
        let mut ts = 0;
        let mut id = 1;
        while ts <= 12 * 60_000 {
            focus_pts.push(focus(id, ts, "code.exe", "db.rs", None));
            id += 1;
            ts += 20_000;
            focus_pts.push(focus(id, ts, "wt.exe", "pwsh", None));
            id += 1;
            ts += 20_000;
        }
        let segs = segment(&EventStream {
            focus: focus_pts,
            ..EventStream::default()
        });
        assert!(
            segs.len() >= 2,
            "12 分鐘該被上限切成至少兩段，得到 {}",
            segs.len()
        );
        assert!(
            segs.iter().any(|s| s.opened_only_by_time_cap()),
            "後段該只被 time_cap 打開"
        );
        let acts = group(&segs);
        assert_eq!(
            acts.len(),
            1,
            "跨 app 工作集被上限切碎後仍是一件事，得到 {} 段 {:?}",
            acts.len(),
            acts.iter().map(|a| a.app.clone()).collect::<Vec<_>>()
        );
        assert_eq!(acts[0].segment_count, segs.len());
    }

    #[test]
    fn a_time_cap_segment_of_a_different_app_still_groups() {
        // 工作集在 10 分鐘窗口裡，代表性 app 可能換邊。只看切刀，不另判同 app。
        let segs = vec![
            seg(0, TIME_CAP_MS, "code.exe", "db.rs", Vec::new()),
            seg(
                TIME_CAP_MS,
                TIME_CAP_MS + 2 * 60_000,
                "wt.exe",
                "pwsh",
                vec![CutKind::TimeCap],
            ),
        ];
        let acts = group(&segs);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].app.as_deref(), Some("code.exe"), "核心較長的那邊");
        assert_eq!(acts[0].core_ms(), TIME_CAP_MS + 2 * 60_000);
    }

    #[test]
    fn app_change_stays_two_activities() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(
                    2,
                    60_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
                focus(
                    3,
                    90_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 2);
        assert_eq!(group(&segs).len(), 2, "前景 app 變更不是同一件事");
    }

    #[test]
    fn host_change_stays_two_activities() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "chrome.exe", "a", Some("https://github.com/x")),
                focus(2, 60_000, "chrome.exe", "b", Some("https://nhi.gov.tw/y")),
                focus(3, 90_000, "chrome.exe", "b", Some("https://nhi.gov.tw/y")),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 2);
        assert_eq!(group(&segs).len(), 2, "換站不是同一件事");
    }

    #[test]
    fn idle_resume_stays_two_activities() {
        let mut input = Vec::new();
        for i in 0..10 {
            let s = i * 10_000;
            input.push(InputPoint {
                id: i + 1,
                ts_start: s,
                ts_end: s + 10_000,
                idle_ms: 10_000,
            });
        }
        input.push(InputPoint {
            id: 11,
            ts_start: 100_000,
            ts_end: 110_000,
            idle_ms: 0,
        });
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 110_000, "code.exe", "db.rs", None),
            ],
            input,
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.iter()
                .any(|s| s.cut_kinds.contains(&CutKind::IdleResume))
        );
        assert!(
            group(&segs).len() >= 2,
            "idle > 90s 後恢復是另一件事，得到 {} 段",
            group(&segs).len()
        );
    }

    #[test]
    fn lock_stays_a_new_activity() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 200_000, "code.exe", "db.rs", None),
            ],
            system: vec![SystemPoint {
                id: 1,
                ts: 60_000,
                kind: SystemKind::Lock,
            }],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(group(&segs).len() >= 2, "鎖定是另一件事");
    }

    #[test]
    fn a_manual_split_is_not_glued_back() {
        // 人手切開的右半 `cut_kinds` 是空的，不是 time_cap。併回去會把
        // alpha.54 的編輯在答案裡默默撤掉。
        let segs = vec![
            seg(0, TIME_CAP_MS, "code.exe", "db.rs", Vec::new()),
            seg(
                TIME_CAP_MS,
                TIME_CAP_MS + 5 * 60_000,
                "code.exe",
                "db.rs",
                vec![CutKind::TimeCap],
            ),
            seg(
                TIME_CAP_MS + 5 * 60_000,
                2 * TIME_CAP_MS,
                "code.exe",
                "db.rs",
                Vec::new(),
            ),
            seg(
                2 * TIME_CAP_MS,
                3 * TIME_CAP_MS,
                "code.exe",
                "db.rs",
                vec![CutKind::TimeCap],
            ),
        ];
        let acts = group(&segs);
        assert_eq!(acts.len(), 2, "切開那一刀要留著，得到 {}", acts.len());
        assert_eq!(acts[0].core_ended_at, TIME_CAP_MS + 5 * 60_000);
        assert_eq!(acts[1].core_started_at, TIME_CAP_MS + 5 * 60_000);
    }

    #[test]
    fn a_gap_is_not_the_same_activity() {
        let segs = vec![
            seg(0, TIME_CAP_MS, "code.exe", "db.rs", Vec::new()),
            seg(
                TIME_CAP_MS + 60_000,
                2 * TIME_CAP_MS + 60_000,
                "code.exe",
                "db.rs",
                vec![CutKind::TimeCap],
            ),
        ];
        assert_eq!(group(&segs).len(), 2, "中間有缺口就不併");
    }

    #[test]
    fn time_cap_plus_app_change_is_a_new_activity() {
        let segs = vec![
            seg(0, TIME_CAP_MS, "code.exe", "db.rs", Vec::new()),
            seg(
                TIME_CAP_MS,
                2 * TIME_CAP_MS,
                "chrome.exe",
                "docs",
                vec![CutKind::AppChange, CutKind::TimeCap],
            ),
        ];
        assert_eq!(
            group(&segs).len(),
            2,
            "剛好落在 10 分鐘邊上的 app 變更仍是兩件事"
        );
    }
}
