//! action log 讀出來給人聽的那一份。
//!
//! **兩個呼叫端共用一份：字母人那一格，和 `sister hands log`。** 這一段本來
//! 住在 `apps/desktop/src-tauri/src/hands.rs`，而 CI 對那個 workspace 只跑
//! clippy 和 build——寫在那邊的 `#[cfg(test)]` **一列都不會被執行**。所以底下
//! 那幾條測試在搬進來之前，從寫下的那天起就沒有真的跑過。
//!
//! 同一個理由讓 `target_policy` 和 [`crate::platform`] 也搬進了這個 crate。

use crate::semi_action::RunConclusion;
use crate::{ActionEvent, ExecutionResult, Replay};
use chrono::{Local, TimeZone};

/// epoch 毫秒讀成人看得懂的時刻。
///
/// **毫秒要留著。** 這一份紀錄有一條承重的性質是「每一列有自己的時刻」——
/// 整輪蓋同一個時間戳是這個 repo 修過的錯。砍到秒的話，兩列差 400 毫秒會在
/// 畫面上長成同一個時間，而那正好把要看的東西藏起來。
///
/// 對不出時刻的時候印原本那個數字，不是印一個猜的。`ts:` 那個前綴是為了讓
/// 讀的人看得出來「這是原始值，不是時間」——和 `sister-cli` 的 `fmt::timestamp`
/// 同一個慣例。
fn at(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("ts:{ms}"),
    }
}

/// 一列一句話，而且**每一句都自己讀得懂**：時刻、動作、結果。
///
/// 讀不懂的那幾列也要出現。安靜地跳過壞掉的列，等於讓一個「她做過但我們解不開」
/// 的動作在畫面上變成從來沒發生過。
pub fn replay_lines(replay: &Replay) -> Vec<String> {
    let mut lines = replay
        .events
        .iter()
        .map(|event| match event {
            ActionEvent::Granted { at_ms, grant } => {
                format!("{} 授權：{}", at(*at_ms), grant.describe())
            }
            ActionEvent::Proposed { at_ms, action } => {
                format!("{} 提出：{}", at(*at_ms), action.describe())
            }
            ActionEvent::Approved { at_ms, action } => {
                format!("{} 核准：{}", at(*at_ms), action.describe())
            }
            ActionEvent::Executed {
                at_ms,
                action,
                result,
            } => match result {
                ExecutionResult::Succeeded { detail } => {
                    format!("{} 已執行：{}；{detail}", at(*at_ms), action.describe())
                }
                ExecutionResult::Failed { error } => {
                    format!("{} 執行失敗：{}；{error}", at(*at_ms), action.describe())
                }
            },
            ActionEvent::Refused {
                at_ms,
                action,
                reason,
            } => format!(
                "{} 未執行：{}；{}",
                at(*at_ms),
                action.describe(),
                reason.message()
            ),
            ActionEvent::StepFinished {
                at_ms,
                step_number,
                action,
                evidence,
            } => {
                // `None` 是「沒有拿到畫面憑據」，不是「檢查過沒問題」。
                let evidence = match evidence {
                    Some(reference) => format!("畫面憑據：{}", reference.as_str()),
                    None => "沒有取得畫面憑據".to_string(),
                };
                format!(
                    "{} 第 {step_number} 步做完：{}；{evidence}",
                    at(*at_ms),
                    action.describe()
                )
            }
            ActionEvent::Aborted {
                at_ms,
                after_completed_steps,
                by,
            } => format!(
                "{} {}",
                at(*at_ms),
                RunConclusion::Aborted {
                    after_completed_steps: *after_completed_steps,
                    by: *by
                }
                .message()
            ),
            ActionEvent::Concluded { at_ms, conclusion } => {
                format!("{} {}", at(*at_ms), conclusion.message())
            }
        })
        .collect::<Vec<_>>();
    lines.extend(
        replay
            .unreadable
            .iter()
            .map(|bad| format!("第 {} 列讀不懂：{}", bad.line_no, bad.why)),
    );
    lines
}

