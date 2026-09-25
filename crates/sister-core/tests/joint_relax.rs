//! alpha.162 驗收：問句中間的「的」「了」切開再找。
//!
//! alpha.161 的放寬整段照原樣對。「誰改了月報連結」剝掉「誰」剩「改了月報連結」，
//! 畫面寫的是「月報連結已更新」：沒有背景時「改了」沒看過，放寬把它當成沒看過的頭
//! 拿掉；背景看過「改了」，就停在它前面，整段對不到。「月報的連結」有沒有背景都
//! 對不到「月報連結」。
//!
//! 這一版多一步：在空白、中文標點和「的」「了」切開，兩個字以上的段每一段都是必要
//! 條件，一個字的段丟掉。走產品同一條 `RetrievalProfile::TextAndFacts`，每一組在
//! 「沒有背景」和「有背景」兩份資料庫上各跑一次。背景看過每一個雜訊詞（前提斷言
//! 逐條確認），主題字一個都沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::config::PrivacyConfig;
use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的索引裡一定看過的說法。
const BACKGROUND: &[&str] = &[
    "我改了一下設定",
    "他更新了版本",
    "上次看到的那個人",
    "不知道是什麼原因",
    "為了這件事",
    "有空的話再說",
    "這是他建議的做法",
    "密碼要記好",
];

const TOPIC_WORDS: &[&str] = &[
    "月報",
    "連結",
    "部署",
    "失敗",
    "客服",
    "專線",
    "電話",
    "退款",
    "會議",
    "翡翠灣",
    "release",
    "candidate",
    "ERR_DEPLOY",
];

/// 他跟她說過、她記下來的一個詞：看過，但不在任何一張畫面上。
const TOLD: &str = "翡翠灣";

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
    let privacy = PrivacyConfig {
        remember_told: true,
        ..Default::default()
    };
    db.remember_told(&privacy, 200, TOLD).unwrap();
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

/// 前提：這些字真的看過。沒有這條，「有背景」那一半會安靜地變回沒有背景。
fn assert_seen(db: &Db, words: &[&str]) {
    assert!(!words.is_empty(), "前提：要有字可以檢查");
    for w in words {
        assert!(!db.search(w, 1).unwrap().is_empty(), "前提：看過「{w}」");
    }
}

/// 兩份資料庫都放寬成 `base`，找到的和直接問 `base` 一模一樣。
fn same_as_everywhere(noisy: &str, base: &str) {
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
    // 反過來：主題真的在夾具裡，上面那條才不是空轉。
    let db = fixture(false);
    assert_seen(&db, &["月報連結", "部署失敗", "客服專線", "release", TOLD]);
    // 背景真的看過每一個雜訊詞，而且沒有背景的那一份沒看過。
    let busy = fixture(true);
    let noise = ["改了", "上次", "看到", "原因", "為了", "的話"];
    assert_seen(&busy, &noise);
    for w in noise {
        assert!(
            db.search(w, 1).unwrap().is_empty(),
            "前提：沒有背景時沒看過「{w}」"
        );
    }
}

/// 「改了」看過之後，上一步停在它前面；這一步在「了」切開，「改」一個字丟掉。
#[test]
fn a_seen_verb_before_a_joint_no_longer_hides_the_topic() {
    same_as_everywhere("誰改了月報連結", "月報連結");
}

/// 兩個字以上的動詞是條件：「更新」寫在那張畫面上，留著也找得到。
#[test]
fn a_verb_of_two_or_more_stays_a_condition() {
    same_as_everywhere("誰更新了月報連結", "更新 月報連結");
}

#[test]
fn a_joint_between_two_topic_words_becomes_two_conditions() {
    same_as_everywhere("月報的連結", "月報 連結");
    same_as_everywhere("月報的連結在哪", "月報 連結");
}

/// the 是虛字，切段時不再是條件；is 不是虛字，照舊是條件，那張畫面上剛好有。
#[test]
fn an_english_filler_is_not_a_condition_after_the_split() {
    same_as_everywhere("where is the release candidate", "is release candidate");
}

/// 兩個字以上的段不管看過沒有都是必要條件。
#[test]
fn every_piece_of_two_or_more_is_still_required() {
    nothing_everywhere("客服的退款專線", "退款沒看過，不可以拿客服專線來湊");
    for q in [
        format!("{TOLD}的客服專線"),
        format!("{TOLD}的部署失敗"),
        format!("部署失敗的{TOLD}"),
    ] {
        nothing_everywhere(&q, "看過的段可能就是他要的那件事，不可以丟掉");
    }
}

