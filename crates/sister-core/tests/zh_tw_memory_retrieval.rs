//! 繁中多日語料走正式入庫 → 檢索 → RAG 來源，不經背景解釋層。
//!
//! 場景刻意像日常：電話、帳單金額、位址列網址、寫到一半被帳單打斷的程式、
//! 隔天改掉的數字、以及一個記憶裡沒有的問法。截圖有無與日期窗必須照實。

use sister_core::db::Db;
use sister_core::grounded_answer::{self, SourceOrigin, SourceRef};
use sister_core::model::{
    AssistiveBlock, FocusEvent, FocusKind, FocusSnapshot, FrameCapture, OcrBlock, SourceKind,
};
use sister_core::question;
use sister_core::retrieval::{RetrievalLimits, RetrievalProfile};

use chrono::{Local, TimeZone};

fn local_ms(year: i32, month: u32, day: u32, hour: u32, min: u32) -> i64 {
    Local
        .with_ymd_and_hms(year, month, day, hour, min, 0)
        .single()
        .expect("local timestamp")
        .timestamp_millis()
}

fn ocr(text: &str) -> Vec<OcrBlock> {
    vec![OcrBlock {
        text: text.into(),
        x: 0,
        y: 0,
        w: 400,
        h: 40,
        confidence: 0.9,
    }]
}

fn chrome(title: &str, url: &str) -> FocusSnapshot {
    FocusSnapshot {
        app_id: Some("chrome.exe".into()),
        app_name: Some("Google Chrome".into()),
        window_title: Some(title.into()),
        url: Some(url.into()),
        pid: Some(1001),
    }
}

fn code_focus() -> FocusSnapshot {
    FocusSnapshot {
        app_id: Some("code.exe".into()),
        app_name: Some("Visual Studio Code".into()),
        window_title: Some("db.rs - AI-Sister".into()),
        url: None,
        pid: Some(1002),
    }
}

fn insert_screen(
    db: &mut Db,
    session: i64,
    ts: i64,
    focus: FocusSnapshot,
    ocr_text: &str,
    assistive_text: Option<&str>,
    image_path: Option<&str>,
) -> i64 {
    let assistive = assistive_text
        .map(|text| AssistiveBlock {
            text: text.into(),
            role: "document".into(),
            bbox: None,
        })
        .into_iter()
        .collect();
    let (frame_id, _, _) = db
        .insert_frame(
            session,
            &FrameCapture {
                ts,
                monitor: 0,
                width: 1920,
                height: 1080,
                dhash: ts as u64,
                image: None,
                image_ext: "png",
                ocr: ocr(ocr_text),
                assistive,
                focus: focus.clone(),
            },
            image_path,
            i64::from(image_path.is_some()),
        )
        .expect("frame");
    db.insert_focus(
        session,
        &FocusEvent {
            ts,
            kind: FocusKind::Focus,
            snapshot: focus,
        },
    )
    .expect("focus");
    frame_id
}

fn plant_corpus(db: &mut Db) -> Planted {
    let now = local_ms(2026, 9, 20, 15, 0);
    let session = db.start_session("test", "test").expect("session");

    let day_before = local_ms(2026, 9, 18, 16, 10);
    let yesterday_bill = local_ms(2026, 9, 19, 10, 4);
    let yesterday_scan = local_ms(2026, 9, 19, 10, 18);
    let yesterday_code = local_ms(2026, 9, 19, 14, 22);
    let today_bill = local_ms(2026, 9, 20, 9, 12);

    insert_screen(
        db,
        session,
        day_before,
        code_focus(),
        "crates/sister-core/src/db.rs\nfn search_during\n還沒改完",
        None,
        None,
    );

    let yesterday_bill_frame = insert_screen(
        db,
        session,
        yesterday_bill,
        chrome(
            "中華電信 客戶服務 - 帳單查詢",
            "https://bill.cht.com.tw/query",
        ),
        "中華電信 帳單查詢\n本期應繳金額 NT$13,450\n繳費期限 2026/09/25",
        Some("中華電信 帳單查詢\n客服專線 0800-080-123\n本期應繳金額 NT$13,450"),
        None,
    );

    let yesterday_scan_frame = insert_screen(
        db,
        session,
        yesterday_scan,
        FocusSnapshot {
            app_id: Some("msedge.exe".into()),
            app_name: Some("Microsoft Edge".into()),
            window_title: Some("電信帳單.pdf".into()),
            url: None,
            pid: Some(1003),
        },
        "門號 ０９１２－３４５－６７８\n本期應繳 ＮＴ＄１３，４５０",
        None,
        None,
    );

    let interrupted_frame = insert_screen(
        db,
        session,
        yesterday_code,
        code_focus(),
        "crates/sister-core/src/db.rs\nerror[E0308]: mismatched types\nexpected `i64`, found `u64`",
        Some("crates/sister-core/src/db.rs\nerror[E0308]: mismatched types"),
        None,
    );

    let today_frame = insert_screen(
        db,
        session,
        today_bill,
        chrome(
            "中華電信 客戶服務 - 帳單查詢",
            "https://bill.cht.com.tw/query",
        ),
        "中華電信 帳單查詢\n本期應繳金額 NT$14,200\n客服專線 0800-080-999",
        Some("本期應繳金額 NT$14,200\n客服專線 0800-080-999"),
        Some("today-bill.png"),
    );

    Planted {
        now,
        yesterday_bill_frame,
        yesterday_scan_frame,
        interrupted_frame,
        today_frame,
    }
}

