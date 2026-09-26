//! alpha.168 驗收：「最新的月報連結」「最後一次看到的月報連結」，她答得和只打主題一樣；
//! 「公司的火星」「誰改了火星」，她照舊說沒有找到。
//!
//! alpha.167 以前，放寬拿不掉開頭的「最新」「最後」。用過一陣子的索引看過這兩個字，不會把
//! 它們當成沒看過的頭拿掉，「最新的月報連結」就是必要條件、空手。反過來，「的」「了」後面
//! 那段他問的東西沒看過、前面看過的時候，索引把後面那段整段當成沒看過的尾巴拿掉：
//! 「公司的火星」改用「公司」，「誰改了火星」改用「改了」，拿寫著公司、改了的畫面來湊。
//!
//! 這一版把「最新」「最新版」「最新版本」「最新一期」「最後」「最後一次」和「上次」一樣當成
//! 口語開頭拿掉；最後一個「的」「了」後面那段整段被拿掉的，不放寬。「最早」「第一次」和
//! 「公司的」「客戶的」照舊是條件。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`，每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景是用過一陣子的畫面：看過每一個說哪一次、說是誰的字（前提斷言
//! 逐條確認），主題字一個都沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的畫面上多半看過的句子。
const BACKGROUND: &[&str] = &[
    "最新消息請看公告",
    "最新版已經可以下載",
    "最新版本已經發布",
    "最新一版已經送出",
    "最新一期雜誌到了",
    "最新一份報告已送出",
    "最後一班車十一點",
    "最後一份文件已歸檔",
    "最後一次提醒大家",
    "最後的晚餐",
    "最早的一班車六點",
    "第一次來台北",
    "公司尾牙在週五",
    "客戶說明天再談",
    "老闆今天不在",
    "我改了密碼",
    "新的一年快樂",
    "截圖存在桌面",
];

/// 背景看過的說哪一次的字。
const WHICH_TIME: &[&str] = &[
    "最新",
    "最新版",
    "最新版本",
    "最新一版",
    "最新一期",
    "最新一份",
    "最後",
    "最後一份",
    "最後一次",
    "最早",
    "第一次",
];

/// 背景看過的說是誰的、做了什麼的字。單獨拿去找都找得到背景那一句，放寬拿它們來湊的話
/// 畫面上就會有東西。
const WHOSE: &[&str] = &["公司", "客戶", "老闆", "改了", "新的"];

const TOPIC_WORDS: &[&str] = &["月報", "連結", "客服", "專線", "火星", "build", "log"];

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
    searched: Option<SearchAdjustment>,
    empty: bool,
}

