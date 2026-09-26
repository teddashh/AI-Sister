//! alpha.165 驗收：問「去哪裡找」「哪裡找得到」「可以在哪裡找到」，她答得和只打主題一樣。
//!
//! alpha.164 以前，放寬只切「哪裡」兩個字。前面的「去」留下來：「去哪裡找月報連結」
//! 剩一個「去」，什麼都不找。後面的「找得到」「可以找到」留在候選字裡，用過一陣子的
//! 索引看過這兩個說法，不會把它們當成沒看過的頭拿掉：「哪裡找得到月報連結」改用
//! 「找得到月報連結」，空手。主題沒看過的時候更糟：「哪裡找得到火星」改用「找得到」，
//! 拿任何一張寫著找得到的畫面來湊。
//!
//! 這一版「哪裡」連同前面緊接的「去」「可以在」「該去」、後面緊接的「找得到」
//! 「可以找到」「看得到」一起當問法拿掉。第一次查詢一個字都不改。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`，每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景是用過一陣子的中文畫面：看過每一個問法用語（前提斷言
//! 逐條確認），主題字一個都沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的中文畫面上多半看過的句子。
const BACKGROUND: &[&str] = &[
    "這個檔案在哪裡都找得到",
    "說明書裡可以找到下載點",
    "明天要去銀行",
    "你該去睡覺了",
    "我們應該去看看",
    "可以在這裡留言",
    "從窗外看得到山",
    "可以看到很多人",
    "排隊才找得到位子",
    "你能找到答案嗎",
    "可以到門口等",
    "鑰匙應該在抽屜裡",
    "終於找到了",
    "上次去的那家店",
];

/// 背景看過的問法用語。
const ASKING_WORDS: &[&str] = &[
    "哪裡",
    "找得到",
    "可以找到",
    "找到",
    "要去",
    "該去",
    "應該去",
    "可以在",
    "看得到",
    "可以看到",
    "才找得到",
    "能找到",
    "可以到",
    "應該在",
    "上次去",
];

const TOPIC_WORDS: &[&str] = &[
    "月報",
    "連結",
    "客服",
    "專線",
    "部署",
    "失敗",
    "ERR_DEPLOY",
    "週報",
    "摘要",
    "匯出",
    "晨會",
    "火星",
];

/// 他沒看過的東西。
const NEVER_SEEN: &str = "火星";

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