struct Planted {
    now: i64,
    yesterday_bill_frame: i64,
    yesterday_scan_frame: i64,
    interrupted_frame: i64,
    today_frame: i64,
}

fn retrieve(db: &mut Db, question: &str, now: i64) -> sister_core::retrieval::Retrieval {
    RetrievalProfile::TextAndFacts
        .retrieve_at(db, question, RetrievalLimits::same(10), now)
        .expect(question)
}

#[test]
fn traditional_chinese_multiday_memory_keeps_sources_dates_and_missing_screenshots_honest() {
    let mut db = Db::open_in_memory().expect("db");
    let planted = plant_corpus(&mut db);
    let yesterday = question::time_range("昨天", planted.now).expect("yesterday");

    let phone = retrieve(&mut db, "昨天電話", planted.now);
    assert_eq!(phone.shape, question::Shape::Keywords);
    assert_eq!(
        phone.time_range.as_ref().map(|r| r.said.as_str()),
        Some("昨天")
    );
    assert!(
        phone
            .answers
            .iter()
            .all(|a| a.latest.ts >= yesterday.from && a.latest.ts < yesterday.to),
        "dated phone facts must stay inside yesterday: {:?}",
        phone
            .answers
            .iter()
            .map(|a| (a.latest.raw.clone(), a.latest.ts))
            .collect::<Vec<_>>()
    );
    assert!(
        phone
            .hits
            .iter()
            .all(|hit| hit.ts >= yesterday.from && hit.ts < yesterday.to),
        "dated text hits must stay inside yesterday"
    );
    let phones: Vec<&str> = phone
        .answers
        .iter()
        .map(|a| a.latest.normalized.as_str())
        .collect();
    assert!(
        phones.contains(&"+886800080123"),
        "UIA 客服專線: {phones:?}"
    );
    assert!(
        phones.contains(&"+886912345678"),
        "OCR-only 全形門號 must become a phone fact: {phones:?}"
    );
    assert!(
        !phones.contains(&"+886800080999"),
        "today's new hotline must not leak into 昨天電話: {phones:?}"
    );
    let uia = phone
        .answers
        .iter()
        .find(|a| a.latest.normalized == "+886800080123")
        .expect("hotline");
    assert_eq!(uia.latest.source_kind, "assistive");
    assert_eq!(uia.latest.frame_id, Some(planted.yesterday_bill_frame));
    assert_eq!(uia.sightings, 1, "同一幀的 UIA 專線只算一次目擊");
    let scanned = phone
        .answers
        .iter()
        .find(|a| a.latest.normalized == "+886912345678")
        .expect("mobile");
    assert_eq!(scanned.latest.source_kind, "ocr");
    assert_eq!(scanned.latest.raw, "０９１２－３４５－６７８");
    assert_eq!(scanned.latest.frame_id, Some(planted.yesterday_scan_frame));

    let bill = retrieve(&mut db, "昨天帳單", planted.now);
    let amounts: Vec<&str> = bill
        .answers
        .iter()
        .map(|a| a.latest.normalized.as_str())
        .collect();
    assert_eq!(
        amounts,
        ["TWD:13450"],
        "yesterday's bill, not today's rewrite"
    );
    assert!(
        bill.answers[0].latest.frame_id == Some(planted.yesterday_bill_frame)
            || bill.answers[0].latest.frame_id == Some(planted.yesterday_scan_frame),
        "bill amount must keep a yesterday frame, got {:?}",
        bill.answers[0].latest.frame_id
    );

    let due = retrieve(&mut db, "昨天應繳", planted.now);
    assert_eq!(
        due.answers[0].latest.normalized, "TWD:13450",
        "應繳 must map to money without requiring 金額"
    );

    for query in ["昨天網址", "昨天那個連結", "昨天網站"] {
        let got = retrieve(&mut db, query, planted.now);
        assert_eq!(
            got.answers.len(),
            1,
            "{query} should hit the address-bar URL fact, got {:?}",
            got.answers
                .iter()
                .map(|a| (a.latest.kind.clone(), a.latest.raw.clone()))
                .collect::<Vec<_>>()
        );
        assert_eq!(got.answers[0].latest.kind, "url");
        assert_eq!(
            got.answers[0].latest.normalized,
            "https://bill.cht.com.tw/query"
        );
        assert_eq!(got.answers[0].latest.source_kind, "url");
        // 位址列現在也會隨保留畫面寫入有 frame 的 URL fact；獨立的
        // focus event URL fact 則沒有 frame。兩者都由 source_kind=url 區分於 OCR。
        assert!(
            got.answers[0].latest.ts >= yesterday.from && got.answers[0].latest.ts < yesterday.to,
            "{query} date bound"
        );
    }

    let interrupted = retrieve(&mut db, "昨天 db.rs", planted.now);
    assert!(
        interrupted.hits.iter().any(
            |hit| hit.frame_id == Some(planted.interrupted_frame) && hit.text.contains("E0308")
        ),
        "interrupted coding evidence missing: {:?}",
        interrupted
            .hits
            .iter()
            .map(|h| (h.frame_id, h.text.clone()))
            .collect::<Vec<_>>()
    );
    let error = retrieve(&mut db, "昨天錯誤", planted.now);
    assert!(
        error.answers.iter().any(|a| a.latest.normalized == "E0308"
            && a.latest.frame_id == Some(planted.interrupted_frame)),
        "error fact should keep the original code frame"
    );

    let latest = retrieve(&mut db, "帳單", planted.now);
    assert_eq!(
        latest.answers[0].latest.normalized, "TWD:14200",
        "undated bill query must surface the newer contradictory amount first"
    );
    assert!(
        latest
            .answers
            .iter()
            .any(|a| a.latest.normalized == "TWD:13450"),
        "older amount remains a distinct fact, not overwritten"
    );
    assert_eq!(latest.answers[0].latest.frame_id, Some(planted.today_frame));

    let miss = retrieve(&mut db, "火星會議連結", planted.now);
    assert!(miss.answers.is_empty(), "no-hit must not invent URL facts");
    assert!(
        miss.hits.is_empty(),
        "no-hit must not return unrelated text"
    );
    assert!(
        grounded_answer::prepare("火星會議連結", &[], &miss.answers, &miss.hits, planted.now)
            .expect("prepare")
            .is_none(),
        "no local evidence means no RAG payload"
    );

    let before = retrieve(&mut db, "前天帳單", planted.now);
    assert!(
        before.answers.is_empty() && before.hits.is_empty(),
        "the day before only has code, not a bill"
    );

    let rag = grounded_answer::prepare("昨天電話", &[], &phone.answers, &phone.hits, planted.now)
        .expect("prepare")
        .expect("yesterday phone evidence");
    assert!(
        rag.sources.iter().any(|source| {
            matches!(source.reference, SourceRef::Fact(_))
                && source.origin == SourceOrigin::Recorded(SourceKind::Assistive)
                && source.frame_id == Some(planted.yesterday_bill_frame)
                && source.ts >= yesterday.from
                && source.ts < yesterday.to
        }),
        "RAG must keep UIA origin and yesterday's frame"
    );
    assert!(
        rag.sources.iter().any(|source| {
            source.origin == SourceOrigin::Recorded(SourceKind::Ocr)
                && source.frame_id == Some(planted.yesterday_scan_frame)
                && source.text.contains("０９１２－３４５－６７８")
        }),
        "RAG must keep the original fullwidth OCR span"
    );
    assert!(
        rag.sources
            .iter()
            .all(|source| source.ts < yesterday.to && !source.text.contains("0800-080-999")),
        "RAG for 昨天電話 must not include today's rewrite"
    );

    let openable = db
        .frames_with_image(&[
            planted.yesterday_bill_frame,
            planted.yesterday_scan_frame,
            planted.interrupted_frame,
            planted.today_frame,
        ])
        .expect("image probe");
    assert!(
        !openable.contains(&planted.yesterday_bill_frame)
            && !openable.contains(&planted.yesterday_scan_frame)
            && !openable.contains(&planted.interrupted_frame),
        "yesterday was text-only; a frame_id is not a screenshot"
    );
    assert!(
        openable.contains(&planted.today_frame),
        "today's bill row actually stored an image path"
    );
    assert_eq!(
        db.frame_context(planted.yesterday_bill_frame)
            .expect("context")
            .expect("row")
            .image_path,
        None
    );

    let filler = retrieve(&mut db, "電話是多少", planted.now);
    assert!(
        filler
            .answers
            .iter()
            .any(|a| a.latest.kind == "phone" && !a.latest.normalized.is_empty()),
        "電話是多少 must not treat leftover filler as a required topic: {:?}",
        filler
            .answers
            .iter()
            .map(|a| a.latest.normalized.clone())
            .collect::<Vec<_>>()
    );

    let please = retrieve(&mut db, "請幫我找昨天電話", planted.now);
    let please_phones: Vec<&str> = please
        .answers
        .iter()
        .map(|a| a.latest.normalized.as_str())
        .collect();
    assert!(
        please_phones.contains(&"+886800080123") || please_phones.contains(&"+886912345678"),
        "請幫我找昨天電話 must still hit a yesterday phone: {please_phones:?}"
    );
    assert!(
        !please_phones.contains(&"+886800080999"),
        "請幫我找昨天電話 must keep yesterday's date bound: {please_phones:?}"
    );

    let english = retrieve(&mut db, "what was the phone number yesterday", planted.now);
    assert!(
        !english.answers.is_empty(),
        "English filler must not become a required topic that yields a false no-hit"
    );
    assert!(
        english.answers.iter().any(|a| a.latest.kind == "phone"),
        "English phone question still maps to L1 phone facts: {:?}",
        english
            .answers
            .iter()
            .map(|a| a.latest.kind.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn topic_filter_runs_before_value_limit_and_keeps_same_number_sources_apart() {
    let mut db = Db::open_in_memory().expect("db");
    let now = local_ms(2026, 9, 20, 15, 0);
    let session = db.start_session("test", "test").expect("session");
    let shared = "0800-222-333";

    let service_ts = local_ms(2026, 9, 19, 9, 0);
    let bill_ts = local_ms(2026, 9, 19, 18, 0);
    let service_frame = insert_screen(
        &mut db,
        session,
        service_ts,
        chrome("客服中心", "https://help.example.test/hotline"),
        "客服專線 0800-222-333",
        Some("客服專線 0800-222-333"),
        None,
    );
    let bill_frame = insert_screen(
        &mut db,
        session,
        bill_ts,
        chrome("本期帳單", "https://bill.example.test/query"),
        "本期帳單 0800-222-333",
        Some("本期帳單 0800-222-333"),
        None,
    );

    for i in 0..20 {
        let ts = local_ms(2026, 9, 20, 10, i);
        let phone = format!("0910-111-{:03}", i);
        insert_screen(
            &mut db,
            session,
            ts,
            chrome("分類廣告", "https://ads.example.test/list"),
            &format!("廣告回撥 {phone}"),
            None,
            None,
        );
    }

    let service = RetrievalProfile::TextAndFacts
        .retrieve_at(&mut db, "客服電話", RetrievalLimits::same(4), now)
        .expect("service topic");
    assert_eq!(
        service.answers.len(),
        1,
        "twenty newer ads must not drown the older 客服 number: {:?}",
        service
            .answers
            .iter()
            .map(|a| (a.latest.normalized.clone(), a.latest.window_title.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(service.answers[0].latest.normalized, "+886800222333");
    assert_eq!(service.answers[0].latest.frame_id, Some(service_frame));
    assert_eq!(
        service.answers[0].latest.window_title.as_deref(),
        Some("客服中心")
    );

    let latest = retrieve(&mut db, "電話", now);
    assert_eq!(
        latest.answers[0].latest.normalized, "+886910111019",
        "undated 電話 still surfaces the newest ad"
    );

    let same = retrieve(&mut db, "客服電話", now);
    assert_eq!(same.answers[0].latest.frame_id, Some(service_frame));
    assert_ne!(
        same.answers[0].latest.frame_id,
        Some(bill_frame),
        "same number on a later 帳單 page must not steal the 客服 source"
    );
    assert!(
        same.answers[0].latest.raw.contains(shared),
        "客服 source keeps the shared number, got {}",
        same.answers[0].latest.raw
    );
}
