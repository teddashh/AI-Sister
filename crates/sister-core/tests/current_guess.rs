use sister_core::brain::{CurrentGuess, OutboundOutcome, StoredOutboundOutcome};
use sister_core::db::RetainedInterpreterAttempts;
use sister_core::heartbeat::{Phase, Presence};

#[test]
fn every_non_recording_presence_has_its_own_sentence() {
    let cases = [
        Presence::NeverStarted,
        Presence::Unreadable,
        Presence::Live(Phase::Booting),
        Presence::Thinking { at: 1, until: 2 },
        Presence::Stopped { at: Some(1) },
        Presence::Stalled {
            at: 1,
            phase: Phase::Recording,
        },
    ];
    let messages: Vec<_> = cases
        .into_iter()
        .map(|presence| CurrentGuess::from_presence(presence).unwrap().message())
        .collect();
    for (i, message) in messages.iter().enumerate() {
        assert!(!message.is_empty());
        assert!(!messages[..i].contains(message));
    }
}

#[test]
fn every_live_recording_outcome_says_what_was_checked() {
    let cases = [
        CurrentGuess::NoSegment,
        CurrentGuess::Paused,
        CurrentGuess::HasCard,
        CurrentGuess::NotWorthInterpreting,
        CurrentGuess::NoConsent,
        CurrentGuess::NoCommand,
        CurrentGuess::BudgetExhausted {
            used: 80,
            limit: 80,
        },
        CurrentGuess::AskedWithoutCard {
            attempts: 3,
            latest_label: OutboundOutcome::Timeout.zh_label().to_string(),
        },
        CurrentGuess::Queued,
    ];
    let messages: Vec<_> = cases.iter().map(CurrentGuess::message).collect();
    for (i, message) in messages.iter().enumerate() {
        assert!(!message.is_empty());
        assert!(!messages[..i].contains(message));
    }
    assert_eq!(
        CurrentGuess::while_recording(Some((true, false)), false, false, 80, 80, None),
        CurrentGuess::HasCard
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), false, true, 0, 80, None),
        CurrentGuess::NoCommand
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), true, false, 0, 80, None),
        CurrentGuess::NoConsent
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), true, true, 80, 80, None),
        CurrentGuess::BudgetExhausted {
            used: 80,
            limit: 80
        }
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), true, true, 0, 80, None),
        CurrentGuess::NotWorthInterpreting
    );
}

#[test]
fn every_branch_of_while_recording_has_a_case() {
    // 上面那支漏掉三條路：沒有段落、值得理解（`Queued`）、以及 command 和同意書
    // 同時缺的那一格。漏掉的分支改成別的 variant 也不會紅。
    assert_eq!(
        CurrentGuess::while_recording(None, true, true, 0, 80, None),
        CurrentGuess::NoSegment,
        "還沒有任何段落"
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, true)), true, true, 0, 80, None),
        CurrentGuess::Queued,
        "值得理解、還沒輪到"
    );
    // 兩個都缺的時候講哪一句，不是品味問題：這張卡在解釋**解釋層為什麼沒有卡**，
    // 所以它報的理由要跟解釋層自己會報的那個一樣。`brain::prepare` 的順序是
    // command → consent → budget（`brain.rs` 的 `let skip = if configured
    // .is_none()`）。順序倒過來，畫面就會指著一個解釋層根本還沒走到的關卡。
    assert_eq!(
        CurrentGuess::while_recording(Some((false, true)), false, false, 0, 80, None),
        CurrentGuess::NoCommand,
        "兩個都缺的時候要跟 brain::prepare 報同一個理由"
    );
    // 有卡就是有卡，其他關卡都不該蓋掉它——卡已經在手上了。
    assert_eq!(
        CurrentGuess::while_recording(Some((true, false)), false, false, 99, 1, None),
        CurrentGuess::HasCard
    );
    assert!(matches!(
        CurrentGuess::while_recording(
            Some((false, true)),
            true,
            true,
            0,
            80,
            Some(RetainedInterpreterAttempts {
                count: 1,
                latest_outcome: StoredOutboundOutcome::Known(OutboundOutcome::Timeout),
            }),
        ),
        CurrentGuess::AskedWithoutCard { .. }
    ));
}

