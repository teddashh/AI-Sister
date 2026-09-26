//! alpha.164 驗收：英文問句答得和只打主題一樣。
//!
//! alpha.163 以前，英文問句詞後面倒裝的助動詞留在放寬的候選字裡，是必要條件：
//! 「when is ERR_DEPLOY_42」改用「is ERR_DEPLOY_42」，畫面上沒有 is 就空手；
//! 「where can I find the build log」改用「can I find the build log」。句子中間的
//! for、with、of 在切段那一步也是條件。更糟的是主題沒看過的時候：「where is the
//! plorkin」把沒看過的 plorkin 拿掉，改用「is the」，拿任何一張寫著 is the 的畫面來湊。
//!
//! 這一版放寬時，問句詞後面緊接的助動詞和主詞代名詞一起拿掉，for、with、of 這幾個
//! 接頭和中文的「的」一樣切開。第一次查詢一個字都不改。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`，每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景是用過一陣子的英文畫面：看過每一個問法用語（前提斷言
//! 逐條確認），主題字一個都沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的英文畫面上多半看過的句子。
const BACKGROUND: &[&str] = &[
    "it is what it is",
    "where is the key",
    "can you find the time",
    "does it mean anything",
    "they will go home at noon",
    "we would ship it by Friday",
    "the store is closed for the day",
    "news from the office about the trip",
    "a cup of tea with milk",
    "has anyone seen this in the morning",
    "why is it failing again",
    "who are you",
];

/// 背景看過的問法用語。
const ASKING_WORDS: &[&str] = &[
    "is", "are", "can", "find", "does", "will", "would", "has", "they", "it", "for", "with",
    "from", "of", "in", "at", "by", "about", "mean", "go", "ship", "failing",
];

const TOPIC_WORDS: &[&str] = &[
    "release",
    "candidate",
    "ERR_DEPLOY",
    "build",
    "log",
    "vendor",
    "contract",
    "signed",
    "artifact",
    "plorkin",
];

/// 他沒看過的東西。
const NEVER_SEEN: &str = "plorkin";

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
    add(
        &mut db,
        session,
        5_000,
        90,
        "Build log uploaded to artifacts",
    );
    add(
        &mut db,
        session,
        5_100,
        91,
        "Vendor contract signed by legal",
    );
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

/// 前提：這些字真的看過。沒有這條，「有背景」那一半會安靜地變回沒有背景。
fn assert_seen(db: &Db, words: &[&str]) {
    assert!(!words.is_empty(), "前提：要有字可以檢查");
    for w in words {
        assert!(!db.search(w, 1).unwrap().is_empty(), "前提：看過「{w}」");
    }
}

/// 兩份資料庫都放寬成 `base`、說明是 `said`，找到的和直接問 `base` 一模一樣。
fn found_as(noisy: &str, base: &str, said: Option<SearchAdjustment>) {
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
        assert_eq!(got.searched, said, "{label}「{noisy}」");
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
        let lower = line.to_lowercase();
        for word in TOPIC_WORDS {
            assert!(
                !lower.contains(&word.to_lowercase()),
                "背景「{line}」含主題字「{word}」"
            );
        }
    }
    let plain = fixture(false);
    assert_seen(&plain, &["ERR_DEPLOY_42", "build log", "vendor contract"]);
    assert!(
        plain.search(NEVER_SEEN, 1).unwrap().is_empty(),
        "前提：沒看過「{NEVER_SEEN}」"
    );
    let busy = fixture(true);
    assert_seen(&busy, ASKING_WORDS);
    assert_seen(&busy, &["is the"]);
}

