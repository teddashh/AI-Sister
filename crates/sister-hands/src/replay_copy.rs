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

/// action-log 沒有任何可讀列時，依檔案是否存在分開兩種歷史。
pub fn empty_run_log_message(file_exists: bool) -> &'static str {
    if file_exists {
        "這份紀錄是空的——**不是**「她從來沒動過手」，是裡面的列被刪光了。"
    } else {
        "還沒有任何動作紀錄。她從來沒有把一個動作端到你面前過。"
    }
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

/// 把扁平 action log 分成「一輪一輪」再讀成人話。
///
/// 這份邏輯刻意放在 `sister-hands`，不放 `apps/desktop`：desktop workspace 的
/// CI 不會執行那裡的單元測試，run 邊界、批准來源和收尾文案若只在那裡測，綠燈
/// 並沒有證明這份稽核報告真的分得對輪。CLI 只負責讀檔和印出這裡的結果。
pub fn recent_run_report_lines(replay: &Replay, shown: usize) -> Vec<String> {
    let mut runs: Vec<Vec<&ActionEvent>> = Vec::new();
    let mut current: Vec<&ActionEvent> = Vec::new();

    for event in &replay.events {
        if matches!(event, ActionEvent::Granted { .. }) && !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
        current.push(event);
        if matches!(
            event,
            ActionEvent::Concluded { .. } | ActionEvent::Aborted { .. }
        ) {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }

    let hidden = runs.len().saturating_sub(shown);
    let mut lines = Vec::new();
    if hidden > 0 {
        lines.push(format!("（更早的 {hidden} 輪沒有顯示）"));
    }
    for (visible_index, run) in runs.into_iter().skip(hidden).enumerate() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(run_report_lines(hidden + visible_index + 1, &run));
    }
    if !replay.unreadable.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(unreadable_lines(replay));
    }
    lines
}