#[test]
fn an_asked_segment_tells_the_reader_both_retained_facts() {
    let message = CurrentGuess::while_recording(
        Some((false, true)),
        true,
        true,
        0,
        80,
        Some(RetainedInterpreterAttempts {
            count: 12,
            latest_outcome: StoredOutboundOutcome::Unknown("legacy".to_string()),
        }),
    )
    .message();

    assert!(
        message.contains("問過 12 次"),
        "保留的次數要說給人聽：{message}"
    );
    assert!(
        message.contains("不認得的結局（legacy）"),
        "保留的結局要用中文標籤說給人聽：{message}"
    );
}

#[test]
fn the_card_never_claims_to_be_about_this_moment() {
    // 解釋層只看**關掉的**段落（`wakeup.rs` 的 `include_open == false` 把右界
    // 設在最後一段的起點），所以唯一會端出卡片的 `HasCard`，講的一定是上一段。
    // 而一段可以在同一個 app 裡開著幾十分鐘。
    //
    // 這一條不是「三句話不一樣」——兩兩不同的斷言擋不住把這句改成
    // 「這是她這一刻的猜測。」（實測：那個變異在只有互斥斷言的時候全綠）。
    let has_card = CurrentGuess::HasCard.message();
    assert!(
        has_card.contains("上一段"),
        "端出卡片的時候要說清楚那是上一段：{has_card}"
    );
    assert!(
        !has_card.contains("這一刻"),
        "一張上一段的猜測不可以說自己是這一刻的：{has_card}"
    );

    // 反面：真的在講此刻的那幾句，該講得出「這一刻」。少了這一條，把
    // `NoSegment` 改成也講「上一段」照樣綠。
    let no_segment = CurrentGuess::NoSegment.message();
    assert!(
        no_segment.contains("這一刻"),
        "這一句講的就是此刻：{no_segment}"
    );
    assert!(
        !no_segment.contains("上一段"),
        "還沒有任何段落，哪來的上一段：{no_segment}"
    );

    // `NotWorthInterpreting` 不可以宣布一件沒發生過的事——解釋層可能根本還沒
    // 走到這一段，那句判斷是這一側拿同一個判準自己算的。
    let not_worth = CurrentGuess::NotWorthInterpreting.message();
    assert!(
        !not_worth.contains("檢查過") && !not_worth.contains("看過"),
        "這是這一側自己算的判斷，不是她做過的動作：{not_worth}"
    );
}

#[test]
fn recording_is_the_only_presence_that_needs_segment_facts() {
    assert_eq!(
        CurrentGuess::from_presence(Presence::Live(Phase::Recording)),
        None
    );
    assert!(matches!(
        CurrentGuess::from_presence(Presence::Thinking { at: 1, until: 2 }),
        Some(CurrentGuess::Thinking)
    ));
}
// ============================================================================
// 由整合者在派工**之前**寫好。收貨時原封不動貼進
// `crates/sister-core/tests/current_guess.rs` 的檔尾，一個字都不改就要綠。
//
// 需要的 import（併進該檔頂端既有的 use，不要重複宣告）：
//   use sister_core::brain::{CurrentGuess, OutboundOutcome, StoredOutboundOutcome};
//   use sister_core::db::RetainedInterpreterAttempts;
//
// 這幾條只斷言**使用者讀得到的字**和**分支順序**，不綁任何一種實作寫法。
//
// 共五條。第五條（`the_old_queued_sentence_is_still_exactly_what_queued_says`）
// 是派工發出去之後補的：我自己檢查針的時候發現，第一條的反向針釘在一句
// **沒有任何測試在守內容**的產品字串上（既有那條只檢查非空＋互不相同），
// 字一改反向針就永遠 match 不到 ＝ 假綠。它加的是限制，不是放寬。
// ============================================================================

