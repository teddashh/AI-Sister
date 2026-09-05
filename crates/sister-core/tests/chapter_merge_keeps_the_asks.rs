//! r28 的驗收測試。**在 delegate 交貨之前寫好的。**
//!
//! 證的是「我事先寫下的考題過了」，不是「它的測試通過它自己的修法」。
//!
//! 病灶：使用者在時間軸上把兩個章節**合併**之後，「她試著問過 N 次」變小了，
//! 而 `brain_outbound` 一列都沒有被刪。因為那支查詢用起點**嚴格相等**去數，
//! 而 `merge_two` 讓右半的 `core_started_at` 消失（合併後的段落取左半的起點），
//! 於是右半那些外送紀錄的鍵再也對不上任何一段。
//!
//! 使用者讀到的那句話說「次數與結局只算**還留著的**外送紀錄」——把數字變小
//! 的原因說成「紀錄被刪掉了」。這一次一列都沒少，是鍵對不上了。
//!
//! ⚠ 這一檔在舊碼上**編不過**（舊簽名只吃一個參數），編不過的紅什麼都沒證明。
//!   有價值的是交貨**之後**的突變。五刀都跑過了，五刀都紅，箭頭後面是這一檔
//!   被點名的那一條：
//!
//!   1. 兩處 `WHERE` 都退回 `= ?1`（＝原本那個 bug）
//!      → `merging_two_chapters_keeps_both_halves_asks`
//!   2. 右界 `< ?2` 改成 `<= ?2`
//!      → `the_right_edge_belongs_to_the_next_chapter`
//!   3. `AND role = 'interpreter'` 改成 `AND 1 = 1`
//!      → `other_roles_are_still_not_counted`
//!   4. 只有子查詢退回相等（COUNT 仍是範圍）
//!      → `the_latest_outcome_comes_from_the_whole_range_not_the_start`
//!   5. `params![core_started_at, core_ended_at]` 兩個引數對調
//!      → `an_unedited_chapter_counts_exactly_what_it_did_before`
//!
//!   每一刀都確認過是 24 個 binary 全部跑完之後的紅，不是編不過的紅。
//!
//! ⚠ 這一檔守的是 `db.rs` 那支查詢，**不是使用者讀到的那句話**。
//!   `apps/desktop/src-tauri/src/main.rs` 那個呼叫端在另一個 workspace，
//!   `cargo test --workspace` 一行都執行不到它（而且這台機器上 `cargo check`
//!   對它會 panic，libdbus-sys 的 build script）。所以「合併之後畫面上真的
//!   變成 5 次」這件事，這裡沒有證據，要在真的 Windows 上按一遍才算數。

use sister_core::brain::{OutboundOutcome, StoredOutboundOutcome};
use sister_core::db::{Db, OutboundInsert};

const A: i64 = 10_000; // 左邊那一章的起點
const B: i64 = 20_000; // 右邊那一章的起點（合併之後這個鍵就沒有段落認領了）
const C: i64 = 30_000; // 合併後那一章的結束（半開區間，不含）

fn db() -> Db {
    Db::open_in_memory().expect("db")
}

/// 插一列外送紀錄。`ts` 和 `segment_core_start` 分開給，因為「最新一列」是照
/// `ts` 排的，而「屬於哪一段」是照 `segment_core_start` 記的——這一輪的 bug
/// 正是這兩者被混為一談。
fn ask(db: &mut Db, segment_core_start: i64, ts: i64, role: &str, outcome: OutboundOutcome) {
    db.insert_brain_outbound(&OutboundInsert {
        ts,
        day_key: "2026-09-04",
        command: "fake-cli",
        args: &[],
        segment_core_start: Some(segment_core_start),
        chars_sent: 100,
        truncated: false,
        outcome: outcome.as_str(),
        duration_ms: 1,
        error: None,
        role,
    })
    .expect("insert outbound");
}

fn attempts(db: &Db, from: i64, to: i64) -> Option<(u32, StoredOutboundOutcome)> {
    db.retained_interpreter_attempts_for_segment(from, to)
        .expect("query")
        .map(|r| (r.count, r.latest_outcome))
}

