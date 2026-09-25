//! alpha.161 驗收：「什麼時候」她答得出來。
//!
//! alpha.157 到 alpha.160 把「什麼時候」和「多少錢」一樣當成答案種類：畫面上沒有
//! 日期就空手。可是她是記錄器——「ERR_DEPLOY_42 什麼時候發生的」，她記下那張畫面的
//! 時候就是答案。這一版的放寬把問時間的字切掉、拿主題去找，找到的話多講一句「底下
//! 每一筆的時間，是我記下那一筆的時候」。畫面上真的寫著日期時間的（「週會 9/30
//! 14:00」），第一次查詢照舊從 facts 答，不放寬、不多講。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`。每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景看過每一個問時間的字（前提斷言逐條確認），主題字一個
//! 都沒有。夾具裡另外有兩組別人的日期時間，答案不能隨手抓它們來湊。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 問時間的口語。每一條問題裡的雜訊字都在這裡出現過。
const BACKGROUND: &[&str] = &[
    "你什麼時候有空",
    "這是什麼時候的事",
    "那是什麼時候發生的",
    "是什麼時候出現的",
    "我什麼時候看到的",
    "作業什麼時候交",
    "現在幾點了",
    "那是幾點發生的",
    "何時出現還不知道",
    "问题是什么时候发生的",
    "when did it happen",
    "when was that",
];

const TOPIC_WORDS: &[&str] = &[
    "部署",
    "失敗",
    "客服",
    "電話",
    "專線",
    "週會",
    "會議",
    "預算",
    "財務",
    "退款",
    "報告",
    "期限",
    "release",
    "candidate",
    "ERR_DEPLOY",
    "ERR_TIMEOUT",
];

/// 夾具裡每一個日期時間。沒有主題的問題、和主題不在旁邊的問題，一個都不能答出來。
const DATES: &[&str] = &["9/30", "14:00", "2026-09-20", "03:14"];

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
    add(&mut db, session, 5_000, 90, "週會 9/30 14:00 在三樓會議室");
    add(
        &mut db,
        session,
        5_100,
        91,
        "2026-09-20 03:14 ERR_TIMEOUT_7 連線逾時",
    );
    add(&mut db, session, 5_200, 92, "預算表要交給財務");
    if background {
        for (i, line) in BACKGROUND.iter().enumerate() {
            add(&mut db, session, 6_000 + i as i64, 100 + i as u64, line);
        }
    }
    db
}

struct Seen {
    answers: String,
    raws: Vec<String>,
    hits: String,
    hit_texts: Vec<String>,
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
        raws: r.answers.iter().map(|a| a.latest.raw.clone()).collect(),
        hits: format!("{:?}", r.hits),
        hit_texts: r.hits.iter().map(|h| h.text.clone()).collect(),
        same_things: format!("{facts:?} {hits:?}"),
        searched: r.searched,
        empty: r.answers.is_empty() && r.hits.is_empty(),
    }
}

fn relaxed_when(x: &str) -> Option<SearchAdjustment> {
    Some(SearchAdjustment::RelaxedWhen(x.into()))
}

fn both() -> [(&'static str, Db); 2] {
    [("沒有背景", fixture(false)), ("有背景", fixture(true))]
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

/// `noisy` 拿掉 `base` 之後剩下的雜訊：中文切成和索引同一種雙字，英文取整個詞。
/// 跨在主題與雜訊交界的那個雙字不算：它本來就不會在畫面上。
fn noise(noisy: &str, base: &str) -> Vec<String> {
    let at = noisy
        .find(base)
        .unwrap_or_else(|| panic!("「{base}」要是「{noisy}」的一段"));
    let mut out = Vec::new();
    for piece in [&noisy[..at], &noisy[at + base.len()..]] {
        let chars: Vec<char> = piece.chars().collect();
        for w in chars.windows(2) {
            if cjk(w[0]) && cjk(w[1]) {
                out.push(w.iter().collect());
            }
        }
        out.extend(
            piece
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_owned),
        );
    }
    out
}

/// 前提：背景真的看過這些雜訊。沒有這條，「有背景」那一半會安靜地變回沒有背景。
fn assert_noise_is_seen(db: &Db, noisy: &str, base: &str) {
    let words = noise(noisy, base);
    assert!(!words.is_empty(), "前提：「{noisy}」要有雜訊可以檢查");
    for w in words {
        assert!(
            !db.search(&w, 1).unwrap().is_empty(),
            "前提：背景要看過「{w}」（來自「{noisy}」）"
        );
    }
}

/// 放寬成 `base`、找到的和直接問 `base` 一模一樣，而且說明講的是記下的時間。
fn found_as_when(noisy: &str, base: &str) {
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
            relaxed_when(base),
            "{label}「{noisy}」：要說實際拿去找的是「{base}」，時間是記下的時候"
        );
        assert_eq!(
            got.answers, want.answers,
            "{label}「{noisy}」：facts 要和「{base}」一模一樣，不能多抓一個日期"
        );
        assert_eq!(
            got.hits, want.hits,
            "{label}「{noisy}」：原文要和「{base}」一模一樣"
        );
    }
}

