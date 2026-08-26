use sister_core::brain::CurrentGuess;
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
        CurrentGuess::Queued,
    ];
    let messages: Vec<_> = cases.iter().map(CurrentGuess::message).collect();
    for (i, message) in messages.iter().enumerate() {
        assert!(!message.is_empty());
        assert!(!messages[..i].contains(message));
    }
    assert_eq!(
        CurrentGuess::while_recording(Some((true, false)), false, false, 80, 80),
        CurrentGuess::HasCard
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), false, true, 0, 80),
        CurrentGuess::NoCommand
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), true, false, 0, 80),
        CurrentGuess::NoConsent
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), true, true, 80, 80),
        CurrentGuess::BudgetExhausted {
            used: 80,
            limit: 80
        }
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, false)), true, true, 0, 80),
        CurrentGuess::NotWorthInterpreting
    );
}

#[test]
fn every_branch_of_while_recording_has_a_case() {
    // 上面那支漏掉三條路：沒有段落、值得理解（`Queued`）、以及 command 和同意書
    // 同時缺的那一格。漏掉的分支改成別的 variant 也不會紅。
    assert_eq!(
        CurrentGuess::while_recording(None, true, true, 0, 80),
        CurrentGuess::NoSegment,
        "還沒有任何段落"
    );
    assert_eq!(
        CurrentGuess::while_recording(Some((false, true)), true, true, 0, 80),
        CurrentGuess::Queued,
        "值得理解、還沒輪到"
    );
    // 兩個都缺的時候講哪一句，不是品味問題：這張卡在解釋**解釋層為什麼沒有卡**，
    // 所以它報的理由要跟解釋層自己會報的那個一樣。`brain::prepare` 的順序是
    // command → consent → budget（`brain.rs` 的 `let skip = if configured
    // .is_none()`）。順序倒過來，畫面就會指著一個解釋層根本還沒走到的關卡。
    assert_eq!(
        CurrentGuess::while_recording(Some((false, true)), false, false, 0, 80),
        CurrentGuess::NoCommand,
        "兩個都缺的時候要跟 brain::prepare 報同一個理由"
    );
    // 有卡就是有卡，其他關卡都不該蓋掉它——卡已經在手上了。
    assert_eq!(
        CurrentGuess::while_recording(Some((true, false)), false, false, 99, 1),
        CurrentGuess::HasCard
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