/// 這一條就是 bug 本身：合併之後右半那些提問不准消失。
///
/// 改之前這裡會是 3（只數得到鍵 = A 的那些），而畫面上那句話會把「少了 2 次」
/// 說成「紀錄沒留著」——那 2 列一直都在。
#[test]
fn merging_two_chapters_keeps_both_halves_asks() {
    let mut db = db();
    for i in 0..3 {
        ask(&mut db, A, A + i, "interpreter", OutboundOutcome::NoAnswer);
    }
    for i in 0..2 {
        ask(&mut db, B, B + i, "interpreter", OutboundOutcome::Timeout);
    }
    let (n, _) = attempts(&db, A, C).expect("合併後的章節應該找得到提問");
    assert_eq!(n, 5, "合併之後右半那 2 次不見了，而那 2 列一列都沒被刪");
}

/// 右界是**開**的。閉區間會讓下一章的第一次提問被這一章數走，於是同一列被
/// 兩章各記一次——那是把一個少算的 bug 換成一個多算的 bug。
#[test]
fn the_right_edge_belongs_to_the_next_chapter() {
    let mut db = db();
    ask(&mut db, A, A, "interpreter", OutboundOutcome::NoAnswer);
    ask(&mut db, C, C, "interpreter", OutboundOutcome::NoAnswer);
    let (n, _) = attempts(&db, A, C).expect("有列");
    assert_eq!(n, 1, "C 那一列屬於下一章，不該被 [A, C) 數進來");
}

/// 沒有人編輯過章節的時候，這個改動不准改變任何數字。
#[test]
fn an_unedited_chapter_counts_exactly_what_it_did_before() {
    let mut db = db();
    for i in 0..4 {
        ask(&mut db, A, A + i, "interpreter", OutboundOutcome::NoAnswer);
    }
    let (ranged, _) = attempts(&db, A, B).expect("有列");
    assert_eq!(ranged, 4, "一般情況（沒編輯過）的數字必須和以前一樣");
}

/// `role` 那道濾網是上一輪釘的，換成範圍查之後不准被順手拿掉。
#[test]
fn other_roles_are_still_not_counted() {
    let mut db = db();
    ask(&mut db, A, A, "interpreter", OutboundOutcome::NoAnswer);
    ask(&mut db, B, B + 1, "reviewer", OutboundOutcome::Success);
    let (n, _) = attempts(&db, A, C).expect("有列");
    assert_eq!(n, 1, "審閱層那一列被數進解釋層的次數裡了");
}

/// 「最近一次是⋯」要取**整個範圍裡**最新那一列，不是起點那一格。
///
/// 這一條和上面那條 count 是不同的軸：只把 COUNT 改成範圍、子查詢還留著相等
/// 的話，次數會對而結局是舊的——畫面上就是「問過 5 次，最近一次是逾時」，
/// 而真正最近那一次是成功。
#[test]
fn the_latest_outcome_comes_from_the_whole_range_not_the_start() {
    let mut db = db();
    ask(&mut db, A, A, "interpreter", OutboundOutcome::Timeout);
    ask(&mut db, B, B, "interpreter", OutboundOutcome::Success);
    let (n, latest) = attempts(&db, A, C).expect("有列");
    assert_eq!(n, 2);
    assert_eq!(
        latest,
        StoredOutboundOutcome::Known(OutboundOutcome::Success),
        "最近一次應該是右半那一列（ts 比較晚），不是起點那一列"
    );
}

/// 範圍裡一列都沒有的時候回 `None`——「沒問過」和「問過 0 次」是兩件事，
/// 上游靠這個分辨。
#[test]
fn a_chapter_nobody_asked_about_has_no_row_at_all() {
    let mut db = db();
    ask(&mut db, C, C, "interpreter", OutboundOutcome::NoAnswer);
    assert!(
        attempts(&db, A, B).is_none(),
        "範圍裡沒有提問就不該回 Some——那會讓上游印出「問過 0 次」"
    );
}