/// 問句詞後面的 is：畫面上沒有 is 也找得到。when 照「什麼時候」多講一句。
#[test]
fn the_auxiliary_after_a_question_word_is_not_a_condition() {
    for (q, base, said) in [
        (
            "when is ERR_DEPLOY_42",
            "ERR_DEPLOY_42",
            relaxed_when("ERR_DEPLOY_42"),
        ),
        (
            "When Is ERR_DEPLOY_42",
            "ERR_DEPLOY_42",
            relaxed_when("ERR_DEPLOY_42"),
        ),
        (
            "so when is ERR_DEPLOY_42",
            "ERR_DEPLOY_42",
            relaxed_when("ERR_DEPLOY_42"),
        ),
        (
            "when is ERR_DEPLOY_42 發生的",
            "ERR_DEPLOY_42",
            relaxed_when("ERR_DEPLOY_42"),
        ),
        (
            "where is ERR_DEPLOY_42",
            "ERR_DEPLOY_42",
            relaxed("ERR_DEPLOY_42"),
        ),
        (
            "what is ERR_DEPLOY_42",
            "ERR_DEPLOY_42",
            relaxed("ERR_DEPLOY_42"),
        ),
        ("who is the vendor", "vendor", relaxed("vendor")),
        ("where is the build log", "build log", relaxed("build log")),
        ("WHERE IS THE BUILD LOG", "BUILD LOG", relaxed("BUILD LOG")),
        (
            "why isn't the build log uploaded",
            "build log uploaded",
            relaxed("build log uploaded"),
        ),
        (
            "when is the vendor contract",
            "vendor contract",
            relaxed_when("vendor contract"),
        ),
    ] {
        found_as(q, base, said);
    }
}

/// 助動詞後面的主詞也一起拿掉；find 本來就是問句頭尾的用語。
#[test]
fn the_subject_after_the_auxiliary_goes_with_it() {
    for q in [
        "where can I find the build log",
        "where do I find the build log",
        "where did you find the build log",
        "where’d you find the build log",
    ] {
        found_as(q, "build log", relaxed("build log"));
    }
}

/// for、with、of 和「月報的連結」的「的」一樣切開：畫面上不一定有。
#[test]
fn a_joint_word_is_split_like_de() {
    for (q, base, said) in [
        (
            "the contract with the vendor",
            "contract vendor",
            relaxed("contract vendor"),
        ),
        (
            "where is the log for the build",
            "log build",
            relaxed("log build"),
        ),
        (
            "the release candidate for alpha",
            "release candidate alpha",
            relaxed("release candidate alpha"),
        ),
        (
            "when was the contract with the vendor signed",
            "contract vendor signed",
            relaxed_when("contract vendor signed"),
        ),
    ] {
        found_as(q, base, said);
    }
}

/// 主題沒看過就是空手，不拿寫著 is the 的畫面來湊。for、with、of 後面那段看過
/// 也一樣，不改用那一段去找。
#[test]
fn a_topic_never_seen_is_still_nothing() {
    for q in [
        format!("where is the {NEVER_SEEN}"),
        format!("when is the {NEVER_SEEN}"),
        format!("who is the {NEVER_SEEN}"),
        format!("where can I find the {NEVER_SEEN}"),
        // 接頭後面那段看過也一樣：問的是接頭前面那個東西。
        format!("where is the {NEVER_SEEN} for alpha"),
        format!("{NEVER_SEEN} for alpha"),
        format!("where is the {NEVER_SEEN} with the vendor"),
        format!("the {NEVER_SEEN} of ERR_DEPLOY_42"),
        "where is it".to_string(),
        "when is it".to_string(),
    ] {
        nothing_everywhere(&q, "主題沒看過，不可以拿別的畫面來湊");
    }
}

