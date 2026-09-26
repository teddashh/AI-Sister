//! alpha.163 驗收：同一件事換一個問法，答得和原本的問法一樣。
//!
//! alpha.162 以前，「部署失敗是什麼時候」「為什麼部署失敗」找得到，換成名詞的問法
//! 就不一定：「部署失敗的時間」在每一份背景上都空手（「時間」是類型詞，放寬碰到就
//! 停）；「部署失敗的原因」「上次看到的月報連結」只有沒背景時找得到（「原因」「上次」
//! 看過之後就是必要條件）。
//!
//! 這一版放寬時，把尾巴的「時間」「原因」和開頭的「上次」「上一次」「之前」當成問法
//! 剝掉。「時間」照「什麼時候」多講一句：底下的時間是她記下的時候。第一次查詢一個字
//! 都不改，畫面上寫著日期的（「週會的時間」）照舊從 facts 答，見 `when_questions.rs`。
//!
//! 走產品同一條 `RetrievalProfile::TextAndFacts`，每一組都在「沒有背景」和「有背景」
//! 兩份資料庫上各跑一次。背景看過每一個問法用語（前提斷言逐條確認），主題字一個都
//! 沒有；對照組先證明自己答得出來、沒有改字。

use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalProfile, SearchAdjustment};

/// 用過一陣子的索引裡多半看過的說法。
const BACKGROUND: &[&str] = &[
    "不知道是什麼原因",
    "開會的時間到了",
    "上次看到的那個人",
    "上一次也是這樣",
    "之前說過的話",
];

/// 背景看過、沒有背景時沒看過的問法用語。
const ASKING_WORDS: &[&str] = &["原因", "時間", "上次", "上一次", "之前", "看到"];

