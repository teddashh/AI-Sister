//! r28 第二半的驗收測試。**在 delegate 交貨之前寫好的。**
//!
//! r28 的前半把「她試著問過 N 次」改成照**範圍**去數，於是合併兩個章節之後
//! 那個數字對了。可是決定「有沒有卡片」的那半還停在**起點相等**上，畫面就變成
//! 這樣（合併「左半問了 3 次沒答案」＋「右半問了 2 次、最後一次成功並寫出卡片」）：
//!
//! ```text
//! 最新一段值得理解，她試著問過 5 次，最近一次是成功（success），現在手上沒有卡片。
//! ```
//!
//! 「最近一次是成功」和「現在手上沒有卡片」在同一句話裡互相打臉，而那張卡就躺在
//! `l2_card` 裡，鍵是右半的起點。**r28 只出前半的話，是拿一個少算換一句自相矛盾**，
//! 所以這一檔和前半必須一起出貨。
//!
//! ## 這一檔要求的東西
//!
//! 一支新的資料層函式：
//!
//! ```text
//! Db::l2_versions_for_chapter(core_started_at, core_ended_at) -> Result<Vec<L2CardRow>>
//! ```
//!
//! 語意（三句，缺一不可）：
//!
//! 1. 在 `segment_core_start ∈ [起點, 結束)` 且**沒有墓碑**的卡片裡，取
//!    `segment_core_start` **最大**的那一個當作脈絡的鍵。
//! 2. 回傳**那一個鍵**的全部未墓碑版本，由舊到新（和 `l2_versions_for_segment`
//!    同一個形狀，好讓 `latest_with_previous` 原封不動繼續用）。
//! 3. 範圍裡一張活著的卡都沒有就回空的。
//!
//! 第 2 句是這一檔最不明顯、也最容易被做錯的一句：把 `WHERE` 直接換成範圍
//! （`>= ?1 AND < ?2`）會讓**兩個章節的卡片被串成同一條版本史**，於是
//! `latest_with_previous` 會拿左半的卡當成右半那張卡的「原版：⋯」——那是把一句
//! 自相矛盾換成一句更難發現的假話。釘住它的是
//! `two_lineages_in_one_range_are_not_spliced_into_one_history`。
//!
//! ## 這一檔守得到什麼、守不到什麼
//!
//! ⚠ 守得到的是**資料層那三句語意**。使用者真正讀到的那句話由
//!   `apps/desktop/src-tauri/src/main.rs` 的 `memory_current_guess` 組出來，它在
//!   另一個 workspace，`cargo test --workspace` 一行都執行不到（這台機器上
//!   `cargo check` 對它還會 panic，libdbus-sys 的 build script）。所以「合併之後
//!   畫面上真的出現那張卡」要在真的 Windows 上按一遍才算數；接線那半由
//!   `scripts/check-current-guess-wiring.py` 用原始碼比對去釘。
//!
//! ⚠ `brain.rs` 的 `collect_jobs` 是同一個 bug 的第二個消費端（「這一章有卡了嗎？
//!   有就跳過」，起點相等的話合併之後她會把已經有卡的章節**再問一次**）。它和
//!   這一檔在**同一輪**一起修好了，也一起有了測試——但那條測試不在這裡。那支
//!   函式是私有的，這一檔（整合測試）呼叫不到它，它的證據在 `brain.rs` 的
//!   `mod tests` 裡，叫 `collect_jobs_skips_a_merged_chapter_with_a_card_on_its_right_half`。
//!   **不要因為這一檔全綠就以為那半有人守**——守它的是那一條，不是這七條。
//!
//!   （這段話原本寫的是「也**還在**用起點相等」「這裡沒有它的證據」。兩句在寫下
//!   的當時都是真的——我是在交貨之前寫這個檔頭的——而交貨之後兩句都反了。
//!   考題的檔頭會比考題本身更早過期，因為它描述的是**還沒發生的事**。）
//!
//! ⚠ 交貨**之前**這一檔編不過（那支函式還不存在），而編不過的紅什麼都沒證明。
//!   有價值的是交貨**之後**的突變。

use sister_core::brain::{OutboundOutcome, latest_with_previous};
use sister_core::db::{Db, L2Author, L2CardRow, L2Insert, OutboundInsert};

const A: i64 = 10_000; // 左邊那一章的起點
const B: i64 = 20_000; // 右邊那一章的起點（合併之後這個鍵就沒有段落認領了）
const C: i64 = 30_000; // 合併後那一章的結束（半開區間，不含）

fn db() -> Db {
    Db::open_in_memory().expect("db")
}

/// 寫一張卡。同一個 `segment_core_start` 寫第二次就是同一條脈絡的下一版
/// （`insert_l2_card` 自己接 `supersedes`）。
fn card(db: &mut Db, segment_core_start: i64, activity: &str, author: L2Author) -> i64 {
    db.insert_l2_card(&L2Insert {
        segment_core_start,
        segment_ref: &format!("segment:{segment_core_start}"),
        activity,
        entities_json: "[]".to_owned(),
        continues_json: None,
        commitments_json: "[]".to_owned(),
        model_confidence: 0.7,
        evidence_json: "[]".to_owned(),
        open_questions_json: "[]".to_owned(),
        author,
    })
    .expect("insert l2 card")
}

