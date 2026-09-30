//! 接手（takeover）的提議、回答，以及重開之後讀得回來的狀態。
//!
//! **狀態全部寫在 `action-log.jsonl`，和它底下那一輪同一份檔案。** `sister forget`、
//! 匯出與保留期本來就管得到那個檔案；另開一個狀態檔的話，每一條刪除路都要各自
//! 再接一次，而漏接的那一條會讓「已經忘掉了」變成假話。
//!
//! 一次接手在紀錄上的樣子：
//!
//! ```text
//! handoff_offered   她端出來的範圍（授權書）＋她不接的那幾件
//! handoff_answered  他的回答；說好的話帶著底下那一輪的 run_id
//! granted           （只有說好才有）同一張授權書，這一輪的開頭
//! …                 每一步
//! concluded／aborted
//! ```
//!
//! 行程在任何一列之後被殺掉，檔案裡留下的都是這一串的某個前綴。所以「重開之後
//! 是什麼狀態」只看兩件事：最後一次提議走到哪一列，以及現在有沒有行程還拿著
//! 接手鎖（有就是還在跑，沒有就是中斷了）。

use crate::semi_action::{Grant, RunConclusionRecord};
use crate::{ActionEvent, ActionSnapshot, RefusalReason, Replay};
use serde::{Deserialize, Serialize};

/// 一次提議從端出來到他回答，最多等這麼久。
///
/// 也是那張授權書的期限：他答「好」之後才開始的步驟，仍然要在這個時間內做完。
/// 三小時後回來順手打一個「好」，那時候她看過的畫面早就不是現在的畫面了
/// （SPEC §9.5：接手前的預檢不能拿過期的東西充數）。
pub const OFFER_WINDOW_MS: u64 = 10 * 60 * 1_000;

/// 提議裡她不接的那一件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftOut {
    pub commitment_id: i64,
    pub text: String,
    /// `None`：那張卡的下一步讀不懂，沒有動作可以記。
    pub action: Option<ActionSnapshot>,
    pub why: LeftOutWhy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "left_out", rename_all = "snake_case")]
pub enum LeftOutWhy {
    /// 下一步那一欄讀不懂。
    NextStepUnreadable { reason: String },
    /// 真的跑下去，這一步會在交給作業系統之前被擋。
    ///
    /// 用的是執行隘口那一列 `refused` 的**同一個型別**：提議上說「這件我不接」的
    /// 理由，和她真的跑到那一步時會寫下的理由，不可以是兩套說法。
    WouldBeRefused { reason: RefusalReason },
    /// 這張卡的這一步，先前一輪憑綁著這張卡的授權書已經做成過（`step_finished`）。
    ///
    /// 卡還活著是因為整理承諾的那一段還沒看到後續；再開一次同一個網址不會讓事情
    /// 更完成，只會讓他以為她在原地打轉。
    AlreadyDone { at_ms: i64 },
}

impl LeftOutWhy {
    pub fn message(&self) -> String {
        match self {
            Self::NextStepUnreadable { reason } => format!("下一步讀不懂：{reason}"),
            Self::WouldBeRefused { reason } => reason.message(),
            Self::AlreadyDone { at_ms } => {
                format!("這一步 {} 已經做過了", crate::replay_copy::at(*at_ms))
            }
        }
    }
}

/// 這張卡的這一步最後一次被做成是什麼時候。沒做成過就是 `None`。
///
/// 只認**綁著這張卡**的授權書底下的 `step_finished`：`sister do --use-grant`
/// 那一張綁一張卡，接手那一張綁端出去的每一張。當場按的那種授權書沒有綁卡，
/// 那幾輪做的事不算在任何一張卡上——同一個網址不等於同一個承諾。
pub fn step_done_at(replay: &Replay, commitment_id: i64, action: &ActionSnapshot) -> Option<i64> {
    let mut grant: Option<&Grant> = None;
    let mut done_at = None;
    for event in &replay.events {
        match event {
            ActionEvent::Granted { grant: this, .. } => grant = Some(this),
            ActionEvent::StepFinished {
                at_ms,
                action: finished,
                ..
            } if finished == action
                && grant.is_some_and(|grant| {
                    grant
                        .approved_commitments()
                        .any(|approved| approved.id == commitment_id && approved.action == *action)
                }) =>
            {
                done_at = Some(*at_ms);
            }
            _ => {}
        }
    }
    done_at
}