const TOPIC_WORDS: &[&str] = &[
    "部署",
    "失敗",
    "月報",
    "連結",
    "客服",
    "專線",
    "電話",
    "退款",
    "同步",
    "週報",
    "網址",
    "release",
    "candidate",
    "ERR_DEPLOY",
    "ERR_SYNC",
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
    raws: Vec<String>,
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
        raws: r.answers.iter().map(|a| a.latest.raw.clone()).collect(),
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

/// 兩種問法在兩份資料庫上說一樣的話、找到一樣的東西，而且真的找到了。
fn asked_the_same(a: &str, b: &str) {
    for (label, mut db) in both() {
        let x = ask(&mut db, a);
        let y = ask(&mut db, b);
        assert!(!x.empty, "{label}「{a}」要找得到");
        assert_eq!(x.searched, y.searched, "{label}「{a}」和「{b}」");
        assert_eq!(x.answers, y.answers, "{label}「{a}」和「{b}」");
        assert_eq!(x.hits, y.hits, "{label}「{a}」和「{b}」");
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
        &["部署失敗", "月報連結", "客服專線", "ERR_DEPLOY_42"],
    );
    let busy = fixture(true);
    assert_seen(&busy, ASKING_WORDS);
    for w in ASKING_WORDS {
        assert!(
            plain.search(w, 1).unwrap().is_empty(),
            "前提：沒有背景時沒看過「{w}」"
        );
    }
}

/// 尾巴的「時間」就是「什麼時候」：改用主題去找，多講一句底下的時間是記下的時候。
#[test]
fn a_trailing_time_is_asked_like_when() {
    for (q, base) in [
        ("部署失敗的時間", "部署失敗"),
        ("部署失敗時間", "部署失敗"),
        ("部署失敗的時間是幾點", "部署失敗"),
        ("月報連結的時間", "月報連結"),
        ("ERR_DEPLOY_42 的時間", "ERR_DEPLOY_42"),
    ] {
        found_as(q, base, relaxed_when(base));
    }
    asked_the_same("部署失敗的時間", "部署失敗是什麼時候");
    asked_the_same("月報連結的時間", "月報連結什麼時候");
}

/// 尾巴的「原因」就是「為什麼」：改用主題去找，不多講時間。
#[test]
fn a_trailing_reason_is_asked_like_why() {
    for (q, base) in [
        ("部署失敗的原因", "部署失敗"),
        ("部署失敗原因", "部署失敗"),
        ("部署失敗的原因是什麼", "部署失敗"),
        ("ERR_DEPLOY_42 的原因", "ERR_DEPLOY_42"),
    ] {
        found_as(q, base, relaxed(base));
    }
    asked_the_same("部署失敗的原因", "為什麼部署失敗");
}

/// 開頭的「上次」「上一次」「之前」是問法，不是他要找的字。
#[test]
fn last_time_and_before_are_how_he_asks() {
    for (q, base) in [
        ("上次看到的月報連結", "月報連結"),
        ("上次的月報連結", "月報連結"),
        ("上一次的月報連結", "月報連結"),
        ("之前的月報連結", "月報連結"),
        ("之前看到的部署失敗", "部署失敗"),
        ("我上次看到的客服專線", "客服專線"),
    ] {
        found_as(q, base, relaxed(base));
    }
    for (label, mut db) in both() {
        let got = ask(&mut db, "我上次看到的客服專線");
        assert_eq!(got.raws, vec!["0800-000-123".to_owned()], "{label}");
    }
}

/// 頭尾一起：開頭的「上次」和尾巴的「時間」「原因」都剝掉。
#[test]
fn both_ends_at_once() {
    found_as("上次部署失敗的時間", "部署失敗", relaxed_when("部署失敗"));
    found_as("上次部署失敗的原因", "部署失敗", relaxed("部署失敗"));
}

/// 截圖的時間不是期限，也不是帳單上的日期。這兩個照舊是他要的答案種類，畫面上沒有
/// 就空手。
#[test]
fn a_deadline_or_a_date_is_still_what_he_asked_for() {
    nothing_everywhere("部署失敗的期限", "他問的是期限；畫面上沒有就是空手");
    nothing_everywhere("部署失敗的日期", "他問的是日期；畫面上沒有就是空手");
}

/// 主題沒看過就空手。alpha.162 在有背景的那一份上，「退款的原因」「上次的退款」會
/// 改用「原因」「上次」，拿背景那一句來湊。
#[test]
fn an_unseen_topic_still_gets_nothing() {
    for q in ["退款的時間", "退款的原因", "上次的退款", "之前看到的退款"] {
        nothing_everywhere(q, "退款沒看過，不可以拿背景那一句來湊");
    }
}

/// 放寬過但還是空手：底下沒有東西，就不講「底下每一筆的時間」。
#[test]
fn an_empty_relaxed_time_question_says_only_what_it_searched() {
    for (label, mut db) in both() {
        assert_seen(&db, &["部署", "candidate"]);
        let got = ask(&mut db, "部署 candidate 的時間");
        assert!(got.empty, "{label}：{}", got.hits);
        assert_eq!(got.searched, relaxed("部署 candidate"), "{label}");
    }
}

/// 登記在案的代價：「上次」「之前」和「所以」「到底」一樣是口語開頭。後面只剩「那個人」
/// 「看到的東西」「那個事件」「的原因」這種話時，看過「個人」「東西」「事件」「原因」
/// 的機器上，放寬拿去找的就是這幾個字（「那個人」往左退一個字剩「個人」），和
/// 「所以……」「到底什麼原因」一模一樣。尾巴的「原因」前面沒有別的字就不切
/// （`TAIL_ASKS`），所以「上次的原因」找的是「原因」，不是什麼都不找。
#[test]
fn a_lead_in_before_a_vague_word_pays_the_same_cost_as_so() {
    fn with_things(background: bool) -> Db {
        let mut db = fixture(background);
        let session = db.start_session("test", "test").unwrap();
        add(&mut db, session, 7_000, 120, "東西放在桌上");
        add(&mut db, session, 7_001, 121, "這個事件已經結案");
        db
    }
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = with_things(background);
        for (so_q, same) in [
            ("所以那個人", ["上次那個人", "之前那個人"]),
            ("所以看到的東西", ["上次看到的東西", "之前看到的東西"]),
            ("所以那個事件", ["上次那個事件", "之前那個事件"]),
            ("到底什麼原因", ["上次的原因", "之前的原因"]),
        ] {
            let so = ask(&mut db, so_q);
            for q in same {
                let got = ask(&mut db, q);
                assert_eq!(got.searched, so.searched, "{label}「{q}」");
                assert_eq!(got.answers, so.answers, "{label}「{q}」");
                assert_eq!(got.hits, so.hits, "{label}「{q}」");
            }
        }
    }
    let mut db = with_things(true);
    for (q, said) in [
        ("上次那個人", "個人"),
        ("上次看到的東西", "東西"),
        ("之前那個事件", "事件"),
        ("上次的原因", "原因"),
    ] {
        let got = ask(&mut db, q);
        assert_eq!(
            got.searched,
            relaxed(said),
            "版本說明寫的就是這一格：「{q}」"
        );
        assert!(!got.empty, "「{q}」");
    }
}

