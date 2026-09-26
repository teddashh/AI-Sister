//! alpha.166 驗收：說「找不到月報連結」「有沒有看到月報連結」「月報連結不見了」，她答得
//! 和只打主題一樣。
//!
//! alpha.165 以前，放寬只拿掉問句詞和「哪裡」前後的問法。他說自己怎麼找的那幾個字
//! 留在候選字裡：用過一陣子的索引看過「找不到」「看到」「不見」，不會把它們當成沒看過的
//! 頭尾拿掉。「找不到月報連結」照原樣找、空手；「有沒有看到月報連結」改用「看到月報連結」、
//! 空手。主題沒看過的時候更糟：「找不到火星」改用「找不到」，拿任何一張寫著找不到的畫面
//! 來湊。
//!
//! 這一版開頭的「找不到」「有沒有」「看到」「想找」，結尾的「找不到」「不見了」「有沒有」，
//! 連同前面先接的「沒」「一直」「還是」，一起當問法拿掉。第一次查詢一個字都不改。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`，每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景是用過一陣子的中文畫面：看過每一個找法（前提斷言逐條
//! 確認），主題字一個都沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的中文畫面上多半看過的句子。
const BACKGROUND: &[&str] = &[
    "怎麼找都找不到",
    "我沒看到你的訊息",
    "有沒有人看到我的傘",
    "從這裡看不到海",
    "錢包不見了",
    "終於找到了",
    "找得到路嗎",
    "一直找不到停車位",
    "還是看不到",
    "可以查到進度",
    "查得到嗎",
    "查不到紀錄",
    "想找個地方吃飯",
    "想看電影",
    "我想要找工作",
    "尋找失物",
    "搜一下就有",
    "搜不到",
    "還沒看到",
    "沒有找到",
    "找不著",
    "從窗外看得到山",
    "哪個地方比較近",
    "登入的畫面",
];

/// 背景看過的找法。
const ASKING_WORDS: &[&str] = &[
    "找不到",
    "沒看到",
    "有沒有",
    "看到",
    "看不到",
    "不見",
    "找到",
    "找得到",
    "一直找不到",
    "還是看不到",
    "可以查到",
    "查得到",
    "查不到",
    "想找",
    "想看",
    "想要找",
    "尋找",
    "搜一下",
    "搜不到",
    "還沒看到",
    "沒有找到",
    "找不著",
    "看得到",
    "哪個地方",
    "的畫面",
];

const TOPIC_WORDS: &[&str] = &[
    "月報", "連結", "客服", "專線", "部署", "失敗", "火星", "首都", "欄位",
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
    assert_seen(&plain, &["月報連結", "客服專線", "部署失敗"]);
    assert!(
        plain.search(NEVER_SEEN, 1).unwrap().is_empty(),
        "前提：沒看過「{NEVER_SEEN}」"
    );
    let busy = fixture(true);
    assert_seen(&busy, ASKING_WORDS);
}

/// 主題前面的找法，連同先接的「沒」「一直」「還是」「能」「可以」，一起是問法。
#[test]
fn looking_before_the_topic_is_not_a_condition() {
    for q in [
        "找不到月報連結",
        "我找不到月報連結",
        "一直找不到月報連結",
        "還是找不到月報連結",
        "怎麼都找不到月報連結",
        "找不著月報連結",
        "有沒有看到月報連結",
        "沒看到月報連結",
        "我沒看到月報連結",
        "沒有看到月報連結",
        "還沒看到月報連結",
        "看不到月報連結",
        "我看不到月報連結",
        "找得到月報連結嗎",
        "看得到月報連結嗎",
        "查得到月報連結嗎",
        "有找到月報連結嗎",
        "找到月報連結了嗎",
        "能找到月報連結嗎",
        "查不到月報連結",
        "可以查到月報連結嗎",
        "想看月報連結",
        "想要找月報連結",
        "我想找月報連結",
        "尋找月報連結",
        "搜一下月報連結",
        "有沒有月報連結",
    ] {
        found_as(q, "月報連結");
    }
}

