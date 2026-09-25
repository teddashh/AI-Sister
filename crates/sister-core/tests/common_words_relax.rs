//! alpha.157 R2 驗收：她看過的常用字，不能讓放寬停在它們前面。
//!
//! R1 的放寬只放掉「索引裡一個都沒有」的字。帳單夾具只有三張畫面，「怎麼」「哪裡」
//! 「請問」在那裡從來沒出現過，所以規則看起來很好用。用過一陣子的索引裡，這些字
//! 一定都看過——這支測試先把它們放進背景（前提斷言逐條確認真的看過），再問同樣的
//! 問題。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`。每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次；對照組先證明自己答得出來、沒有改字，而且兩份資料庫答出
//! 同一件事——背景不能替主題補票。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 一般中文畫面上常見的句子。每一條問題裡的雜訊字都在這裡出現過；
/// 主題字（部署、客服、專線、電信、帳單、月報、連結、退款、會議、release…）一個都沒有。
const BACKGROUND: &[&str] = &[
    "這到底是怎麼回事",
    "這句話是什麼意思",
    "現在怎麼辦",
    "不知道是什麼原因",
    "這個字怎麼打",
    "我查一下再回你",
    "新版出了嗎",
    "壞了要怎麼修",
    "鑰匙在哪裡",
    "包裹寄到哪了",
    "請問一下",
    "告訴我結果",
    "幫我看一下這個",
    "為什麼會這樣",
    "怎麼找都找不到",
    "晚點打電話給你",
    "門要怎麼開",
];

const TOPIC_WORDS: &[&str] = &[
    "部署",
    "失敗",
    "客服",
    "專線",
    "電信",
    "帳單",
    "月報",
    "連結",
    "退款",
    "會議",
    "火星",
    "release",
    "candidate",
    "ERR_DEPLOY",
];

fn add(db: &mut Db, session: i64, ts: i64, dhash: u64, text: &str) {
    db.insert_frame(
        session,
        &FrameCapture {
            assistive: Vec::new(),
            ts,
            monitor: 0,
            width: 800,
            height: 600,
            dhash,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: text.into(),
                x: 0,
                y: 0,
                w: 300,
                h: 20,
                confidence: 1.0,
            }],
            focus: FocusSnapshot::default(),
        },
        None,
        0,
    )
    .unwrap();
}

fn fixture(background: bool) -> Db {
    let corpus: sister_core::replay::Corpus = serde_json::from_str(include_str!(
        "../../../scenarios/recall-baseline.corpus.json"
    ))
    .unwrap();
    let mut db = Db::open_in_memory().unwrap();
    db.import_replay(&corpus, 100).unwrap();
    let session = db.start_session("test", "test").unwrap();
    add(&mut db, session, 5_000, 90, "月報連結已更新");
    if background {
        for (i, line) in BACKGROUND.iter().enumerate() {
            add(&mut db, session, 6_000 + i as i64, 100 + i as u64, line);
        }
    }
    db
}

struct Seen {
    answers: String,
    hits: String,
    /// 不含 bm25 分數的投影：背景會改變分數，不會改變找到哪幾筆。
    same_things: String,
    searched: Option<SearchAdjustment>,
    empty: bool,
}

fn ask(db: &mut Db, q: &str) -> Seen {
    let r = RetrievalProfile::TextAndFacts.retrieve(db, q, 5).unwrap();
    let facts: Vec<(i64, &str, usize)> = r
        .answers
        .iter()
        .map(|a| (a.latest.id, a.latest.raw.as_str(), a.sightings))
        .collect();
    let hits: Vec<(i64, &str)> = r
        .hits
        .iter()
        .map(|h| (h.chunk_id, h.text.as_str()))
        .collect();
    Seen {
        answers: format!("{:?}", r.answers),
        hits: format!("{:?}", r.hits),
        same_things: format!("{facts:?} {hits:?}"),
        searched: r.searched,
        empty: r.answers.is_empty() && r.hits.is_empty(),
    }
}

