//! action log 讀出來給人聽的那一份。
//!
//! **兩個呼叫端共用一份：字母人那一格，和 `sister hands log`。** 這一段本來
//! 住在 `apps/desktop/src-tauri/src/hands.rs`，而 CI 對那個 workspace 只跑
//! clippy 和 build——寫在那邊的 `#[cfg(test)]` **一列都不會被執行**。所以底下
//! 那幾條測試在搬進來之前，從寫下的那天起就沒有真的跑過。
//!
//! 同一個理由讓 `target_policy` 和 [`crate::platform`] 也搬進了這個 crate。

use crate::semi_action::RunConclusion;
use crate::{ActionEvent, ApprovedBy, ExecutionResult, Replay};
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
pub(crate) fn at(ms: i64) -> String {
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
            ActionEvent::Approved { at_ms, action, by } => match by {
                Some(ApprovedBy::Press) => {
                    format!("{} 他當場按了：{}", at(*at_ms), action.describe())
                }
                Some(ApprovedBy::StandingGrant) => format!(
                    "{} 憑先前簽好的票自己跑，沒有人在鍵盤前面：{}",
                    at(*at_ms),
                    action.describe()
                ),
                None => format!(
                    "{} 這一列沒有記批准來源（舊版）：{}",
                    at(*at_ms),
                    action.describe()
                ),
            },
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
                // `None` 是舊版根本沒查，不是新版查過但沒有。
                let evidence = match evidence {
                    Some(evidence) => evidence.message(),
                    None => "這一列是舊版寫的；當時沒有去查畫面憑據。".to_string(),
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
    lines.extend(unreadable_lines(replay));
    lines
}

/// 讀不懂的那幾列。它們**不是**她做過的事，是我們解不開的那幾行。
fn unreadable_lines(replay: &Replay) -> Vec<String> {
    replay
        .unreadable
        .iter()
        .map(|bad| format!("第 {} 列讀不懂：{}", bad.line_no, bad.why))
        .collect()
}

/// 最近 `shown` 列，加上一句「上面還有幾列」。
///
/// 這裡的截斷要說出來。安靜地只給最後 20 列，畫面讀起來會是「她總共就做過
/// 這 20 件事」——那是一句沒有人寫、但畫面替你講了的話。
///
/// **上限只砍事件那一疊，讀不懂的那幾列一律留著。** 兩疊混在一起數的話，
/// 一份「5 個事件 + 30 列壞掉」的紀錄在 `shown = 20` 底下會把 5 個事件全部
/// 擠出畫面，只剩滿螢幕的「讀不懂」——讀起來是「她從來沒做過事」，而她做過
/// 五件。截斷那句話仍然在，但沒有人會從那句話推回「被蓋掉的正好是全部的動作」。
pub fn recent_replay_lines(replay: &Replay, shown: usize) -> Vec<String> {
    let mut all = replay_lines(replay);
    let bad = unreadable_lines(replay);
    all.truncate(all.len() - bad.len());
    if all.len() <= shown {
        all.extend(bad);
        return all;
    }
    let hidden = all.len() - shown;
    let mut lines = vec![format!("（更早的 {hidden} 列沒有顯示）")];
    lines.extend(all.into_iter().skip(hidden));
    lines.extend(bad);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semi_action::StepEvidence;
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

    fn finished_with(evidence: Option<StepEvidence>) -> Vec<String> {
        replay_lines(&Replay {
            events: vec![ActionEvent::StepFinished {
                at_ms: 10_000,
                step_number: 1,
                action: ActionSnapshot::OpenUrl {
                    url: "https://a".into(),
                },
                evidence,
            }],
            unreadable: vec![],
        })
    }

    /// 動作前的 frame 不是動作後的驗證；兩句話相同就是這輪要修的 bug。
    #[test]
    fn before_and_after_frames_are_not_the_same_evidence_sentence() {
        let after = finished_with(Some(StepEvidence::After {
            frame_id: 8,
            frame_at_ms: 10_100,
            has_image: true,
        }));
        let before = finished_with(Some(StepEvidence::Before {
            frame_id: 7,
            frame_at_ms: 9_900,
            earlier_by_ms: 100,
            has_image: true,
        }));
        assert!(after[0].contains("做完之後"), "{after:?}");
        assert!(before[0].contains("不是做完之後"), "{before:?}");
        assert!(before[0].contains("100 毫秒"), "{before:?}");
        assert_ne!(after[0], before[0]);
    }

    /// 沒在錄時永遠不會來，正在錄但這個時間窗沒有 frame 則仍可能再來一張。
    #[test]
    fn not_recording_and_recording_without_a_frame_are_not_the_same_sentence() {
        let stopped = finished_with(Some(StepEvidence::NotRecording {
            reason: crate::semi_action::NotRecordingReason::Stopped { at_ms: Some(9_000) },
        }));
        let quiet = finished_with(Some(StepEvidence::NoFrameNearby));
        assert!(stopped[0].contains("收工"), "{stopped:?}");
        assert!(stopped[0].contains("所以不會有"), "{stopped:?}");
        assert!(quiet[0].contains("正在錄"), "{quiet:?}");
        assert!(quiet[0].contains("一張 frame 都沒有"), "{quiet:?}");
        assert_ne!(stopped[0], quiet[0]);
    }

    #[test]
    fn before_and_after_without_images_are_not_the_same_evidence_sentence() {
        let after = finished_with(Some(StepEvidence::After {
            frame_id: 9,
            frame_at_ms: 10_050,
            has_image: false,
        }));
        let before = finished_with(Some(StepEvidence::Before {
            frame_id: 8,
            frame_at_ms: 9_950,
            earlier_by_ms: 50,
            has_image: false,
        }));
        assert!(after[0].contains("做完之後"), "{after:?}");
        assert!(!after[0].contains("不是做完之後"), "{after:?}");
        assert!(before[0].contains("不是做完之後"), "{before:?}");
        assert!(after[0].contains("沒有截圖"), "{after:?}");
        assert!(before[0].contains("沒有截圖"), "{before:?}");
        assert_ne!(after[0], before[0]);
    }

    #[test]
    fn unreadable_and_stopped_are_not_the_same_sentence() {
        use crate::semi_action::NotRecordingReason;
        let unreadable = finished_with(Some(StepEvidence::NotRecording {
            reason: NotRecordingReason::Unreadable,
        }));
        let stopped = finished_with(Some(StepEvidence::NotRecording {
            reason: NotRecordingReason::Stopped { at_ms: Some(9_000) },
        }));
        assert!(unreadable[0].contains("讀不懂"), "{unreadable:?}");
        assert!(unreadable[0].contains("說不準"), "{unreadable:?}");
        assert!(!unreadable[0].contains("所以不會有"), "{unreadable:?}");
        assert!(stopped[0].contains("收工"), "{stopped:?}");
        assert!(stopped[0].contains("所以不會有"), "{stopped:?}");
        assert_ne!(unreadable[0], stopped[0]);
    }

    /// **哪幾句講得出「不會有」，哪幾句講不出——這一格的重點就只有這件事。**
    ///
    /// 六種理由分兩堆：她**確定**不會再有新畫面的（從來沒錄過、收工了、錄製
    /// 已停只剩解釋層在收尾），和她**不知道**的（心跳斷了、才剛起來、狀態檔
    /// 讀不懂）。第二堆講「不會有」就是一句她沒查過的斷言，而讀的人會拿那句話
    /// 當「不用再去看了」——`Booting` 尤其冤，她正要開始錄。
    ///
    /// 上面那條測試只釘了六種裡的兩種。實測過：把 `Booting` 那一句改成
    /// 「所以不會有這一步前後的新畫面憑據」，十六組測試全綠。
    #[test]
    fn only_the_reasons_that_can_know_the_future_say_there_will_be_none() {
        use crate::semi_action::NotRecordingReason as R;
        let certain = [
            R::NeverStarted,
            R::Stopped { at_ms: Some(9_000) },
            R::Stopped { at_ms: None },
            R::Thinking { until_ms: 12_000 },
        ];
        let uncertain = [R::Stalled { at_ms: 8_000 }, R::Booting, R::Unreadable];

        // **這個 match 什麼都不做，而它是承重的——不要刪。**
        //
        // 上面兩個陣列是字面量，`NotRecordingReason` 多一種變體的時候它們不會
        // 編不過，只會靜靜地少測一種。這裡放一個沒有 `_` 的 match，多一種就
        // 編不過，而編譯錯誤指在這幾行——加那一種的人必須先回答一個問題：
        // **那一句到底講不講得出「不會有」？**
        for reason in certain.iter().chain(uncertain.iter()) {
            match reason {
                R::NeverStarted
                | R::Stopped { .. }
                | R::Thinking { .. }
                | R::Stalled { .. }
                | R::Booting
                | R::Unreadable => {}
            }
        }

        for reason in certain {
            let line = finished_with(Some(StepEvidence::NotRecording { reason }))[0].clone();
            assert!(
                line.contains("不會有"),
                "這一種是確定的，卻沒把「不會有」講出來（{reason:?}）：{line}"
            );
        }
        for reason in uncertain {
            let line = finished_with(Some(StepEvidence::NotRecording { reason }))[0].clone();
            assert!(
                !line.contains("不會有"),
                "她根本不知道，卻替一件沒查過的事宣布了「不會有」（{reason:?}）：{line}"
            );
        }
    }

    #[test]
    fn an_old_unchecked_row_says_it_was_not_checked() {
        let legacy = finished_with(None);
        assert!(legacy[0].contains("舊版"), "{legacy:?}");
        assert!(legacy[0].contains("沒有去查"), "{legacy:?}");
    }

    /// 一堆讀不懂的列不可以把她真的做過的事擠出畫面。
    ///
    /// 兩疊混在一起數上限的話，「5 個事件 + 30 列壞掉」在 `shown = 20` 底下
    /// 會只剩滿螢幕的「讀不懂」——讀起來是「她從來沒做過事」，而她做過五件。
    #[test]
    fn broken_lines_do_not_push_real_actions_off_the_screen() {
        let replay = Replay {
            events: (1..=5)
                .map(|n| ActionEvent::Proposed {
                    at_ms: n,
                    action: ActionSnapshot::OpenUrl {
                        url: format!("https://{n}"),
                    },
                })
                .collect(),
            unreadable: (1..=30)
                .map(|n| UnreadableLine {
                    line_no: n,
                    why: "bad json".into(),
                })
                .collect(),
        };
        let lines = recent_replay_lines(&replay, 20);
        for n in 1..=5 {
            assert!(
                lines.iter().any(|l| l.contains(&format!("https://{n}"))),
                "第 {n} 個動作被壞掉的列擠掉了：{lines:?}"
            );
        }
        assert!(
            !lines.iter().any(|l| l.contains("沒有顯示")),
            "事件只有 5 個、上限 20，不該講截斷：{lines:?}"
        );
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
                    by: Some(ApprovedBy::Press),
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
        let by_pulled_switch = replay_lines(&Replay {
            events: vec![ActionEvent::Aborted {
                at_ms: 1,
                after_completed_steps: 2,
                by: crate::semi_action::AbortActor::HandsPulled,
            }],
            unreadable: vec![],
        });
        assert!(by_user[0].contains("使用者"), "{by_user:?}");
        assert!(by_system[0].contains("系統"), "{by_system:?}");
        assert!(
            by_pulled_switch[0].contains("外部拔手開關"),
            "{by_pulled_switch:?}"
        );
        assert_ne!(by_user[0], by_system[0]);
    }
}