/// 最近 `shown` 列，加上一句「上面還有幾列」。
///
/// 這裡的截斷要說出來。安靜地只給最後 20 列，畫面讀起來會是「她總共就做過
/// 這 20 件事」——那是一句沒有人寫、但畫面替你講了的話。
pub fn recent_replay_lines(replay: &Replay, shown: usize) -> Vec<String> {
    let all = replay_lines(replay);
    if all.len() <= shown {
        return all;
    }
    let hidden = all.len() - shown;
    let mut lines = vec![format!("（更早的 {hidden} 列沒有顯示）")];
    lines.extend(all.into_iter().skip(hidden));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActionSnapshot, UnreadableLine};

    #[test]
    fn replay_copy_names_bad_line_numbers_and_does_not_count_events_as_actions() {
        let replay = Replay {
            events: vec![
                ActionEvent::Proposed {
                    at_ms: 1,
                    action: ActionSnapshot::OpenUrl {
                        url: "https://a".into(),
                    },
                },
                ActionEvent::Executed {
                    at_ms: 2,
                    action: ActionSnapshot::OpenUrl {
                        url: "https://a".into(),
                    },
                    result: ExecutionResult::Succeeded {
                        detail: "ok".into(),
                    },
                },
            ],
            unreadable: vec![UnreadableLine {
                line_no: 3,
                why: "bad json".into(),
            }],
        };
        let lines = replay_lines(&replay);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("第 3 列") && line.contains("讀不懂"))
        );
        assert!(!lines.iter().any(|line| line.contains("做了 2 件事")));
    }

    /// 只顯示最近幾列的時候，被蓋掉的那幾列要有人講。
    #[test]
    fn a_truncated_action_log_says_how_many_it_is_not_showing() {
        let replay = Replay {
            events: (1..=5)
                .map(|n| ActionEvent::Proposed {
                    at_ms: n,
                    action: ActionSnapshot::OpenUrl {
                        url: format!("https://{n}"),
                    },
                })
                .collect(),
            unreadable: vec![],
        };
        let lines = recent_replay_lines(&replay, 2);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("更早的 3 列"), "{lines:?}");
        assert!(
            lines[1].contains("https://4") && lines[2].contains("https://5"),
            "{lines:?}"
        );
        // 沒有超過上限的時候不要憑空多一句「更早的 0 列」。
        assert_eq!(recent_replay_lines(&replay, 9).len(), 5);
    }

    /// 提出、核准、已執行、做完、收尾——五種列**各自講各自那一件事**。
    ///
    /// 這一條在字母人那半邊沒有過（那邊只驗了壞掉的列和截斷）。少了它，
    /// 把「未執行」和「執行失敗」寫成同一句話不會有任何東西變紅——而那兩句
    /// 差的正是「東西有沒有離開她的手」。
    #[test]
    fn refused_and_failed_are_not_the_same_sentence() {
        let action = ActionSnapshot::OpenUrl {
            url: "https://a".into(),
        };
        let refused = replay_lines(&Replay {
            events: vec![ActionEvent::Refused {
                at_ms: 1,
                action: action.clone(),
                reason: crate::RefusalReason::ObserveHasNoHands,
            }],
            unreadable: vec![],
        });
        let failed = replay_lines(&Replay {
            events: vec![ActionEvent::Executed {
                at_ms: 1,
                action,
                result: ExecutionResult::Failed {
                    error: "作業系統拒絕".into(),
                },
            }],
            unreadable: vec![],
        });
        assert!(refused[0].contains("未執行"), "{refused:?}");
        assert!(failed[0].contains("執行失敗"), "{failed:?}");
        // 「未執行」是「執行失敗」的子字串會讓上面兩條同時成立而毫無意義，
        // 所以直接釘住兩句話不相等，並且各自帶著自己的理由。
        assert_ne!(refused[0], failed[0]);
        assert!(refused[0].contains("她沒有手"), "{refused:?}");
        assert!(failed[0].contains("作業系統拒絕"), "{failed:?}");
    }

    /// 時刻要讀得懂，**而且毫秒不可以被砍掉**。
    ///
    /// 「每一列有自己的時刻」是這份紀錄承重的性質之一（整輪蓋同一個時間戳
    /// 是修過的錯）。把時刻格式化到秒的話，兩列差 400 毫秒會在畫面上長成
    /// 同一個時間——剛好把要看的東西藏起來，而且沒有任何測試會變紅。
    #[test]
    fn two_rows_less_than_a_second_apart_still_read_differently() {
        let action = ActionSnapshot::OpenUrl {
            url: "https://a".into(),
        };
        let lines = replay_lines(&Replay {
            events: vec![
                ActionEvent::Proposed {
                    at_ms: 1_756_200_000_000,
                    action: action.clone(),
                },
                ActionEvent::Approved {
                    at_ms: 1_756_200_000_400,
                    action,
                },
            ],
            unreadable: vec![],
        });
        let stamp = |line: &str| line.split(' ').take(2).collect::<Vec<_>>().join(" ");
        assert_ne!(
            stamp(&lines[0]),
            stamp(&lines[1]),
            "差 400 毫秒的兩列不可以印出同一個時刻：{lines:?}"
        );
        // 讀得懂：是年月日時分秒毫秒，不是那一長串 epoch 毫秒。
        // 不釘死哪一天——那會跟著跑測試那台機器的時區走。釘的是形狀。
        let shape: String = stamp(&lines[0])
            .chars()
            .map(|c| if c.is_ascii_digit() { 'd' } else { c })
            .collect();
        assert_eq!(shape, "dddd-dd-dd dd:dd:dd.ddd", "{lines:?}");
        assert!(
            !lines[0].contains("1756200000000"),
            "原始毫秒不該直接端上畫面：{lines:?}"
        );
    }

    /// 對不出時刻的時候印原始值，而且看得出來那是原始值。
    #[test]
    fn a_timestamp_we_cannot_read_says_so_instead_of_guessing() {
        let lines = replay_lines(&Replay {
            events: vec![ActionEvent::Proposed {
                at_ms: i64::MAX,
                action: ActionSnapshot::OpenUrl {
                    url: "https://a".into(),
                },
            }],
            unreadable: vec![],
        });
        assert!(lines[0].starts_with("ts:"), "{lines:?}");
    }

    /// 中止那一列要說出**是誰喊的停**，而且不可以被讀成「這一輪好好地結束了」。
    #[test]
    fn an_abort_line_names_who_called_it() {
        let by_user = replay_lines(&Replay {
            events: vec![ActionEvent::Aborted {
                at_ms: 1,
                after_completed_steps: 2,
                by: crate::semi_action::AbortActor::User,
            }],
            unreadable: vec![],
        });
        let by_system = replay_lines(&Replay {
            events: vec![ActionEvent::Aborted {
                at_ms: 1,
                after_completed_steps: 2,
                by: crate::semi_action::AbortActor::System,
            }],
            unreadable: vec![],
        });
        assert!(by_user[0].contains("使用者"), "{by_user:?}");
        assert!(by_system[0].contains("系統"), "{by_system:?}");
        assert_ne!(by_user[0], by_system[0]);
    }
}