fn add_background(db: &mut Db, session: i64) {
    for (i, line) in BACKGROUND.iter().enumerate() {
        add(db, session, 6_000 + i as i64, 100 + i as u64, line);
    }
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
    add(&mut db, session, 5_100, 91, "週報摘要已寄出");
    add(&mut db, session, 5_200, 92, "匯出功能在右上角");
    add(&mut db, session, 5_300, 93, "晨會改到三樓");
    if background {
        add_background(&mut db, session);
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

fn both() -> [(&'static str, Db); 2] {
    [("沒有背景", fixture(false)), ("有背景", fixture(true))]
}

fn clean(db: &mut Db, q: &str) -> Seen {
    let s = ask(db, q);
    assert!(!s.empty, "對照組「{q}」自己要答得出來");
    assert_eq!(s.searched, None, "對照組「{q}」本身不可以是放寬過的");
    s
}

/// 前提：這些字真的看過。沒有這條，「有背景」那一半會安靜地變回沒有背景。
fn assert_seen(db: &Db, words: &[&str]) {
    assert!(!words.is_empty(), "前提：要有字可以檢查");
    for w in words {
        assert!(!db.search(w, 1).unwrap().is_empty(), "前提：看過「{w}」");
    }
}

/// 兩份資料庫都改用 `base` 去找，找到的和直接問 `base` 一模一樣。
fn found_as(noisy: &str, base: &str) {
    let mut plain = fixture(false);
    let mut busy = fixture(true);
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
        assert_eq!(got.searched, relaxed(base), "{label}「{noisy}」");
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
    for (label, mut db) in both() {
        let got = ask(&mut db, q);
        assert!(got.empty, "{label}「{q}」：{why}：{}", got.hits);
        assert_eq!(
            got.searched, None,
            "{label}「{q}」：沒有放寬就不說改用了什麼"
        );
    }
}

#[test]
fn the_background_holds_every_asking_word_and_no_topic_word() {
    for line in BACKGROUND {
        for word in TOPIC_WORDS {
            assert!(!line.contains(word), "背景「{line}」含主題字「{word}」");
        }
    }
    let plain = fixture(false);
    assert_seen(
        &plain,
        &[
            "月報連結",
            "客服專線",
            "ERR_DEPLOY_42",
            "週報摘要",
            "匯出功能",
            "晨會",
        ],
    );
    assert!(
        plain.search(NEVER_SEEN, 1).unwrap().is_empty(),
        "前提：沒看過「{NEVER_SEEN}」"
    );
    let busy = fixture(true);
    assert_seen(&busy, ASKING_WORDS);
}

/// 「哪裡」前面的「去」、後面的「找得到」「可以找到」「看得到」一起是問法。
#[test]
fn where_to_find_is_not_a_condition() {
    for q in [
        "去哪裡找月報連結",
        "去哪找月報連結",
        "要去哪裡找月報連結",
        "我要去哪裡找月報連結",
        "該去哪裡找月報連結",
        "應該去哪裡找月報連結",
        "上次去哪裡找月報連結",
        "哪裡找得到月報連結",
        "在哪裡找得到月報連結",
        "在哪裡可以找到月報連結",
        "哪裡能找到月報連結",
        "我在哪裡找到月報連結的",
        "哪裡看得到月報連結",
        "哪裡可以看到月報連結",
        "在哪裡才找得到月報連結",
        "要去哪裡才找得到月報連結",
    ] {
        found_as(q, "月報連結");
    }
    found_as("哪裡找得到 ERR_DEPLOY_42", "ERR_DEPLOY_42");
}

/// 主題在前面也一樣：「月報連結該去哪裡找」問的是月報連結。
#[test]
fn the_topic_can_come_first() {
    for q in [
        "月報連結該去哪裡找",
        "月報連結可以在哪裡找到",
        "月報連結應該在哪裡找",
        "月報連結可以到哪裡找",
    ] {
        found_as(q, "月報連結");
    }
    // 尾巴的「原因」照 alpha.163 拿掉。
    found_as("部署失敗的原因去哪裡找", "部署失敗");
    found_as("去哪裡找部署失敗的原因", "部署失敗");
}

/// 問號碼的，「我最後看到的是」照樣是那支號碼。
#[test]
fn a_phone_number_still_comes_back() {
    found_as("去哪裡找客服專線", "客服專線");
    for (label, mut db) in both() {
        let got = ask(&mut db, "去哪裡找客服專線");
        assert!(
            got.answers.contains("0800-000-123"),
            "{label}：{}",
            got.answers
        );
    }
}

/// 「要」「能」「會」常是名詞的最後一個字，不當成「可以」「該」那種問法拿掉。
#[test]
fn a_name_ending_like_an_asking_word_keeps_its_last_character() {
    for (q, base) in [
        ("摘要去哪裡找", "摘要"),
        ("週報摘要去哪裡找", "週報摘要"),
        ("晨會在哪裡開", "晨會"),
        ("匯出功能在哪裡找得到", "匯出功能"),
    ] {
        found_as(q, base);
    }
}

/// 主題沒看過就是空手，不拿寫著找得到的畫面來湊；只有問法的也一樣。
#[test]
fn a_topic_never_seen_is_still_nothing() {
    for q in [
        format!("去哪裡找{NEVER_SEEN}"),
        format!("哪裡找得到{NEVER_SEEN}"),
        format!("在哪裡可以找到{NEVER_SEEN}"),
        format!("{NEVER_SEEN}去哪裡找"),
        "哪裡找得到".to_string(),
        "哪裡看得到".to_string(),
        "哪裡可以看到".to_string(),
        "去哪裡找".to_string(),
    ] {
        nothing_everywhere(&q, "主題沒看過，不可以拿別的畫面來湊");
    }
}

/// 登記在案、這一版不修：名詞只有兩個字、緊接著「在哪裡」，而且第一個字是虛字
/// （「看板」的「看」）或最後一個字是問句頭尾的用語（「摘要」的「要」）。問句頭尾
/// 那一步把「在哪裡」和「要」一起剝掉，剩一個字，不放寬。
#[test]
fn a_two_character_name_before_zai_nali_is_still_nothing() {
    for (label, mut db) in both() {
        let got = ask(&mut db, "摘要在哪裡");
        assert!(got.empty, "{label}：{}", got.hits);
        assert_eq!(got.searched, None, "{label}");
    }
    let mut db = fixture(false);
    let session = db.start_session("test", "test").unwrap();
    add(&mut db, session, 7_000, 120, "看板已更新");
    let got = ask(&mut db, "看板在哪裡");
    assert!(got.empty, "{}", got.hits);
    assert_eq!(got.searched, None);
    let got = ask(&mut db, "哪裡有看板");
    assert!(!got.empty, "對照組：換個問法找得到");
}

/// 登記在案、這一版不修：「要」不在「去」前面的問法裡（「摘要去哪裡找」要留著「摘要」），
/// 「月報連結要去哪裡找」剝完是「月報連結要」，要靠索引那一步把沒看過的「結要」放掉。
/// 畫面上看過「連結要」的機器上，那一步停在「結要」，改用「月報連結要」、空手。「該去」
/// 在表上，不靠索引，所以 Windows 清單問「該去」不問「要去」。
#[test]
fn yao_before_qu_leans_on_the_index() {
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = fixture(background);
        let session = db.start_session("test", "test").unwrap();
        add(&mut db, session, 7_000, 130, "這個連結要記得更新");
        assert_seen(&db, &["連結要"]);
        let got = ask(&mut db, "月報連結要去哪裡找");
        assert!(got.empty, "{label}：{}", got.hits);
        assert_eq!(got.searched, relaxed("月報連結要"), "{label}");
        let got = ask(&mut db, "月報連結該去哪裡找");
        assert_eq!(got.searched, relaxed("月報連結"), "{label}");
        assert!(got.hits.contains("月報連結已更新"), "{label}：{}", got.hits);
        let got = clean(&mut db, "月報連結");
        assert!(got.hits.contains("月報連結已更新"), "{label}：{}", got.hits);
    }
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.165 那一節逐題照打：記事本那兩行是同一張畫面。
/// 清單引號裡的句子就是這裡的字面值，這裡改了清單要跟著改。
///
/// 同一台機器上多半也照 alpha.162 到 alpha.164 那幾節打過記事本，那幾張也在。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str = "報價單連結已寄出\n維修專線 02-2345-6789";
    const NOTEPAD_162: &str = "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123";
    const NOTEPAD_163: &str = "週報網址已更新\n同步失敗 ERR_SYNC_7\n客服專線 0800-000-123";
    const NOTEPAD_164: &str =
        "Orion build log uploaded\nvendor contract signed by legal\nERR_UPLOAD_9 retry queued";
    const LINK: &str = "我對不到你打的那一串，所以改用「報價單連結」去找。";
    const PHONE: &str = "我對不到你打的那一串，所以改用「維修專線」去找。";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = Db::open_in_memory().unwrap();
        let session = db.start_session("test", "test").unwrap();
        if background {
            add_background(&mut db, session);
            assert_seen(&db, ASKING_WORDS);
        }
        add(&mut db, session, 2_000, 87, NOTEPAD_162);
        add(&mut db, session, 3_000, 88, NOTEPAD_163);
        add(&mut db, session, 4_000, 89, NOTEPAD_164);
        add(&mut db, session, 5_000, 90, NOTEPAD);

        let notepad_only = |got: &Seen, q: &str| {
            assert!(
                got.hits.contains("報價單連結已寄出"),
                "{label}「{q}」要找到記事本那一張：{}",
                got.hits
            );
            for line in BACKGROUND {
                assert!(!got.hits.contains(line), "{label}「{q}」：{}", got.hits);
            }
            for other in ["週報網址已更新", "Orion build log"] {
                assert!(!got.hits.contains(other), "{label}「{q}」：{}", got.hits);
            }
        };
        let link = ask(&mut db, "報價單連結");
        let phone = ask(&mut db, "維修專線");
        for (q, got) in [("報價單連結", &link), ("維修專線", &phone)] {
            assert_eq!(got.searched, None, "{label}基準線「{q}」");
            notepad_only(got, q);
        }
        for (q, said, want) in [
            ("去哪裡找報價單連結", LINK, &link),
            ("報價單連結該去哪裡找", LINK, &link),
            ("哪裡找得到報價單連結", LINK, &link),
            ("報價單連結可以在哪裡找到", LINK, &link),
            ("去哪裡找維修專線", PHONE, &phone),
        ] {
            let got = ask(&mut db, q);
            assert_eq!(
                got.searched
                    .as_ref()
                    .map(SearchAdjustment::message)
                    .as_deref(),
                Some(said),
                "{label}「{q}」"
            );
            notepad_only(&got, q);
            assert_eq!(got.same_things, want.same_things, "{label}「{q}」");
        }
        let got = ask(&mut db, "去哪裡找維修專線");
        assert!(
            got.answers.contains("02-2345-6789") && !got.answers.contains("0800-000-123"),
            "{label}：「我最後看到的是」底下是記事本那支號碼：{}",
            got.answers
        );
        assert!(
            db.search("鴕鳥", 1).unwrap().is_empty(),
            "前提：沒看過「鴕鳥」"
        );
        for q in ["去哪裡找鴕鳥", "哪裡找得到鴕鳥"] {
            let got = ask(&mut db, q);
            assert!(got.empty, "{label}「{q}」要印「沒有找到。」：{}", got.hits);
            assert_eq!(
                got.searched, None,
                "{label}「{q}」標題下面不可以有「改用」那一行"
            );
        }
    }
}