/// 他對一次提議的回答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum HandoffAnswer {
    /// 他說好。緊接著的 `granted` 帶同一個 `run_id`。
    Accepted { run_id: String },
    /// 他說不要。
    Declined,
    /// 他還沒回答，輸入就結束了。什麼都沒做。
    NoAnswer,
    /// 他答「好」的時候提議已經過期。什麼都沒做。
    TooLate { answered_after_ms: u64 },
    /// 他答「好」的時候，時鐘比提議端出來的那一刻還早，期限沒辦法驗。什麼都沒做。
    ClockWentBack,
}

/// 產生一次提議的關聯 ID。和 run ID 一樣不是授權票，只是讓提議、回答和底下
/// 那一輪對得回同一件事。
pub fn new_handoff_id(offered_at_ms: i64) -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(b"AI-Sister takeover handoff v1\0");
    hasher.update(offered_at_ms.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(nonce.to_le_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(32 + "handoff-sha256:".len());
    out.push_str("handoff-sha256:");
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

/// 紀錄裡最後一次提議，以及它走到了哪裡。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRecord<'a> {
    pub handoff_id: &'a str,
    /// `None`：提議那一列不在了（`sister forget` 只刪到一半）。
    pub offered: Option<Offered<'a>>,
    /// `None`：沒有回答那一列。
    pub answer: Option<(i64, &'a HandoffAnswer)>,
    /// 他說好之後那一輪，從 `granted` 起。只有說好、而且找得到開頭才有。
    pub run: Option<Vec<&'a ActionEvent>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offered<'a> {
    pub at_ms: i64,
    pub grant: &'a Grant,
    pub left_out: &'a [LeftOut],
}

/// 最後一次提議現在的狀態。
///
/// **「還在跑」和「中斷了」從紀錄上分不出來**——兩者都是一串沒有收尾的列。分得出來
/// 的只有接手鎖：有行程拿著它就是還在跑。所以這裡要呼叫端把那個答案傳進來，而不是
/// 自己猜一個。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffState {
    /// 提議端出去了，另一個行程還在等他回答。
    AwaitingAnswer {
        offered_at_ms: Option<i64>,
    },
    /// 提議端出去了，沒有回答，也已經沒有行程在等：那個行程在他回答之前就結束了。
    NeverAnswered {
        offered_at_ms: Option<i64>,
    },
    Declined {
        at_ms: i64,
    },
    NoAnswer {
        at_ms: i64,
    },
    TooLate {
        at_ms: i64,
        answered_after_ms: u64,
    },
    ClockWentBack {
        at_ms: i64,
    },
    /// 他說好，那一輪還在跑。
    Running {
        completed_steps: u32,
    },
    /// 他說好，那一輪沒有收尾，而且已經沒有行程在跑它。
    Interrupted {
        completed_steps: u32,
        last_at_ms: i64,
    },
    /// 他說好，那一輪收尾了。
    Finished {
        end: RunEnd,
    },
}

/// 那一輪怎麼收尾的。兩種列各自原封不動，句子由它們自己的 `message` 講。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    Concluded {
        at_ms: i64,
        conclusion: RunConclusionRecord,
    },
    Aborted {
        at_ms: i64,
        after_completed_steps: u32,
        by: crate::semi_action::AbortActor,
    },
}

