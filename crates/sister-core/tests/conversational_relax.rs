//! alpha.157 R3 驗收：口語開頭、「哪」開頭的問法，以及放寬之後不能只剩一個字或只剩類型詞。
//!
//! R2 在問句詞前面有內容時留前面、丟後面。前面是「所以」「可是」「我想問」這種
//! 口語開頭時，她就拿那兩個字去找；用過一陣子的索引裡它們一定看過，於是
//! 「所以為什麼部署失敗」改用「所以」去找，回來五張不相干的畫面。同一輪還量到：
//!
//! - 「怎麼回事」切到只剩「回」一個字，拿它去找。
//! - 「電話怎麼打」「應繳金額怎麼算」「哪個網址」切到只剩類型詞，拿任意一支電話、
//!   任意一筆金額、任意一個網址來湊。alpha.155 的版本說明寫過她不會這樣做。
//! - 「哪裡有客服電話」只切掉「哪」，改用「裡有客服電話」去找。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`。每一組都在「沒有背景」和
//! 「有背景」兩份資料庫上各跑一次；背景只放那一組需要的雜訊，前提斷言逐條確認
//! 那些字真的看過、而且背景沒有替主題補票。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 主題字。背景一個都不准有，否則「有背景」那一半找到的可能是背景自己。
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
    "應繳",
    "金額",
    "release",
    "candidate",
    "ERR_DEPLOY",
];

