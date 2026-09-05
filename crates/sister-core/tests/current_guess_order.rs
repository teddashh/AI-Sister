// ============================================================================
// r25 的驗收測試。**在 delegate 交貨之前寫好的。**
//
// 這一檔證的是「我事先寫下的考題過了」，不是「它的測試通過它自己的修法」。
//
// 這一輪是純搬家：把 `memory_current_guess` 的**判斷順序**從 `apps/desktop`
// （另一個 workspace，`cargo test --workspace` 碰不到）搬進 `sister-core`。
// 所以這一檔的斷言分兩種：
//
//   1. **順序**——誰贏過誰，以及早退時不准碰資料庫（那是行為，不是實作細節）
//   2. **搬家不准改變任何東西**——六個參數的接線、使用者看到的字
//
// ⚠ 這一檔**不會**在舊碼上「斷言失敗」，它在舊碼上**編不過**（`decide` 還不
//   存在）。編不過的紅和斷言失敗的紅是兩種東西，前者什麼都沒證明。
//   所以這一檔的價值在**交貨之後的突變**：把順序調換、把 fetch 改成先跑、
//   把 Err 吞掉、把六個參數接錯——每一種都要有人紅。清單在
//   `/home/ted-h/tmp-tests/validate-r25.py`（7 刀，實跑 7/7 被咬住）。
//   `main.rs` 那半在另一個 workspace，`cargo test` 碰不到，由
//   `scripts/check-current-guess-wiring.py` 守，它自己的正負對照在
//   `/home/ted-h/tmp-tests/validate-gate-r25.py`（31 刀：26 want=1、5 want=0，
//   另加一項「跑一次 `cargo fmt` 不准把閘門弄紅」）。那支腳本在 repo 外面。
// ============================================================================

use sister_core::brain::{CurrentGuess, LatestClosedSegment, RecordingFacts};
use sister_core::db::RetainedInterpreterAttempts;
use sister_core::heartbeat::{Phase, Presence};
use std::cell::Cell;

/// `from_presence` 回 `Some` 的那六格——也就是「還沒進入正在錄」的每一種。
/// `Presence::Live(Phase::Recording)` 是**唯一**會往下走的那一格
/// （`brain.rs` 的 `from_presence`，`Presence::Live(Phase::Recording) => None`）。
///
/// ⚠ 底下 `at` / `until` / `phase` 那幾個數字是**沒有人讀的**：`from_presence`
/// 對這三格是 `Presence::Thinking { .. }` 這種忽略 payload 的樣式。它們在這裡
/// 只是「要構造出這個 variant 就得填」，不是取樣點——改成別的值不會有任何一條
/// 測試變色。要測時間相關的行為請去 `heartbeat` 那邊，不要在這裡加數字然後
/// 以為多蓋到了一條軸。
fn early_returns() -> Vec<(Presence, CurrentGuess)> {
    vec![
        (Presence::NeverStarted, CurrentGuess::NeverStarted),
        (Presence::Unreadable, CurrentGuess::Unreadable),
        (Presence::Live(Phase::Booting), CurrentGuess::Booting),
        (
            Presence::Thinking {
                at: 1_000,
                until: 2_000,
            },
            CurrentGuess::Thinking,
        ),
        (Presence::Stopped { at: Some(3_000) }, CurrentGuess::Stopped),
        (
            Presence::Stalled {
                at: 4_000,
                phase: Phase::Recording,
            },
            CurrentGuess::Stalled,
        ),
    ]
}

fn facts(
    latest_closed: Option<LatestClosedSegment>,
    has_command: bool,
    consented: bool,
    used_today: u32,
    daily_budget: u32,
    previous_attempts: Option<RetainedInterpreterAttempts>,
) -> RecordingFacts {
    RecordingFacts {
        latest_closed,
        has_command,
        consented,
        used_today,
        daily_budget,
        previous_attempts,
    }
}