/// 同一段問過 17 次還說「正在等解釋層處理」，那句話對讀的人是假的。
#[test]
fn a_segment_already_asked_says_so_instead_of_only_queued() {
    let fresh = CurrentGuess::while_recording(Some((false, true)), true, true, 0, 80, None);
    assert_eq!(
        fresh,
        CurrentGuess::Queued,
        "沒問過的那一段要照舊，不准被新分支吃掉：{fresh:?}"
    );

    let asked = CurrentGuess::while_recording(
        Some((false, true)),
        true,
        true,
        0,
        80,
        Some(RetainedInterpreterAttempts {
            count: 17,
            latest_outcome: StoredOutboundOutcome::Known(OutboundOutcome::Timeout),
        }),
    );
    let msg = asked.message();

    // 針取整個片語，不取「17」——預算那句也有數字，短針會假綠。
    assert!(msg.contains("問過 17 次"), "沒說出這一段被問過幾次：{msg}");
    assert!(
        msg.contains(OutboundOutcome::Timeout.zh_label()),
        "沒說上一次的結局是什麼：{msg}"
    );
    // 反向針：舊那句不准同時端出來。它是無條件的整句，不是一個詞。
    assert!(
        !msg.contains("最新一段值得理解，正在等解釋層處理。"),
        "同一句話裡同時說「正在等」和「問過 17 次」：{msg}"
    );
}

/// 上面那條的反向針釘在「最新一段值得理解，正在等解釋層處理。」上。
/// 反向針只有在那串字**確實存在**的時候才有牙齒——字一改，它就永遠 match 不到，
/// 變成一條假綠。這一條就是那根釘子。
///
/// （既有的 `every_live_recording_outcome_says_what_was_checked` 只檢查
/// 「非空」和「互不相同」，沒有釘住任何一句話的內容。）
#[test]
fn the_old_queued_sentence_is_still_exactly_what_queued_says() {
    assert_eq!(
        CurrentGuess::Queued.message(),
        "最新一段值得理解，正在等解釋層處理。",
        "舊那句話被動過了：上面那條的反向針會永遠 match 不到，整條變成假綠"
    );
}

/// 那個數字要跟著資料庫走。只驗一個值的話，寫死也會綠。
#[test]
fn the_number_in_that_sentence_tracks_the_stored_attempt_count() {
    let mut seen = Vec::new();
    for n in [2u32, 17, 143] {
        let msg = CurrentGuess::while_recording(
            Some((false, true)),
            true,
            true,
            0,
            80,
            Some(RetainedInterpreterAttempts {
                count: n,
                latest_outcome: StoredOutboundOutcome::Known(OutboundOutcome::NoAnswer),
            }),
        )
        .message();
        assert!(
            msg.contains(&format!("問過 {n} 次")),
            "問過 {n} 次，句子裡卻不是這個數字：{msg}"
        );
        seen.push(msg);
    }
    assert_eq!(
        seen.iter().collect::<std::collections::BTreeSet<_>>().len(),
        seen.len(),
        "三個不同的次數端出一模一樣的句子＝那個數字沒有真的被用上：{seen:?}"
    );
}