/// 登記在案的代價：主詞後面的動詞照舊是條件。背景看過 mean、go 的話，
/// 「what does ERR_DEPLOY_42 mean」改用「ERR_DEPLOY_42 mean」，空手；沒看過就像
/// 以前一樣當成沒看過的尾巴拿掉。
#[test]
fn the_main_verb_after_the_subject_is_still_a_condition() {
    for (q, base, busy_said) in [
        (
            "what does ERR_DEPLOY_42 mean",
            "ERR_DEPLOY_42",
            "ERR_DEPLOY_42 mean",
        ),
        ("where does the build log go", "build log", "build log go"),
        (
            "why is ERR_DEPLOY_42 failing",
            "ERR_DEPLOY_42",
            "ERR_DEPLOY_42 failing",
        ),
        (
            "when will the release candidate ship",
            "release candidate",
            "release candidate ship",
        ),
    ] {
        let mut plain = fixture(false);
        let want = clean(&mut plain, base);
        let got = ask(&mut plain, q);
        assert_eq!(got.hits, want.hits, "沒有背景「{q}」");
        let mut busy = fixture(true);
        let got = ask(&mut busy, q);
        assert!(got.empty, "有背景「{q}」：{}", got.hits);
        let said = got.searched.as_ref().map(SearchAdjustment::message);
        assert!(
            said.as_deref()
                .is_some_and(|s| s.contains(&format!("改用「{busy_said}」"))),
            "有背景「{q}」：{said:?}"
        );
    }
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.164 那一節逐題照打：記事本那三行是同一張畫面。
/// 清單引號裡的句子就是這裡的字面值，這裡改了清單要跟著改。
///
/// 同一台機器上多半也照 alpha.162、alpha.163 那兩節打過記事本，那兩張也在。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str =
        "Orion build log uploaded\nvendor contract signed by legal\nERR_UPLOAD_9 retry queued";
    const NOTEPAD_162: &str = "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123";
    const NOTEPAD_163: &str = "週報網址已更新\n同步失敗 ERR_SYNC_7\n客服專線 0800-000-123";
    const LOG: &str = "我對不到你打的那一串，所以改用「Orion build log」去找。";
    const ERR: &str = "我對不到你打的那一串，所以改用「ERR_UPLOAD_9」去找。";
    const SIGNED: &str = "我對不到你打的那一串，所以改用「vendor contract signed」去找。底下每一筆的時間，是我記下那一筆的時候。";
    const LOG_FOR: &str = "我對不到你打的那一串，所以改用「build log Orion」去找。";
    const WITH: &str = "我對不到你打的那一串，所以改用「contract vendor」去找。";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = Db::open_in_memory().unwrap();
        let session = db.start_session("test", "test").unwrap();
        if background {
            add_background(&mut db, session);
            assert_seen(&db, ASKING_WORDS);
            assert_seen(&db, &["is the"]);
        }
        add(&mut db, session, 3_000, 88, NOTEPAD_162);
        add(&mut db, session, 4_000, 89, NOTEPAD_163);
        add(&mut db, session, 5_000, 90, NOTEPAD);

        let notepad_only = |got: &Seen, q: &str| {
            assert!(
                got.hits.contains("Orion build log uploaded"),
                "{label}「{q}」要找到記事本那一張：{}",
                got.hits
            );
            for line in BACKGROUND {
                assert!(!got.hits.contains(line), "{label}「{q}」：{}", got.hits);
            }
            assert!(
                !got.hits.contains("週報網址已更新"),
                "{label}「{q}」：{}",
                got.hits
            );
        };
        let log = ask(&mut db, "Orion build log");
        let err = ask(&mut db, "ERR_UPLOAD_9");
        let contract = ask(&mut db, "vendor contract");
        for (q, got) in [
            ("Orion build log", &log),
            ("ERR_UPLOAD_9", &err),
            ("vendor contract", &contract),
        ] {
            assert_eq!(got.searched, None, "{label}基準線「{q}」");
            notepad_only(got, q);
        }
        for (q, said, same_as) in [
            ("where is the Orion build log", LOG, Some(&log)),
            ("what is ERR_UPLOAD_9", ERR, Some(&err)),
            ("when was the vendor contract signed", SIGNED, None),
            ("where can I find the Orion build log", LOG, Some(&log)),
            ("where is the build log for Orion", LOG_FOR, None),
            ("the contract with the vendor", WITH, None),
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
            if let Some(want) = same_as {
                assert_eq!(got.same_things, want.same_things, "{label}「{q}」");
            }
        }
        assert!(
            db.search("zorblat", 1).unwrap().is_empty(),
            "前提：沒看過 zorblat"
        );
        for q in ["where is the zorblat", "where is the zorblat for Orion"] {
            let got = ask(&mut db, q);
            assert!(got.empty, "{label}「{q}」要印「沒有找到。」：{}", got.hits);
            assert_eq!(
                got.searched, None,
                "{label}「{q}」標題下面不可以有「改用」那一行"
            );
        }
    }
}