/// 一個會數自己被叫過幾次的 fetch。惰性那幾條全靠它。
struct Counting<'a>(&'a Cell<u32>, RecordingFacts);

impl Counting<'_> {
    fn take(self) -> impl FnOnce() -> Result<RecordingFacts, String> {
        let Counting(calls, f) = self;
        move || {
            calls.set(calls.get() + 1);
            Ok(f)
        }
    }
}

fn plain() -> RecordingFacts {
    facts(
        Some(LatestClosedSegment {
            has_card: false,
            worth_interpreting: true,
        }),
        true,
        true,
        0,
        80,
        None,
    )
}

// ---------------------------------------------------------------- 順序 ----

/// presence 說「還沒進入正在錄」的時候，暫停旗標**改變不了答案**。
///
/// 這一條釘的是搬家前 `main.rs:1773` 在 `:1780` **前面**這件事。兩個早退對調
/// 之後，暫停中而心跳過期的機器會說「記錄已暫停」而不是「心跳已過期」——
/// 前者聽起來是他自己按的，後者是故障。
#[test]
fn presence_that_says_not_recording_wins_over_pause() {
    for (presence, want) in early_returns() {
        for paused in [false, true] {
            let calls = Cell::new(0);
            let got: Result<CurrentGuess, String> =
                CurrentGuess::decide(presence, paused, Counting(&calls, plain()).take());
            assert_eq!(
                got.as_ref(),
                Ok(&want),
                "presence={presence:?} paused={paused} 走錯格"
            );
        }
    }
}

/// 暫停贏過資料庫會說的每一句話。
#[test]
fn pause_wins_over_everything_the_database_would_say() {
    let calls = Cell::new(0);
    let got: Result<CurrentGuess, String> = CurrentGuess::decide(
        Presence::Live(Phase::Recording),
        true,
        Counting(&calls, plain()).take(),
    );
    assert_eq!(got, Ok(CurrentGuess::Paused));
}

// -------------------------------------------------------------- 惰性 ----

/// **早退的時候一次都不准去碰資料庫。**
///
/// 這不是實作細節，是使用者看得到的行為：今天資料庫壞掉而錄製正暫停，畫面顯示
/// 的是「記錄已暫停⋯」**不是錯誤字串**。天真的重構（先把所有輸入算好再傳進純
/// 函式）會把這個性質弄壞，而且**弄壞之後上面那兩條測試照樣全綠**——它們只看
/// 回傳值。所以要單獨數呼叫次數。
///
/// **這條保證的範圍是 `fetch`，不是 `paused`。** `main.rs` 那半是先把
/// `is_paused(dir)` 算好、再把 `bool` 傳進來的（`memory_current_guess`），
/// 所以 presence 早退的那幾格還是會多做兩次檔案系統呼叫。
///
/// 那樣是安全的，但**理由不是「它很便宜」**——`pause::is_paused` 其實做**兩次**
/// syscall，不是一次：`flag_path(dir).try_exists()` 一次，而 `decide_for` 的
/// 第一行 `dir_state(data_dir)` 又是一次 `std::fs::metadata`
/// （`pause.rs:101`、`pause.rs:70`、`dir_state.rs:17`）。
///
/// 真正的理由是**它改變不了任何一格顯示**：它回一個 `bool`，沒有副作用、沒有
/// 錯誤路徑，而且 fail-closed（讀不到就當暫停）。算出來的那個值在 presence
/// 早退時直接被丟掉，**正因為 presence 贏過 paused**——也就是上面
/// `presence_that_says_not_recording_wins_over_pause` 對 `[false, true]` 兩個
/// 值都跑過的那件事。
///
/// 這句話有兩半，牙齒只長在一半上：「算出來的值被丟掉」有測試（就是上面那條
/// 兩個值都跑過的），「`is_paused` 沒有副作用」**沒有任何人守**——那是我讀
/// `pause.rs` 讀出來的，今天成立，而哪天有人在裡面加一行寫檔，這一整段就從
/// 「理由」變成「藉口」，而且不會有人紅。
///
/// 寫這一段是為了擋下一輪「順手把它也改成惰性」：那要改 `decide` 的簽章
/// （`paused: impl FnOnce() -> bool`），換到的是零個使用者看得到的差別，
/// 而弄錯順序就會讓暫停中又心跳過期的機器說「記錄已暫停」——聽起來是他自己
/// 按的，而其實是故障。
///
/// （這段話的第一版寫「只做一次 `try_exists`」，是假的。我把「便宜」當成了
/// 理由，而便宜從來不是理由——正確與否才是。審查照著去查證會發現對不上，
/// 然後不知道該信哪一半，那比沒有這段註解更糟。）
#[test]
fn the_database_is_never_touched_when_the_order_returns_early() {
    for (presence, _) in early_returns() {
        for paused in [false, true] {
            let calls = Cell::new(0);
            let _: Result<CurrentGuess, String> =
                CurrentGuess::decide(presence, paused, Counting(&calls, plain()).take());
            assert_eq!(
                calls.get(),
                0,
                "presence={presence:?} paused={paused}：早退卻去開了資料庫"
            );
        }
    }

    let calls = Cell::new(0);
    let _: Result<CurrentGuess, String> = CurrentGuess::decide(
        Presence::Live(Phase::Recording),
        true,
        Counting(&calls, plain()).take(),
    );
    assert_eq!(calls.get(), 0, "暫停中卻去開了資料庫");
}