impl<'a> HandoffRecord<'a> {
    /// 說好之後那一輪做完了幾步（`step_finished` 的列數）。沒有那一輪就是 0。
    pub fn completed_steps(&self) -> u32 {
        let count = self.run.as_deref().map_or(0, |run| {
            run.iter()
                .filter(|event| matches!(event, ActionEvent::StepFinished { .. }))
                .count()
        });
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    pub fn state(&self, a_takeover_is_running: bool) -> HandoffState {
        let offered_at_ms = self.offered.map(|offered| offered.at_ms);
        let Some((at_ms, answer)) = self.answer else {
            return if a_takeover_is_running {
                HandoffState::AwaitingAnswer { offered_at_ms }
            } else {
                HandoffState::NeverAnswered { offered_at_ms }
            };
        };
        match answer {
            HandoffAnswer::Declined => HandoffState::Declined { at_ms },
            HandoffAnswer::NoAnswer => HandoffState::NoAnswer { at_ms },
            HandoffAnswer::TooLate { answered_after_ms } => HandoffState::TooLate {
                at_ms,
                answered_after_ms: *answered_after_ms,
            },
            HandoffAnswer::ClockWentBack => HandoffState::ClockWentBack { at_ms },
            HandoffAnswer::Accepted { .. } => {
                let run = self.run.as_deref().unwrap_or(&[]);
                let end = run.iter().rev().find_map(|event| match event {
                    ActionEvent::Concluded { at_ms, conclusion } => Some(RunEnd::Concluded {
                        at_ms: *at_ms,
                        conclusion: *conclusion,
                    }),
                    ActionEvent::Aborted {
                        at_ms,
                        after_completed_steps,
                        by,
                    } => Some(RunEnd::Aborted {
                        at_ms: *at_ms,
                        after_completed_steps: *after_completed_steps,
                        by: *by,
                    }),
                    _ => None,
                });
                if let Some(end) = end {
                    return HandoffState::Finished { end };
                }
                let completed_steps = self.completed_steps();
                if a_takeover_is_running {
                    HandoffState::Running { completed_steps }
                } else {
                    // 說好之後連 `granted` 都還沒寫就停了的話，最後一列是回答那一列。
                    let last_at_ms = run.last().map_or(at_ms, |event| event.at_ms());
                    HandoffState::Interrupted {
                        completed_steps,
                        last_at_ms,
                    }
                }
            }
        }
    }
}

/// 紀錄裡最後一次提議。一次都沒有提議過就回 `None`。
///
/// 以 ID 對，不以位置對：`sister forget` 可能只刪掉提議那一列、留下回答那一列，
/// 那時候「最後一列提議」是更早的另一次，而最後一次其實是只剩回答的那一個。
pub fn last_handoff(replay: &Replay) -> Option<HandoffRecord<'_>> {
    let (last_index, handoff_id) =
        replay
            .events
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, event)| match event {
                ActionEvent::HandoffOffered { handoff_id, .. }
                | ActionEvent::HandoffAnswered { handoff_id, .. } => {
                    Some((index, handoff_id.as_str()))
                }
                _ => None,
            })?;
    let mut offered = None;
    let mut answer = None;
    let mut answer_index = None;
    for (index, event) in replay.events.iter().enumerate().take(last_index + 1) {
        match event {
            ActionEvent::HandoffOffered {
                at_ms,
                handoff_id: id,
                grant,
                left_out,
            } if id == handoff_id => {
                offered = Some(Offered {
                    at_ms: *at_ms,
                    grant,
                    left_out,
                });
            }
            ActionEvent::HandoffAnswered {
                at_ms,
                handoff_id: id,
                answer: given,
            } if id == handoff_id => {
                answer = Some((*at_ms, given));
                answer_index = Some(index);
            }
            _ => {}
        }
    }
    let run = match (answer, answer_index) {
        (Some((_, HandoffAnswer::Accepted { run_id })), Some(index)) => {
            run_after(&replay.events[index + 1..], run_id)
        }
        _ => None,
    };
    Some(HandoffRecord {
        handoff_id,
        offered,
        answer,
        run,
    })
}