/// 上一次的結局也要跟著走，而且不認得的 token 仍然包成中文再端出去。
#[test]
fn the_latest_outcome_in_that_sentence_tracks_what_is_stored() {
    let with = |outcome: StoredOutboundOutcome| {
        CurrentGuess::while_recording(
            Some((false, true)),
            true,
            true,
            0,
            80,
            Some(RetainedInterpreterAttempts {
                count: 3,
                latest_outcome: outcome,
            }),
        )
        .message()
    };

    let timeout = with(StoredOutboundOutcome::Known(OutboundOutcome::Timeout));
    let bad_json = with(StoredOutboundOutcome::Known(OutboundOutcome::BadJson));
    assert!(
        timeout.contains(OutboundOutcome::Timeout.zh_label()),
        "逾時沒講出來：{timeout}"
    );
    assert!(
        bad_json.contains(OutboundOutcome::BadJson.zh_label()),
        "JSON 不能用沒講出來：{bad_json}"
    );
    assert_ne!(
        timeout, bad_json,
        "兩種結局端出同一句話＝結局根本沒被讀進去"
    );

    // 不認得的 token 要走 `StoredOutboundOutcome::zh_label()` 那條路，
    // 不准把裸 token 直接接在中文句子上。
    let unknown = with(StoredOutboundOutcome::Unknown("wat".to_string()));
    assert!(
        unknown.contains("不認得的結局"),
        "不認得的 token 沒有被包成中文：{unknown}"
    );
}

/// 「問過了」不准蓋過「她根本問不了」那幾個原因，也不准蓋過「已經有卡片」。
/// 這一條同時是分支順序的規格：新分支只能站在原本 `Queued` 那個位置。
#[test]
fn already_asked_never_overrides_the_states_that_come_before_it() {
    let prev = || {
        Some(RetainedInterpreterAttempts {
            count: 9,
            latest_outcome: StoredOutboundOutcome::Known(OutboundOutcome::Timeout),
        })
    };

    let cases: Vec<(&str, CurrentGuess, CurrentGuess)> = vec![
        (
            "已經有卡片了，就講那張卡片",
            CurrentGuess::while_recording(Some((true, true)), true, true, 0, 80, prev()),
            CurrentGuess::HasCard,
        ),
        (
            "沒設定 CLI，她根本問不出去",
            CurrentGuess::while_recording(Some((false, true)), false, true, 0, 80, prev()),
            CurrentGuess::NoCommand,
        ),
        (
            "同意書沒簽，她一次都不會呼叫",
            CurrentGuess::while_recording(Some((false, true)), true, false, 0, 80, prev()),
            CurrentGuess::NoConsent,
        ),
        (
            "今天的額度用完了",
            CurrentGuess::while_recording(Some((false, true)), true, true, 80, 80, prev()),
            CurrentGuess::BudgetExhausted {
                used: 80,
                limit: 80,
            },
        ),
        (
            "這一段依判準不值得理解",
            CurrentGuess::while_recording(Some((false, false)), true, true, 0, 80, prev()),
            CurrentGuess::NotWorthInterpreting,
        ),
        (
            "根本還沒有段落",
            CurrentGuess::while_recording(None, true, true, 0, 80, prev()),
            CurrentGuess::NoSegment,
        ),
    ];

    for (why, got, want) in cases {
        assert_eq!(got, want, "{why}：{got:?}");
    }
}
// ============================================================================
// r24 的驗收測試。第六版考題。
//
// 前五版錯法各不相同，寫在這裡免得寫第七版時重犯：
//
//   v1（r19）：把針釘在「還沒寫成卡片」上——而那六個字正是要改的東西。
//             照它驗收，**正確的修法會紅**。
//   v2（r20）：針太短（`contains("還留著")` 三個字、方向不管）／反針釘在產品
//             已經不說的字串上（恆真）／把要禁的講法寫進正針裡。
//   v3（r21）：**針與針之間是自由空間**。全部是 `contains` 正針 ＋ 逐字 `!contains`
//             反針，於是「插字」而不是「改字」的攻擊全部繞得過去。
//   v4（r22）：改成整句 `assert_eq!`，但**只釘了三個 (次數, 結局) 組合**。
//             洞沒有補起來，只是從 `attempts` 搬到 `latest_outcome`：在
//             `message()` 裡插一條 `if latest_label.starts_with("CLI 叫不起來")`
//             的特例臂、句子照原樣、句尾接假話——20 個 binary 全綠。
//   v5（r23）：**把 `latest_outcome` 那一軸補滿了，洞就搬回 `attempts`。**
//             句子是兩條軸的乘積，而整句 `assert_eq!` 只跑在
//             `attempts ∈ {1,3,4,5,7}` 五個點上。把特例臂改掛在
//             `if *attempts >= 10`、句尾接一句避開禁詞表的假話——又是全綠。
//             同一刀改成 `>= 8`、`== 17` 也全綠，實測三次。
//             **而 v5 的 doc 寫的是「任何一種結局的句子被插字、改字、換說法，
//             都會在這裡紅」——那句話是假的。**
//
// 這一版學到的東西，寫在前面：
//
//   1. **句子有幾個變數，就是幾條軸。** 把一條軸補滿只會把洞推到另一條。
//      這一檔的句子有兩個 `{}`，所以底下是**二維**的迴圈。
//   2. **取樣證不了「對每一個 n 都成立」。** `attempts` 是 `u32`，43 億個點。
//      取樣能咬死的是**門檻型**特例臂（見 `SAMPLED_ATTEMPTS` 的 doc），
//      咬不死等值型（`if attempts == 1000`）。**這件事寫在下面，
//      而不是寫一句「都會紅」然後被下一輪打臉。**
//   3. 句子的字面只寫在 `expected()` 一處；標籤走 `label_of` 的窮舉 match。
//      為什麼期望值要手寫而不是用產品同一條規則算：對「有沒有指到某個值」，
//      用同一條規則算是對的；對「格式長什麼樣」，那是毀滅性的——測試會跟著
//      產品一起錯。這一檔測的正是格式，所以字面照抄。
// ============================================================================

