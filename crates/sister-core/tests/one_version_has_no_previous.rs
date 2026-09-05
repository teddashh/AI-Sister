//! 只有一版的時候，那張卡不准對自己說「原版：⋯」。
//!
//! `apps/desktop/src-tauri/src/main.rs` 的 `memory_current_guess`（畫面上
//! 「這一刻」那張卡）這樣挑前一版：
//!
//! ```text
//! let previous = versions.get(versions.len().saturating_sub(2));
//! ```
//!
//! `versions` 只有一列的時候，`1.saturating_sub(2)` 是 **0**，於是 `get(0)`
//! 拿到的就是 `versions.last()` 自己。`view_from_row_with_previous` 老實地把
//! 它填進 `previous_activity`，而 `ui/timeline.js:673` 只問這個欄位是不是
//! 空的：
//!
//! ```text
//! if (card.previous_activity) { … prev.textContent = `原版：${…}`; }
//! ```
//!
//! 畫面上就變成同一句話講兩次：
//!
//! ```text
//!   上一段 · 14:05 開始
//!   修好型別錯誤
//!   原版：修好型別錯誤
//! ```
//!
//! （第一行是 `timeline.js:653`：`place === "current"` 走的是自己那一臂，印
//! 時間、不印信心。「她猜的（模型說的信心 …）」是 `:667` 最後那個 `else`，
//! 只有日列表那些卡到得了——我原本在這裡把那一行當成「這一刻」那張卡的樣本，
//! 那是假的。壞掉的是第三行：`:673` 的「原版：」**不看 `place`**，兩種卡都印。）
//!
//! 而「只有一版」不是邊角料，是**最常見的情況**——每張卡片剛被解釋層寫出來
//! 的時候都只有第 1 版，要等審閱層修訂過才會有第 2 版。
//!
//! **改之前**，同一個檔案裡另外兩個呼叫端（`attach_l2`、`memory_guesses`）
//! 各自寫了一次 `if versions.len() >= 2 { … } else { None }`。三處同一個決定，
//! 兩處對、一處錯——所以修法是把這個決定**收成一支函式**，而不是在第三處再抄
//! 一次那個 `if`。改完之後那個形狀在這個 branch 一行都不剩，三處都走
//! `latest_with_previous`。（這裡刻意不寫行號：那三處會漂，而寫錯的行號比沒有
//! 行號更浪費下一個人的時間。）
//!
//! 這一檔跑在 `crates/` 裡是刻意的：`apps/desktop` 是另一個 workspace，
//! `cargo test --workspace` 碰不到它，在 `main.rs` 裡加測試等於沒加。
//!
//! ⚠ 但要把話說完，不然這句話讀起來像「所以呼叫端有人守了」：
//! **這一檔守的是那支函式，不是呼叫端。** 把 `main.rs` 裡 `memory_current_guess`
//! 那一行原封不動改回 `versions.get(versions.len().saturating_sub(2))`，
//! 下面每一條都是綠的——我實測過。呼叫端唯一的守門人是
//! `scripts/check-current-guess-wiring.py` 的 E 段，而它是字串比對：擋得住
//! 「helper 被整個拆掉」，擋不住「helper 有呼叫但算出來的答案被丟掉」
//! （實測 `previous.or_else(|| versions.first())` 閘門是綠的，而那正是這個
//! bug 一字不差地回來）。所以不要因為「Rust 測試已經蓋到了」就放寬那一段。

use sister_core::brain::{latest_with_previous, view_from_row_with_previous};
use sister_core::db::{Db, L2Author, L2CardRow, L2Insert};

fn row(id: i64, version: i32, activity: &str, author: L2Author) -> L2CardRow {
    L2CardRow {
        id,
        segment_core_start: 1_000,
        segment_ref: "segment:1000".to_owned(),
        version,
        supersedes: None,
        activity: activity.to_owned(),
        entities_json: "[]".to_owned(),
        continues_json: None,
        commitments_json: "[]".to_owned(),
        model_confidence: 0.7,
        evidence_json: "[]".to_owned(),
        open_questions_json: "[]".to_owned(),
        created_at: 2_000,
        author,
        tombstoned_at: None,
    }
}

/// 這是使用者實際讀到的那一格：`previous_activity` 有值，畫面就多印一行
/// 「原版：⋯」。只有一版的時候那一行的內容會和上面一模一樣。
fn previous_line(versions: &[L2CardRow]) -> Option<String> {
    let (latest, previous) = latest_with_previous(versions)?;
    view_from_row_with_previous(latest, previous).previous_activity
}

