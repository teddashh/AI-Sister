use sister_hands::semi_action::RunConclusion;
use sister_hands::target_policy::{validate_file, validate_url, validate_window_title};
use sister_hands::{
    ActionEvent, ActionLog, ExecutionResult, Executor, Level, Outcome, Replay, Suggestion,
};
use std::path::Path;

pub fn outcome_message(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Refused { reason } => format!("她沒有動手：{}", reason.message()),
        Outcome::Failed { error } => format!("她動手了，但執行失敗：{error}"),
        Outcome::Done { detail } => format!("動作完成：{detail}"),
    }
}

pub fn replay_lines(replay: &Replay) -> Vec<String> {
    let mut lines = replay
        .events
        .iter()
        .map(|event| match event {
            ActionEvent::Proposed { at_ms, action } => format!("{at_ms} 提出：{}", action.describe()),
            ActionEvent::Approved { at_ms, action } => format!("{at_ms} 核准：{}", action.describe()),
            ActionEvent::Executed { at_ms, action, result } => match result {
                ExecutionResult::Succeeded { detail } => format!("{at_ms} 已執行：{}；{detail}", action.describe()),
                ExecutionResult::Failed { error } => format!("{at_ms} 執行失敗：{}；{error}", action.describe()),
            },
            ActionEvent::Refused { at_ms, action, reason } => format!("{at_ms} 未執行：{}；{}", action.describe(), reason.message()),
            ActionEvent::StepFinished { at_ms, step_number, action, evidence } => {
                // `None` 是「沒有拿到畫面憑據」，不是「檢查過沒問題」。
                let evidence = match evidence {
                    Some(reference) => format!("畫面憑據：{}", reference.as_str()),
                    None => "沒有取得畫面憑據".to_string(),
                };
                format!("{at_ms} 第 {step_number} 步做完：{}；{evidence}", action.describe())
            }
            ActionEvent::Aborted { at_ms, after_completed_steps, by } => format!(
                "{at_ms} {}",
                RunConclusion::Aborted { after_completed_steps: *after_completed_steps, by: *by }.message()
            ),
            ActionEvent::Concluded { at_ms, conclusion } => format!("{at_ms} {}", conclusion.message()),
        })
        .collect::<Vec<_>>();
    lines.extend(replay.unreadable.iter().map(|bad| {
        format!("第 {} 列讀不懂：{}", bad.line_no, bad.why)
    }));
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

pub struct PlatformExecutor;

impl Executor for PlatformExecutor {
    fn execute(&mut self, suggestion: &Suggestion) -> Result<String, String> {
        platform_execute(suggestion)
    }
}

#[cfg(not(windows))]
fn platform_execute(_suggestion: &Suggestion) -> Result<String, String> {
    Err("這台機器上做不到：這一版只有 Windows 平台執行層".into())
}

#[cfg(windows)]
fn platform_execute(suggestion: &Suggestion) -> Result<String, String> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible, SW_SHOWNORMAL, SetForegroundWindow};
    use windows::core::{BOOL, PCWSTR};

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        value.encode_wide().chain(Some(0)).collect()
    }
    fn shell_open(value: &std::ffi::OsStr) -> Result<String, String> {
        let value = wide(value);
        let verb = wide(std::ffi::OsStr::new("open"));
        let result = unsafe { ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(value.as_ptr()), None, None, SW_SHOWNORMAL) };
        if result.0 as isize <= 32 { Err(format!("作業系統拒絕開啟（ShellExecuteW={}）", result.0 as isize)) } else { Ok("作業系統已接受開啟請求".into()) }
    }
    unsafe extern "system" fn find(hwnd: HWND, raw: LPARAM) -> BOOL {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() { return BOOL(1); }
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len <= 0 { return BOOL(1); }
        let mut buf = vec![0u16; len as usize + 1];
        let got = unsafe { GetWindowTextW(hwnd, &mut buf) };
        let state = unsafe { &mut *(raw.0 as *mut (&str, Option<HWND>)) };
        if String::from_utf16_lossy(&buf[..got as usize]).contains(state.0) {
            state.1 = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
    match suggestion {
        Suggestion::OpenUrl { url, .. } => { validate_url(url)?; shell_open(std::ffi::OsStr::new(url)) }
        Suggestion::OpenFile { path, .. } => { validate_file(path)?; shell_open(path.as_os_str()) },
        Suggestion::FocusWindow { title, .. } => {
            validate_window_title(title)?;
            let mut state = (title.as_str(), None);
            unsafe { let _ = EnumWindows(Some(find), LPARAM(&mut state as *mut _ as isize)); }
            let hwnd = state.1.ok_or_else(|| format!("找不到標題含「{title}」的視窗"))?;
            if unsafe { SetForegroundWindow(hwnd) }.as_bool() { Ok(format!("已聚焦視窗：{title}")) } else { Err(format!("Windows 不允許聚焦視窗：{title}")) }
        }
    }
}

pub fn execute_logged(data_dir: &Path, raw: &str, now: i64) -> Result<String, String> {
    let button = sister_hands::SuggestionButton::parse_json(raw)
        .map_err(|error| format!("按鈕內容已經讀不懂，沒有動手：{error}"))?;
    let suggestion = button.press();
    let action = suggestion.snapshot();
    let log = ActionLog::in_data_dir(data_dir);
    // **這裡只寫 `Approved`。** 提出是她把按鈕畫上去的那一刻，不是他按下去的
    // 這一刻；兩列都蓋上 `now` 的話，回放的人會看到一次零秒的思考，而那個零
    // 是我們沒有量到提出時間、不是他真的沒有猶豫。少一列比多一列假的好。
    log.append(&ActionEvent::Approved { at_ms: now, action: action.clone() }).map_err(|e| format!("寫入 approved 失敗，沒有動手：{e:#}"))?;
    let mut executor = PlatformExecutor;
    let outcome = sister_hands::execute_with(Level::Suggest, &mut executor, &suggestion);
    let terminal = match &outcome {
        Outcome::Refused { reason } => ActionEvent::Refused { at_ms: now, action, reason: reason.clone() },
        Outcome::Failed { error } => ActionEvent::Executed { at_ms: now, action, result: ExecutionResult::Failed { error: error.clone() } },
        Outcome::Done { detail } => ActionEvent::Executed { at_ms: now, action, result: ExecutionResult::Succeeded { detail: detail.clone() } },
    };
    log.append(&terminal).map_err(|e| format!("動作已有結果，但寫入結果日誌失敗：{e:#}"))?;
    Ok(outcome_message(&outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sister_hands::{ActionEvent, ActionSnapshot, ExecutionResult, Outcome, RefusalReason};

    #[test]
    fn outcome_copy_says_refused_failed_and_done_without_cross_talking() {
        let refused = outcome_message(&Outcome::Refused {
            reason: RefusalReason::ObserveHasNoHands,
        });
        assert!(refused.contains("沒有動手"));
        assert!(!refused.contains("失敗"));
        assert!(!refused.contains("完成"));

        let failed = outcome_message(&Outcome::Failed { error: "找不到".into() });
        assert!(failed.contains("失敗"));
        assert!(!failed.contains("沒有動手"));
        assert!(!failed.contains("完成"));

        let done = outcome_message(&Outcome::Done { detail: "已接受".into() });
        assert!(done.contains("完成"));
        assert!(!done.contains("沒有動手"));
        assert!(!done.contains("失敗"));
    }

    #[test]
    fn replay_copy_names_bad_line_numbers_and_does_not_count_events_as_actions() {
        let replay = sister_hands::Replay {
            events: vec![
                ActionEvent::Proposed { at_ms: 1, action: ActionSnapshot::OpenUrl { url: "https://a".into() } },
                ActionEvent::Executed { at_ms: 2, action: ActionSnapshot::OpenUrl { url: "https://a".into() }, result: ExecutionResult::Succeeded { detail: "ok".into() } },
            ],
            unreadable: vec![sister_hands::UnreadableLine { line_no: 3, why: "bad json".into() }],
        };
        let lines = replay_lines(&replay);
        assert!(lines.iter().any(|line| line.contains("第 3 列") && line.contains("讀不懂")));
        assert!(!lines.iter().any(|line| line.contains("做了 2 件事")));
    }

    /// 只顯示最近幾列的時候，被蓋掉的那幾列要有人講。
    #[test]
    fn a_truncated_action_log_says_how_many_it_is_not_showing() {
        let replay = sister_hands::Replay {
            events: (1..=5)
                .map(|n| ActionEvent::Proposed {
                    at_ms: n,
                    action: ActionSnapshot::OpenUrl { url: format!("https://{n}") },
                })
                .collect(),
            unreadable: vec![],
        };
        let lines = recent_replay_lines(&replay, 2);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("更早的 3 列"), "{lines:?}");
        assert!(lines[1].contains("https://4") && lines[2].contains("https://5"), "{lines:?}");
        // 沒有超過上限的時候不要憑空多一句「更早的 0 列」。
        assert_eq!(recent_replay_lines(&replay, 9).len(), 5);
    }
}