/// 反向：真的需要的時候**要**去拿，而且只拿一次。
///
/// 沒有這一條的話，把 `fetch` 整個拿掉、永遠回一個寫死的值，上面那些惰性斷言
/// 會更綠。
#[test]
fn the_database_is_read_exactly_once_when_the_order_needs_it() {
    let calls = Cell::new(0);
    let _: Result<CurrentGuess, String> = CurrentGuess::decide(
        Presence::Live(Phase::Recording),
        false,
        Counting(&calls, plain()).take(),
    );
    assert_eq!(
        calls.get(),
        1,
        "正在錄又沒暫停，卻沒有去讀資料庫（或讀了不只一次）"
    );
}

// ---------------------------------------------------------------- 錯誤 ----

/// 查詢失敗要往上傳，**不准被吞成某一格 `CurrentGuess`**。
///
/// 吞掉的話，資料庫壞掉會長得像「這一刻還沒有任何段落」——一句完全正常的話。
#[test]
fn a_failed_read_is_not_swallowed_into_a_normal_looking_sentence() {
    let got: Result<CurrentGuess, String> =
        CurrentGuess::decide(Presence::Live(Phase::Recording), false, || {
            Err("資料庫壞了".to_string())
        });
    assert_eq!(got, Err("資料庫壞了".to_string()));
}

// ------------------------------------------------------- 接線不准接錯 ----

#[test]
fn no_closed_segment_is_no_segment() {
    let got: Result<CurrentGuess, String> =
        CurrentGuess::decide(Presence::Live(Phase::Recording), false, || {
            Ok(facts(None, true, true, 0, 80, None))
        });
    assert_eq!(got, Ok(CurrentGuess::NoSegment));
}

