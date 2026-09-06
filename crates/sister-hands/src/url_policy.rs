//! 「我一個人在跑的時候，可不可以自己按網址？」——他的答案，以及它擋下來的那一格。
//!
//! 這一格擋的是 PHASES #42：**被埋在一個已經授權的 app 畫面上的網址，照樣會被
//! 執行**。授權書的四維檢查對它無能為力——那個 app 本來就在授權內，那一步也真的
//! 是「開一個網址」。唯一還能問的問題是：**這個網址，她說得出它從哪來嗎？**
//!
//! ## 兩件事刻意不做
//!
//! **一、不偵測「他在不在電腦前」。** 產品裡沒有這個訊號：`Presence` 那六種講的是
//! **她**在不在錄，而最接近的 `input_metrics` 有兩種零（沒有列＝不知道，有列而
//! 加總是 0＝真的沒動）。把承諾建在一個會回「我不知道」的訊號上，等於發一張她
//! 守不住的票。所以「你在」的定義就是**你按下去**——[`ApprovedBy::Press`]。
//!
//! **二、不判斷這個網址安不安全。** 那是列不完的那一邊，而
//! `target_policy::OPENABLE` 那份白名單的註解已經把這個決定講完了：
//! 「列不完的那一邊，不能是會執行的那一邊。」這裡問的是**來源**，不是善惡。
//!
//! ## 「還沒問過」不是一個答案
//!
//! [`UrlOpenAnswer`] 只有兩個變體，兩個都是他真的講過的話。「還沒問過」是
//! `Option::None`——它在型別上就**不可能**被寫成一個答案，所以沒有人能不小心
//! 替他選一邊。[`UrlOpenPolicy::from_answer`] 把 `None` 翻成
//! [`UrlOpenPolicy::NotAskedYet`]，而它拒絕時講的那句話跟「你說了不要」是
//! 兩句不同的話——這正是這個 repo 一路在修的那顆「兩種 0」。

use serde::{Deserialize, Serialize};

use crate::ApprovedBy;
use crate::{ActionSnapshot, UrlOriginGap};

/// 他真的講過的話。**只有兩個變體，因為他只可能答這兩個。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UrlOpenAnswer {
    /// 「等我在。」網址一律要他當場按，票帶不動。
    OnlyOnMyPress,
    /// 「可以，但你要說得出它從哪來。」
    WhenYouCanNameTheOrigin,
}

impl UrlOpenAnswer {
    pub const ALL: [Self; 2] = [Self::OnlyOnMyPress, Self::WhenYouCanNameTheOrigin];

    /// 他選的時候看到的那一句。**問句和答句只有這裡寫一份**，否則
    /// `sister url-policy` 問的和 `sister doctor` 回報的會在某一版分家。
    pub const fn line(self) -> &'static str {
        match self {
            Self::OnlyOnMyPress => "等我在——網址一律要我當場按，你一個人跑的時候先擱著。",
            Self::WhenYouCanNameTheOrigin => {
                "可以自己按，但條件是你說得出這個網址從哪來（那個站要在你自己的紀錄裡出現過）。"
            }
        }
    }

    /// 設定檔裡寫的那個字。`serde` 那一份是同一份，這裡只是給人看的入口。
    pub const fn key(self) -> &'static str {
        match self {
            Self::OnlyOnMyPress => "only-on-my-press",
            Self::WhenYouCanNameTheOrigin => "when-you-can-name-the-origin",
        }
    }
}

/// 這一次要用哪一條規則。三種，其中一種**不是他選的**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlOpenPolicy {
    /// 她還沒問過他。不開，但理由不是「他說了不要」。
    NotAskedYet,
    Answered(UrlOpenAnswer),
}

impl UrlOpenPolicy {
    /// 設定檔裡沒有那一欄 ⟹ 還沒問過。**這是唯一一條進到 `NotAskedYet` 的路。**
    pub const fn from_answer(answer: Option<UrlOpenAnswer>) -> Self {
        match answer {
            Some(a) => Self::Answered(a),
            None => Self::NotAskedYet,
        }
    }
}

