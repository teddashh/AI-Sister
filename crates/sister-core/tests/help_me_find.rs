//! alpha.167 驗收：請她幫忙找，「可以幫我找月報連結嗎」「月報連結麻煩妳幫我查一下」
//! 「can you find the build log」，她答得和只打主題一樣。
//!
//! alpha.166 以前，放寬拿得掉開頭的「幫我找」，拿不掉前面的「可以」「能不能」「麻煩你」，
//! 也拿不掉主題後面的「可以幫我找」。用過一陣子的索引看過「幫我」「可以」，不會把它們當成
//! 沒看過的頭尾拿掉：「可以幫我找月報連結嗎」改用「幫我找月報連結」、空手；主題沒看過的時候
//! 更糟，「可以幫我找火星嗎」改用「幫我」，拿一張寫著幫我的畫面來湊。英文的「can you find
//! the build log」只要索引看過 can 就空手，語料裡「release candidate」的 can 也算。
//!
//! 這一版把「幫我」「幫忙」「替我」接「找」「查」「搜」「看」「翻一下」、前面或後面先接的
//! 「可以」「能不能」「麻煩你」「請」，和英文的「can you」「please」「help me」接 find、
//! look up，一起當問法拿掉。「妳」和「你」一樣是虛字。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`，每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景是用過一陣子的畫面：看過每一個請她幫忙的說法（前提斷言逐條
//! 確認），主題字一個都沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的畫面上多半看過的句子。
const BACKGROUND: &[&str] = &[
    "可以幫我拿一下嗎",
    "能不能幫我看一下這個",
    "可不可以幫忙搬家",
    "麻煩你幫我關門",
    "麻煩妳了",
    "請你幫我拿傘",
    "請妳先走",
    "請幫忙填表",
    "替我向他問好",
    "幫我找找鑰匙",
    "幫我找出原因",
    "幫我查查天氣",
    "幫我查詢餘額",
    "幫我搜尋餐廳",
    "幫我搜一下地址",
    "幫我翻一下這頁",
    "妳可以先回家",
    "能找到就好",
    "can you help me with this",
    "could you send it again",
    "would you like some tea",
    "will you be there tonight",
    "please look up the address",
    "help me find my keys",
    "search for anything",
    "pull up a chair",
    "look for the sign",
    "locate your nearest store",
    "show me the way",
];

/// 背景看過的說法。
const ASKING_WORDS: &[&str] = &[
    "可以幫我",
    "能不能",
    "可不可以",
    "麻煩你",
    "麻煩妳",
    "請你",
    "請妳",
    "幫忙",
    "替我",
    "幫我找",
    "找找",
    "找出",
    "查查",
    "查詢",
    "搜尋",
    "搜一下",
    "翻一下",
    "妳可以",
    "找到",
    "can you",
    "could you",
    "would you",
    "will you",
    "please",
    "look up",
    "help me",
    "find",
    "search for",
    "pull up",
    "look for",
    "locate",
    "show me",
];

const TOPIC_WORDS: &[&str] = &[
    "月報",
    "連結",
    "客服",
    "專線",
    "部署",
    "火星",
    "build",
    "log",
    "err_deploy",
    "mars",
];

/// 他沒看過的東西。
const NEVER_SEEN: &str = "火星";
const NEVER_SEEN_EN: &str = "mars";

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
    add(
        &mut db,
        session,
        5_100,
        91,
        "Build log uploaded to artifacts",
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
        let lower = line.to_lowercase();
        for word in TOPIC_WORDS {
            assert!(!lower.contains(word), "背景「{line}」含主題字「{word}」");
        }
    }
    let plain = fixture(false);
    assert_seen(&plain, &["月報連結", "客服專線", "build log"]);
    for w in [NEVER_SEEN, NEVER_SEEN_EN] {
        assert!(
            plain.search(w, 1).unwrap().is_empty(),
            "前提：沒看過「{w}」"
        );
    }
    let busy = fixture(true);
    assert_seen(&busy, ASKING_WORDS);
    for w in [NEVER_SEEN, NEVER_SEEN_EN] {
        assert!(
            busy.search(w, 1).unwrap().is_empty(),
            "前提：背景裡也沒有「{w}」"
        );
    }
}

