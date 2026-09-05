//! r28c 的驗收測試。**在 delegate 交貨之前寫好的。**
//!
//! # 病灶（我親手量到的，不是讀出來的）
//!
//! 使用者在時間軸上把兩章合併之後，那一章**永遠是空白的**——卡片還活著、
//! 一列都沒被刪，只是再也沒有人畫得出來。探針量到的三個數字：
//!
//! ```text
//! 合併前 segment 起點 = [1700000050000, 1700000230000]
//! 卡片的鍵            = [1700000230000]      ← 寫給右半的那張，還活著
//! attach_l2 比對的鍵  = [1700000050000, 1700000050000]  ← 合併後只剩左半的起點
//! 嚴格相等濾出來的卡  = []                   ← 畫面上什麼都沒有
//! ```
//!
//! `apps/desktop` 的 `attach_l2` 用 `c.segment_core_start == start` 去挑卡片，
//! 而合併讓右半的起點從段落上消失了（`merge_two` 取左半的起點）。
//!
//! # 為什麼是 r28 造成的
//!
//! 這個空白**本來會自己好**：r28 之前 `collect_jobs` 問的是
//! `latest_l2_for_segment(左起點)`，合併後那個鍵查不到卡，於是下一輪會重新
//! 解釋、寫一張鍵在左起點的新卡，時間軸就畫得出來了（代價是多問一次）。
//!
//! r28 把 `collect_jobs` 改成範圍查詢之後，它**看得到**右半那張卡，於是
//! `continue` 跳過這一章——省下了那次外送，也**永久拿掉了那條修復路徑**，
//! 而顯示端一行都沒有跟著改。`sister interpret --at 左起點` 也救不回來：
//! `only_core_start` 那道濾網排在有卡那道閘門**之前**，而有卡那道沒有
//! `--at` 的旁路（`worth_interpreting` 有）。
//!
//! 也就是說 r28 用「不再重複問」換到了「合併過的章節永遠空白」。這一檔守的
//! 是換回來的那半：**不重複問，而且畫得出來。**
//!
//! # 這一檔守的東西住在哪裡
//!
//! 選卡片的邏輯本來整段長在 `apps/desktop/src-tauri/src/main.rs` 裡，而那是
//! 另一個 workspace——`cargo test --workspace` 一行都執行不到它。所以修法是
//! 照 alpha.87 的前例把邏輯搬進 `crates/`（`brain::chapter_l2_views`），
//! `attach_l2` 只剩下呼叫它。搬進來之後這一檔才有牙齒。
//!
//! ⚠ 這一檔在修法落地**之前**編不過（`chapter_l2_views` 還不存在），而
//!   編不過的紅什麼都沒證明。有價值的是交貨**之後**的突變，記在 RESULT 裡。
//!
//! ⚠ 夾具全部走真的寫入路徑（`insert_focus` → `chapters_for_range` →
//!   `merge_chapters` → `insert_l2_card` → `l2_in_range`）。手工拼出來的
//!   `L2CardRow` 只會證明「這個欄位填得進去」，不會證明合併真的會製造出
//!   這種鍵。

use sister_core::brain::chapter_l2_views;
use sister_core::db::{Db, L2Author, L2CardRow, L2Insert};
use sister_core::model::{FocusEvent, FocusKind, FocusSnapshot};

/// 切出三章，回傳它們的核心起點。
///
/// 四個 focus 事件切出**三**段（N 個事件 = N-1 段，最後一個事件只負責關掉
/// 前一段）。我第一版寫三個事件、把 helper 叫做 `three_chapters`，實測只有
/// 兩段——`activities_for_range(..)[2]` 直接 index out of bounds。
fn three_chapters(db: &mut Db, ts: i64) -> Vec<i64> {
    let sid = db.start_session("test", "0").expect("session");
    for (offset, app) in [
        (0, "code.exe"),
        (180_000, "chrome.exe"),
        (360_000, "notion.exe"),
        (540_000, "slack.exe"),
    ] {
        db.insert_focus(
            sid,
            &FocusEvent {
                ts: ts + offset,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some(app.into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus");
    }
    db.chapters_for_range(ts, ts + 720_000)
        .expect("chapters")
        .iter()
        .map(|s| s.core_started_at)
        .collect()
}

fn write_card(db: &mut Db, key: i64, activity: &str, author: L2Author) {
    db.insert_l2_card(&L2Insert {
        segment_core_start: key,
        segment_ref: &format!("seg:{key}"),
        activity,
        entities_json: "[]".into(),
        continues_json: None,
        commitments_json: "[]".into(),
        model_confidence: 0.7,
        evidence_json: "[]".into(),
        open_questions_json: "[]".into(),
        author,
    })
    .expect("card");
}

/// 這一章的範圍（合併／切開之後重算過的），照桌面拿到的那份資料算。
fn chapter_range(db: &mut Db, ts: i64, nth: usize) -> (i64, i64) {
    let acts = db
        .activities_for_range(ts, ts + 720_000)
        .expect("activities");
    let a = &acts[nth];
    (a.core_started_at, a.core_ended_at)
}

fn cards(db: &Db, ts: i64) -> Vec<L2CardRow> {
    db.l2_in_range(ts, ts + 720_000).expect("l2_in_range")
}

fn activities_shown(views: &[sister_core::brain::L2View]) -> Vec<&str> {
    views.iter().map(|v| v.activity.as_str()).collect()
}

const TS: i64 = 1_700_000_050_000;

/// 病灶本身。合併之後，寫給右半的那張卡還是要畫得出來。
#[test]
fn a_merged_chapter_shows_the_card_written_for_its_right_half() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[1], "右半已經有卡", L2Author::Interpreter);
    db.merge_chapters(starts[0], starts[1], TS, TS + 720_000)
        .expect("merge");

    let (from, to) = chapter_range(&mut db, TS, 0);
    let views = chapter_l2_views(&cards(&db, TS), from, to);

    assert_eq!(
        activities_shown(&views),
        vec!["右半已經有卡"],
        "合併過的章節不可以是空白的——那張卡還活著，一列都沒被刪"
    );
}

/// 沒被編輯過的那些章節，畫出來的東西要和以前一模一樣。
///
/// 這一條是修法的**反向護欄**：把選法從「每個段落起點各一張」改成範圍之後，
/// 最容易造成的迴歸是「三段各有一張卡的章節，只剩下一張」。
#[test]
fn an_unmerged_chapter_with_a_card_shows_exactly_that_card() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[0], "第一章", L2Author::Interpreter);
    write_card(&mut db, starts[1], "第二章", L2Author::Interpreter);
    write_card(&mut db, starts[2], "第三章", L2Author::Interpreter);

    for (nth, want) in [(0, "第一章"), (1, "第二章"), (2, "第三章")] {
        let (from, to) = chapter_range(&mut db, TS, nth);
        let views = chapter_l2_views(&cards(&db, TS), from, to);
        assert_eq!(
            activities_shown(&views),
            vec![want],
            "第 {nth} 章畫出來的必須還是它自己那張"
        );
    }
}

