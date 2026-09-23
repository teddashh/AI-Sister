//! 「三天前那通客服電話，100 毫秒內查得回來」——量它，不要宣稱它。
//!
//! PHASES.md 的 Phase 0 退場條件第三條寫著 `sister query 電話` 要在 100ms 內
//! 撈回三天前畫面上的電話並附出處。在這個檔案存在之前，那句話唯一的實作是
//! `db.rs` 上的一行註解：「延遲預算 < 100ms（SPEC §8.2）」。
//!
//! 而現有的單元測試都跑在十來列的資料庫上。在一個十列的表上量到 0.2 ms，
//! 對「一年份的螢幕」這個問題完全沒有資訊量——那不是通過，那是還沒開始問。
//!
//! 所以這裡的重點不是那個 100，是**那個 100 是在多大的資料上量到的**。
//! 測試會把語料規模印出來，任何人都能自己判斷這個數字能外推到哪裡。

use sister_core::db::Db;
use sister_core::model::{FocusEvent, FocusKind, FocusSnapshot, FrameCapture, OcrBlock};
use std::time::Instant;

/// 一個工作日的規模：8 小時、每 5 秒一張留下來的畫面。
const FRAMES_PER_DAY: usize = 5_760;
const LINES_PER_FRAME: usize = 12;
const SWITCH_EVERY: usize = 60;