fn activities(rows: &[L2CardRow]) -> Vec<&str> {
    rows.iter().map(|r| r.activity.as_str()).collect()
}

fn chapter(db: &Db, from: i64, to: i64) -> Vec<L2CardRow> {
    db.l2_versions_for_chapter(from, to).expect("query")
}

/// 這一條就是 bug 本身：合併之後，右半那張卡不准從畫面上消失。
///
/// 改之前這裡是空的——而同一畫面上「最近一次是成功」還在，兩句話互相打臉。
#[test]
fn a_merged_chapter_shows_the_card_written_for_its_right_half() {
    let mut db = db();
    card(&mut db, B, "在讀 rusqlite 的文件", L2Author::Interpreter);
    let rows = chapter(&db, A, C);
    assert_eq!(
        activities(&rows),
        vec!["在讀 rusqlite 的文件"],
        "合併之後右半那張卡不見了，而它一列都沒被刪"
    );
}

/// 沒有人編輯過章節的時候，這個改動不准改變畫面上的任何一個字。
///
/// 第一條斷言對的是 `l2_versions_for_segment`——同一顆資料庫、同一個章節，兩支
/// 函式必須給出同一份版本史，而**那一條不手抄期望值**：抄一份就會漂。
///
/// 第二條斷言就**是**手抄的（`vec!["第一版", "第二版"]`），那是刻意的：只有第一
/// 條的話，兩支函式一起壞掉、一起回空的，它照樣是綠的。手抄那份釘的是「順序由舊
/// 到新」和「真的有兩版」，那是第一條證不出來的東西。
///
/// 兩條都只比 `activity` 字串（`activities()` 就是這樣寫的），`version` / `id` /
/// `author` / `model_confidence` 沒有人比——所以「一模一樣的版本史」這句話在這裡
/// 不成立，這裡驗的是「同一串活動描述、同一個順序」。
#[test]
fn an_unedited_chapter_returns_exactly_what_it_did_before() {
    let mut db = db();
    card(&mut db, A, "第一版", L2Author::Interpreter);
    card(&mut db, A, "第二版", L2Author::Reviewer);
    let before = db.l2_versions_for_segment(A).expect("query");
    let after = chapter(&db, A, B);
    assert_eq!(
        activities(&after),
        activities(&before),
        "沒編輯過的章節，新舊兩支函式必須給同一份版本史"
    );
    assert_eq!(
        activities(&after),
        vec!["第一版", "第二版"],
        "版本要由舊到新"
    );
}

/// 右界是**開**的。閉區間會讓下一章的卡片被這一章顯示走，於是同一張卡出現在
/// 兩個章節上——那是把一張看不見的卡換成一張站錯地方的卡。
#[test]
fn the_right_edge_belongs_to_the_next_chapter() {
    let mut db = db();
    card(&mut db, C, "下一章的事", L2Author::Interpreter);
    assert!(
        chapter(&db, A, C).is_empty(),
        "C 那張卡屬於下一章，不該被 [A, C) 顯示走"
    );
}

/// **範圍查最容易做錯的那一刀。**
///
/// 把 `WHERE segment_core_start = ?1` 直接換成 `>= ?1 AND < ?2` 就會把兩條脈絡的
/// 版本混成一份清單，於是 `latest_with_previous` 拿左半的卡當右半那張卡的
/// 「原版：⋯」——畫面上會出現一張卡說自己的前身是另一個章節的內容。
///
/// 這裡故意讓左半有**兩版**、右半只有**一版**：混在一起的話清單長度是 3、
/// 「原版」有值；做對的話長度是 1、沒有原版。
#[test]
fn two_lineages_in_one_range_are_not_spliced_into_one_history() {
    let mut db = db();
    card(&mut db, A, "左半第一版", L2Author::Interpreter);
    card(&mut db, A, "左半第二版", L2Author::Reviewer);
    card(&mut db, B, "右半唯一版", L2Author::Interpreter);

    let rows = chapter(&db, A, C);
    assert_eq!(
        activities(&rows),
        vec!["右半唯一版"],
        "兩個章節的卡片被串成同一條版本史了"
    );
    let (latest, previous) = latest_with_previous(&rows).expect("有卡");
    assert_eq!(latest.activity, "右半唯一版");
    assert!(
        previous.is_none(),
        "這張卡只有一版，卻被安上一個來自別的章節的「原版：{}」",
        previous.map(|p| p.activity.as_str()).unwrap_or("")
    );
}