/// 左界含在裡面。
///
/// 這一條是為了釘住下界：把 `>= from` 放寬成 `>= from - 一天`，每一章都會
/// 掛出前一章的卡，而在 r28b 的查詢上那一刀是**綠的**（沒有人守下界）。
#[test]
fn a_card_on_the_left_edge_belongs_to_this_chapter() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[0], "第一章", L2Author::Interpreter);
    write_card(&mut db, starts[1], "第二章", L2Author::Interpreter);

    let (from, to) = chapter_range(&mut db, TS, 1);
    let views = chapter_l2_views(&cards(&db, TS), from, to);

    assert_eq!(
        activities_shown(&views),
        vec!["第二章"],
        "第二章只能拿到自己那張——下界一鬆就會把第一章的卡掛上來"
    );
}

/// 右界不含。相鄰兩章不可以把同一張卡各數一次。
#[test]
fn the_right_edge_belongs_to_the_next_chapter() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[1], "第二章", L2Author::Interpreter);

    let (from, to) = chapter_range(&mut db, TS, 0);
    let views = chapter_l2_views(&cards(&db, TS), from, to);

    assert!(
        views.is_empty(),
        "第二章的起點就是第一章的迄點，右界開著才不會被兩章各認領一次；實際畫出來的是 {:?}",
        activities_shown(&views)
    );
}

/// 使用者當場改過的那張，合併之後還要看得見。
///
/// 時間軸上寫著「你改過的。下一輪不會蓋掉。」，而合併之後如果只照鍵挑最大的
/// 那一條脈絡，機器寫給右半的猜測就會蓋過使用者親手改的左半——那句承諾當場
/// 變成假話，而且 `l2_card` 裡那道「使用者改過的假設不會被下一輪蓋掉」的
/// `ensure!` 是**按鍵**檢查的，跨鍵完全擋不住。
#[test]
fn a_user_correction_is_still_visible_after_a_merge() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[0], "我自己改的", L2Author::User);
    write_card(&mut db, starts[1], "機器猜的", L2Author::Interpreter);
    db.merge_chapters(starts[0], starts[1], TS, TS + 720_000)
        .expect("merge");

    let (from, to) = chapter_range(&mut db, TS, 0);
    let views = chapter_l2_views(&cards(&db, TS), from, to);

    assert!(
        activities_shown(&views).contains(&"我自己改的"),
        "使用者改過的那張不可以因為合併就消失；實際畫出來的是 {:?}",
        activities_shown(&views)
    );
}

/// 自己沒有卡的那一章，就是沒有卡——不可以去借隔壁的。
///
/// 「空白」和「掛著別人的卡」都是壞的，而修法要把前者變成後者才算失敗。
/// 這一條和上面那條左界一起夾住：範圍要**剛好**是這一章。
#[test]
fn a_chapter_with_no_card_of_its_own_shows_nothing() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[0], "第一章", L2Author::Interpreter);
    write_card(&mut db, starts[1], "第二章", L2Author::Interpreter);
    // 第三章故意沒有卡。

    let (from, to) = chapter_range(&mut db, TS, 2);
    let views = chapter_l2_views(&cards(&db, TS), from, to);

    assert!(
        views.is_empty(),
        "第三章自己沒有卡，畫面上就該是空的；實際畫出來的是 {:?}",
        activities_shown(&views)
    );
}

/// 改過的卡片要帶著上一版，「原版：⋯」那一行才有東西可寫。
#[test]
fn a_revised_card_still_carries_its_previous_version() {
    let mut db = Db::open_in_memory().expect("db");
    let starts = three_chapters(&mut db, TS);
    write_card(&mut db, starts[1], "第一版", L2Author::Interpreter);
    write_card(&mut db, starts[1], "第二版", L2Author::User);
    db.merge_chapters(starts[0], starts[1], TS, TS + 720_000)
        .expect("merge");

    let (from, to) = chapter_range(&mut db, TS, 0);
    let views = chapter_l2_views(&cards(&db, TS), from, to);

    let v = views.first().expect("要有一張卡");
    assert_eq!(v.activity, "第二版", "畫出來的要是最新那一版");
    assert_eq!(
        v.previous_activity.as_deref(),
        Some("第一版"),
        "上一版要跟著出來，否則「原版：⋯」那一行沒有東西可寫"
    );
}