fn nothing_everywhere(q: &str, why: &str) {
    for (label, mut db) in both() {
        let got = ask(&mut db, q);
        assert!(got.empty, "{label}「{q}」：{why}");
        assert_eq!(
            got.searched, None,
            "{label}「{q}」：沒有放寬就不說改用了什麼"
        );
    }
}

#[test]
fn the_fixture_holds_what_the_other_tests_lean_on() {
    for line in BACKGROUND {
        for word in TOPIC_WORDS {
            assert!(!line.contains(word), "背景「{line}」含主題字「{word}」");
        }
        for date in DATES {
            assert!(!line.contains(date), "背景「{line}」含日期時間「{date}」");
        }
    }
    // 反過來：主題字和日期時間真的在夾具裡，上面那兩條才不是空轉。
    let mut db = fixture(false);
    for word in [
        "部署",
        "客服",
        "週會",
        "會議",
        "預算",
        "release",
        "ERR_DEPLOY_42",
        "ERR_TIMEOUT_7",
    ] {
        assert!(
            !db.search(word, 1).unwrap().is_empty(),
            "前提：夾具有「{word}」"
        );
    }
    let mut raws = ask(&mut db, "週會幾點").raws;
    raws.extend(ask(&mut db, "ERR_TIMEOUT_7 幾點").raws);
    for date in DATES {
        assert!(
            raws.iter().any(|raw| raw == date),
            "前提：「{date}」是一筆 facts，別的問題才有東西可以誤抓：{raws:?}"
        );
    }
}

#[test]
fn a_when_question_about_something_she_saw_finds_it() {
    for (noisy, base) in [
        ("ERR_DEPLOY_42 什麼時候發生的", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42 是什麼時候出現的", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42是什麼時候", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42 何時出現", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42 什么时候发生的", "ERR_DEPLOY_42"),
        ("我什麼時候看到 ERR_DEPLOY_42", "ERR_DEPLOY_42"),
        ("when did ERR_DEPLOY_42 happen", "ERR_DEPLOY_42"),
        ("when was ERR_DEPLOY_42", "ERR_DEPLOY_42"),
        ("部署失敗是什麼時候", "部署失敗"),
        ("什麼時候看到部署失敗", "部署失敗"),
        ("部署失敗幾點發生的", "部署失敗"),
        ("release candidate 什麼時候", "release candidate"),
    ] {
        found_as_when(noisy, base);
    }
}

/// 文件寫的代價：還沒到的事，畫面上又沒寫日期，她給的是看到那張畫面的時間。
/// 所以說明要講明那是記下的時候。
#[test]
fn something_not_due_yet_gets_the_moment_she_saw_it_and_says_so() {
    found_as_when("預算表什麼時候交", "預算表");
    for (label, mut db) in both() {
        let got = ask(&mut db, "預算表什麼時候交");
        assert_eq!(
            got.hit_texts,
            vec!["預算表要交給財務".to_owned()],
            "{label}：前提：那張畫面上沒有日期"
        );
        assert!(got.raws.is_empty(), "{label}：{:?}", got.raws);
    }
}