/// 要灌幾天。預設 1 天，因為 `cargo test` 每次都要跑，而 debug build 灌一天
/// 要 3 秒——灌一個月就是 100 秒，那會變成一個大家開始想辦法跳過的測試。
///
/// CI 另外用 `SISTER_BENCH_DAYS=30` 加 `--release` 跑一次真正的規模。
/// 一天的數字證明得了「FTS5 沒有一開始就垮」，證明不了一年；一年份要等
/// Phase 2 的重播語料庫才有辦法誠實地給。
fn days() -> usize {
    std::env::var("SISTER_BENCH_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

/// SPEC §8.2 的硬指標。有索引的查詢必須待在裡面。
const BUDGET_MS: f64 = 100.0;

/// 剩下那條沒有索引的路（**一個字**的中文）的天花板。**放得很鬆是故意的。**
///
/// schema 3 之前，兩個字的中文也在這條路上；bigram 索引把它接走了之後，
/// 只剩單字查詢還要掃表——`cjk_bigrams("工")` 產不出任何雙字，沒東西可查。
///
/// 界線本身不在這裡驗——用時間去驗界線是脆的：CI 的機器比開發機慢一倍，
/// 一個貼著實測值的天花板就會因為機器慢而變紅，然後大家開始往上調它，
/// 直到它不再代表任何事。界線是 `db.rs` 的 `LIKE_SCAN_DAYS`。
///
/// 這裡只擋災難級的回歸——例如掃描開始 join 別的表、或整個索引沒被用到。
const CAPPED_CEILING_MS: f64 = 600.0;

fn needle_at() -> usize {
    FRAMES_PER_DAY * days() / 2
}

/// 第 `i` 張畫面的時間戳。
///
/// 這裡本來是 `start + i * 5_000`——一天 5,760 張、每張間隔 5 秒，一路連著
/// 排下去。那代表「30 天份的畫面」在時間軸上只佔了 5,760×30×5 秒 = **10 天**。
/// 於是任何「只往回看 30 天」的界線都碰不到邊，量出來當然跟沒有界線一樣。
///
/// 我差點就這樣收工了：程式碼裡有界線、註解說它有效、測試是綠的。真正戳破
/// 它的是把語料加到 60 天——時間直接翻倍（101.5 → 204.4 ms）。一個界線如果
/// 從來沒有被碰到過，它跟不存在是同一件事。
///
/// 所以現在一天的畫面就待在那一天裡：早上 9 點開始、每 5 秒一張、8 小時。
fn ts_of(i: usize) -> i64 {
    const DAY_MS: i64 = 86_400_000;
    const NINE_AM: i64 = 9 * 3_600_000;
    // 從「今天」往回數 `days()` 天開始灌，最後一張落在最近。
    let base = 1_700_000_000_000i64 - days() as i64 * DAY_MS;
    let day = (i / FRAMES_PER_DAY) as i64;
    let within = (i % FRAMES_PER_DAY) as i64 * 5_000;
    base + day * DAY_MS + NINE_AM + within
}

fn frame(ts: i64, i: usize) -> FrameCapture {
    // 每一張畫面長得不一樣，不然 FTS 的倒排索引只有幾個 term，量到的會是
    // 一個不存在的最佳情況。
    let mut lines: Vec<String> = (0..LINES_PER_FRAME - 1)
        .map(|k| {
            format!(
                "工單 {}-{} 客戶回報 系統回應逾時 錯誤碼 E{:04} 已轉交二線",
                i,
                k,
                (i * 7 + k) % 9999
            )
        })
        .collect();

    // 針要在草堆的正中間，不是最後一列——SQLite 的 FTS 不會因為排序而作弊，
    // 但 `LIMIT` 加上 `ORDER BY ts DESC` 會，所以把它放在時間軸中間。
    if i == needle_at() {
        lines.push("中華電信 客服專線 0800-080-123 帳單問題請按 2".to_string());
    } else {
        lines.push(format!("備註 {i} 無特殊狀況"));
    }

    FrameCapture {
        assistive: Vec::new(),
        ts,
        monitor: 0,
        width: 2560,
        height: 1440,
        dhash: i as u64,
        image: None,
        image_ext: "webp",
        ocr: lines
            .iter()
            .enumerate()
            .map(|(k, t)| OcrBlock {
                text: t.clone(),
                x: 0,
                y: k as i32 * 20,
                w: 1200,
                h: 18,
                confidence: -1.0,
            })
            .collect(),
        focus: FocusSnapshot {
            app_id: Some("chrome.exe".into()),
            app_name: Some("Google Chrome".into()),
            window_title: Some(format!("工單系統 — #{i}")),
            url: Some("https://tickets.example.com/queue".into()),
            pid: Some(4242),
        },
    }
}

#[test]
fn finding_a_phone_number_in_a_days_worth_of_screens_stays_under_budget() {
    let mut db = Db::open_in_memory().expect("open");
    let session = db.start_session("bench", "0.0.1").expect("session");

    let frames = FRAMES_PER_DAY * days();
    let built = Instant::now();
    let mut focus_events = 0;
    for i in 0..frames {
        let mut capture = frame(ts_of(i), i);
        let app = ["chrome.exe", "code.exe", "terminal.exe", "notepad.exe"][(i / SWITCH_EVERY) % 4];
        capture.focus.app_id = Some(app.into());
        capture.focus.app_name = Some(app.into());
        db.insert_frame(session, &capture, None, 0).expect("insert");
        // insert_frame 不會寫 focus_events；章節需要的焦點事件須另寫入。
        if i % SWITCH_EVERY == 0 {
            db.insert_focus(
                session,
                &FocusEvent {
                    ts: capture.ts,
                    kind: FocusKind::Focus,
                    snapshot: capture.focus,
                },
            )
            .expect("insert focus");
            focus_events += 1;
        }
    }
    println!("焦點事件：{focus_events} 筆（每 {SWITCH_EVERY} 張切換，四個 app 輪替）");
    let chunks = frames * LINES_PER_FRAME;
    println!(
        "語料：{} 天 = {frames} 張畫面 × {LINES_PER_FRAME} 行 = {chunks} 行字（灌了 {:.1} 秒）",
        days(),
        built.elapsed().as_secs_f64()
    );

    // 使用者不會只用一種問法，而不同問法走的是完全不同的路。
    //
    //   有索引：trigram（≥3 字的任意子字串）、unicode61（整個 token）、
    //           bigram（切好的相鄰雙字）
    //   沒索引：一個字的中文——三個索引都接不住，只剩掃表
    //
    // 「兩個字的中文」以前在下面那一列：trigram 太短比不了，而 unicode61 把
    // 「客服專線」整串當成**一個** token，MATCH "客服" 是 0 筆，於是只能掃表。
    // schema 3 的 bigram 索引就是為了這一行存在的。
    for (label, q, indexed) in [
        ("三個字以上（trigram）", "客服專線", true),
        ("整個 token（unicode61）", "0800", true),
        ("兩個字的中文（bigram）", "客服", true),
        (
            "查不到的東西（bigram 確定沒有）",
            "這個字串不存在於任何一張畫面",
            true,
        ),
        ("一個字的中文（沒有索引）", "工", false),
    ] {
        // 先熱一次，量的是穩態而不是第一次把索引拉進 page cache 的成本。
        let _ = db.search(q, 20).expect("warm");

        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let hits = db.search(q, 20).expect("search");
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            if q == "客服專線" {
                assert!(!hits.is_empty(), "「客服專線」在語料裡，卻查不到");
                assert!(
                    hits.iter().any(|h| h.frame_id.is_some()),
                    "查得到但沒有出處——出處是這個產品的全部意義"
                );
            }
        }

        let mark = if indexed { "索引" } else { "掃描" };
        println!("  {mark}  {label:<26} {q:<16} {best:>6.1} ms");

        if indexed {
            assert!(
                best < BUDGET_MS,
                "{label}（{q}）花了 {best:.1} ms，超過 {BUDGET_MS} ms 的預算。\n\
                 這條路有索引，變慢代表索引沒被用到——去看 EXPLAIN QUERY PLAN。"
            );
        } else {
            // 這條路沒有索引，成本 = 掃 `LIKE_SCAN_DAYS` 天的資料。它**本來就**
            // 貼著預算，所以這裡不假裝它有通過。
            //
            // 這個斷言守的是另一件事：**成本不會再跟著使用時間長大**。界線
            // 拿掉的話，60 天會變兩倍、一年會變十二倍，而這行會紅。
            assert!(
                best < CAPPED_CEILING_MS,
                "{label}（{q}）花了 {best:.1} ms。這條路沒有索引，成本應該被\n\
                 `LIKE_SCAN_DAYS`（30 天）夾住而不隨語料長大——超過 {CAPPED_CEILING_MS} ms\n\
                 代表界線失效了，資料庫越用越久就會越慢，沒有盡頭。\n\
                 語料：{chunks} 行字。"
            );
        }
    }

    measure_answer_steps(&mut db, frames);

    println!(
        "\n  兩個字的中文（「客服」）與查不到的東西，都在 schema 3 的 bigram 索引上。\n  \
         在這之前它們走的是「掃 30 天」——45 天語料上分別是 224 ms 與 96.7 ms。\n  \
         剩下只有**一個字**的查詢還在掃描，因為單字產不出雙字。"
    );
}

/// 中位數只用來觀察，不替章節、盲點或判讀新增時間門檻。
fn measure<T>(mut step: impl FnMut() -> T) -> (T, f64) {
    let mut samples = Vec::new();
    let mut last = None;
    for _ in 0..5 {
        let start = Instant::now();
        last = Some(step());
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    (last.unwrap(), samples[2])
}

fn measure_answer_steps(db: &mut Db, frames: usize) {
    let now = 1_700_000_000_000;
    // 交集不代表有段落。實際算出非零活動才選它；全空必須紅。
    let question = ["昨天", "前天", "這禮拜", "上禮拜", "今天"]
        .into_iter()
        .find(|question| {
            let chapters = db.chapters_for_question_read_only(question, now).unwrap();
            let count = chapters
                .as_ref()
                .map_or(0, |(_, activities)| activities.len());
            println!("章節候選：{question} = {count} 段");
            count > 0
        });
    assert!(
        question.is_some(),
        "一種問法都撈不到章節：focus_events 語料沒有走到章節路徑"
    );
    let question = question.unwrap();
    println!("回答步驟：{question}，各取 5 次中位數（無新增時間門檻）");
    let ((range, early), ms) = measure(|| {
        db.chapters_for_question_read_only(question, now)
            .unwrap()
            .unwrap()
    });
    assert!(!early.is_empty(), "先開口必須真的算出章節");
    println!(
        "  chapters_for_question_read_only：{} 段，{ms:.3} ms",
        early.len()
    );
    let ((_, saved), ms) = measure(|| db.chapters_for_question(question, now).unwrap().unwrap());
    assert_eq!(early, saved, "正式與唯讀章節應相同");
    println!("  chapters_for_question：{} 段，{ms:.3} ms", saved.len());

    // 盲點會數整份 DB；仍用上面那份完整畫面語料，不另造小資料庫。
    let data_dir =
        std::env::temp_dir().join(format!("sister-search-latency-{}", std::process::id()));
    assert!(!data_dir.exists(), "盲點夾具不可讀到別場的錄製狀態");
    let (blind, ms) = measure(|| {
        sister_core::answer::blind_spots_during(db, &data_dir, question, Some(&range)).unwrap()
    });
    assert!(
        blind.frames > 0 && blind.chunks > 0,
        "盲點必須數到非零 frames／chunks"
    );
    assert_eq!(blind.frames as usize, frames);
    println!(
        "  blind_spots_during：{} frames／{} chunks，{ms:.3} ms",
        blind.frames, blind.chunks
    );
    let ((cards, truncated), ms) =
        measure(|| db.readings_spanning(range.from, range.to, 18).unwrap());
    println!(
        "  readings_spanning：{} 張卡，truncated={truncated}，{ms:.3} ms",
        cards.len()
    );
    let ((cards, truncated), ms) =
        measure(|| db.readings_near(ts_of(frames / 2), 300_000, 18).unwrap());
    println!(
        "  readings_near：{} 張卡，truncated={truncated}，{ms:.3} ms",
        cards.len()
    );
    println!("  判讀夾具未灌 L2 卡；上述兩列只量空卡表查詢，不代表有卡時的延遲。");
}