fn relaxed(x: &str) -> Option<SearchAdjustment> {
    Some(SearchAdjustment::Relaxed(x.into()))
}

fn clean(db: &mut Db, q: &str) -> Seen {
    let s = ask(db, q);
    assert!(!s.empty, "對照組「{q}」自己要答得出來");
    assert_eq!(s.searched, None, "對照組「{q}」本身不可以是放寬過的");
    s
}

fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// `noisy` 拿掉 `base` 之後剩下的雜訊，切成和索引同一種雙字。跨在主題與雜訊
/// 交界的那個雙字（「敗怎」）不算：它本來就不會在畫面上。
fn noise_grams(noisy: &str, base: &str) -> Vec<String> {
    let at = noisy
        .find(base)
        .unwrap_or_else(|| panic!("「{base}」要是「{noisy}」的一段"));
    let mut grams = Vec::new();
    for piece in [&noisy[..at], &noisy[at + base.len()..]] {
        let chars: Vec<char> = piece.chars().collect();
        for w in chars.windows(2) {
            if cjk(w[0]) && cjk(w[1]) {
                grams.push(w.iter().collect());
            }
        }
    }
    grams
}

/// 前提：背景真的有看過這些雜訊。沒有這條，「有背景」那一半會安靜地變回 R1 的夾具。
fn assert_noise_is_seen(db: &Db, noisy: &str, base: &str) {
    let grams = noise_grams(noisy, base);
    assert!(!grams.is_empty(), "前提：「{noisy}」要有雜訊雙字可以檢查");
    for g in grams {
        assert!(
            !db.search(&g, 1).unwrap().is_empty(),
            "前提：背景要看過「{g}」（來自「{noisy}」）"
        );
    }
}

fn same_as_everywhere(noisy: &str, base: &str) {
    let mut plain = fixture(false);
    let mut busy = fixture(true);
    assert_noise_is_seen(&busy, noisy, base);
    let want_plain = clean(&mut plain, base);
    let want_busy = clean(&mut busy, base);
    assert_eq!(
        want_busy.same_things, want_plain.same_things,
        "前提：背景不可以替「{base}」多找或少找"
    );
    for (label, db, want) in [
        ("沒有背景", &mut plain, &want_plain),
        ("有背景", &mut busy, &want_busy),
    ] {
        let got = ask(db, noisy);
        assert_eq!(
            got.searched,
            relaxed(base),
            "{label}「{noisy}」：要說實際拿去找的是「{base}」"
        );
        assert_eq!(
            got.answers, want.answers,
            "{label}「{noisy}」：facts 要和「{base}」一模一樣"
        );
        assert_eq!(
            got.hits, want.hits,
            "{label}「{noisy}」：原文要和「{base}」一模一樣"
        );
    }
}

fn nothing_everywhere(q: &str, why: &str) {
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = fixture(background);
        let got = ask(&mut db, q);
        assert!(got.empty, "{label}「{q}」：{why}");
        assert_eq!(
            got.searched, None,
            "{label}「{q}」：沒有放寬就不說改用了什麼"
        );
    }
}

#[test]
fn the_background_holds_no_topic_word() {
    for line in BACKGROUND {
        for word in TOPIC_WORDS {
            assert!(!line.contains(word), "背景「{line}」含主題字「{word}」");
        }
    }
    // 反過來：主題字真的在夾具裡，上面那條才不是空轉。
    let mut db = fixture(false);
    for word in [
        "部署",
        "客服",
        "專線",
        "電信",
        "帳單",
        "月報",
        "release",
        "ERR_DEPLOY_42",
    ] {
        assert!(
            !db.search(word, 1).unwrap().is_empty(),
            "前提：夾具有「{word}」"
        );
    }
    let got = ask(&mut db, "月報連結");
    assert!(!got.empty, "前提：月報那張畫面在");
}