#[test]
fn a_date_written_beside_the_topic_is_answered_first_without_a_note() {
    for (q, want) in [
        ("週會是什麼時候", &["9/30", "14:00"][..]),
        ("週會什麼時候", &["9/30", "14:00"][..]),
        ("週會何時", &["9/30", "14:00"][..]),
        ("週會幾點", &["9/30", "14:00"][..]),
        ("週會是幾點", &["9/30", "14:00"][..]),
        ("週會在幾點", &["9/30", "14:00"][..]),
        ("週會的時間", &["9/30", "14:00"][..]),
        ("幾點的會議", &["9/30", "14:00"][..]),
        ("ERR_TIMEOUT_7 什麼時候發生的", &["2026-09-20", "03:14"][..]),
    ] {
        for (label, mut db) in both() {
            let got = ask(&mut db, q);
            assert_eq!(got.searched, None, "{label}「{q}」：第一次就答到，不改字");
            let mut raws = got.raws.clone();
            raws.sort();
            let mut want: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
            want.sort();
            assert_eq!(raws, want, "{label}「{q}」");
        }
    }
}

#[test]
fn the_joint_left_by_a_type_word_does_not_hide_the_answer() {
    for (label, mut db) in both() {
        for q in ["客服的電話", "客服的電話幾號"] {
            let got = ask(&mut db, q);
            assert_eq!(got.searched, None, "{label}「{q}」");
            assert_eq!(got.raws, vec!["0800-000-123".to_owned()], "{label}「{q}」");
        }
    }
}

/// 沒有在問哪一件事：拿最近看到的任意一個日期時間回答，看起來就像在報現在幾點。
#[test]
fn a_when_question_about_nothing_gets_no_date() {
    for (label, mut db) in both() {
        for q in ["什麼時候", "幾點", "幾點了", "when"] {
            let got = ask(&mut db, q);
            assert!(got.raws.is_empty(), "{label}「{q}」：{:?}", got.raws);
            assert_eq!(got.searched, None, "{label}「{q}」");
        }
        // 名詞類型詞照答，問時間的那一半不跟著抓日期
        for q in ["電話什麼時候打", "客服電話什麼時候打"] {
            let got = ask(&mut db, q);
            assert_eq!(got.raws, vec!["0800-000-123".to_owned()], "{label}「{q}」");
        }
    }
}

/// 放寬過但還是空手：底下沒有東西，就不講「底下每一筆的時間」。
#[test]
fn an_empty_relaxed_when_question_says_only_what_it_searched() {
    assert_noise_is_seen(&fixture(true), "部署 candidate 什麼時候", "部署 candidate");
    for (label, mut db) in both() {
        for word in ["部署", "candidate"] {
            assert!(
                !db.search(word, 1).unwrap().is_empty(),
                "前提：看過「{word}」"
            );
        }
        let got = ask(&mut db, "部署 candidate 什麼時候");
        assert!(got.empty, "{label}");
        assert_eq!(
            got.searched,
            Some(SearchAdjustment::Relaxed("部署 candidate".into())),
            "{label}"
        );
    }
}

#[test]
fn an_unseen_topic_still_gets_nothing() {
    nothing_everywhere("退款什麼時候", "主題沒看過，不可以拿別的日期來湊");
    nothing_everywhere("報告期限是什麼時候", "主題沒看過，不可以拿別的日期來湊");
}

/// 文件寫的代價：問句開頭的詞，第一個字剛好像虛字的話會被剝掉。「到期日」剩「期日」，
/// 「星期日」旁邊的日期也算進來。
#[test]
fn a_head_word_that_starts_like_filler_pays_the_documented_cost() {
    for (label, mut db) in both() {
        let session = db.start_session("calendar", "calendar").unwrap();
        add(&mut db, session, 7_000, 300, "2026/09/27 星期日");
        let got = ask(&mut db, "到期日是什麼時候");
        assert_eq!(got.searched, None, "{label}");
        let mut raws = got.raws.clone();
        raws.sort();
        assert_eq!(raws, vec!["2026/09/27", "星期日"], "{label}");
    }
}

/// 文件寫的還是找不到：英文的 when is 句型，is 不在問法表裡，也不是虛字。
#[test]
fn an_english_when_is_question_pays_the_documented_cost() {
    for (label, mut db) in both() {
        let got = ask(&mut db, "when is the release candidate");
        assert!(got.empty, "{label}");
        assert_eq!(
            got.searched,
            Some(SearchAdjustment::Relaxed("is the release candidate".into())),
            "{label}"
        );
    }
}

/// 錢照舊不切：截圖的時間不是金額。
#[test]
fn money_is_still_what_he_asked_for() {
    nothing_everywhere(
        "ERR_DEPLOY_42 多少錢",
        "他問的是金額；畫面上沒有金額就是空手",
    );
}