/// 沒有段落的時候，**其餘五個參數一個都不影響答案**。
///
/// 這一條不是湊數的：搬家之後 `main.rs` 在「沒有已關閉段落」那條路上把
/// `used_today` **寫死成 0**（不去跑那個日期＋計數的查詢，因為那一格用不到）。
/// 那個寫死只有在 `NoSegment` 排在預算檢查**前面**的時候才是安全的。
///
/// 今天是安全的（`while_recording` 第一件事就是 `let Some(..) = segment else`）。
/// 但 #144 正在討論**把早退重新排序**，而 `main.rs` 那半沒有任何測試——
/// 排序一動，那個 0 就從死資料變成會說謊的活資料：預算真的用完了，而 `main.rs`
/// 餵進來的是 0，於是 `BudgetExhausted` 那一格永遠到不了——畫面**不會**說
/// 「今天的解釋預算已用完」，它會退回去講別的理由。所以在這裡把「NoSegment
/// 贏過所有東西」釘死。
/// （這一段原本寫成「畫面會說『今天的額度還沒用』」——那句話 repo 裡不存在，
/// 是我自己造的產品字串；而且方向也反了，那一格的病是**不說**不是說錯。）
#[test]
fn without_a_closed_segment_none_of_the_other_five_arguments_can_change_the_answer() {
    for has_command in [true, false] {
        for consented in [true, false] {
            for (used, budget) in [(0u32, 80u32), (80, 80), (u32::MAX, 0)] {
                for prev in [
                    None,
                    Some(RetainedInterpreterAttempts {
                        count: 9,
                        latest_outcome: sister_core::brain::StoredOutboundOutcome::Known(
                            sister_core::brain::OutboundOutcome::Success,
                        ),
                    }),
                ] {
                    assert_eq!(
                        CurrentGuess::while_recording(
                            None,
                            has_command,
                            consented,
                            used,
                            budget,
                            prev
                        ),
                        CurrentGuess::NoSegment,
                        "cmd={has_command} consent={consented} used={used} budget={budget}：\
                         沒有段落卻不是 NoSegment——main.rs 那個寫死的 used_today: 0 現在會說謊"
                    );
                }
            }
        }
    }
}

/// **六個參數的接線，跑一張表。**
///
/// `decide` 底下就是 `while_recording`，所以拿 `while_recording` 當期望值在這裡
/// 是對的——這一條測的是**接線**（第 3 個參數接到 `consented` 還是
/// `has_command`），不是格式。
///
/// 格式那一半有兩個地方在守，**兩個都要看**：這一檔最下面那條窮舉 match
/// （15 句逐字），以及 `current_guess.rs`（`:255` 把 `Queued` 整句釘死、
/// `:430` 的 `fn expected()` 手寫整條 `AskedWithoutCard` 樣板再跑 5×8 格、
/// `:644` 把結局標籤釘成字面）。
///
/// ⚠ 這一段的上一版寫「**不是** `current_guess.rs`——我原本這樣寫，打開才
/// 發現那一檔一句字面都沒釘」。**那是假的，而且我是從一句真話「修正」成假話
/// 的。** 成因：`current_guess.rs:386` 有一句有範圍的自我批評（「既有的
/// `every_live_recording_outcome_says_what_was_checked` 只檢查非空和互不相同」
/// ——主詞是**一條測試**），我把主詞放大成整個檔案，還替它補了一段理由。
/// 修正的敘事會讓假話更可信：下一個審查看到「打開才發現」，會判定這句已經
/// 查證過而跳過。**「我打開 X 發現 X 沒有 Y」這種話一定要帶行號**，不然它
/// 躲得掉一整輪。
///
/// 網格要讓每一個參數都**單獨可分辨**，不然對調了也全綠：
///   - `has_command` / `consented` 各自為 false 時走到不同的一格
///     （`NoCommand` vs `NoConsent`），所以兩個都要有單獨為 false 的列
///   - `(used, budget)` 放一組不對稱的 `(5, 80)`：對調成 `(80, 5)` 就會
///     從「沒用完」翻成「用完了」
#[test]
fn every_one_of_the_six_arguments_is_wired_to_the_right_slot() {
    let latests = [
        None,
        Some((true, true)),
        Some((false, true)),
        Some((false, false)),
    ];
    let budgets = [(0u32, 80u32), (80, 80), (5, 80)];
    let attempts = [
        None,
        Some(RetainedInterpreterAttempts {
            count: 17,
            latest_outcome: sister_core::brain::StoredOutboundOutcome::Known(
                sister_core::brain::OutboundOutcome::Timeout,
            ),
        }),
    ];

    for latest in latests {
        for has_command in [true, false] {
            for consented in [true, false] {
                for (used, budget) in budgets {
                    for prev in &attempts {
                        let want = CurrentGuess::while_recording(
                            latest,
                            has_command,
                            consented,
                            used,
                            budget,
                            prev.clone(),
                        );
                        let got: Result<CurrentGuess, String> =
                            CurrentGuess::decide(Presence::Live(Phase::Recording), false, || {
                                Ok(facts(
                                    latest.map(|(has_card, worth_interpreting)| {
                                        LatestClosedSegment {
                                            has_card,
                                            worth_interpreting,
                                        }
                                    }),
                                    has_command,
                                    consented,
                                    used,
                                    budget,
                                    prev.clone(),
                                ))
                            });
                        assert_eq!(
                            got,
                            Ok(want),
                            "接線錯了：latest={latest:?} cmd={has_command} \
                             consent={consented} used={used} budget={budget} prev={prev:?}"
                        );
                    }
                }
            }
        }
    }
}