fn run_report_lines(run_number: usize, events: &[&ActionEvent]) -> Vec<String> {
    let mut lines = vec![format!("── 第 {run_number} 輪 ──")];
    match events.first() {
        Some(ActionEvent::Granted { at_ms, grant }) => lines.push(format!(
            "開頭：{}；這一輪授權：{}",
            at(*at_ms),
            grant.describe()
        )),
        // **不可以寫成「開頭不在紀錄裡」。** 那句話斷言有一個開頭、而它不見了，
        // 於是讀的人會去找是誰把它裁掉的。但字母人上那顆按鈕（`Level::Suggest`）
        // 走的根本不是授權書那條路——它從來沒有過開頭。「被裁掉了」和「這條路
        // 本來就沒有」在這一列上分不出來，所以兩種都要說，不要替它挑一種。
        Some(first) => lines.push(format!(
            "開頭：{}；這一輪沒有授權書那一列，所以不知道當初授權了什麼——可能是開頭被裁掉了（`hands forget`、保留期），也可能它本來就不是 `sister do` 開的（字母人上按的那一顆不經過授權書）。",
            at(first.at_ms())
        )),
        None => return lines,
    }

    // 「她提出的第幾件事」。和事件裡那個 `step_number`（做完的第幾步）是兩回事。
    //
    // **不可以只在 `Proposed` 加一。** `action-log.jsonl` 有兩個生產者，而字母人
    // 那一個刻意不寫 `Proposed`（見 `apps/desktop/src-tauri/src/hands.rs:26`——
    // 提出的時刻沒有量到，寧可少一列也不要多一列假的）。只數 `Proposed` 的話，
    // 字母人上按的每一顆按鈕都印成「第 0 步」：那個 0 不是「第零步」，是「這裡
    // 從來沒有加過」，而且 N 顆不同的按鈕會全部共用同一個編號。
    //
    // 規矩：`Proposed` 開一步；`Approved` 在前一列不是 `Proposed` 的時候也開一步。
    let mut proposed_number = 0_u32;
    let mut previous_was_proposed = false;
    for event in events {
        let opens_a_step = match event {
            ActionEvent::Proposed { .. } => true,
            ActionEvent::Approved { .. } => !previous_was_proposed,
            _ => false,
        };
        if opens_a_step {
            proposed_number += 1;
        }
        previous_was_proposed = matches!(event, ActionEvent::Proposed { .. });
        match event {
            ActionEvent::Granted { .. } => {}
            ActionEvent::Proposed { at_ms, action } => {
                lines.push(format!(
                    "第 {proposed_number} 步：{}；提出動作：{}",
                    at(*at_ms),
                    action.describe()
                ));
            }
            ActionEvent::Approved { at_ms, action, by } => {
                let by = match by {
                    Some(ApprovedBy::Press) => "他當場按的",
                    Some(ApprovedBy::StandingGrant) => {
                        "憑先前簽好的票自己跑的；當時沒有人在鍵盤前面"
                    }
                    None => "舊版沒有記批准來源，所以不知道是誰批准的",
                };
                lines.push(format!(
                    "第 {proposed_number} 步批准：{}；{by}；{}",
                    at(*at_ms),
                    action.describe()
                ));
            }
            ActionEvent::Executed {
                at_ms,
                action,
                result,
            } => {
                let result = match result {
                    ExecutionResult::Succeeded { detail } => format!("成功：{detail}"),
                    ExecutionResult::Failed { error } => format!("失敗：{error}"),
                };
                lines.push(format!(
                    "第 {proposed_number} 步結果：{}；{}；{result}",
                    at(*at_ms),
                    action.describe()
                ));
            }
            ActionEvent::Refused {
                at_ms,
                action,
                reason,
            } => lines.push(format!(
                "第 {proposed_number} 步結果：{}；{}；被拒絕：{}",
                at(*at_ms),
                action.describe(),
                reason.message()
            )),
            ActionEvent::StepFinished {
                at_ms,
                step_number,
                action,
                evidence,
            } => {
                let evidence = match evidence {
                    Some(evidence) => evidence.message(),
                    None => "這一列是舊版寫的；當時沒有去查畫面憑據。".to_string(),
                };
                // **這裡有兩個不同的數字，不能共用「第 N 步」這個標籤。**
                // 上面那個是「她提出的第幾件事」（每一次 `Proposed` 都加一）；
                // 事件裡帶的 `step_number` 是 `finish_step` 給的，而 `finish_step`
                // 只在 `Outcome::Done` 時才被呼叫，數的是「她真的做完的第幾步」。
                // 第一件被拒絕、第二件做完的時候，這兩個數字是 2 和 1——都印成
                // 「第 N 步」的話，同一個動作會在報告裡拿到兩個編號。
                lines.push(format!(
                    "第 {proposed_number} 步畫面證據：{}；{}；{evidence}（這是她這一輪做完的第 {step_number} 步）",
                    at(*at_ms),
                    action.describe()
                ));
            }
            ActionEvent::Aborted {
                at_ms,
                after_completed_steps,
                by,
            } => lines.push(format!(
                "收尾：{} {}",
                at(*at_ms),
                RunConclusion::Aborted {
                    after_completed_steps: *after_completed_steps,
                    by: *by,
                }
                .message()
            )),
            ActionEvent::Concluded { at_ms, conclusion } => {
                lines.push(format!("收尾：{} {}", at(*at_ms), conclusion.message()));
            }
        }
    }
    if !matches!(
        events.last(),
        Some(ActionEvent::Concluded { .. } | ActionEvent::Aborted { .. })
    ) {
        lines.push("收尾：這一輪的紀錄到這裡就沒有了。".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semi_action::{StepEvidence, StepWait};
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
            wait: StepWait::Waited { ms: 2_000 },
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
        let quiet = finished_with(Some(StepEvidence::NoFrameNearby {
            wait: StepWait::Waited { ms: 2_000 },
        }));
        assert!(stopped[0].contains("收工"), "{stopped:?}");
        assert!(stopped[0].contains("所以不會有"), "{stopped:?}");
        assert!(quiet[0].contains("正在錄"), "{quiet:?}");
        assert!(quiet[0].contains("一張 frame 都沒有"), "{quiet:?}");
        assert_ne!(stopped[0], quiet[0]);
    }

    #[test]
    fn before_says_whether_it_really_waited() {
        let message = |wait| {
            StepEvidence::Before {
                frame_id: 7,
                frame_at_ms: 9_900,
                earlier_by_ms: 100,
                has_image: true,
                wait,
            }
            .message()
        };
        let did_not_wait = message(StepWait::DidNotWait {
            because: crate::semi_action::NotRecordingReason::Stopped { at_ms: Some(9_000) },
        });
        let waited = message(StepWait::Waited { ms: 2_000 });
        assert!(
            did_not_wait.contains("收工") && did_not_wait.contains("不會有"),
            "{did_not_wait}"
        );
        assert!(
            waited.contains("等了 2000 毫秒") && waited.contains("還是沒有等到"),
            "{waited}"
        );
        assert_ne!(did_not_wait, waited);
    }

    #[test]
    fn alpha_79_no_frame_nearby_json_still_reads() {
        let old = r#"{"event":"step_finished","at_ms":10000,"step_number":1,"action":{"action":"open_url","url":"https://a"},"evidence":{"kind":"no_frame_nearby"}}"#;
        let event: ActionEvent = serde_json::from_str(old).expect("alpha.79 action-log row");
        let ActionEvent::StepFinished {
            evidence: Some(evidence),
            ..
        } = event
        else {
            panic!("舊列沒有讀成 step_finished evidence");
        };
        assert!(matches!(
            evidence,
            StepEvidence::NoFrameNearby {
                wait: StepWait::NotRecorded
            }
        ));
        let message = evidence.message();
        assert!(message.contains("她當時正在錄"), "{message}");
        assert!(!message.contains("在不在錄沒有記"), "{message}");
    }

    #[test]
    fn alpha_79_before_json_does_not_invent_recording_presence() {
        let old = r#"{"kind":"before","frame_id":7,"frame_at_ms":9900,"earlier_by_ms":100,"has_image":true}"#;
        let evidence: StepEvidence = serde_json::from_str(old).expect("alpha.79 evidence");
        assert!(matches!(
            evidence,
            StepEvidence::Before {
                wait: StepWait::NotRecorded,
                ..
            }
        ));
        let message = evidence.message();
        assert!(message.contains("舊版"), "{message}");
        assert!(message.contains("在不在錄沒有記"), "{message}");
        assert!(!message.contains("沒在錄"), "{message}");
        assert!(!message.contains("等了 0 毫秒"), "{message}");
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
            wait: StepWait::Waited { ms: 2_000 },
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

    fn report(events: Vec<ActionEvent>, limit: usize) -> String {
        recent_run_report_lines(
            &Replay {
                events,
                unreadable: vec![],
            },
            limit,
        )
        .join("\n")
    }

    /// 字母人那顆按鈕寫的是 `Approved` + 結果，**沒有 `Proposed`**
    /// （`apps/desktop/src-tauri/src/hands.rs:26`）。只在 `Proposed` 加一的話，
    /// 每一顆按鈕都印成「第 0 步」，而且 N 顆全部共用那一個編號。
    #[test]
    fn desktop_button_rows_get_their_own_step_numbers_not_a_shared_zero() {
        let press = |url: &str, at_ms: i64| {
            vec![
                ActionEvent::Approved {
                    at_ms,
                    action: ActionSnapshot::OpenUrl { url: url.into() },
                    by: Some(ApprovedBy::Press),
                },
                ActionEvent::Executed {
                    at_ms: at_ms + 1,
                    action: ActionSnapshot::OpenUrl { url: url.into() },
                    result: ExecutionResult::Succeeded {
                        detail: "ok".into(),
                    },
                },
            ]
        };
        let mut events = press("https://first", 1_700_000_000_000);
        events.extend(press("https://second", 1_700_000_010_000));
        let text = report(events, 5);

        assert!(
            !text.contains("第 0 步"),
            "沒有 Proposed 不等於「第零步」，那個 0 是從來沒有加過：\n{text}"
        );
        assert!(text.contains("第 1 步批准"), "{text}");
        assert!(
            text.contains("第 2 步批准"),
            "兩顆不同的按鈕不可以共用一個編號：\n{text}"
        );
    }

    /// 沒有 `Granted` 的那一輪不可以斷言「開頭被裁掉了」——字母人那條路
    /// 從來就沒有授權書。兩種都要說，不要替它挑一種。
    #[test]
    fn a_run_without_a_grant_row_does_not_claim_the_opening_was_trimmed() {
        let text = report(
            vec![ActionEvent::Approved {
                at_ms: 1_700_000_000_000,
                action: ActionSnapshot::OpenUrl {
                    url: "https://a".into(),
                },
                by: Some(ApprovedBy::Press),
            }],
            5,
        );
        assert!(text.contains("被裁掉"), "{text}");
        assert!(
            text.contains("字母人"),
            "「本來就沒有授權書」那一種也要講出來：\n{text}"
        );
    }

    fn grant(task: &str) -> crate::semi_action::Grant {
        use crate::semi_action::{
            ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, StepLimit, Task,
        };
        Grant::new(
            Task::new(task),
            AllowedApps::new([App::new("chrome.exe")]),
            AllowedActions::new([ActionKind::OpenUrl]),
            Expiry::after_issued(1, 60_000),
            StepLimit::new(3).expect("3 > 0"),
        )
    }

    fn proposed(at_ms: i64, url: &str) -> ActionEvent {
        ActionEvent::Proposed {
            at_ms,
            action: ActionSnapshot::OpenUrl { url: url.into() },
        }
    }

    /// 一輪裡有兩個不同的「第幾步」，而且它們會分家。
    ///
    /// 提出的第幾件事，每次 `Proposed` 加一；事件裡的 `step_number` 由
    /// `finish_step` 給，而那個只在 `Outcome::Done` 時才被呼叫。第一件被拒絕、
    /// 第二件做完的時候，這兩個數字是 2 和 1。兩個都印成「第 N 步」的話，同一個
    /// 動作在報告裡會有兩個編號，而讀的人沒有任何線索知道是哪一種。
    #[test]
    fn a_refused_step_makes_the_two_step_numbers_diverge_and_they_must_not_share_a_label() {
        let text = report(
            vec![
                ActionEvent::Granted {
                    at_ms: 1,
                    grant: grant("編號"),
                },
                proposed(2, "https://refused"),
                ActionEvent::Refused {
                    at_ms: 3,
                    action: ActionSnapshot::OpenUrl {
                        url: "https://refused".into(),
                    },
                    reason: crate::RefusalReason::ObserveHasNoHands,
                },
                proposed(4, "https://done"),
                ActionEvent::StepFinished {
                    at_ms: 5,
                    step_number: 1,
                    action: ActionSnapshot::OpenUrl {
                        url: "https://done".into(),
                    },
                    evidence: None,
                },
            ],
            5,
        );
        let evidence = text
            .lines()
            .find(|l| l.contains("https://done") && l.contains("畫面證據"))
            .unwrap_or_else(|| panic!("{text}"));
        // 那一行講的是**第二件**提出的事，不是「第 1 步」。
        assert!(evidence.contains("第 2 步畫面證據"), "{evidence}");
        assert!(!evidence.starts_with("第 1 步"), "{evidence}");
        // 而做完的第 1 步這個事實還在，只是它有自己的名字。
        assert!(evidence.contains("做完的第 1 步"), "{evidence}");
    }

    #[test]
    fn two_runs_do_not_share_steps() {
        let text = report(
            vec![
                ActionEvent::Granted {
                    at_ms: 1,
                    grant: grant("第一輪"),
                },
                proposed(2, "https://first"),
                ActionEvent::Granted {
                    at_ms: 4,
                    grant: grant("第二輪"),
                },
                proposed(5, "https://second"),
                ActionEvent::Concluded {
                    at_ms: 6,
                    conclusion: crate::semi_action::RunConclusionRecord::Completed {
                        asked: Some(1),
                        decided_by: Some(ApprovedBy::Press),
                    },
                },
            ],
            5,
        );
        let second = text.split("── 第 2 輪 ──").nth(1).expect("第二輪");
        assert!(second.contains("https://second"), "{text}");
        assert!(!second.contains("https://first"), "{text}");
    }

    #[test]
    fn an_unclosed_run_says_the_record_ends_without_claiming_completion() {
        let text = report(
            vec![
                ActionEvent::Granted {
                    at_ms: 1,
                    grant: grant("未收尾"),
                },
                proposed(2, "https://a"),
            ],
            5,
        );
        assert!(text.contains("這一輪的紀錄到這裡就沒有了"), "{text}");
        assert!(!text.contains("完成"), "{text}");
    }

    #[test]
    fn orphan_steps_say_the_opening_is_missing_without_printing_an_empty_scope() {
        let text = report(vec![proposed(2, "https://orphan")], 5);
        assert!(text.contains("沒有授權書那一列"), "{text}");
        assert!(text.contains("不知道當初授權了什麼"), "{text}");
        assert!(!text.contains("這一輪授權："), "{text}");
    }

    #[test]
    fn three_approval_sources_have_three_non_overlapping_sentences() {
        let action = ActionSnapshot::OpenUrl {
            url: "https://a".into(),
        };
        let sentence = |by| {
            report(
                vec![ActionEvent::Approved {
                    at_ms: 1,
                    action: action.clone(),
                    by,
                }],
                5,
            )
        };
        let press = sentence(Some(ApprovedBy::Press));
        let ticket = sentence(Some(ApprovedBy::StandingGrant));
        let legacy = sentence(None);
        for (left, right) in [(&press, &ticket), (&press, &legacy), (&ticket, &legacy)] {
            assert!(!left.contains(right), "{left:?} 包含 {right:?}");
            assert!(!right.contains(left), "{right:?} 包含 {left:?}");
        }
        assert!(press.contains("他當場按的"), "{press}");
        assert!(ticket.contains("憑先前簽好的票自己跑的"), "{ticket}");
        assert!(legacy.contains("舊版沒有記批准來源"), "{legacy}");
    }

    #[test]
    fn missing_evidence_image_says_the_row_exists_but_the_image_does_not() {
        let text = report(
            vec![ActionEvent::StepFinished {
                at_ms: 2,
                step_number: 1,
                action: ActionSnapshot::OpenUrl {
                    url: "https://a".into(),
                },
                evidence: Some(StepEvidence::After {
                    frame_id: 7,
                    frame_at_ms: 3,
                    has_image: false,
                }),
            }],
            5,
        );
        assert!(text.contains("紀錄在，圖不在"), "{text}");
        assert!(text.contains("frame #7"), "{text}");
    }

    #[test]
    fn legacy_completed_and_measured_zero_are_different_sentences() {
        use crate::semi_action::RunConclusionRecord;
        let legacy = report(
            vec![ActionEvent::Concluded {
                at_ms: 2,
                conclusion: RunConclusionRecord::Completed {
                    asked: None,
                    decided_by: None,
                },
            }],
            5,
        );
        let zero = report(
            vec![ActionEvent::Concluded {
                at_ms: 2,
                conclusion: RunConclusionRecord::Completed {
                    asked: Some(0),
                    decided_by: Some(ApprovedBy::Press),
                },
            }],
            5,
        );
        assert_ne!(legacy, zero);
        assert!(legacy.contains("沒有記是誰決定的"), "{legacy}");
        assert!(zero.contains("一步都沒有問到你"), "{zero}");
    }

    #[test]
    fn completed_decision_sources_have_three_non_overlapping_sentences() {
        use crate::semi_action::RunConclusionRecord;
        let sentence = |decided_by| {
            RunConclusionRecord::Completed {
                asked: Some(1),
                decided_by,
            }
            .message()
        };
        let press = sentence(Some(ApprovedBy::Press));
        let ticket = sentence(Some(ApprovedBy::StandingGrant));
        let legacy = sentence(None);
        assert!(press.contains("問到你面前"), "{press}");
        assert!(!ticket.contains('問'), "{ticket}");
        assert!(legacy.contains("沒有記是誰決定的"), "{legacy}");
        for (left, right) in [(&press, &ticket), (&press, &legacy), (&ticket, &legacy)] {
            assert_ne!(left, right);
        }
    }

    #[test]
    fn pulled_hands_abort_names_the_external_switch() {
        let text = report(
            vec![ActionEvent::Aborted {
                at_ms: 2,
                after_completed_steps: 1,
                by: crate::semi_action::AbortActor::HandsPulled,
            }],
            5,
        );
        assert!(text.contains("外部拔手開關"), "{text}");
        assert!(!text.contains("中止者：使用者"), "{text}");
    }

    #[test]
    fn an_empty_existing_log_and_a_missing_log_are_not_the_same_history() {
        let emptied = empty_run_log_message(true);
        let never_existed = empty_run_log_message(false);
        assert_ne!(emptied, never_existed);
        assert!(emptied.contains("列被刪光"), "{emptied}");
        assert!(never_existed.contains("從來沒有"), "{never_existed}");
    }

    #[test]
    fn run_limit_keeps_the_newest_runs_and_names_the_hidden_count() {
        let mut events = Vec::new();
        for n in 1..=3 {
            events.push(ActionEvent::Granted {
                at_ms: n * 10,
                grant: grant(&format!("第{n}輪")),
            });
            events.push(proposed(n * 10 + 1, &format!("https://run-{n}")));
            events.push(ActionEvent::Concluded {
                at_ms: n * 10 + 2,
                conclusion: crate::semi_action::RunConclusionRecord::Completed {
                    asked: Some(1),
                    decided_by: Some(ApprovedBy::Press),
                },
            });
        }
        let text = report(events, 2);
        assert!(text.contains("更早的 1 輪沒有顯示"), "{text}");
        assert!(!text.contains("https://run-1"), "{text}");
        assert!(text.contains("https://run-2"), "{text}");
        assert!(text.contains("https://run-3"), "{text}");
    }
}