/// 主題在前面也一樣：「月報連結不見了」問的是月報連結。
#[test]
fn looking_after_the_topic_is_not_a_condition() {
    for q in [
        "月報連結找不到",
        "月報連結一直找不到",
        "月報連結還是找不到",
        "月報連結找不著",
        "月報連結不見了",
        "月報連結看不到",
        "月報連結有沒有",
        "月報連結沒找到",
        "月報連結還沒找到",
        "月報連結有找到嗎",
        "月報連結找得到嗎",
    ] {
        found_as(q, "月報連結");
    }
}

/// 問句詞拿掉之後露出來的找法也拿掉；尾巴的「原因」照 alpha.163 拿掉。
#[test]
fn looking_next_to_a_question_word_is_not_a_condition() {
    for q in [
        "為什麼找不到月報連結",
        "怎麼找不到月報連結",
        "怎麼沒看到月報連結",
        "為什麼我看不到月報連結",
        "月報連結怎麼找不到",
        "月報連結怎麼不見了",
        "月報連結找不到怎麼辦",
    ] {
        found_as(q, "月報連結");
    }
    found_as("找不到部署失敗的原因", "部署失敗");
    found_as("部署失敗的原因一直找不到", "部署失敗");
}

/// 「哪裡」後面的「查得到」「查詢」、「哪個地方」也是問法；alpha.165 的「哪裡」照舊。
#[test]
fn where_takes_searching_and_which_place_too() {
    for q in [
        "哪裡查得到月報連結",
        "哪裡可以查到月報連結",
        "哪裡查詢月報連結",
        "哪裡可以查詢月報連結",
        "去哪裡搜尋月報連結",
        "哪個地方有月報連結",
        "月報連結可以在哪裡找到",
        "月報連結在哪裡可以找到",
        "月報連結哪裡看得到",
    ] {
        found_as(q, "月報連結");
    }
}

