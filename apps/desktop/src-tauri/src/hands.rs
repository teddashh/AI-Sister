use sister_hands::{ActionEvent, ActionLog, ExecutionResult, Level, Outcome};
use std::path::Path;

// 按下去之後回給他的那一句話搬進 `sister_hands::outcome_message` 了，理由和
// 底下 `recent_replay_lines` 那一段一樣：住在這裡的測試一列都不會被執行。
pub use sister_hands::outcome_message;

// 回放那段文案搬進 `sister_hands::replay_copy` 了，理由寫在那個模組開頭：CI 對
// 這個 workspace 只跑 clippy 和 build，所以住在這裡的測試一列都不會被執行。
// 這裡只接 `recent_replay_lines`——字母人那一格永遠是截斷過的那一種，
// 而 `replay_lines`（全量）的呼叫端是 `sister hands log`。
pub use sister_hands::replay_copy::recent_replay_lines;

pub use sister_hands::platform::PlatformExecutor;

pub fn execute_logged(data_dir: &Path, raw: &str, now: i64) -> Result<String, String> {
    let button = sister_hands::SuggestionButton::parse_json(raw)
        .map_err(|error| format!("按鈕內容已經讀不懂，沒有動手：{error}"))?;
    let suggestion = button.press();
    let action = suggestion.snapshot();
    let log = ActionLog::in_data_dir(data_dir);
    // **這裡只寫 `Approved`。** 提出是她把按鈕畫上去的那一刻，不是他按下去的
    // 這一刻；兩列都蓋上 `now` 的話，回放的人會看到一次零秒的思考，而那個零
    // 是我們沒有量到提出時間、不是他真的沒有猶豫。少一列比多一列假的好。
    log.append(&ActionEvent::Approved {
        at_ms: now,
        action: action.clone(),
        by: Some(sister_hands::ApprovedBy::Press),
    })
    .map_err(|e| format!("寫入 approved 失敗，沒有動手：{e:#}"))?;
    let mut executor = PlatformExecutor::new(data_dir);
    let outcome = sister_hands::execute_with(Level::Suggest, &mut executor, &suggestion);
    let terminal = match &outcome {
        Outcome::Refused { reason } => ActionEvent::Refused {
            at_ms: now,
            action,
            reason: reason.clone(),
        },
        Outcome::Failed { error } => ActionEvent::Executed {
            at_ms: now,
            action,
            result: ExecutionResult::Failed {
                error: error.clone(),
            },
        },
        Outcome::Done { detail } => ActionEvent::Executed {
            at_ms: now,
            action,
            result: ExecutionResult::Succeeded {
                detail: detail.clone(),
            },
        },
    };
    log.append(&terminal)
        .map_err(|e| format!("動作已有結果，但寫入結果日誌失敗：{e:#}"))?;
    Ok(outcome_message(&outcome))
}