// ------------------------------------------------ 搬家不准改使用者的字 ----

/// `CurrentGuess` 有幾格。加一格的時候下面那個窮舉 `match` 會先編不過，
/// 補完那一臂之後這個數字和 `every_variant()` 也要一起補。
const VARIANTS: usize = 15;

/// **窮舉**：`CurrentGuess` 多一格，這支函式就編不過（沒有 `_ =>`）。
///
/// 字面照抄一份，不是回去叫 `message()`——拿受測物自己當期望值的話，字改了
/// 兩邊一起改，永遠不會有人紅。`AskedWithoutCard` 那一格照抄整個 `format!`
/// 樣板，所以樣板動了、或兩個佔位符對調了，這裡都會紅。
fn expected_sentence(guess: &CurrentGuess) -> String {
    match guess {
        CurrentGuess::NeverStarted => "還沒有開始錄製，所以沒有這一刻的猜測。".to_string(),
        CurrentGuess::Unreadable => "讀不到錄製狀態，現在不能說她正在看。".to_string(),
        CurrentGuess::Booting => "錄製正在啟動，還沒有進入正在看的狀態。".to_string(),
        CurrentGuess::Thinking => "錄製已停，解釋層正在把最後一段收尾；這不是正在錄。".to_string(),
        CurrentGuess::Stopped => "錄製已停止，所以沒有這一刻的猜測。".to_string(),
        CurrentGuess::Stalled => "錄製心跳已過期；她現在有沒有在看，說不準。".to_string(),
        CurrentGuess::Paused => "記錄已暫停；她現在沒有在看，所以沒有這一刻的猜測。".to_string(),
        CurrentGuess::NoSegment => "正在錄，但這一刻還沒有任何段落。".to_string(),
        CurrentGuess::HasCard => {
            "她對上一段的猜測（下面那張）。現在這一段還開著，要等它結束她才會看。".to_string()
        }
        CurrentGuess::NotWorthInterpreting => {
            "上一段依目前判準不值得產生假設：沒有換 app、沒有閒置後回來、沒有貼大東西、沒有卡住、也沒有錯誤碼。".to_string()
        }
        CurrentGuess::NoConsent => {
            "最新一段還沒有假設；第二張同意書尚未簽署，解釋層一次都不會呼叫 CLI。".to_string()
        }
        CurrentGuess::NoCommand => {
            "最新一段還沒有假設；[brain] command 尚未設定，解釋層沒有 CLI 可以呼叫。".to_string()
        }
        CurrentGuess::BudgetExhausted { .. } => {
            "最新一段還沒有假設；今天的解釋預算已用完，今天不會再產生新卡。".to_string()
        }
        CurrentGuess::AskedWithoutCard {
            attempts,
            latest_label,
        } => format!(
            "最新一段值得理解，她試著問過 {attempts} 次，最近一次是{latest_label}，現在手上沒有卡片。次數與結局只算還留著的外送紀錄；她不會因為問過幾次就放棄這一段，但下一次會不會輪到它，這張卡看不出來。"
        ),
        CurrentGuess::Queued => "最新一段值得理解，正在等解釋層處理。".to_string(),
    }
}