/// 類型詞那一段留著：截圖的時間不是期限，放掉「電話」就失去種類。尾巴的「時間」
/// alpha.163 起照「什麼時候」答，見 `asked_another_way.rs`。
#[test]
fn a_type_word_piece_is_what_he_asked_for() {
    nothing_everywhere("部署失敗的期限", "他問的是期限；畫面上沒有就是空手");
    nothing_everywhere("退款的電話", "主題沒看過，不可以拿別的電話來湊");
}

/// 從剝完的字切，不從上一步的候選字切。背景看過「議的」（「他建議的做法」）而沒看過
/// 「會議」，上一步只拿掉「會議」這個雙字，候選字是「議的密碼」。從那裡切，「議」
/// 一個字被丟掉，就改用「密碼」拿背景那一句來湊。
#[test]
fn a_split_starts_from_the_whole_question_not_from_a_half_cut_word() {
    let mut busy = fixture(true);
    assert_seen(&busy, &["議的", "密碼"]);
    assert!(
        busy.search("會議", 1).unwrap().is_empty(),
        "前提：沒看過「會議」"
    );
    let got = ask(&mut busy, "會議的密碼");
    assert!(got.empty, "不可以拿別的密碼來湊：{}", got.hits);
    assert_eq!(
        got.searched,
        relaxed("議的密碼"),
        "切段空手，照舊報上一步的候選字"
    );
}

/// 上一步原樣對得到就不切段：拆開的條件對得到的比較多，原樣那一張比較準。
#[test]
fn a_phrase_the_previous_step_finds_is_not_split() {
    let mut db = Db::open_in_memory().unwrap();
    let session = db.start_session("test", "test").unwrap();
    add(&mut db, session, 100, 1, "月報的連結已更新");
    add(&mut db, session, 200, 2, "連結整理：月報下週交");
    let apart = ask(&mut db, "月報 連結");
    assert_eq!(apart.searched, None);
    assert!(
        apart.hits.contains("月報的連結已更新") && apart.hits.contains("連結整理"),
        "前提：拆開的條件兩張都對得到：{}",
        apart.hits
    );
    let got = ask(&mut db, "請問月報的連結");
    assert_eq!(got.searched, relaxed("月報的連結"));
    assert!(got.hits.contains("月報的連結已更新"), "{}", got.hits);
    assert!(
        !got.hits.contains("連結整理"),
        "上一步找得到就不切段：{}",
        got.hits
    );
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.162 那一節逐題照打：記事本那四行是同一張畫面，
/// 索引是用過的（背景看過「改了」）。清單引號裡的句子就是這裡的字面值，這裡改了
/// 清單要跟著改。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str =
        "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123\nstaging build is ready";
    let mut db = Db::open_in_memory().unwrap();
    let session = db.start_session("test", "test").unwrap();
    for (i, line) in BACKGROUND.iter().enumerate() {
        add(&mut db, session, 1_000 + i as i64, 100 + i as u64, line);
    }
    add(&mut db, session, 5_000, 90, NOTEPAD);
    assert_seen(&db, &["改了"]);

    let notepad_only = |got: &Seen, q: &str| {
        assert!(
            got.hits.contains("週報網址已更新"),
            "「{q}」要找到記事本那一張：{}",
            got.hits
        );
        assert!(
            !got.hits.contains("上次看到的那個人") && !got.hits.contains("我改了一下設定"),
            "「{q}」：{}",
            got.hits
        );
    };
    for q in ["週報網址", "staging build"] {
        let got = ask(&mut db, q);
        assert_eq!(got.searched, None, "基準線「{q}」");
        notepad_only(&got, q);
    }
    for (q, said) in [
        (
            "週報的網址",
            "我對不到你打的那一串，所以改用「週報 網址」去找。",
        ),
        (
            "週報的網址在哪",
            "我對不到你打的那一串，所以改用「週報 網址」去找。",
        ),
        (
            "誰改了週報網址",
            "我對不到你打的那一串，所以改用「週報網址」去找。",
        ),
        (
            "誰更新了週報網址",
            "我對不到你打的那一串，所以改用「更新 週報網址」去找。",
        ),
        (
            "where is the staging build",
            "我對不到你打的那一串，所以改用「is staging build」去找。",
        ),
        (
            "when is the staging build",
            "我對不到你打的那一串，所以改用「is staging build」去找。底下每一筆的時間，是我記下那一筆的時候。",
        ),
    ] {
        let got = ask(&mut db, q);
        assert_eq!(
            got.searched
                .as_ref()
                .map(SearchAdjustment::message)
                .as_deref(),
            Some(said),
            "「{q}」"
        );
        notepad_only(&got, q);
    }
    let q = "週報的備份網址";
    let got = ask(&mut db, q);
    assert!(got.empty, "「{q}」要印「沒有找到。」：{}", got.hits);
    assert_eq!(got.searched, None, "「{q}」標題下面不可以有「改用」那一行");
}