#[test]
fn one_version_shows_no_original() {
    let versions = vec![row(1, 1, "修好型別錯誤", L2Author::Interpreter)];
    assert_eq!(
        previous_line(&versions),
        None,
        "只有一版，畫面卻會印出「原版：修好型別錯誤」——和它正上方那一行同一句話"
    );
}

#[test]
fn two_versions_show_the_older_one() {
    let versions = vec![
        row(1, 1, "修好型別錯誤", L2Author::Interpreter),
        row(2, 2, "在追一個編譯錯誤", L2Author::Reviewer),
    ];
    assert_eq!(
        previous_line(&versions),
        Some("修好型別錯誤".to_owned()),
        "審閱層改過之後，原版那一行要講**改之前**那句"
    );
}

/// 三版是用來分辨「前一版」和「第一版」的。少了這一條，把 `previous` 實作成
/// `versions.first()` 會讓上面兩條都綠——而畫面上會拿一句已經被改過兩次的舊
/// 話當成「原版」。
#[test]
fn three_versions_show_the_one_just_before_not_the_oldest() {
    let versions = vec![
        row(1, 1, "第一版：修好型別錯誤", L2Author::Interpreter),
        row(2, 2, "第二版：在追一個編譯錯誤", L2Author::Reviewer),
        row(3, 3, "第三版：在讀型別文件", L2Author::Reviewer),
    ];
    assert_eq!(
        previous_line(&versions),
        Some("第二版：在追一個編譯錯誤".to_owned()),
        "原版那一行要講緊鄰的前一版，不是最早那一版"
    );
}

/// 一版都沒有的時候連卡片都不該有。
#[test]
fn no_versions_means_no_card() {
    assert!(latest_with_previous(&[] as &[L2CardRow]).is_none());
}

/// 顯示的那一張永遠是**最新**的，不是最舊的。
///
/// 這一條和上面三條是不同的軸：那三條問「原版是誰」，這一條問「主角是誰」。
///
/// 我原本在這裡寫「兩個都錯的話（`first()` 配 `get(1)`）光看原版那一行是分辨
/// 不出來的」——**那句話是假的**，我後來真的打了那一刀：
/// `two_versions_show_the_older_one` 也紅了（原版那一行變成「在追一個編譯
/// 錯誤」）。留這一條的理由不是「只有它抓得到」，而是它**直接**斷言
/// `latest.activity`，不經過 `previous_activity` 那一層轉手——挑錯主角的突變
/// 不必先害到原版那一行，才會被這裡看見。
#[test]
fn the_card_shown_is_the_newest_version() {
    let versions = vec![
        row(1, 1, "第一版", L2Author::Interpreter),
        row(2, 2, "第二版", L2Author::Reviewer),
    ];
    let (latest, _) = latest_with_previous(&versions).expect("有兩版");
    assert_eq!(latest.activity, "第二版", "顯示的是最舊那一版");
}

#[test]
fn database_versions_are_oldest_first_and_select_the_immediate_previous() {
    let mut db = Db::open_in_memory().expect("db");
    let segment_core_start = 1_000;

    for (activity, author) in [
        ("第一版", L2Author::Interpreter),
        ("第二版", L2Author::Reviewer),
        ("第三版", L2Author::Reviewer),
    ] {
        db.insert_l2_card(&L2Insert {
            segment_core_start,
            segment_ref: "segment:1000",
            activity,
            entities_json: "[]".to_owned(),
            continues_json: None,
            commitments_json: "[]".to_owned(),
            model_confidence: 0.7,
            evidence_json: "[]".to_owned(),
            open_questions_json: "[]".to_owned(),
            author,
        })
        .expect("insert l2 card");
    }

    let versions = db
        .l2_versions_for_segment(segment_core_start)
        .expect("versions");
    assert_eq!(
        versions.first().map(|row| row.activity.as_str()),
        Some("第一版")
    );
    assert_eq!(
        versions.last().map(|row| row.activity.as_str()),
        Some("第三版")
    );

    let (latest, previous) = latest_with_previous(&versions).expect("有三版");
    assert_eq!(latest.activity, "第三版");
    assert_eq!(previous.map(|row| row.activity.as_str()), Some("第二版"));
}