fn every_variant() -> Vec<CurrentGuess> {
    vec![
        CurrentGuess::NeverStarted,
        CurrentGuess::Unreadable,
        CurrentGuess::Booting,
        CurrentGuess::Thinking,
        CurrentGuess::Stopped,
        CurrentGuess::Stalled,
        CurrentGuess::Paused,
        CurrentGuess::NoSegment,
        CurrentGuess::HasCard,
        CurrentGuess::NotWorthInterpreting,
        CurrentGuess::NoConsent,
        CurrentGuess::NoCommand,
        CurrentGuess::BudgetExhausted {
            used: 80,
            limit: 80,
        },
        CurrentGuess::AskedWithoutCard {
            attempts: 17,
            latest_label: "逾時".to_string(),
        },
        CurrentGuess::Queued,
    ]
}

/// 這一輪是純搬家。**使用者讀到的字一個都不准動——15 句全部。**
///
/// 上一版這裡是一張手寫的 9 格陣列，而 `CurrentGuess` 有 15 格。名字宣稱
/// 「一個字都沒動」，實際只釘住 9 句——**測試的名字宣稱整組、覆蓋率只有一部分**。
///
/// 我沒有用讀的判斷剩下那幾句有沒有別人守，我一句一句換掉跑
/// （`/home/ted-h/tmp-tests/message-coverage-r25.py`，15 刀）。
///
/// 這裡原本寫「四句零覆蓋」，**是錯的，而且錯得太樂觀**。把 `brain.rs`（產品
/// 自己）和這一檔排除掉之後，整個 repo 對這 15 句的字面覆蓋是：
///
///   - 4 句在別處也找得到：`Queued` 與 `AskedWithoutCard`（`current_guess.rs`
///     逐字釘死，見下面那一段）、`HasCard` 與 `NoSegment`（`timeline.js` 的
///     展示抄本抄了同樣的字，`NoSegment` 另外被
///     `check-current-guess-wiring.py` 的錯誤訊息引到——那兩處都不是測試）
///   - **11 句在這一檔之外找不到第二處字面**：`NeverStarted` / `Unreadable` /
///     `Booting` / `Thinking` / `Stopped` / `Stalled` / `Paused` 這七句，加上
///     `NotWorthInterpreting` / `NoConsent` / `NoCommand` / `BudgetExhausted`
///     那四句
///
/// 「四」是怎麼來的：那支腳本的 `who` 欄印的是 `sorted(set(...))[:3]`——
/// **顯示截斷**，不是量測結果；真正的條數印在它左邊那一欄，我讀錯欄了。同一個
/// 錯法讓「`AskedWithoutCard` 有三條測試守」也是假的，實測那一刀紅了 9 條。
///
/// 結論不變、而且更強：那 11 句裡有 4 句正好是「她為什麼還沒給你卡片」的四個
/// 理由——指錯關卡的話，使用者會去簽一張已經簽過的同意書。所以這裡改成窮舉
/// `match`：手寫清單會漂，窮舉不會。
#[test]
fn moving_the_order_did_not_touch_a_single_word_on_screen() {
    let all = every_variant();
    assert_eq!(
        all.len(),
        VARIANTS,
        "every_variant() 少了幾格——窮舉 match 只逼你寫出那一句話，逼不出你去跑它"
    );

    let mut seen: Vec<String> = Vec::new();
    for guess in &all {
        let got = guess.message();
        assert_eq!(got, expected_sentence(guess), "{guess:?} 的字被搬家改掉了");
        assert!(!got.trim().is_empty(), "{guess:?} 的句子是空的");
        // 兩格吐出一模一樣的字，等於分辨它們的那個條件零覆蓋。
        assert!(
            !seen.contains(&got),
            "{guess:?} 和前面某一格講同一句話：{got}"
        );
        seen.push(got);
    }
}