/// 這一臂該說的話，逐字。**整句的字面只寫在這裡一處。**
///
/// 改產品文案就要改這裡。底下有 6 條測試直接拿它當期望值
/// （`grep -c 'expected(' ` 數得出來），另外還有幾條各自釘住句子裡的**片語**
/// （`the_count_says_tried_not_reached` 的「她問過 N 次」、
/// `the_sentence_never_promises_...` 的兩句 hedge），所以改動 hedge 的話
/// 紅的條數會比 6 多。**不要把這裡的條數當成「總共會紅幾條」。**
fn expected(attempts: u32, latest_label: &str) -> String {
    format!(
        "最新一段值得理解，她試著問過 {attempts} 次，最近一次是{latest_label}，現在手上沒有卡片。次數與結局只算還留著的外送紀錄；她不會因為問過幾次就放棄這一段，但下一次會不會輪到它，這張卡看不出來。"
    )
}

/// 每一種結局的標籤，**手寫字面**。
///
/// `StoredOutboundOutcome::Known` 的格式是「中文（token）」（`brain.rs`），
/// 這裡把兩半都寫死：標籤對調、token 改名、格式改寫，三種都會紅。
///
/// **這道窮舉 match 擋得住什麼、擋不住什麼，講清楚：**
/// `OutboundOutcome` 加第六種變體時，這支函式**編不過**，改的人一定會站在這裡。
/// 但它只逼人把新那一格的**字面寫下來**，不會逼人把它加進 `ALL_OUTCOMES`——
/// 而驅動迴圈的是那張陣列，`[OutboundOutcome; 5]` 加了變體照樣編得過。
/// 所以：**閘門是「強迫來訪」，不是「強迫覆蓋」。** 補完這裡請往下三行，
/// 把新那一格也加進 `ALL_OUTCOMES`，否則它的句子從頭到尾不會被渲染過一次。
///
/// （r23 的 doc 在這個位置寫的是「every 靠的是這個窮舉 match，不是靠那張手寫
/// 陣列」——正好講反了，兩個審查鏡頭各自獨立抓到。Rust 沒有窮舉變體的辦法，
/// 想要真的閘門就得上 derive macro；在那之前，這裡就是手寫的。）
fn label_of(outcome: OutboundOutcome) -> &'static str {
    match outcome {
        // ⚠ 在這裡補完新變體之後，**也要把它加進底下的 `ALL_OUTCOMES`**，
        //   不然迴圈跑不到它，而測試會全綠。
        OutboundOutcome::Success => "成功（success）",
        OutboundOutcome::SpawnFailed => "CLI 叫不起來／失敗（spawn_failed）",
        OutboundOutcome::Timeout => "逾時（timeout）",
        OutboundOutcome::NoAnswer => "CLI 跑完但沒有回答（no_answer）",
        OutboundOutcome::BadJson => "拿回的 JSON 不能用（bad_json）",
    }
}

