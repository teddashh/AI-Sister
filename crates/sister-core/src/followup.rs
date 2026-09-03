//! 被動回答尾端的「順帶確認」。
//!
//! 這個模組不是主動開口候選來源，尤其不得由 `gatekeeper` 讀取。只有使用者先
//! 開啟對話、產品正在組裝該次回答時，呼叫端才可以使用這裡的結果。

use crate::{Millis, db::CommitmentRow};

/// 兩次順帶確認至少隔七天。這是實作選擇，沒量過。
pub const FOLLOWUP_INTERVAL_MS: Millis = 7 * 24 * 60 * 60 * 1_000;

/// 沒有 due 的卡放到這麼久沒動靜，才算「掛著」。這是實作選擇，沒量過。
///
/// 為什麼要有這一條：SPEC §7.2.4 自己舉的例子是
/// 「順便一提，Cloudflare 那件還掛著嗎？」——而「Cloudflare 那件」正是一張
/// **沒有講明時間**的卡。只認 `due_at` 的話，這個機制對它自己的範例是啞的，
/// 而真正會被螢幕外做掉的，多半就是這種沒約時間的雜事。
pub const FOLLOWUP_STALE_MS: Millis = 14 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowupState {
    pub last_asked_at: Millis,
    pub commitment_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowupDecision {
    NoEligibleCommitment,
    CoolingDown { eligible_id: i64, until: Millis },
    Ask { commitment_id: i64, text: String },
}

pub fn decide(
    commitments: &[CommitmentRow],
    now: Millis,
    previous: Option<&FollowupState>,
) -> FollowupDecision {
    let candidate = commitments
        .iter()
        .filter(|c| c.status == "open" && is_hanging(c, now))
        .filter(|c| previous.is_none_or(|p| p.commitment_id != c.id))
        .min_by_key(|c| (c.due_at, c.id));
    let Some(candidate) = candidate else {
        return FollowupDecision::NoEligibleCommitment;
    };
    if let Some(previous) = previous {
        let until = previous.last_asked_at.saturating_add(FOLLOWUP_INTERVAL_MS);
        if now < until {
            return FollowupDecision::CoolingDown {
                eligible_id: candidate.id,
                until,
            };
        }
    }
    FollowupDecision::Ask {
        commitment_id: candidate.id,
        text: followup_sentence(&candidate.text),
    }
}

/// 這張卡「掛著」了嗎——有講時間的看時間，沒講時間的看放了多久。
///
/// 兩種卡問的是同一件事，但**量的不是同一個東西**，所以分兩臂寫而不是拿
/// `due_at.unwrap_or(created_at)` 把它們併成一條式子：併起來以後，
/// 「五點要接小孩，現在五點半」和「兩週前記下的雜事」會走進同一個數字，
/// 而它們該不該被問的理由完全不同。
fn is_hanging(c: &CommitmentRow, now: Millis) -> bool {
    match c.due_at {
        Some(due) => due <= now,
        None => now.saturating_sub(c.created_at) >= FOLLOWUP_STALE_MS,
    }
}

pub fn followup_sentence(commitment_text: &str) -> String {
    format!(
        "順便一提，我最後看到的是「{}」還掛著；這件事後來有完成嗎？",
        commitment_text.trim()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseIntent {
    NotAClosure,
    Unrecognized,
    Ambiguous {
        commitment_ids: Vec<i64>,
    },
    Close {
        commitment_id: i64,
        kill_note: String,
    },
}

/// 問句的收尾。命中任何一個就**不是**結案語。
///
/// 這道門是這個函式最要緊的一行。沒有它的話，
/// 「Cloudflare DNS 完成了嗎？」會逐字命中卡片原文、也逐字命中「完成了」，
/// 於是一句**提問**會把那張卡改成 `dead`——而 SPEC §7.2 規則 1 對 `dead`
/// 的承諾是「永不再提」。上一版的保守（要求逐字含卡片原文）擋不掉這一種，
/// 因為問句本來就會逐字唸出那張卡的名字。
/// 「沒」單獨收尾是「還沒」不是「沒了」，所以它也在這裡。
const QUESTION_TAILS: [&str; 5] = ["嗎", "呢", "?", "？", "沒"];

pub fn resolve_close_intent(message: &str, commitments: &[CommitmentRow]) -> CloseIntent {
    let said = message.trim();
    // 「沒了」和「了沒」共用同兩個字，順序相反：前者是結案，後者是在問。
    // 所以問句要先判，而且判的是**結尾**不是包含——「沒了，不用管它」開頭
    // 就結案完了，不該被句中的字擋掉。
    let asking = QUESTION_TAILS
        .iter()
        .any(|tail| said.trim_end_matches(['。', '.', ' ']).ends_with(tail));
    if asking {
        return CloseIntent::NotAClosure;
    }
    let closing = ["弄好了", "沒了", "完成了"]
        .iter()
        .any(|word| said.contains(word));
    if !closing {
        return CloseIntent::NotAClosure;
    }
    let matches: Vec<i64> = commitments
        .iter()
        .filter(|c| c.status == "open" && !c.text.trim().is_empty() && said.contains(c.text.trim()))
        .map(|c| c.id)
        .collect();
    match matches.as_slice() {
        [] => CloseIntent::Unrecognized,
        [id] => CloseIntent::Close {
            commitment_id: *id,
            kill_note: said.to_string(),
        },
        _ => CloseIntent::Ambiguous {
            commitment_ids: matches,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: i64, text: &str, due_at: Option<Millis>) -> CommitmentRow {
        CommitmentRow {
            id,
            text: text.into(),
            kind: "followup".into(),
            born_from: 1,
            evidence_json: "[]".into(),
            agreed_evidence_json: Some("[]".into()),
            people_json: "[]".into(),
            due_hint: None,
            due_source: None,
            due_at,
            status: "open".into(),
            confidence: 0.8,
            allowed_next_step: None,
            allowed_next_step_fact: None,
            last_evidence_seen_at: None,
            kill_note: None,
            created_at: 1,
            updated_at: 1,
            tombstoned_at: None,
        }
    }

    #[test]
    fn followup_words_are_uncertain_and_quote_the_card() {
        let text = followup_sentence("Cloudflare DNS");
        assert!(text.contains("順便一提"));
        assert!(text.contains("我最後看到的是"));
        assert!(text.contains("Cloudflare DNS"));
        assert!(text.contains('？'));
        assert!(!text.contains("你還沒做"));
        assert!(!text.contains("你沒完成"));
    }

    #[test]
    fn no_candidate_and_cooldown_are_different_answers() {
        assert_eq!(
            decide(&[], 10, None),
            FollowupDecision::NoEligibleCommitment
        );
        let cards = [card(2, "Cloudflare DNS", Some(5))];
        assert!(matches!(
            decide(
                &cards,
                10,
                Some(&FollowupState {
                    last_asked_at: 9,
                    commitment_id: 1
                })
            ),
            FollowupDecision::CoolingDown { eligible_id: 2, .. }
        ));
    }

    #[test]
    fn vague_closure_never_guesses_a_card() {
        let cards = [
            card(1, "Cloudflare DNS", Some(5)),
            card(2, "寄帳單", Some(5)),
        ];
        assert_eq!(
            resolve_close_intent("那個弄好了", &cards),
            CloseIntent::Unrecognized
        );
        assert!(matches!(
            resolve_close_intent("Cloudflare DNS 和寄帳單都弄好了", &cards),
            CloseIntent::Ambiguous { .. }
        ));
    }

    /// 問一張卡的近況**不能**把它殺掉。
    ///
    /// 「完成了嗎？」逐字含卡片原文、也逐字含「完成了」，所以只靠「含卡片
    /// 原文」那道保守門是擋不住的——而 `dead` 在 SPEC §7.2 的承諾是「永不
    /// 再提」，殺錯了沒有回頭路。
    #[test]
    fn asking_about_a_card_must_not_kill_it() {
        let cards = [card(1, "Cloudflare DNS", Some(5))];
        for asked in [
            "Cloudflare DNS 完成了嗎？",
            "Cloudflare DNS 完成了嗎",
            "Cloudflare DNS 弄好了沒",
            "Cloudflare DNS 完成了?",
            "Cloudflare DNS 弄好了呢",
        ] {
            assert_eq!(
                resolve_close_intent(asked, &cards),
                CloseIntent::NotAClosure,
                "{asked} 是問句，不是結案語"
            );
        }
        // 而陳述句還是要走得到 Close——只留上面那半的話，把整個函式改成
        // 永遠回 NotAClosure 也會全綠。
        assert!(matches!(
            resolve_close_intent("Cloudflare DNS 弄好了。", &cards),
            CloseIntent::Close {
                commitment_id: 1,
                ..
            }
        ));
    }

    /// 沒講時間的卡也要問得到——SPEC §7.2.4 的範例就是這一種。
    #[test]
    fn a_card_with_no_due_is_still_asked_once_it_goes_stale() {
        let fresh = [card(1, "Cloudflare 那件", None)];
        assert_eq!(
            decide(&fresh, 1 + FOLLOWUP_STALE_MS - 1, None),
            FollowupDecision::NoEligibleCommitment,
            "才剛記下就問，就是 nag"
        );
        assert!(
            matches!(
                decide(&fresh, 1 + FOLLOWUP_STALE_MS, None),
                FollowupDecision::Ask {
                    commitment_id: 1,
                    ..
                }
            ),
            "放了兩週沒動靜的卡問不到，這個機制對自己的範例是啞的"
        );
    }

    #[test]
    fn explicit_closure_keeps_the_users_words_as_kill_note() {
        let cards = [card(1, "Cloudflare DNS", Some(5))];
        assert_eq!(
            resolve_close_intent("Cloudflare DNS 弄好了", &cards),
            CloseIntent::Close {
                commitment_id: 1,
                kill_note: "Cloudflare DNS 弄好了".into()
            }
        );
    }
}
