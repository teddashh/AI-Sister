//! Exercise the actual desktop projection on hosts without WebKit/GTK.
#[allow(dead_code)]
#[path = "../../../apps/desktop/src-tauri/src/answer_readings.rs"]
mod desktop;

use sister_core::db::{Db, L2Author, L2Insert};
use sister_core::grounded_answer::{self as grounded, Reading, ReadingOrigin};

fn insert(db: &mut Db, at: i64, activity: &str) -> i64 {
    db.insert_l2_card(&L2Insert {
        segment_core_start: at,
        segment_ref: &format!("segment:{at}"),
        activity,
        entities_json: "[]".into(),
        continues_json: None,
        commitments_json: "[]".into(),
        model_confidence: 0.8,
        evidence_json: "[]".into(),
        open_questions_json: "[]".into(),
        author: L2Author::Interpreter,
    })
    .unwrap()
}

#[test]
fn card_only_answer_reaches_prompt_and_desktop_from_read_only_db() {
    let dir = std::env::temp_dir().join(format!("a152-readonly-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sister.db");
    let id = {
        let mut db = Db::open(&path).unwrap();
        insert(&mut db, 100, "退款申請已送出")
    };
    let db = Db::open_read_only(&path).unwrap();
    let (readings, more) =
        grounded::match_answer_readings(&db, &["退款".into()], vec![], false).unwrap();
    assert_eq!(readings.len(), 1);
    assert_eq!(readings[0].card_id, id);
    assert!(readings[0].matched);
    assert!(!more);
    let prepared = grounded::prepare("退款", &readings, &[], &[], 1000)
        .unwrap()
        .unwrap();
    assert!(prepared.payload.contains("退款申請已送出"));
    assert_eq!(
        desktop::Reading::from_core(&readings[0], &Default::default())
            .activity
            .as_deref(),
        Some("退款申請已送出")
    );
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn eight_time_cards_cannot_displace_content_match() {
    let mut db = Db::open_in_memory().unwrap();
    for at in 1..=10 {
        insert(&mut db, at, "時間背景");
    }
    let id = insert(&mut db, 100, "退款申請");
    let (time, more) = db.readings_spanning(0, 20, 8).unwrap();
    assert_eq!(time.len(), 8);
    assert!(more);
    let (readings, truncated) =
        grounded::match_answer_readings(&db, &["退款".into()], time, more).unwrap();
    assert_eq!(readings.len(), 8);
    assert!(readings.iter().any(|r| r.card_id == id && r.matched));
    assert_eq!(readings.iter().filter(|r| !r.matched).count(), 7);
    assert!(
        readings
            .windows(2)
            .all(|r| (r[0].at, r[0].card_id) < (r[1].at, r[1].card_id))
    );
    assert!(truncated);
}

/// 同一張卡被時間路和內容路同時撈到，只留一張、而且算內容命中。
///
/// **這條守的是「`from_card` 建構時就把 matched 設對了」，不是「去重那一行
/// 把它救回來」。** `match_answer_readings` 裡那句 `existing.matched = true;`
/// 今天是 no-op（整行拿掉仍然 10 綠 0 紅，2026-09-23 實測），所以讀到這個
/// 名字的時候不要以為那一行有人守著——理由寫在它自己上面。
#[test]
fn duplicate_time_and_content_card_keeps_match() {
    let mut db = Db::open_in_memory().unwrap();
    let id = insert(&mut db, 100, "退款申請");
    let time = db.readings_spanning(0, 200, 8).unwrap().0;
    let (readings, more) =
        grounded::match_answer_readings(&db, &["退款".into(), "退款申請".into()], time, false)
            .unwrap();
    assert_eq!(readings.len(), 1);
    assert_eq!(readings[0].card_id, id);
    assert!(readings[0].matched);
    assert!(!more);
}

#[test]
fn every_planned_query_shares_four_content_slots_and_reports_omissions() {
    let mut db = Db::open_in_memory().unwrap();
    for at in 1..=6 {
        insert(&mut db, at, "退款申請");
    }
    let later = insert(&mut db, 100, "修好登入");
    let (readings, more) =
        grounded::match_answer_readings(&db, &["退款".into(), "登入".into()], vec![], false)
            .unwrap();
    assert_eq!(readings.len(), 4);
    assert!(readings.iter().all(|r| r.matched));
    assert!(readings.iter().any(|r| r.card_id == later));
    assert!(more);
    // A single non-overflowing query must not inherit another call's truncation.
    let (readings, more) =
        grounded::match_answer_readings(&db, &["登入".into()], vec![], false).unwrap();
    assert_eq!(readings.len(), 1);
    assert!(!more);
}

#[test]
fn no_content_match_keeps_readings_and_prompt_byte_identical() {
    let mut db = Db::open_in_memory().unwrap();
    for at in 1..=10 {
        insert(&mut db, at, "時間背景");
    }
    let nonce = regex::Regex::new(r"nonce=[0-9a-f]{32}").unwrap();
    for count in [0, 4, 8] {
        let (time, old_more) = db.readings_spanning(0, 20, count).unwrap();
        let old = time
            .iter()
            .map(|r| Reading::from_card(r, ReadingOrigin::Time))
            .collect::<Vec<_>>();
        let (new, new_more) =
            grounded::match_answer_readings(&db, &["不存在".into()], time, old_more).unwrap();
        assert_eq!(old, new);
        assert_eq!(old_more, new_more);
        let mut before = grounded::prepare("不存在", &old, &[], &[], 1000).unwrap();
        let mut after = grounded::prepare("不存在", &new, &[], &[], 1000).unwrap();
        // The only per-call randomness is the existing security fence nonce.
        // Hold that value equal; compare every other prompt byte and source field.
        for prepared in [&mut before, &mut after].into_iter().flatten() {
            assert_eq!(nonce.find_iter(&prepared.payload).count(), 2);
            prepared.payload = nonce
                .replace_all(&prepared.payload, "nonce=FIXED_TEST_NONCE")
                .into_owned();
        }
        assert_eq!(before, after);
    }
}

#[test]
fn combined_query_overflow_does_not_demote_matches_to_background() {
    let mut db = Db::open_in_memory().unwrap();
    for at in 1..=3 {
        insert(&mut db, at, "退款申請");
    }
    for at in 4..=6 {
        insert(&mut db, at, "修好登入");
    }
    for at in 7..=10 {
        insert(&mut db, at, "時間背景");
    }
    let time = db.readings_spanning(0, 20, 10).unwrap().0;
    assert_eq!(time.len(), 10);
    assert!(!db.search_readings("退款", 4).unwrap().1);
    assert!(!db.search_readings("登入", 4).unwrap().1);
    let (readings, more) =
        grounded::match_answer_readings(&db, &["退款".into(), "登入".into()], time, false).unwrap();
    assert_eq!(readings.len(), 8);
    assert_eq!(readings.iter().filter(|r| r.matched).count(), 4);
    assert!(
        readings
            .iter()
            .filter(|r| !r.matched)
            .all(|r| r.activity == "時間背景")
    );
    assert!(more);
}

fn time_only_cards() -> Vec<Reading> {
    let mut db = Db::open_in_memory().unwrap();
    let now = 1_757_299_200_000;
    let asked = db
        .chapters_for_question_read_only("今天", now)
        .unwrap()
        .unwrap();
    assert!(asked.1.is_empty());
    insert(&mut db, asked.0.from + 1000, "那段時間在處理退款");
    let rows = db.readings_spanning(asked.0.from, asked.0.to, 8).unwrap().0;
    assert_eq!(rows.len(), 1);
    rows.iter()
        .map(|r| Reading::from_card(r, ReadingOrigin::Time))
        .collect()
}

#[test]
fn a153_time_only_card_becomes_visible_answer() {
    let mut readings = time_only_cards();
    desktop::present_time_readings(
        Some(&((), Vec::<i32>::new())),
        &[] as &[i32],
        &[] as &[i32],
        &mut readings,
    );
    assert_eq!(
        desktop::Reading::from_core(&readings[0], &Default::default())
            .activity
            .as_deref(),
        Some("那段時間在處理退款")
    );
    assert_eq!(
        desktop::answer_hit_count::<i32, i32>(&[], &[], &readings),
        1
    );
    assert!(!desktop::needs_answer_blind_spots::<i32, i32>(
        &[],
        &[],
        &readings,
        &desktop::BrainAnswer::not_configured()
    ));
}

#[test]
fn a153_chapters_keep_background_byte_identical() {
    assert_background_unchanged(Some(&((), vec![1])), &[], &[]);
}
#[test]
fn a153_hits_keep_background_byte_identical() {
    assert_background_unchanged(Some(&((), vec![])), &[], &[1]);
}
#[test]
fn a153_facts_keep_background_byte_identical() {
    assert_background_unchanged(Some(&((), vec![])), &[1], &[]);
}
#[test]
fn a153_no_range_keeps_background_byte_identical() {
    assert_background_unchanged(None, &[], &[]);
}
fn assert_background_unchanged(asked: Option<&((), Vec<i32>)>, facts: &[i32], hits: &[i32]) {
    let mut readings = time_only_cards();
    let before = serde_json::to_vec(&desktop::Reading::from_core(
        &readings[0],
        &Default::default(),
    ))
    .unwrap();
    desktop::present_time_readings(asked, facts, hits, &mut readings);
    let after = serde_json::to_vec(&desktop::Reading::from_core(
        &readings[0],
        &Default::default(),
    ))
    .unwrap();
    assert_eq!(before, after);
}