/// 手寫，而且**沒有任何東西保證它是完整的**（見 `label_of` 的 doc）。
const ALL_OUTCOMES: [OutboundOutcome; 5] = [
    OutboundOutcome::Success,
    OutboundOutcome::SpawnFailed,
    OutboundOutcome::Timeout,
    OutboundOutcome::NoAnswer,
    OutboundOutcome::BadJson,
];

/// `attempts` 這一軸的取樣。**這幾個數字是選過的，不是隨手寫的：**
///
/// - `1` 是真實下界（`retained_interpreter_attempts_for_segment` 沒有列時回
///   `None`，走 `Queued`，所以 `0` 到不了這一臂）。它咬死 `<=` 型門檻。
/// - `u32::MAX` 是**上界探針**：任何 `if attempts >= K` 的特例臂，不管 K 設在
///   哪裡，`u32::MAX` 一定過得了它 ⇒ 這一條就會紅。這是 v5 那三刀
///   （`>= 10`、`>= 8`、`== 17`）之後補上的，前兩刀被這個點咬死。
/// - `17`、`143` 是既有測試用過的數字，順手把那幾格從 `contains` 升級成整句。
/// - 中間幾個是實務上的常見值。
///
/// **咬不死的：等值型門檻**（`if attempts == 1000`）。43 億個點取樣 8 個，
/// 證不了「對每一個 n 都成立」。這一行是這一檔唯一誠實的講法——
/// v4 和 v5 都在對應的位置寫了「都會紅」，兩次都是假的。
const SAMPLED_ATTEMPTS: [u32; 8] = [1, 2, 3, 5, 7, 17, 143, u32::MAX];

/// 不認得的 token 這一軸的取樣。
///
/// `latest_label` 是 `String`（`StoredOutboundOutcome` 沒 derive `Serialize`，
/// 放不進 payload），所以 `Unknown` 那半的值域是 `brain_outbound.outcome` 欄的
/// **任意內容**——原理上補不完，這裡只能取樣。挑的是四種形狀：
/// 一般的、**看起來像已知標籤的**（審查鏡頭就是拿這種形狀做出突變的）、
/// 空字串、很長的。
const SAMPLED_TOKENS: [&str; 5] = [
    "weird_token",
    "refused",
    "成功（success）",
    "",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];

/// **兩條軸的乘積，每一格都是整句 `assert_eq!`。**
///
/// v4 只釘三個組合 ⇒ 洞跑到 `latest_outcome`；v5 補滿 `latest_outcome`
/// ⇒ 洞跑回 `attempts`。這一條同時跑滿兩條軸的取樣（5 × 8 = 40 格），
/// 掛在任一條軸上的門檻型特例臂都會在這裡紅。
///
/// 名字裡沒有「every」：結局那一軸靠的是手寫的 `ALL_OUTCOMES`，
/// 次數那一軸是取樣。兩個都不是「每一個」。
#[test]
fn the_whole_sentence_is_pinned_across_both_axes() {
    for outcome in ALL_OUTCOMES {
        for n in SAMPLED_ATTEMPTS {
            assert_eq!(
                asked_message(n, known(outcome)),
                expected(n, label_of(outcome)),
                "({n}, {outcome:?}) 這一格的句子和字面對不上"
            );
        }
    }
}