#[test]
fn a_question_tail_she_has_seen_is_still_dropped() {
    for (noisy, base) in [
        ("ERR_DEPLOY_42 怎麼回事", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42 是什麼意思", "ERR_DEPLOY_42"),
        ("部署失敗怎麼辦", "部署失敗"),
        ("部署失敗是什麼原因", "部署失敗"),
        ("客服專線怎麼打", "客服專線"),
        ("月報連結在哪裡", "月報連結"),
        ("電信帳單寄到哪", "電信帳單"),
        ("release candidate 出了嗎", "release candidate"),
    ] {
        same_as_everywhere(noisy, base);
    }
}

#[test]
fn a_seen_tail_after_a_type_word_keeps_the_type_word() {
    for (noisy, base) in [
        ("客服電話怎麼打", "客服電話"),
        ("客服電話號碼怎麼打", "客服電話號碼"),
        ("ERR_DEPLOY_42 錯誤怎麼修", "ERR_DEPLOY_42 錯誤"),
    ] {
        let mut plain = fixture(false);
        let want = clean(&mut plain, base);
        assert_ne!(
            want.answers, "[]",
            "前提：「{base}」是靠類型詞拿到 facts 答案的"
        );
        same_as_everywhere(noisy, base);
    }
}

#[test]
fn a_request_she_has_seen_at_the_front_is_still_dropped() {
    for (noisy, base) in [
        ("請問月報連結", "月報連結"),
        ("告訴我 月報連結", "月報連結"),
        ("查一下 ERR_DEPLOY_42 怎麼回事", "ERR_DEPLOY_42"),
        ("為什麼部署失敗", "部署失敗"),
        ("怎麼找客服電話", "客服電話"),
    ] {
        same_as_everywhere(noisy, base);
    }
}

#[test]
fn a_request_in_front_of_a_type_word_still_answers_the_type() {
    // 「專線」是類型詞，facts 在第一次查詢就可能答出來；這裡只要求答到同一支號碼，
    // 不規定那是第一次查到的還是放寬後查到的。
    let noisy = "幫我看一下客服專線";
    let base = "客服專線";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = fixture(background);
        if background {
            assert_noise_is_seen(&db, noisy, base);
        }
        let want = clean(&mut db, base);
        assert_ne!(want.answers, "[]", "前提：「{base}」答得出號碼");
        let got = ask(&mut db, noisy);
        assert_eq!(got.answers, want.answers, "{label}「{noisy}」");
    }
}

#[test]
fn an_unseen_topic_still_gets_nothing_even_when_every_other_word_is_seen() {
    let busy = fixture(true);
    for (noisy, seen) in [
        ("退款電話怎麼打", &["電話", "怎麼打"][..]),
        ("退款專線在哪裡", &["專線", "在哪裡"][..]),
        ("火星會議連結怎麼開", &["連結", "怎麼開"][..]),
    ] {
        for word in seen {
            assert_noise_is_seen(&busy, word, "");
        }
        nothing_everywhere(noisy, "主題沒看過，不可以拿別的電話／網址來湊");
    }
}

#[test]
fn a_type_word_that_contains_a_question_word_is_what_he_asked_for() {
    // 「什麼時候」「多少錢」本身就是答案種類。把它從「什麼」切掉，就變成去找
    // release candidate 這個詞本身——那不是他問的。
    nothing_everywhere(
        "release candidate 什麼時候",
        "他問的是日期；畫面上沒有日期就是空手",
    );
}

#[test]
fn an_unknown_word_in_the_middle_is_still_required_with_a_background() {
    for q in ["客服 退款 專線", "部署 退款 失敗"] {
        nothing_everywhere(q, "中間那個沒看過的字仍是必要條件");
    }
}

#[test]
fn a_question_that_already_finds_something_is_left_alone_with_a_background() {
    let mut busy = fixture(true);
    for q in [
        "部署失敗 ERR_DEPLOY_42",
        "客服專線",
        "release candidate",
        "月報連結",
    ] {
        let got = ask(&mut busy, q);
        assert!(!got.empty, "{q}");
        assert_eq!(got.searched, None, "{q}：原查詢有東西就不改字");
    }
}