/// 她對「這個網址從哪來」問得出來的答案。
///
/// 四種，而**後面三種都是「說不出來」**——分開是因為它們要他做的事完全不同：
/// 換一個網址、去修擷取、還是根本沒得修。壓成一個 `bool` 的那一版會讓
/// 「這一個網址可疑」和「我這條路整個沒在跑」長得一模一樣。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlOrigin {
    /// 這個站在她自己的紀錄裡出現過。
    InHerRecord,
    /// 查了，沒有。
    NotInHerRecord,
    /// 她根本沒在讀網址，所以對**每一個**網址都答不出來。
    SheIsNotReadingUrls,
    /// 那一串字讀不出一個站名。
    NotAReadableSite,
}

/// 這一步的網址過不過得了他選的那道規則——過得了回 `None`。
///
/// `origin` 是**惰性**的：政策還沒問過、或者他選了「一律當場按」的時候，
/// 這裡不會去碰資料庫。查一次要掃 `focus_events`，而那兩種情況下查了也是白查。
///
/// [`ApprovedBy::Press`] 一律放行——**兩種答案都放行**，因為兩種答案講的都是
/// 「我一個人在跑的時候」。這是它和 `never_inherited_refusal` 最大的差別：
/// 那一支兩種來源都擋（SPEC §9.2），這一支只擋票。
pub fn url_origin_gap(
    action: &ActionSnapshot,
    approved_by: ApprovedBy,
    policy: UrlOpenPolicy,
    origin: impl FnOnce(&str) -> UrlOrigin,
) -> Option<UrlOriginGap> {
    // 不是開網址的那一步，這道閘門沒有意見。判斷放在裡面不放在呼叫端，
    // 是因為呼叫端只有一個而它會忘記。
    let ActionSnapshot::OpenUrl { url } = action else {
        return None;
    };
    if matches!(approved_by, ApprovedBy::Press) {
        return None;
    }
    match policy {
        UrlOpenPolicy::NotAskedYet => Some(UrlOriginGap::NotAskedYet),
        UrlOpenPolicy::Answered(UrlOpenAnswer::OnlyOnMyPress) => {
            Some(UrlOriginGap::YouSaidPressItYourself)
        }
        UrlOpenPolicy::Answered(UrlOpenAnswer::WhenYouCanNameTheOrigin) => match origin(url) {
            UrlOrigin::InHerRecord => None,
            UrlOrigin::NotInHerRecord => Some(UrlOriginGap::NotInHerRecord),
            UrlOrigin::SheIsNotReadingUrls => Some(UrlOriginGap::SheIsNotReadingUrls),
            UrlOrigin::NotAReadableSite => Some(UrlOriginGap::NotAReadableSite),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn open(url: &str) -> ActionSnapshot {
        ActionSnapshot::OpenUrl { url: url.into() }
    }
    fn never(_: &str) -> UrlOrigin {
        panic!("這條路不該去查來源");
    }

    /// 他當場按了，兩種答案都放行。**這是這道閘門和 `NeverInherited` 的分界**：
    /// 那一類按了照樣擋，這一類按了就是他要的。
    #[test]
    fn a_live_press_opens_it_under_either_answer() {
        for answer in UrlOpenAnswer::ALL {
            assert_eq!(
                url_origin_gap(
                    &open("https://example.com/"),
                    ApprovedBy::Press,
                    UrlOpenPolicy::Answered(answer),
                    never,
                ),
                None,
                "{} 這個答案下，當場按竟然被擋了",
                answer.key()
            );
        }
        assert_eq!(
            url_origin_gap(
                &open("https://example.com/"),
                ApprovedBy::Press,
                UrlOpenPolicy::NotAskedYet,
                never,
            ),
            None
        );
    }

    /// 「還沒問過」和「你說了要自己按」是兩句不同的話。合併的那一天，
    /// 一個從來沒被問過的人會讀到「這是你選的」。
    #[test]
    fn not_asked_yet_is_not_the_same_refusal_as_he_said_press_it() {
        let not_asked = url_origin_gap(
            &open("https://example.com/"),
            ApprovedBy::StandingGrant,
            UrlOpenPolicy::NotAskedYet,
            never,
        );
        let he_said = url_origin_gap(
            &open("https://example.com/"),
            ApprovedBy::StandingGrant,
            UrlOpenPolicy::Answered(UrlOpenAnswer::OnlyOnMyPress),
            never,
        );
        assert_eq!(not_asked, Some(UrlOriginGap::NotAskedYet));
        assert_eq!(he_said, Some(UrlOriginGap::YouSaidPressItYourself));
        assert_ne!(not_asked, he_said);
        let a = not_asked
            .unwrap()
            .unattended_message(None, "sister url-policy");
        let b = he_said
            .unwrap()
            .unattended_message(None, "sister url-policy");
        assert_ne!(a, b, "兩種不開的理由印出同一句話");
        assert!(
            a.contains("還沒問"),
            "沒問過那一句要說得出「我還沒問」：{a}"
        );
        assert!(!b.contains("還沒問"), "他答過了，不可以說我還沒問：{b}");
    }

    /// 政策決定得了的時候不可以去查資料庫——`never` 會 panic。
    #[test]
    fn it_does_not_look_up_the_origin_when_the_answer_already_settles_it() {
        // 兩條會 panic 的路，上面兩條測試已經跑過了；這裡釘的是「有被呼叫」
        // 的那一條真的會被呼叫，否則上面的 `never` 只是永遠沒被碰到。
        let asked = Cell::new(0);
        let gap = url_origin_gap(
            &open("https://example.com/"),
            ApprovedBy::StandingGrant,
            UrlOpenPolicy::Answered(UrlOpenAnswer::WhenYouCanNameTheOrigin),
            |url| {
                asked.set(asked.get() + 1);
                assert_eq!(url, "https://example.com/", "查的不是這一步的網址");
                UrlOrigin::InHerRecord
            },
        );
        assert_eq!(gap, None);
        assert_eq!(asked.get(), 1, "說得出來源那條路沒有真的去查");
    }

    /// 三種「說不出來」各自是一句不同的話。
    #[test]
    fn the_three_ways_of_not_knowing_name_different_causes() {
        let cases = [
            (UrlOrigin::NotInHerRecord, UrlOriginGap::NotInHerRecord),
            (
                UrlOrigin::SheIsNotReadingUrls,
                UrlOriginGap::SheIsNotReadingUrls,
            ),
            (UrlOrigin::NotAReadableSite, UrlOriginGap::NotAReadableSite),
        ];
        let mut said = Vec::new();
        for (origin, expected) in cases {
            let gap = url_origin_gap(
                &open("https://example.com/"),
                ApprovedBy::StandingGrant,
                UrlOpenPolicy::Answered(UrlOpenAnswer::WhenYouCanNameTheOrigin),
                |_| origin,
            );
            assert_eq!(gap, Some(expected));
            said.push(gap.unwrap().unattended_message(Some("example.com"), "x"));
        }
        for i in 0..said.len() {
            for j in (i + 1)..said.len() {
                assert_ne!(said[i], said[j], "兩種說不出來印出同一句話");
            }
        }
        // 「這一整條路沒在跑」不可以讀起來像「這一個網址可疑」。
        assert!(
            said[1].contains("每一個"),
            "沒在讀網址那一句要說清楚它對每一個網址都成立：{}",
            said[1]
        );
    }

    /// 不是開網址的那些步，這道閘門一個字都不該說。
    #[test]
    fn it_has_no_opinion_about_steps_that_are_not_opening_a_url() {
        for action in [
            ActionSnapshot::OpenFile {
                path: "/tmp/a.txt".into(),
            },
            ActionSnapshot::FocusWindow {
                title: "記事本".into(),
            },
        ] {
            assert_eq!(
                url_origin_gap(
                    &action,
                    ApprovedBy::StandingGrant,
                    UrlOpenPolicy::NotAskedYet,
                    never,
                ),
                None,
                "{action:?} 被這道網址閘門擋下來了"
            );
        }
    }

    /// 設定檔沒有那一欄 ⟹ 還沒問過。這是「兩種 0」在型別上的分界。
    #[test]
    fn an_absent_setting_means_not_asked_not_a_default_answer() {
        assert_eq!(UrlOpenPolicy::from_answer(None), UrlOpenPolicy::NotAskedYet);
        for answer in UrlOpenAnswer::ALL {
            assert_eq!(
                UrlOpenPolicy::from_answer(Some(answer)),
                UrlOpenPolicy::Answered(answer)
            );
        }
    }
}