/// `docs/WINDOWS-CHECKLIST.md` alpha.163 那一節逐題照打：記事本那三行是同一張畫面。
/// 清單引號裡的句子就是這裡的字面值，這裡改了清單要跟著改。
///
/// 同一台機器上多半也照 alpha.162 那一節打過那三行，那一張也在。兩節互相不能湊：
/// 這一節的字不可以讓 alpha.162 最重要的那一條（`週報的備份網址`）找到東西。
#[test]
fn the_windows_checklist_hears_what_the_checklist_says() {
    const NOTEPAD: &str = "週報網址已更新\n同步失敗 ERR_SYNC_7\n客服專線 0800-000-123";
    const NOTEPAD_162: &str = "週報網址已更新\n部署失敗 ERR_DEPLOY_42\n客服專線 0800-000-123";
    const SYNC_WHEN: &str =
        "我對不到你打的那一串，所以改用「同步失敗」去找。底下每一筆的時間，是我記下那一筆的時候。";
    const SYNC: &str = "我對不到你打的那一串，所以改用「同步失敗」去找。";
    const URL: &str = "我對不到你打的那一串，所以改用「週報網址」去找。";
    const PHONE: &str = "我對不到你打的那一串，所以改用「客服專線」去找。";
    for (label, background) in [("沒有背景", false), ("有背景", true)] {
        let mut db = Db::open_in_memory().unwrap();
        let session = db.start_session("test", "test").unwrap();
        if background {
            add_background(&mut db, session);
            assert_seen(&db, ASKING_WORDS);
        }
        add(&mut db, session, 4_000, 89, NOTEPAD_162);
        add(&mut db, session, 5_000, 90, NOTEPAD);

        let notepad_only = |got: &Seen, q: &str| {
            assert!(
                got.hits.contains("同步失敗 ERR_SYNC_7"),
                "{label}「{q}」要找到記事本那一張：{}",
                got.hits
            );
            for line in BACKGROUND {
                assert!(!got.hits.contains(line), "{label}「{q}」：{}", got.hits);
            }
        };
        let sync = ask(&mut db, "同步失敗");
        let url = ask(&mut db, "週報網址");
        for (q, got) in [("同步失敗", &sync), ("週報網址", &url)] {
            assert_eq!(got.searched, None, "{label}基準線「{q}」");
            notepad_only(got, q);
        }
        for (q, said, same_as) in [
            ("同步失敗的時間", SYNC_WHEN, &sync),
            ("同步失敗是什麼時候", SYNC_WHEN, &sync),
            ("同步失敗的原因", SYNC, &sync),
            ("為什麼同步失敗", SYNC, &sync),
            ("上次看到的週報網址", URL, &url),
            ("之前的週報網址", URL, &url),
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
            assert_eq!(got.same_things, same_as.same_things, "{label}「{q}」");
        }
        let got = ask(&mut db, "我上次看到的客服專線");
        assert_eq!(
            got.searched
                .as_ref()
                .map(SearchAdjustment::message)
                .as_deref(),
            Some(PHONE),
            "{label}"
        );
        assert_eq!(got.raws, vec!["0800-000-123".to_owned()], "{label}");
        for q in ["同步失敗的期限", "退款的原因", "週報的備份網址"] {
            let got = ask(&mut db, q);
            assert!(got.empty, "{label}「{q}」要印「沒有找到。」：{}", got.hits);
            assert_eq!(
                got.searched, None,
                "{label}「{q}」標題下面不可以有「改用」那一行"
            );
        }
    }
}
