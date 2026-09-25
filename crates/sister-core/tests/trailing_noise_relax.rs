//! alpha.157 驗收：尾巴對不到的字也放掉，類型詞留著；「請問／告訴我」開頭的類型題，
//! 主題明明看過，不再被當成「主題全部沒看過」。
//!
//! 只用公開 API，走產品同一條 `RetrievalProfile::TextAndFacts`（桌面與 CLI 的
//! `ask`／`query` 都是這一條）。每一組「放寬後的答案」都拿乾淨問法的實際輸出當對照，
//! 對照組先證明自己答得出來而且沒有改字——否則「兩邊一樣」可能是兩邊都空。

use sister_core::config::PrivacyConfig;
use sister_core::db::Db;
use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
use sister_core::retrieval::{RetrievalLimits, RetrievalProfile, SearchAdjustment};

fn replay_db() -> Db {
    let corpus: sister_core::replay::Corpus = serde_json::from_str(include_str!(
        "../../../scenarios/recall-baseline.corpus.json"
    ))
    .unwrap();
    let mut db = Db::open_in_memory().unwrap();
    db.import_replay(&corpus, 100).unwrap();
    db
}

fn db_with(text: &str) -> Db {
    let mut db = Db::open_in_memory().unwrap();
    let session = db.start_session("test", "test").unwrap();
    db.insert_frame(
        session,
        &FrameCapture {
            assistive: Vec::new(),
            ts: 100,
            monitor: 0,
            width: 800,
            height: 600,
            dhash: 1,
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

fn clean(db: &mut Db, q: &str) -> Seen {
    let s = ask(db, q);
    assert!(!s.empty, "對照組「{q}」自己要答得出來");
    assert_eq!(s.searched, None, "對照組「{q}」本身不可以是放寬過的");
    s
}

fn same_as(db: &mut Db, noisy: &str, base: &str) {
    let want = clean(db, base);
    let got = ask(db, noisy);
    assert_eq!(
        got.searched,
        relaxed(base),
        "{noisy}：要說實際拿去找的是「{base}」"
    );
    assert_eq!(
        got.answers, want.answers,
        "{noisy}：facts 要和「{base}」一模一樣"
    );
    assert_eq!(got.hits, want.hits, "{noisy}：原文要和「{base}」一模一樣");
}

/// 零命中的三字雜訊。`glue` 是會黏在它前面的那個字：黏起來的雙字也不可以在索引裡，
/// 否則尾巴的第一個字會和主題接成一個看過的詞。
fn unseen_noises(db: &Db, count: usize, glue: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for code in 0x4e00u32..0x9fff {
        let a = char::from_u32(code).unwrap();
        let b = char::from_u32(code + 1).unwrap();
        let noise = format!("{a}{b}{a}");
        let mut probes = vec![noise.clone(), format!("{a}{b}"), format!("{b}{a}")];
        probes.extend(glue.iter().map(|g| format!("{g}{a}")));
        if sister_core::question::terms(&noise) == noise
            && sister_core::facts::kinds_for_query(&noise).is_empty()
            && probes.iter().all(|p| db.search(p, 1).unwrap().is_empty())
        {
            out.push(noise);
            if out.len() == count {
                return out;
            }
        }
    }
    panic!("找不到足夠的零命中雜訊");
}

#[test]
fn trailing_noise_is_dropped_and_finds_what_the_clean_question_finds() {
    let mut db = replay_db();
    for (noisy, base) in [
        ("ERR_DEPLOY_42 怎麼回事", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42怎麼回事", "ERR_DEPLOY_42"),
        ("ERR_DEPLOY_42 是什麼意思", "ERR_DEPLOY_42"),
        ("部署失敗怎麼辦", "部署失敗"),
        ("部署失敗是什麼原因", "部署失敗"),
        ("客服專線怎麼打", "客服專線"),
        ("release candidate 出了嗎", "release candidate"),
        // 頭尾同時有
        ("查一下 ERR_DEPLOY_42 怎麼回事", "ERR_DEPLOY_42"),
    ] {
        same_as(&mut db, noisy, base);
    }
}

#[test]
fn generated_unseen_tails_are_dropped() {
    let mut db = replay_db();
    for tail in unseen_noises(&db, 16, &["線", "敗"]) {
        same_as(&mut db, &format!("客服專線{tail}"), "客服專線");
        same_as(&mut db, &format!("部署失敗{tail}"), "部署失敗");
        same_as(&mut db, &format!("ERR_DEPLOY_42 {tail}"), "ERR_DEPLOY_42");
    }
}

#[test]
fn a_trailing_type_word_is_what_he_asked_for_and_stays() {
    let mut db = replay_db();
    for (noisy, base) in [
        ("客服電話怎麼打", "客服電話"),
        ("客服電話號碼怎麼打", "客服電話號碼"),
        ("部署錯誤怎麼修", "部署錯誤"),
        ("ERR_DEPLOY_42 錯誤怎麼修", "ERR_DEPLOY_42 錯誤"),
    ] {
        let want = clean(&mut db, base);
        assert_ne!(
            want.answers, "[]",
            "前提：「{base}」是靠類型詞拿到 facts 答案的"
        );
        same_as(&mut db, noisy, base);
    }
    // alpha.155 的招牌句不能倒退：開頭的「找」放掉，尾巴的「電話」留著。
    let r = RetrievalProfile::TextAndFacts
        .retrieve(&mut db, "找客服電話", 5)
        .unwrap();
    assert_eq!(r.searched, relaxed("客服電話"));
    assert_eq!(
        r.answers.first().map(|a| a.latest.raw.as_str()),
        Some("0800-000-123")
    );
}

#[test]
fn an_unseen_topic_with_a_type_word_still_gets_nothing() {
    let mut db = replay_db();
    for q in [
        "退款電話怎麼打",
        "退款專線在哪裡",
        "火星會議連結怎麼開",
        "火星會議的網址是什麼",
    ] {
        let got = ask(&mut db, q);
        assert!(got.empty, "{q}：主題沒看過，不可以拿別的電話／網址來湊");
        assert_eq!(got.searched, None, "{q}：沒有放寬就不說改用了什麼");
    }
    for tail in unseen_noises(&db, 8, &[]) {
        let q = format!("{tail}電話怎麼打");
        let got = ask(&mut db, &q);
        assert!(got.empty, "{q}");
        assert_eq!(got.searched, None, "{q}");
    }
}

#[test]
fn an_unknown_word_in_the_middle_is_still_required() {
    let mut db = replay_db();
    for q in [
        "客服 退款 專線",
        "部署 退款 失敗",
        "ERR_DEPLOY_42 退款 release",
    ] {
        let got = ask(&mut db, q);
        assert!(got.empty, "{q}：中間那個沒看過的字仍是必要條件");
        assert_eq!(got.searched, None, "{q}");
    }
}

#[test]
fn a_tail_she_has_seen_is_still_required() {
    let mut db = replay_db();
    let noise = unseen_noises(&db, 1, &["線", "敗"]).pop().unwrap();
    let privacy = PrivacyConfig {
        remember_told: true,
        ..Default::default()
    };
    db.remember_told(&privacy, 200, &noise).unwrap();
    assert!(
        !db.search(&noise, 1).unwrap().is_empty(),
        "前提：記下之後查得到"
    );
    for q in [
        format!("客服專線 {noise}"),
        format!("ERR_DEPLOY_42 {noise}"),
        format!("部署失敗{noise}"),
    ] {
        let got = ask(&mut db, &q);
        assert!(got.empty, "{q}：看過的尾巴仍是必要條件");
        assert_eq!(got.searched, None, "{q}");
    }
}

#[test]
fn a_known_topic_is_not_mistaken_for_an_unseen_one() {
    // 「主題全部沒看過就不放寬」這條保護，原本拿「主題要不要改」去判斷；主題一個字
    // 都不用改的時候（請問月報連結 → 主題「月報」）也被當成沒看過而整題空手。
    let mut db = db_with("月報連結已更新");
    for q in ["請問月報連結", "告訴我 月報連結", "月報連結在哪裡"] {
        same_as(&mut db, q, "月報連結");
    }
    // 同一份資料的反面：主題真的沒看過。刪掉那條保護的話，「報連」會把它接回去。
    for q in ["請問年報連結", "告訴我 年報連結"] {
        let got = ask(&mut db, q);
        assert!(got.empty, "{q}");
        assert_eq!(got.searched, None, "{q}");
    }
}

#[test]
fn a_dropped_tail_does_not_widen_the_date() {
    let mut db = replay_db();
    let r = RetrievalProfile::TextAndFacts
        .retrieve_at(
            &mut db,
            "昨天 ERR_DEPLOY_42 怎麼回事",
            RetrievalLimits::same(5),
            200,
        )
        .unwrap();
    assert!(
        r.answers.is_empty() && r.hits.is_empty(),
        "昨天不能拿到今天的錯誤碼"
    );
    assert_eq!(r.searched, relaxed("ERR_DEPLOY_42"), "放寬的字照樣要說");
    // 對照：同一個時鐘問「今天」找得到，證明上面的空是日期擋的，不是放寬沒發生。
    let r = RetrievalProfile::TextAndFacts
        .retrieve_at(
            &mut db,
            "今天 ERR_DEPLOY_42 怎麼回事",
            RetrievalLimits::same(5),
            2_000,
        )
        .unwrap();
    assert!(!r.hits.is_empty(), "今天那一格要找得到");
    assert_eq!(r.searched, relaxed("ERR_DEPLOY_42"));
}

#[test]
fn a_question_that_already_finds_something_is_left_alone() {
    let mut db = replay_db();
    for q in ["部署失敗 ERR_DEPLOY_42", "客服專線", "release candidate"] {
        let got = ask(&mut db, q);
        assert!(!got.empty, "{q}");
        assert_eq!(got.searched, None, "{q}：原查詢有東西就不改字");
    }
}