/// 口語開頭與「哪」開頭的問法。底下每一條問題的雜訊雙字都在這裡出現過。
const CHATTY: &[&str] = &[
    "所以為什麼會這樣",
    "我想問為什麼會這樣",
    "我想問一下這個",
    "所以現在怎麼辦",
    "但是我不知道",
    "可是我不想去",
    "不過東西在哪裡",
    "你知道嗎",
    "你記得嗎",
    "到底為什麼會這樣",
    "還有嗎",
    "哪裡有賣",
    "哪一個比較好",
    "晚點打電話給你",
    "這個字怎麼打",
    "but why not",
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

/// 帳單夾具＋「月報連結已更新」，再加上這一組自己的背景。
fn fixture(background: &[&str]) -> Db {
    for line in background {
        for word in TOPIC_WORDS {
            assert!(!line.contains(word), "背景「{line}」含主題字「{word}」");
        }
    }
    let corpus: sister_core::replay::Corpus = serde_json::from_str(include_str!(
        "../../../scenarios/recall-baseline.corpus.json"
    ))
    .unwrap();
    let mut db = Db::open_in_memory().unwrap();
    db.import_replay(&corpus, 100).unwrap();
    let session = db.start_session("test", "test").unwrap();
    add(&mut db, session, 5_000, 90, "月報連結已更新");
    for (i, line) in background.iter().enumerate() {
        add(&mut db, session, 6_000 + i as i64, 100 + i as u64, line);
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
/// 交界的那個雙字不算：它本來就不會在畫面上。
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

/// 前提：背景真的看過這些字。沒有這條，「有背景」那一半會安靜地變回剛裝好那一天。
fn assert_seen(db: &Db, words: &[&str], why: &str) {
    assert!(!words.is_empty(), "前提（{why}）：要有字可以檢查");
    for w in words {
        assert!(
            !db.search(w, 1).unwrap().is_empty(),
            "前提（{why}）：背景要看過「{w}」"
        );
    }
}

/// 前提：整句沒有出現在背景裡。否則第一次查詢就命中，根本不會走放寬。
fn assert_not_on_screen(db: &Db, text: &str) {
    assert!(
        db.search(text, 1).unwrap().is_empty(),
        "前提：背景不可以直接含「{text}」，那樣第一次查詢就命中了"
    );
}

/// 放寬之後要說拿「base」去找，而且答案和只打「base」一模一樣——兩份資料庫都是。
fn same_as_everywhere(noisy: &str, base: &str, background: &[&str], extra_seen: &[&str]) {
    let mut plain = fixture(&[]);
    let mut busy = fixture(background);
    let grams = noise_grams(noisy, base);
    let mut seen: Vec<&str> = grams.iter().map(String::as_str).collect();
    seen.extend_from_slice(extra_seen);
    assert_seen(&busy, &seen, noisy);
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

/// 兩份資料庫都空手，而且不說改用了什麼。
fn nothing_everywhere(q: &str, why: &str, background: &[&str]) {
    for (label, bg) in [("沒有背景", &[][..]), ("有背景", background)] {
        let mut db = fixture(bg);
        let got = ask(&mut db, q);
        assert!(
            got.empty,
            "{label}「{q}」：{why}（searched={:?} answers={} hits={}）",
            got.searched, got.answers, got.hits
        );
        assert_eq!(
            got.searched, None,
            "{label}「{q}」：沒有放寬就不說改用了什麼"
        );
    }
}

#[test]
fn the_topic_words_really_are_in_the_fixture() {
    // 反過來：主題字真的在夾具裡，背景排除它們才不是空轉。
    let db = fixture(&[]);
    for word in [
        "部署",
        "客服",
        "專線",
        "電信",
        "帳單",
        "月報",
        "應繳金額",
        "ERR_DEPLOY_42",
    ] {
        assert!(
            !db.search(word, 1).unwrap().is_empty(),
            "前提：夾具有「{word}」"
        );
    }
}

#[test]
fn a_spoken_lead_in_is_not_what_he_asked_about() {
    for (noisy, base) in [
        ("所以為什麼部署失敗", "部署失敗"),
        ("我想問為什麼部署失敗", "部署失敗"),
        ("所以部署失敗怎麼辦", "部署失敗"),
        ("我想問一下部署失敗怎麼辦", "部署失敗"),
        ("到底為什麼部署失敗", "部署失敗"),
        ("但是客服電話呢", "客服電話"),
        ("你知道客服電話嗎", "客服電話"),
        ("不過月報連結在哪裡", "月報連結"),
        ("你記得月報連結嗎", "月報連結"),
        ("還有月報連結呢", "月報連結"),
    ] {
        same_as_everywhere(noisy, base, CHATTY, &[]);
    }
}

#[test]
fn an_english_lead_in_is_not_what_he_asked_about() {
    same_as_everywhere(
        "but why ERR_DEPLOY_42",
        "ERR_DEPLOY_42",
        CHATTY,
        &["but", "why"],
    );
}

#[test]
fn a_question_word_that_starts_with_na_is_cut_whole() {
    for (noisy, base) in [
        ("哪裡有客服電話", "客服電話"),
        ("哪一個客服電話", "客服電話"),
    ] {
        same_as_everywhere(noisy, base, CHATTY, &[]);
    }
}

#[test]
fn a_lead_in_does_not_let_an_unseen_topic_through() {
    // R2 的主題保護拿「所以退款」當主題：「所以」看過，保護就放行。
    for (q, seen) in [
        ("所以退款電話怎麼打", &["所以", "電話", "怎麼", "麼打"][..]),
        ("可是退款專線在哪裡", &["可是", "專線", "在哪", "哪裡"][..]),
        ("哪裡有退款電話", &["哪裡", "裡有", "電話"][..]),
    ] {
        let busy = fixture(CHATTY);
        assert_seen(&busy, seen, q);
        nothing_everywhere(q, "主題沒看過，不可以拿別的電話來湊", CHATTY);
    }
}

/// 單獨一個中文字要前後有空白，unicode61 才會把它當一個 token，索引才算「看過」。
/// Windows OCR 常把中文逐字隔開，所以用過的索引裡這種 token 很多。
fn assert_lone_char_token(background: &[&str], ch: &str) {
    assert!(
        background
            .iter()
            .any(|line| line.split_whitespace().any(|t| t == ch)),
        "前提：背景要有單獨成詞的「{ch}」"
    );
}

#[test]
fn a_bare_question_never_searches_one_leftover_character() {
    let bg: &[&str] = &["請按 回 上一頁", "先 辦 再說", "可是我不想去"];
    assert_lone_char_token(bg, "回");
    assert_lone_char_token(bg, "辦");
    let busy = fixture(bg);
    assert_seen(&busy, &["可是"], "口語開頭");
    for q in ["怎麼回事", "這是怎麼回事", "怎麼辦", "可是怎麼辦"] {
        assert_not_on_screen(&busy, q);
        nothing_everywhere(q, "沒有主題；剩一個字不是查詢", bg);
    }
}

#[test]
fn a_type_word_alone_never_stands_in_for_the_topic() {
    for (q, bg, kind_only) in [
        (
            "電話怎麼打",
            &["晚點打電話給你", "回電 0912-345-678", "這個字怎麼打"][..],
            "電話",
        ),
        (
            "應繳金額怎麼算",
            &["午餐 NT$240", "這要怎麼算"][..],
            "應繳金額",
        ),
        (
            "哪個網址",
            &["網址貼在這裡", "https://example.org/menu", "哪個比較好"][..],
            "網址",
        ),
    ] {
        // 前提：只拿類型詞去找，背景真的會多出一筆別的東西——這一條擋的就是那一筆。
        let mut plain = fixture(&[]);
        let mut busy = fixture(bg);
        let alone_plain = ask(&mut plain, kind_only);
        let alone_busy = ask(&mut busy, kind_only);
        assert_ne!(
            alone_busy.answers, "[]",
            "前提：「{kind_only}」在背景上有答案"
        );
        assert_ne!(
            alone_busy.answers, alone_plain.answers,
            "前提：背景替「{kind_only}」多了一筆別的"
        );
        assert_seen(&busy, &[kind_only], q);
        nothing_everywhere(q, "類型詞不能冒充主題", bg);
    }
}