fn ask(db: &mut Db, q: &str) -> Seen {
    let r = RetrievalProfile::TextAndFacts.retrieve(db, q, 5).unwrap();
    Seen {
        answers: format!("{:?}", r.answers),
        hits: format!("{:?}", r.hits),
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
    for (label, mut db) in both() {
        let want = clean(&mut db, base);
        let got = ask(&mut db, noisy);
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

fn nothing(label: &str, db: &mut Db, q: &str, why: &str) {
    let got = ask(db, q);
    assert!(got.empty, "{label}「{q}」：{why}：{}", got.hits);
    assert_eq!(
        got.searched, None,
        "{label}「{q}」：沒有放寬就不說改用了什麼"
    );
}

#[test]
fn the_background_holds_every_which_time_word_and_no_topic_word() {
    for line in BACKGROUND {
        let lower = line.to_lowercase();
        for word in TOPIC_WORDS {
            assert!(!lower.contains(word), "背景「{line}」含主題字「{word}」");
        }
    }
    let plain = fixture(false);
    assert_seen(&plain, &["月報連結", "客服專線", "build log"]);
    for w in WHICH_TIME.iter().chain(WHOSE).chain([&NEVER_SEEN, &"截圖"]) {
        assert!(
            plain.search(w, 1).unwrap().is_empty(),
            "前提：沒有背景時沒看過「{w}」"
        );
    }
    let busy = fixture(true);
    assert_seen(&busy, WHICH_TIME);
    assert_seen(&busy, WHOSE);
    assert_seen(&busy, &["截圖"]);
    assert!(
        busy.search(NEVER_SEEN, 1).unwrap().is_empty(),
        "前提：背景裡也沒有「{NEVER_SEEN}」"
    );
}

/// 開頭說哪一次的字，連同整段收的「最新版」「最新一期」「最後一次」，一起拿掉。
#[test]
fn which_time_before_the_topic_is_not_a_condition() {
    for q in [
        "最新的月報連結",
        "最新月報連結",
        "最新版的月報連結",
        "最新版本的月報連結",
        "最新一版的月報連結",
        "最新一期的月報連結",
        "最新一份月報連結",
        "最後的月報連結",
        "最後一份月報連結",
        "最後一次看到的月報連結",
        "最後一次的月報連結",
        "最新的月報連結在哪裡",
        "找不到最新的月報連結",
        "幫我找最新的月報連結",
        "所以最新的月報連結呢",
    ] {
        found_as(q, "月報連結");
    }
}

/// 問號碼的，「我最後看到的是」照樣是那支號碼。
#[test]
fn a_phone_number_still_comes_back() {
    for q in ["最新的客服專線", "最後的客服專線", "最新的客服專線是多少"] {
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

/// 「最早」「第一次」不是口語開頭：同分的時候新的排前面，名額滿了先被擠掉的是最早那幾筆。
/// 看過就是條件，空手；沒看過的機器上照舊被索引當成沒看過的頭拿掉。
#[test]
fn the_earliest_is_still_a_condition() {
    for q in ["最早的月報連結", "第一次看到的月報連結"] {
        let mut busy = fixture(true);
        nothing("有背景", &mut busy, q, "看過的「最早」「第一次」是條件");
        let mut plain = fixture(false);
        assert_eq!(
            ask(&mut plain, q).searched,
            relaxed("月報連結"),
            "沒有背景「{q}」"
        );
    }
}

/// 說是誰的照舊是條件：看過「公司」，「公司的月報連結」就要有公司。
#[test]
fn whose_is_still_a_condition() {
    let mut busy = fixture(true);
    for q in ["公司的月報連結", "客戶的月報連結", "老闆的月報連結"] {
        nothing(
            "有背景",
            &mut busy,
            q,
            "看過的「公司」「客戶」「老闆」是條件",
        );
    }
}

/// 主題沒看過就是空手，不拿寫著公司、改了、最新的畫面來湊。
#[test]
fn a_topic_never_seen_is_still_nothing() {
    for q in [
        format!("公司的{NEVER_SEEN}"),
        format!("客戶的{NEVER_SEEN}"),
        format!("老闆的{NEVER_SEEN}"),
        format!("新的{NEVER_SEEN}"),
        format!("誰改了{NEVER_SEEN}"),
        format!("改了{NEVER_SEEN}"),
        format!("最新的{NEVER_SEEN}"),
        format!("最後的{NEVER_SEEN}"),
        format!("最後一次看到的{NEVER_SEEN}"),
        format!("公司的{NEVER_SEEN}在哪"),
        format!("找不到公司的{NEVER_SEEN}"),
        format!("幫我找公司的{NEVER_SEEN}"),
    ] {
        for (label, mut db) in both() {
            nothing(label, &mut db, &q, "主題沒看過，不可以拿別的畫面來湊");
        }
    }
}

/// 代價：「的」後面那段是一般的字、剛好沒看過的時候，和「火星」分不出來。「月報連結的截圖」
/// 在沒看過「截圖」的機器上，alpha.167 改用「月報連結」找得到，這一版空手。看過「截圖」的
/// 機器上兩版都空手：「截圖」是條件。
#[test]
fn a_plain_word_after_de_never_seen_pays_the_documented_cost() {
    for (label, mut db) in both() {
        nothing(
            label,
            &mut db,
            "月報連結的截圖",
            "「的」後面整段拿掉就不放寬",
        );
    }
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.168 那一節逐題照打：記事本那三行是同一張畫面。
/// 清單引號裡的句子就是這裡的字面值，這裡改了清單要跟著改。
///
/// 同一台機器上多半也照 alpha.162 到 alpha.167 那幾節打過記事本，那幾張也在。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str = "採購清單已寄出\n訂房專線 0800-333-444\n公司尾牙改了地點";
    const NOTEPAD_162: &str = "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123";
    const NOTEPAD_163: &str = "週報網址已更新\n同步失敗 ERR_SYNC_7\n客服專線 0800-000-123";
    const NOTEPAD_164: &str =
        "Orion build log uploaded\nvendor contract signed by legal\nERR_UPLOAD_9 retry queued";
    const NOTEPAD_165: &str = "報價單連結已寄出\n維修專線 02-2345-6789";
    const NOTEPAD_166: &str = "合約草稿已存檔\n保險專線 0800-222-333";
    const NOTEPAD_167: &str =
        "出差報告已上傳\n客訴專線 0800-555-777\nAtlas shipping manifest uploaded";
    const LIST: &str = "我對不到你打的那一串，所以改用「採購清單」去找。";
    const PHONE: &str = "我對不到你打的那一串，所以改用「訂房專線」去找。";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = Db::open_in_memory().unwrap();
        let session = db.start_session("test", "test").unwrap();
        if background {
            add_background(&mut db, session);
            assert_seen(&db, WHICH_TIME);
        }
        add(&mut db, session, 2_000, 85, NOTEPAD_162);
        add(&mut db, session, 3_000, 86, NOTEPAD_163);
        add(&mut db, session, 4_000, 87, NOTEPAD_164);
        add(&mut db, session, 4_500, 88, NOTEPAD_165);
        add(&mut db, session, 4_800, 89, NOTEPAD_166);
        add(&mut db, session, 4_900, 84, NOTEPAD_167);
        add(&mut db, session, 5_000, 90, NOTEPAD);
        // 記事本自己那一行讓「公司」「改了」一定看過：反過來驗那兩題，alpha.167 會改用它們
        // 找到記事本這一張。
        assert_seen(&db, &["公司", "改了"]);

        let notepad_only = |got: &Seen, q: &str| {
            assert!(
                got.hits.contains("採購清單已寄出"),
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
                "出差報告已上傳",
            ] {
                assert!(!got.hits.contains(other), "{label}「{q}」：{}", got.hits);
            }
        };
        let list = ask(&mut db, "採購清單");
        let phone = ask(&mut db, "訂房專線");
        for (q, got) in [("採購清單", &list), ("訂房專線", &phone)] {
            assert_eq!(got.searched, None, "{label}基準線「{q}」");
            notepad_only(got, q);
        }
        for (q, said, want) in [
            ("最新的採購清單", LIST, &list),
            ("最後一次看到的採購清單", LIST, &list),
            ("最新的訂房專線", PHONE, &phone),
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
            assert_eq!(got.answers, want.answers, "{label}「{q}」");
            assert_eq!(got.hits, want.hits, "{label}「{q}」");
        }
        let got = ask(&mut db, "最新的訂房專線");
        assert!(
            got.answers.contains("0800-333-444")
                && !got.answers.contains("0800-000-123")
                && !got.answers.contains("02-2345-6789")
                && !got.answers.contains("0800-222-333")
                && !got.answers.contains("0800-555-777"),
            "{label}：「我最後看到的是」底下是記事本那支號碼：{}",
            got.answers
        );
        assert!(
            db.search("鴕鳥", 1).unwrap().is_empty(),
            "前提：沒看過「鴕鳥」"
        );
        for q in ["公司的鴕鳥", "誰改了鴕鳥"] {
            let got = ask(&mut db, q);
            assert!(got.empty, "{label}「{q}」要印「沒有找到。」：{}", got.hits);
            assert_eq!(
                got.searched, None,
                "{label}「{q}」標題下面不可以有「改用」那一行"
            );
        }
    }
}