/// 忘掉右半之後，要退回左半那張**還活著**的卡，不是交出一張墓碑、也不是變成空的。
///
/// 這裡走的是真的 `forget`，不是手動蓋欄位——`collect_cascade_parents` 收 `l2_card`
/// 用的就是 `segment_core_start` 的半開區間，所以 `forget(B, C, …)` 剛好只碰右半。
#[test]
fn a_forgotten_right_half_falls_back_to_the_live_card_on_the_left() {
    let mut db = db();
    card(&mut db, A, "還記得的那件事", L2Author::Interpreter);
    card(&mut db, B, "被忘掉的那件事", L2Author::Interpreter);
    db.forget(B, C, None).expect("forget");

    let rows = chapter(&db, A, C);
    assert_eq!(
        activities(&rows),
        vec!["還記得的那件事"],
        "取鍵的時候沒有把墓碑排除掉：最大的那個鍵是死的，於是整章變成沒有卡片"
    );
}

/// 唯一那張卡被忘掉之後，這一章就是真的沒有卡片。
///
/// 「現在手上沒有卡片」這句話必須還能是真的——墓碑不算卡片，而且它的內容已經被
/// 清空了（`forget` 連字一起清），交出去等於在畫面上印一張空白的卡。
#[test]
fn a_chapter_whose_only_card_was_forgotten_has_no_card_at_all() {
    let mut db = db();
    card(&mut db, B, "被忘掉的那件事", L2Author::Interpreter);
    db.forget(B, C, None).expect("forget");
    assert!(
        chapter(&db, A, C).is_empty(),
        "被忘掉的卡片還在畫面上（而它的內容已經被清空了）"
    );
}

/// **這一條問的是那句話自己：數字和卡片必須回答同一個問題。**
///
/// 上面六條各自釘一個性質；這一條把 r28 的兩半擺在同一顆資料庫上，重現使用者
/// 那句自相矛盾的話。改之前：次數那半看 `[A, C)` 算出「問過 5 次，最近一次是
/// 成功」，卡片那半看 `= A` 算出「沒有卡片」。
#[test]
fn the_count_and_the_card_answer_the_same_question() {
    let mut db = db();
    for i in 0..3 {
        ask(&mut db, A, A + i, OutboundOutcome::NoAnswer);
    }
    for i in 0..2 {
        ask(&mut db, B, B + i, OutboundOutcome::Success);
    }
    card(&mut db, B, "在讀 rusqlite 的文件", L2Author::Interpreter);

    let attempts = db
        .retained_interpreter_attempts_for_segment(A, C)
        .expect("query")
        .expect("這一段問過");
    assert_eq!(attempts.count, 5, "次數那半（r28 前半）應該已經是範圍查了");

    assert!(
        !chapter(&db, A, C).is_empty(),
        "同一句話裡：「她試著問過 {} 次，最近一次是{}」＋「現在手上沒有卡片」——\
         而那張卡就躺在 l2_card 裡，鍵是 {B}",
        attempts.count,
        attempts.latest_outcome.zh_label(),
    );
}

/// 插一列外送紀錄。和次數那半的驗收測試用同一個形狀。
fn ask(db: &mut Db, segment_core_start: i64, ts: i64, outcome: OutboundOutcome) {
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
        role: "interpreter",
    })
    .expect("insert outbound");
}

/// 合併之後，使用者親手改過的那張不可以被機器的猜測擠掉。
///
/// `l2_versions_for_chapter` 用 `MAX(segment_core_start)` 挑脈絡，而 `MAX`
/// **不看作者**。使用者改的在左半（鍵小）、機器猜的在右半（鍵大），合併之後
/// 挑出來的是機器那張——而時間軸上就寫著「你改過的。下一輪不會蓋掉。」
///
/// `insert_l2_card` 那道 `ensure!("使用者改過的假設不會被下一輪蓋掉")` 是
/// **按鍵**檢查的（`latest_l2_for_segment(ins.segment_core_start)`），跨鍵
/// 一點都擋不住——它擋的是「同一段被蓋掉」，這裡是「另一段贏了」。
#[test]
fn a_user_correction_outranks_a_machine_guess_from_the_other_half() {
    let mut db = db();
    card(&mut db, A, "我自己改的", L2Author::User);
    card(&mut db, B, "機器猜的", L2Author::Interpreter);

    assert!(
        activities(&chapter(&db, A, C)).contains(&"我自己改的"),
        "使用者改過的那張不可以因為合併就被機器的猜測擠掉；實際挑出來的是 {:?}",
        activities(&chapter(&db, A, C))
    );
}

/// 自己沒有卡的那一章，不准去借前一章的。
///
/// `MAX(segment_core_start)` 會把下界的鬆動蓋住：只要這一章**自己有卡**，
/// 挑出來的就還是自己那張，放寬下界看不出差別。要它現形，這一章必須是空的。
/// 我把子查詢的 `>= ?1` 換成 `>= ?1 - 86400000` 跑過一次——**26 個 binary 全綠**，
/// 而那個 bug 在畫面上是「每一章都掛著前一章的假設」。
#[test]
fn a_chapter_with_no_card_does_not_borrow_the_previous_chapters() {
    let mut db = db();
    card(&mut db, A, "第一章", L2Author::Interpreter);
    // 第二章（B..C）自己一張卡都沒有。

    assert!(
        chapter(&db, B, C).is_empty(),
        "第二章自己沒有卡，就是沒有卡——實際挑出來的是 {:?}",
        activities(&chapter(&db, B, C))
    );
}