/// 問號碼的，「我最後看到的是」照樣是那支號碼。
#[test]
fn a_phone_number_still_comes_back() {
    for q in ["有沒有看到客服專線", "找不到客服專線"] {
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

/// 主題沒看過就是空手，不拿寫著找不到、看到、不見的畫面來湊。「火星的畫面」問的是
/// 火星：拿掉沒看過的「火星」只剩「的畫面」，不改用從「的」開始的字。
#[test]
fn a_topic_never_seen_is_still_nothing() {
    for q in [
        format!("找不到{NEVER_SEEN}"),
        format!("有沒有看到{NEVER_SEEN}"),
        format!("沒看到{NEVER_SEEN}"),
        format!("{NEVER_SEEN}不見了"),
        format!("{NEVER_SEEN}找不到"),
        format!("尋找{NEVER_SEEN}"),
        format!("有沒有{NEVER_SEEN}"),
        format!("哪個地方有{NEVER_SEEN}"),
        format!("找不到{NEVER_SEEN}的畫面"),
        format!("{NEVER_SEEN}的畫面"),
    ] {
        nothing_everywhere(&q, "主題沒看過，不可以拿別的畫面來湊");
    }
}

/// 「的」前面沒看過的，也可能只是在說哪一個：「最新的月報連結」問的是月報連結，
/// 照舊改用「月報連結」。擋「火星的畫面」的是「的」開頭那一條，不是「的」前面沒看過。
#[test]
fn a_word_before_de_never_seen_can_be_just_which_one() {
    for (label, db) in both() {
        for w in ["最新", "公司"] {
            assert!(
                db.search(w, 1).unwrap().is_empty(),
                "前提：{label}沒看過「{w}」"
            );
        }
    }
    for (q, base) in [
        ("最新的月報連結", "月報連結"),
        ("公司的月報連結", "月報連結"),
        ("找不到最新的月報連結", "月報連結"),
        ("最新的客服專線", "客服專線"),
        ("公司的客服專線", "客服專線"),
    ] {
        found_as(q, base);
    }
}

/// 名詞的頭尾長得像找法的，照舊留著：「搜尋」「查詢」在開頭是名詞，「首都」的「都」
/// 不是「都找不到」的「都」。
#[test]
fn a_name_that_looks_like_looking_is_kept() {
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = fixture(background);
        let session = db.start_session("test", "test").unwrap();
        add(&mut db, session, 7_000, 140, "搜尋欄位已停用");
        add(&mut db, session, 7_100, 141, "首都機場快線時刻表");
        for (q, base) in [("搜尋欄位在哪裡", "搜尋欄位"), ("首都找不到", "首都")]
        {
            let want = clean(&mut db, base);
            let got = ask(&mut db, q);
            assert_eq!(got.searched, relaxed(base), "{label}「{q}」");
            assert_eq!(got.same_things, want.same_things, "{label}「{q}」");
        }
    }
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.166 那一節逐題照打：記事本那兩行是同一張畫面。
/// 清單引號裡的句子就是這裡的字面值，這裡改了清單要跟著改。
///
/// 同一台機器上多半也照 alpha.162 到 alpha.165 那幾節打過記事本，那幾張也在。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str = "合約草稿已存檔\n保險專線 0800-222-333";
    const NOTEPAD_162: &str = "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123";
    const NOTEPAD_163: &str = "週報網址已更新\n同步失敗 ERR_SYNC_7\n客服專線 0800-000-123";
    const NOTEPAD_164: &str =
        "Orion build log uploaded\nvendor contract signed by legal\nERR_UPLOAD_9 retry queued";
    const NOTEPAD_165: &str = "報價單連結已寄出\n維修專線 02-2345-6789";
    const DRAFT: &str = "我對不到你打的那一串，所以改用「合約草稿」去找。";
    const PHONE: &str = "我對不到你打的那一串，所以改用「保險專線」去找。";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = Db::open_in_memory().unwrap();
        let session = db.start_session("test", "test").unwrap();
        if background {
            add_background(&mut db, session);
            assert_seen(&db, ASKING_WORDS);
        }
        add(&mut db, session, 2_000, 86, NOTEPAD_162);
        add(&mut db, session, 3_000, 87, NOTEPAD_163);
        add(&mut db, session, 4_000, 88, NOTEPAD_164);
        add(&mut db, session, 4_500, 89, NOTEPAD_165);
        add(&mut db, session, 5_000, 90, NOTEPAD);

        let notepad_only = |got: &Seen, q: &str| {
            assert!(
                got.hits.contains("合約草稿已存檔"),
                "{label}「{q}」要找到記事本那一張：{}",
                got.hits
            );
            for line in BACKGROUND {
                assert!(!got.hits.contains(line), "{label}「{q}」：{}", got.hits);
            }
            for other in ["週報網址已更新", "Orion build log", "報價單連結已寄出"] {
                assert!(!got.hits.contains(other), "{label}「{q}」：{}", got.hits);
            }
        };
        let draft = ask(&mut db, "合約草稿");
        let phone = ask(&mut db, "保險專線");
        for (q, got) in [("合約草稿", &draft), ("保險專線", &phone)] {
            assert_eq!(got.searched, None, "{label}基準線「{q}」");
            notepad_only(got, q);
        }
        for (q, said, want) in [
            ("找不到合約草稿", DRAFT, &draft),
            ("有沒有看到合約草稿", DRAFT, &draft),
            ("合約草稿不見了", DRAFT, &draft),
            ("合約草稿一直找不到", DRAFT, &draft),
            ("為什麼找不到保險專線", PHONE, &phone),
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
        let got = ask(&mut db, "為什麼找不到保險專線");
        assert!(
            got.answers.contains("0800-222-333")
                && !got.answers.contains("0800-000-123")
                && !got.answers.contains("02-2345-6789"),
            "{label}：「我最後看到的是」底下是記事本那支號碼：{}",
            got.answers
        );
        assert!(
            db.search("鴕鳥", 1).unwrap().is_empty(),
            "前提：沒看過「鴕鳥」"
        );
        for q in ["找不到鴕鳥", "有沒有看到鴕鳥"] {
            let got = ask(&mut db, q);
            assert!(got.empty, "{label}「{q}」要印「沒有找到。」：{}", got.hits);
            assert_eq!(
                got.searched, None,
                "{label}「{q}」標題下面不可以有「改用」那一行"
            );
        }
    }
}