/// 主題前面請她幫忙，連同先接的「可以」「能不能」「麻煩你」「請」，一起是問法。
#[test]
fn asking_her_to_look_before_the_topic_is_not_a_condition() {
    for q in [
        "可以幫我找月報連結嗎",
        "可以幫我找一下月報連結嗎",
        "能幫我找月報連結嗎",
        "能不能幫我找月報連結",
        "可不可以幫我找月報連結",
        "你可以幫我找月報連結嗎",
        "妳可以幫我找月報連結嗎",
        "妳能不能幫我找月報連結",
        "妳幫我找月報連結",
        "麻煩幫我找月報連結",
        "麻煩你幫我找月報連結",
        "麻煩妳幫我查一下月報連結",
        "請你幫我找月報連結",
        "請妳幫我找月報連結",
        "請幫忙找月報連結",
        "幫忙找一下月報連結",
        "可以幫忙找月報連結嗎",
        "替我找月報連結",
        "可以幫我查月報連結嗎",
        "能不能幫我查一下月報連結",
        "幫我搜月報連結",
        "幫我搜一下月報連結",
        "幫我搜尋月報連結",
        "幫我查詢月報連結",
        "幫我找找月報連結",
        "幫我查查月報連結",
        "幫我翻一下月報連結",
        "可以幫我看一下月報連結嗎",
        "能不能找到月報連結",
        "可不可以找到月報連結",
    ] {
        found_as(q, "月報連結");
    }
}

/// 主題在前面也一樣：「月報連結可以幫我找嗎」問的是月報連結。
#[test]
fn asking_her_to_look_after_the_topic_is_not_a_condition() {
    for q in [
        "月報連結可以幫我找嗎",
        "月報連結可以幫我找一下嗎",
        "月報連結能不能幫我找",
        "月報連結麻煩幫我找一下",
        "月報連結麻煩妳幫我查一下",
        "月報連結請你幫我找一下",
        "月報連結妳可以幫我找嗎",
        "月報連結幫我找到了嗎",
        "月報連結幫我查詢一下",
        "月報連結幫我找找",
        "月報連結可以找到嗎",
    ] {
        found_as(q, "月報連結");
    }
}

/// 英文也一樣：「can you」「please」「help me」接 find、look up。
#[test]
fn an_english_request_is_not_a_condition() {
    for q in [
        "can you find the build log",
        "could you find the build log",
        "can you help me find the build log",
        "help me find the build log",
        "please search for the build log",
        "could you please look up the build log",
        "can you pull up the build log",
        "look for the build log",
    ] {
        found_as(q, "build log");
    }
}

/// 「妳」和「你」一樣是虛字：第一次查詢就是那張，不必放寬。
#[test]
fn ni_with_the_female_radical_is_filler_like_ni() {
    for (label, mut db) in both() {
        let want = clean(&mut db, "月報連結");
        for q in ["妳有看到月報連結嗎", "你有看到月報連結嗎"] {
            let got = ask(&mut db, q);
            assert_eq!(got.searched, None, "{label}「{q}」");
            assert_eq!(got.same_things, want.same_things, "{label}「{q}」");
        }
    }
}

/// 問號碼的，「我最後看到的是」照樣是那支號碼。
#[test]
fn a_phone_number_still_comes_back() {
    for q in [
        "可以幫我找客服專線嗎",
        "能不能幫我查客服專線",
        "妳可以幫我查一下客服專線嗎",
        "客服專線可以幫我找嗎",
    ] {
        found_as(q, "客服專線");
        for (label, mut db) in both() {
            let got = ask(&mut db, q);
            assert!(
                got.answers.contains("0800-000-123"),
                "{label}「{q}」：{}",
                got.answers
            );
        }
    }
}

