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

// enum、畫面列出的封閉集合、設定 key 與答句由**同一列**展開。若分開手寫，新增
// variant 卻漏掉 `ALL` 仍會編譯、CLI/桌面/IPC 還會一起漏掉，測試若也走 `ALL`
// 甚至全綠。這個 macro 讓那種「每一行都對，合起來少一個答案」沒有接縫可鑽。
macro_rules! define_url_open_answers {
    ($(#[$meta:meta])* $first:ident => ($first_key:literal, $first_line:literal)
     $(, $(#[$rest_meta:meta])* $rest:ident => ($rest_key:literal, $rest_line:literal))* $(,)?) => {
        /// 他真的講過的話。封閉集合中的每一個變體都是他可見、可選的答案。
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum UrlOpenAnswer {
            $(#[$meta])*
            #[serde(rename = $first_key)]
            $first,
            $(
                $(#[$rest_meta])*
                #[serde(rename = $rest_key)]
                $rest,
            )*
        }

        impl UrlOpenAnswer {
            pub const ALL: [Self; 1 $(+ { let _ = stringify!($rest); 1 })*] = [
                Self::$first,
                $(Self::$rest,)*
            ];

            pub fn from_key(key: &str) -> Option<Self> {
                match key {
                    $first_key => Some(Self::$first),
                    $($rest_key => Some(Self::$rest),)*
                    _ => None,
                }
            }

            pub const fn line(self) -> &'static str {
                match self {
                    Self::$first => $first_line,
                    $(Self::$rest => $rest_line,)*
                }
            }

            pub const fn key(self) -> &'static str {
                match self {
                    Self::$first => $first_key,
                    $(Self::$rest => $rest_key,)*
                }
            }
        }
    };
}

define_url_open_answers! {
    /// 「等我在。」網址一律要他當場按，票帶不動。
    OnlyOnMyPress => (
        "only-on-my-press",
        "等我在——網址一律要我當場按，你一個人跑的時候先擱著。"
    ),
    /// 「可以，但你要說得出它從哪來。」
    WhenYouCanNameTheOrigin => (
        "when-you-can-name-the-origin",
        "可以自己按，但條件是你說得出這個網址從哪來（那個站要在你自己的紀錄裡出現過）。"
    ),
}

/// 她問的那一句。**全 repo 只有這裡寫一份。**
///
/// 兩個地方在問它：`sister url-policy` 和字母人身上那一格。分成兩份的話，
/// 同一個設定會在兩個地方長成兩個不同的問題，而他答的是哪一個沒人說得準。
pub const QUESTION: &str = "有時候我讀到的東西裡會有一個網址。你不在的時候，要我自己按下去嗎？";

/// 還沒答之前她怎麼做。這句話要和 [`UrlOriginGap::NotAskedYet`] 講同一件事：
/// 不開，但理由是「我還沒問」不是「你說了不要」。
///
/// [`UrlOriginGap::NotAskedYet`]: crate::UrlOriginGap::NotAskedYet
pub const BEFORE_YOU_ANSWER: &str = "在你回答之前，我一個人跑的時候不會開網址——\
     但那不是因為你選了「不要」，是因為我還沒問過你。這兩件事在我嘴裡是兩句話。";

impl UrlOpenAnswer {
    /// 他答完之後她回的那一句。CLI 和桌面共用同一份，否則同一個動作在兩個
    /// 地方會得到兩種確認。
    pub fn recorded_line(self) -> String {
        format!("記下來了：{}", self.line())
    }
}

impl std::str::FromStr for UrlOpenAnswer {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_key(value).ok_or_else(|| {
            format!(
                "答案只能是 {}",
                Self::ALL
                    .into_iter()
                    .map(Self::key)
                    .collect::<Vec<_>>()
                    .join(" 或 ")
            )
        })
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
    /// 查過可採信的錄製來源，其中沒有這個站。
    NotInHerRecord,
    /// 目前沒有可確認為已完成 URL、能替這一步背書的錄製來源。
    /// 舊版錄製可能仍留著 URL，但無法排除是正在輸入的半截字，所以不採信。
    NoTrustedRecordedUrls,
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
pub fn try_url_origin_gap<E>(
    action: &ActionSnapshot,
    approved_by: ApprovedBy,
    policy: UrlOpenPolicy,
    origin: impl FnOnce(&str) -> Result<UrlOrigin, E>,
) -> Result<Option<UrlOriginGap>, E> {
    // 不是開網址的那一步，這道閘門沒有意見。判斷放在裡面不放在呼叫端，
    // 是因為呼叫端只有一個而它會忘記。
    let ActionSnapshot::OpenUrl { url } = action else {
        return Ok(None);
    };
    if matches!(approved_by, ApprovedBy::Press) {
        return Ok(None);
    }
    Ok(match policy {
        UrlOpenPolicy::NotAskedYet => Some(UrlOriginGap::NotAskedYet),
        UrlOpenPolicy::Answered(UrlOpenAnswer::OnlyOnMyPress) => {
            Some(UrlOriginGap::YouSaidPressItYourself)
        }
        UrlOpenPolicy::Answered(UrlOpenAnswer::WhenYouCanNameTheOrigin) => match origin(url)? {
            UrlOrigin::InHerRecord => None,
            UrlOrigin::NotInHerRecord => Some(UrlOriginGap::NotInHerRecord),
            UrlOrigin::NoTrustedRecordedUrls => Some(UrlOriginGap::NoTrustedRecordedUrls),
            UrlOrigin::NotAReadableSite => Some(UrlOriginGap::NotAReadableSite),
        },
    })
}

/// 不會失敗的來源查詢所用的薄 wrapper。真正的授權邊界走
/// [`try_url_origin_gap`]：資料庫查詢錯誤必須保留，不能冒充某一種「沒有」。
pub fn url_origin_gap(
    action: &ActionSnapshot,
    approved_by: ApprovedBy,
    policy: UrlOpenPolicy,
    origin: impl FnOnce(&str) -> UrlOrigin,
) -> Option<UrlOriginGap> {
    try_url_origin_gap(action, approved_by, policy, |url| {
        Ok::<_, std::convert::Infallible>(origin(url))
    })
    .expect("Infallible origin lookup")
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

    /// 設定檔和 IPC 送過來的都是**字串**，而字串什麼都可能是。
    /// 認不得的時候要回 `None`，不是挑一個最像的——挑了等於替他做決定，
    /// 而且做完之後畫面會顯示他「答過了」。
    #[test]
    fn a_key_it_does_not_recognise_is_not_an_answer() {
        for answer in UrlOpenAnswer::ALL {
            assert_eq!(
                UrlOpenAnswer::from_key(answer.key()),
                Some(answer),
                "自己的 key 認不回來：{}",
                answer.key()
            );
        }
        for junk in [
            "",
            " ",
            "only-on-my-press ",
            "ONLY-ON-MY-PRESS",
            "only_on_my_press",
            "only",
            "when-you-can-name-the-origin-x",
            "yes",
            "true",
        ] {
            assert_eq!(
                UrlOpenAnswer::from_key(junk),
                None,
                "「{junk}」被認成了一個答案"
            );
        }
    }

    /// 他答完之後兩個入口（CLI、字母人）回的必須是同一句話，而且**兩個答案
    /// 的確認句不可以一樣**——那是他唯一看得到「我剛剛選到哪一個」的地方。
    #[test]
    fn the_recorded_line_repeats_back_which_one_he_chose() {
        let a = UrlOpenAnswer::OnlyOnMyPress.recorded_line();
        let b = UrlOpenAnswer::WhenYouCanNameTheOrigin.recorded_line();
        assert_ne!(a, b);
        assert!(a.contains(UrlOpenAnswer::OnlyOnMyPress.line()), "{a}");
        assert!(
            b.contains(UrlOpenAnswer::WhenYouCanNameTheOrigin.line()),
            "{b}"
        );
    }

    /// 問句和「還沒答之前怎麼做」那一句是**兩件事**：一句在問，一句在說明
    /// 沉默期間的行為。把後者忘了寫的話，一個從來沒被問過的人會以為現在這個
    /// 行為是他自己選的。
    #[test]
    fn the_question_and_what_she_does_before_he_answers_are_two_sentences() {
        assert!(QUESTION.contains("你不在的時候"), "{QUESTION}");
        assert!(
            BEFORE_YOU_ANSWER.contains("是因為我還沒問過你"),
            "{BEFORE_YOU_ANSWER}"
        );
        assert_ne!(QUESTION, BEFORE_YOU_ANSWER);
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
            .unattended_message(None, Some("sister url-policy"));
        let b = he_said
            .unwrap()
            .unattended_message(None, Some("sister url-policy"));
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
                UrlOrigin::NoTrustedRecordedUrls,
                UrlOriginGap::NoTrustedRecordedUrls,
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
            said.push(
                gap.unwrap()
                    .unattended_message(Some("example.com"), Some("x")),
            );
        }
        for i in 0..said.len() {
            for j in (i + 1)..said.len() {
                assert_ne!(said[i], said[j], "兩種說不出來印出同一句話");
            }
        }
        // 「目前沒有任何網址證據」不可以讀起來像「這一個網址可疑」。
        assert!(
            said[1].contains("沒有") && said[1].contains("錄製來源"),
            "沒有可採信 URL 來源那一句要說清楚量到的空集合：{}",
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