/// 回答那一列之後，`run_id` 那一輪的列：從它的 `granted` 到收尾，或到下一輪的邊界。
fn run_after<'a>(events: &'a [ActionEvent], run_id: &str) -> Option<Vec<&'a ActionEvent>> {
    let start = events.iter().position(|event| {
        matches!(
            event,
            ActionEvent::Granted { run_id: Some(id), .. } if id == run_id
        )
    })?;
    let mut run = Vec::new();
    for event in &events[start..] {
        if !run.is_empty()
            && matches!(
                event,
                ActionEvent::Granted { .. } | ActionEvent::HandoffOffered { .. }
            )
        {
            break;
        }
        run.push(event);
        if matches!(
            event,
            ActionEvent::Concluded { .. } | ActionEvent::Aborted { .. }
        ) {
            break;
        }
    }
    Some(run)
}

/// 接手鎖的檔名。空檔案，裡面什麼都不寫；有意義的只有「現在有沒有行程鎖著它」。
pub const LOCK_FILE: &str = "takeover.lock";

/// 一個 `sister takeover` 行程從提議到收尾一直拿著的鎖。放掉（drop）就解開；
/// 行程被殺掉的時候作業系統替它解開，所以不會留下一把沒有人拿著的鎖。
#[derive(Debug)]
pub struct TakeoverLock {
    _file: std::fs::File,
}

#[derive(Debug)]
pub enum LockAttempt {
    Acquired(TakeoverLock),
    /// 另一個行程拿著它：另一個接手還在等回答，或還在跑。
    HeldByAnother,
}

/// 另一邊只是**看一眼**（`is_running` 會短暫拿一把共享鎖）的時候，這邊的獨佔鎖
/// 會撞上它。那一眼只有幾毫秒；真的在跑的接手會拿著好幾分鐘。所以撞到的時候
/// 在這段時間裡重試，過了還拿不到才算「另一個接手在跑」。
const LOCK_RETRY_FOR_MS: u64 = 200;
const LOCK_RETRY_EVERY_MS: u64 = 10;

/// 拿接手鎖。拿不到的時候不等——另一個接手可能要跑好幾分鐘，排在它後面等就是
/// 讓第二個提議在第一個收尾之後才冒出來，而那時候他早就不在終端機前了。
pub fn try_lock(data_dir: &std::path::Path) -> std::io::Result<LockAttempt> {
    use fs4::{FileExt, TryLockError};
    let file = crate::master_stop::open_lock(data_dir, LOCK_FILE)?;
    let mut waited_ms = 0;
    loop {
        match FileExt::try_lock(&file) {
            Ok(()) => return Ok(LockAttempt::Acquired(TakeoverLock { _file: file })),
            Err(TryLockError::WouldBlock) if waited_ms < LOCK_RETRY_FOR_MS => {
                std::thread::sleep(std::time::Duration::from_millis(LOCK_RETRY_EVERY_MS));
                waited_ms += LOCK_RETRY_EVERY_MS;
            }
            Err(TryLockError::WouldBlock) => return Ok(LockAttempt::HeldByAnother),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// 現在有沒有一個接手行程拿著鎖。
///
/// 鎖檔不存在＝從來沒有接手行程在這個資料目錄跑過，當然也沒有在跑。其餘讀不到的
/// 情況回錯誤，**不回 `false`**：「讀不到」講成「沒有在跑」，會把一個正在跑的
/// 接手報成「中斷了」。
pub fn is_running(data_dir: &std::path::Path) -> std::io::Result<bool> {
    use fs4::{FileExt, TryLockError};
    let file = match crate::master_stop::open_existing_lock(data_dir, LOCK_FILE) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok(false)
        }
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}