/// 不認得的 token 是**原樣貼進去**的，不管它長什麼樣。
///
/// 這一條防的是「對某個沒被取樣過的 token 做手腳」——審查鏡頭示範的突變是
/// 把含 `refused` 的 token 換成隔壁那一臂的值（「成功（success）」），
/// 句子一個字都沒動，而使用者讀到的結局是假的。所以 `refused` 和一個
/// **長得像已知標籤**的 token 都在取樣裡。
#[test]
fn an_unknown_token_is_pasted_in_verbatim_whatever_it_looks_like() {
    for token in SAMPLED_TOKENS {
        for n in SAMPLED_ATTEMPTS {
            assert_eq!(
                asked_message(n, StoredOutboundOutcome::Unknown(token.to_string())),
                expected(n, &format!("不認得的結局（{token}）")),
                "({n}, {token:?}) 這一格的句子和字面對不上"
            );
        }
    }
}

/// 整句釘死：一般情況。（上面那兩條已經蓋到，留著是因為刪掉不會有人紅。）
#[test]
fn the_whole_sentence_is_pinned_for_a_known_outcome() {
    assert_eq!(
        asked_message(3, known(OutboundOutcome::Timeout)),
        expected(3, "逾時（timeout）")
    );
}

/// 整句釘死：`attempts == 1`——真實下界，也是最常走的那一格。
#[test]
fn the_first_attempt_gets_the_same_sentence_not_a_special_case() {
    assert_eq!(
        asked_message(1, known(OutboundOutcome::Timeout)),
        expected(1, "逾時（timeout）")
    );
}

/// 整句釘死：不認得的 token 那一條路（走另一個 `zh_label` 分支）。
#[test]
fn an_unknown_outcome_token_gets_the_same_sentence() {
    assert_eq!(
        asked_message(5, StoredOutboundOutcome::Unknown("weird_token".to_string())),
        expected(5, "不認得的結局（weird_token）")
    );
}

/// 這個數字數的是**試過幾次**，不是**問到幾次**。
///
/// `spawn_cli` 起不來時 `payload_chars_written` 是 0（`brain.rs`），
/// 下游照樣寫一列 `brain_outbound`（`chars_sent: 0`、`outcome: spawn_failed`），
/// 而 `COUNT(*)`（`db.rs`）**不看 outcome、也不看 chars_sent**。
/// 所以 `[brain] command` 指到不存在的執行檔時，那 N 列一個位元組都沒送出去。
///
/// 舊句子寫「她問過 N 次」，配上下一個子句「最近一次是 CLI 叫不起來」，
/// 同一句話自己打自己。
#[test]
fn the_count_says_tried_not_reached() {
    let msg = asked_message(4, known(OutboundOutcome::SpawnFailed));
    assert_eq!(msg, expected(4, "CLI 叫不起來／失敗（spawn_failed）"));
    assert!(
        !msg.contains("她問過 4 次"),
        "「問過」對 spawn_failed 不成立——零位元組送出去：{msg}"
    );
}