/// 主題沒看過就是空手，不拿寫著幫我、可以、can 的畫面來湊。
#[test]
fn a_topic_never_seen_is_still_nothing() {
    for q in [
        format!("可以幫我找{NEVER_SEEN}嗎"),
        format!("能不能幫我找{NEVER_SEEN}"),
        format!("妳可以幫我找{NEVER_SEEN}嗎"),
        format!("麻煩幫我查{NEVER_SEEN}"),
        format!("{NEVER_SEEN}可以幫我找嗎"),
        format!("{NEVER_SEEN}麻煩妳幫我查一下"),
        format!("can you find {NEVER_SEEN_EN}"),
        format!("could you help me find {NEVER_SEEN_EN}"),
        format!("please look up {NEVER_SEEN_EN}"),
    ] {
        nothing_everywhere(&q, "主題沒看過，不可以拿別的畫面來湊");
    }
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.167 那一節逐題照打：記事本那三行是同一張畫面。
/// 清單引號裡的句子就是這裡的字面值，這裡改了清單要跟著改。
///
/// 同一台機器上多半也照 alpha.162 到 alpha.166 那幾節打過記事本，那幾張也在。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str = "出差報告已上傳\n客訴專線 0800-555-777\nAtlas shipping manifest uploaded";
    const NOTEPAD_162: &str = "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123";
    const NOTEPAD_163: &str = "週報網址已更新\n同步失敗 ERR_SYNC_7\n客服專線 0800-000-123";
    const NOTEPAD_164: &str =
        "Orion build log uploaded\nvendor contract signed by legal\nERR_UPLOAD_9 retry queued";
    const NOTEPAD_165: &str = "報價單連結已寄出\n維修專線 02-2345-6789";
    const NOTEPAD_166: &str = "合約草稿已存檔\n保險專線 0800-222-333";
    const REPORT: &str = "我對不到你打的那一串，所以改用「出差報告」去找。";
    const MANIFEST: &str = "我對不到你打的那一串，所以改用「Atlas shipping manifest」去找。";
    const PHONE: &str = "我對不到你打的那一串，所以改用「客訴專線」去找。";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = Db::open_in_memory().unwrap();
        let session = db.start_session("test", "test").unwrap();
        if background {
            add_background(&mut db, session);
            assert_seen(&db, ASKING_WORDS);
        }
        add(&mut db, session, 2_000, 85, NOTEPAD_162);
        add(&mut db, session, 3_000, 86, NOTEPAD_163);
        add(&mut db, session, 4_000, 87, NOTEPAD_164);
        add(&mut db, session, 4_500, 88, NOTEPAD_165);
        add(&mut db, session, 4_800, 89, NOTEPAD_166);
        add(&mut db, session, 5_000, 90, NOTEPAD);

        let notepad_only = |got: &Seen, q: &str| {
            assert!(
                got.hits.contains("出差報告已上傳"),
                "{label}「{q}」要找到記事本那一張：{}",
                got.hits
            );
            for line in BACKGROUND {
                assert!(!got.hits.contains(line), "{label}「{q}」：{}", got.hits);
            }
            for other in [
                "週報網址已更新",
                "Orion build log",
                "報價單連結已寄出",
                "合約草稿已存檔",
            ] {
                assert!(!got.hits.contains(other), "{label}「{q}」：{}", got.hits);
            }
        };
        let report = ask(&mut db, "出差報告");
        let phone = ask(&mut db, "客訴專線");
        let manifest = ask(&mut db, "Atlas shipping manifest");
        for (q, got) in [
            ("出差報告", &report),
            ("客訴專線", &phone),
            ("Atlas shipping manifest", &manifest),
        ] {
            assert_eq!(got.searched, None, "{label}基準線「{q}」");
            notepad_only(got, q);
        }
        for (q, said, want) in [
            ("可以幫我找出差報告嗎", REPORT, &report),
            ("麻煩妳幫我查一下出差報告", REPORT, &report),
            ("出差報告能不能幫我找", REPORT, &report),
            ("出差報告可以幫我找一下嗎", REPORT, &report),
            (
                "can you find the Atlas shipping manifest",
                MANIFEST,
                &manifest,
            ),
            (
                "could you help me look up the Atlas shipping manifest",
                MANIFEST,
                &manifest,
            ),
            ("能不能幫我查客訴專線", PHONE, &phone),
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
        let got = ask(&mut db, "能不能幫我查客訴專線");
        assert!(
            got.answers.contains("0800-555-777")
                && !got.answers.contains("0800-000-123")
                && !got.answers.contains("02-2345-6789")
                && !got.answers.contains("0800-222-333"),
            "{label}：「我最後看到的是」底下是記事本那支號碼：{}",
            got.answers
        );
        for w in ["鴕鳥", "zorblat"] {
            assert!(db.search(w, 1).unwrap().is_empty(), "前提：沒看過「{w}」");
        }
        for q in ["可以幫我找鴕鳥嗎", "can you find the zorblat"] {
            let got = ask(&mut db, q);
            assert!(got.empty, "{label}「{q}」要印「沒有找到。」：{}", got.hits);
            assert_eq!(
                got.searched, None,
                "{label}「{q}」標題下面不可以有「改用」那一行"
            );
        }
    }
}