/// **不准再承諾下一次。**
///
/// r21 寫「只要還在錄，下一次醒來還會再問這一段」，四個反例：
///   1. `run()` 的 `prepared.truncate(take)` 砍的是**最新那端**
///      （`collect_jobs` 由新往舊收、`jobs.reverse()` 之後最舊在前），
///      而這張卡講的定義上就是最新的已關閉段落
///   2. `wakeup.rs` 的 `budget_exhausted` 早退，旗標只在跨當地日清掉
///   3. 腦執行緒可能根本沒起來（`maybe_spawn` 回 Err 只印進 record.log）
///   4. 重開一場之後，比新起點早 `LOOKAROUND_MS` 以上的段落永遠掉出範圍
///
/// 「醒來」還是 CLI 行話——桌面使用者看得到的字串裡一次都沒出現過。
///
/// **每一格先整句釘死，再掃禁詞。** 只掃禁詞的話，換個說法就繞過去了
/// （審查鏡頭的突變寫的是「一定還會再**試**」「一定會再**看**」，
/// 禁詞表裡的「一定會再**問**」一個字都沒撞到）。
#[test]
fn the_sentence_never_promises_that_the_next_wakeup_will_ask() {
    for (n, outcome, label) in [
        (1u32, known(OutboundOutcome::Timeout), "逾時（timeout）"),
        (17, known(OutboundOutcome::Success), "成功（success）"),
        (
            2,
            StoredOutboundOutcome::Unknown("legacy".to_string()),
            "不認得的結局（legacy）",
        ),
    ] {
        let msg = asked_message(n, outcome);
        assert_eq!(msg, expected(n, label), "整句對不上：{msg}");
        for banned in [
            "下一次醒來還會再問",
            "還會再問這一段",
            "一定會再問",
            "只要還在錄",
            "醒來",
        ] {
            assert!(
                !msg.contains(banned),
                "承諾了證不出來的事（「{banned}」）：{msg}"
            );
        }
        assert!(
            msg.contains("不會因為問過幾次就放棄"),
            "沒講出「次數不會讓她放棄」——那一半是真的，不可以一起丟掉：{msg}"
        );
        assert!(
            msg.contains("下一次會不會輪到它，這張卡看不出來"),
            "沒承認自己看不出來，讀者會把上一句讀成保證：{msg}"
        );
    }
}

/// 結局文字要有**字面**錨點。
///
/// 拿 `zh_label()` 自己當期望值是自我證成：把 `Success` 和 `Timeout` 的標籤
/// 對調，那種斷言不會有任何反應。上面 `label_of` 和這一條都是字面的。
#[test]
fn the_outcome_labels_are_anchored_to_literals() {
    assert_eq!(OutboundOutcome::Timeout.zh_label(), "逾時");
    assert_eq!(OutboundOutcome::Success.zh_label(), "成功");
    assert_eq!(
        StoredOutboundOutcome::Known(OutboundOutcome::Timeout).zh_label(),
        "逾時（timeout）"
    );
}

/// 這一臂不准借用 `Queued` 那一格的講法。
#[test]
fn the_asked_sentence_never_borrows_the_queued_wording() {
    let msg = asked_message(4, known(OutboundOutcome::BadJson));
    assert_eq!(msg, expected(4, "拿回的 JSON 不能用（bad_json）"));
    assert!(
        !msg.contains("正在等解釋層處理"),
        "那句話是給「還沒問過」的：{msg}"
    );
}

/// 分支選擇仍然由提問史決定，沒問過的那一格不准變。
#[test]
fn the_asked_branch_is_chosen_by_the_attempt_history_alone() {
    let base = |prev| CurrentGuess::while_recording(Some((false, true)), true, true, 0, 80, prev);
    assert_eq!(base(None), CurrentGuess::Queued, "沒問過的那一格不准變");
    assert!(
        matches!(
            base(Some(RetainedInterpreterAttempts {
                count: 1,
                latest_outcome: known(OutboundOutcome::Timeout),
            })),
            CurrentGuess::AskedWithoutCard { .. }
        ),
        "問過了卻沒走到 AskedWithoutCard"
    );
}

fn known(o: OutboundOutcome) -> StoredOutboundOutcome {
    StoredOutboundOutcome::Known(o)
}

/// 夾具集中在一處：抄六個參數抄六次就會有一次抄錯，
/// 而抄錯的那次會安靜地測到別的分支。
fn asked_message(count: u32, latest_outcome: StoredOutboundOutcome) -> String {
    let guess = CurrentGuess::while_recording(
        Some((false, true)),
        true,
        true,
        0,
        80,
        Some(RetainedInterpreterAttempts {
            count,
            latest_outcome,
        }),
    );
    assert!(
        matches!(guess, CurrentGuess::AskedWithoutCard { .. }),
        "夾具沒走到要測的那一格，測到的是 {guess:?}——下面每條斷言都在測別的東西"
    );
    guess.message()
}
