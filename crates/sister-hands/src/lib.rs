//! Phase 6 的平台無關 hands 邊界。
//!
//! 目前有 `observe`、必須由按鈕觸發的 `suggest`，以及受 grant 和逐步核准約束的
//! `semi-action`。真正開 URL、檔案或聚焦視窗的平台接線由呼叫端實作 [`Executor`]。
//!
//! **這個 crate 不依賴 `sister-core`，`sister-core` 也不依賴它。** hands 在
//! SPEC §9 裡是物理隔離的 sidecar；讓記憶那一層編進行動那一層，是把隔離
//! 寫成一句文件上的話。承諾卡的 `allowed_next_step` 是一串字，解析它不需要
//! 認識 `CommitmentRow`——見 [`commitment_action`]。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

pub mod commitment_action;
pub mod semi_action;
pub mod target_policy;

/// 權限階梯（SPEC §9.1）。
///
/// `takeover` 尚未實作，因此不先放進來。
///
/// **這個 enum 沒有 `Default`，而且不要替它加一個。** 字母人那一支唯一的
/// 呼叫端寫死 [`Self::Suggest`]，因為那條路上會走到 [`execute_with`] 的只有
/// 「使用者按了按鈕」這一種事——`Suggestion` 拿不到 [`UserButtonPress`] 就
/// 建不出來。在這裡寫「預設是 observe」而呼叫端傳 suggest，是在文件上立一
/// 條沒有人守的規矩。真的要一個「連按了也不准動」的開關的時候，那是 config
/// 裡的一個欄位，不是這裡的一句話。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 物理上沒有手——[`execute_with`] 在這一級一律拒絕。
    Observe,
    /// 可開 URL／檔案／聚焦視窗，且**僅限使用者點按鈕觸發**。
    Suggest,
    /// 結構化任務授權 + 綁定具體內容的逐步核准。
    SemiAction,
}

/// 只能由 UI 的 suggestion 按鈕 click handler 鑄出的憑證。
///
/// 私有欄位使模型輸出、L0 文字和一般資料解析不能自己拼出這張票。
#[derive(Debug, PartialEq, Eq)]
pub struct UserButtonPress(());

impl UserButtonPress {
    fn from_button_click() -> Self {
        Self(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Suggestion {
    OpenUrl {
        url: String,
        pressed: UserButtonPress,
    },
    OpenFile {
        path: PathBuf,
        pressed: UserButtonPress,
    },
    FocusWindow {
        title: String,
        pressed: UserButtonPress,
    },
}

impl Suggestion {
    pub fn open_url(pressed: UserButtonPress, url: String) -> Self {
        Self::OpenUrl { url, pressed }
    }

    pub fn open_file(pressed: UserButtonPress, path: PathBuf) -> Self {
        Self::OpenFile { path, pressed }
    }

    pub fn focus_window(pressed: UserButtonPress, title: String) -> Self {
        Self::FocusWindow { title, pressed }
    }

    /// 具體動作 + 具體目標（SPEC §9.7：核准綁「把 A 檔上傳到 B 表單」，
    /// 不綁「幫我處理這件事」）。
    ///
    /// 文案只有 [`ActionSnapshot::describe`] 那一份。按鈕上寫的、action log
    /// 裡記的、和真的做出去的，必須是同一句話——分成兩份的那一天，畫面上
    /// 承諾的和日誌裡記下的會是兩件事，而兩邊各自都是真的。
    pub fn describe(&self) -> String {
        self.snapshot().describe()
    }

    pub fn snapshot(&self) -> ActionSnapshot {
        match self {
            Self::OpenUrl { url, .. } => ActionSnapshot::OpenUrl { url: url.clone() },
            Self::OpenFile { path, .. } => ActionSnapshot::OpenFile { path: path.clone() },
            Self::FocusWindow { title, .. } => ActionSnapshot::FocusWindow {
                title: title.clone(),
            },
        }
    }
}

/// 即使未實作也不得由任務授權繼承的五類權限（SPEC §9.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeverInherited {
    Submit,
    Publish,
    Pay,
    Delete,
    OpenTerminal,
}

impl NeverInherited {
    pub const ALL: [Self; 5] = [
        Self::Submit,
        Self::Publish,
        Self::Pay,
        Self::Delete,
        Self::OpenTerminal,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Submit => "送出",
            Self::Publish => "發布",
            Self::Pay => "付款",
            Self::Delete => "刪除",
            Self::OpenTerminal => "開 terminal",
        }
    }
}

/// 這個動作落在永不繼承的哪一類——沒有的話回 `None`。
///
/// 沒有 `_`：新增任何 suggestion 都必須在編譯期回答它是否落入那五類。
/// 這比一個 runtime 檢查有用，因為 runtime 檢查是可以忘記呼叫的。
///
/// 回 `Option<NeverInherited>` 而不是 `bool`，是因為呼叫端要說得出**是哪一類**
/// ——「因為這是付款」和「因為這是刪除」是使用者要看到的兩句不同的話。
pub const fn never_inherited_class(suggestion: &Suggestion) -> Option<NeverInherited> {
    match suggestion {
        Suggestion::OpenUrl { .. } => None,
        Suggestion::OpenFile { .. } => None,
        Suggestion::FocusWindow { .. } => None,
    }
}

pub const fn is_never_inherited(suggestion: &Suggestion) -> bool {
    never_inherited_class(suggestion).is_some()
}

/// 尚未被人按下的 suggestion 按鈕；解析模型產生的欄位最多只能得到這個型別。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionButton(SuggestionDraft);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum SuggestionDraft {
    OpenUrl { url: String },
    OpenFile { path: PathBuf },
    FocusWindow { title: String },
}

impl SuggestionButton {
    pub fn parse_json(value: &str) -> serde_json::Result<Self> {
        serde_json::from_str(value).map(Self)
    }

    /// 按鈕上要寫的字。和按下去之後 [`Suggestion::describe`] 講的是同一句。
    pub fn describe(&self) -> String {
        self.snapshot().describe()
    }

    fn snapshot(&self) -> ActionSnapshot {
        match &self.0 {
            SuggestionDraft::OpenUrl { url } => ActionSnapshot::OpenUrl { url: url.clone() },
            SuggestionDraft::OpenFile { path } => ActionSnapshot::OpenFile { path: path.clone() },
            SuggestionDraft::FocusWindow { title } => ActionSnapshot::FocusWindow {
                title: title.clone(),
            },
        }
    }

    /// UI click handler 的唯一入口；憑證在這裡鑄出，外部不能直接建構。
    pub fn press(self) -> Suggestion {
        let pressed = UserButtonPress::from_button_click();
        match self.0 {
            SuggestionDraft::OpenUrl { url } => Suggestion::open_url(pressed, url),
            SuggestionDraft::OpenFile { path } => Suggestion::open_file(pressed, path),
            SuggestionDraft::FocusWindow { title } => Suggestion::focus_window(pressed, title),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionSnapshot {
    OpenUrl { url: String },
    OpenFile { path: PathBuf },
    FocusWindow { title: String },
}

impl ActionSnapshot {
    /// 全 crate 唯一一份動作文案。
    pub fn describe(&self) -> String {
        match self {
            Self::OpenUrl { url } => format!("開啟網址：{url}"),
            Self::OpenFile { path } => format!("開啟檔案：{}", path.display()),
            Self::FocusWindow { title } => format!("聚焦視窗：{title}"),
        }
    }
}

/// 為什麼**根本沒有交給作業系統**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum RefusalReason {
    /// `observe` 級物理上沒有手。
    ObserveHasNoHands,
    /// 落在永不繼承的五類裡，要單獨核准（SPEC §9.2）。
    NeverInherited { class: NeverInherited },
    /// `semi-action` 必須走會讀 grant 與步級核准的隘口。
    SemiActionNeedsGrantAndStepApproval,
    /// grant 的四維 scope 裡有一維不涵蓋這一步（SPEC §9.1）。
    NotCoveredByGrant {
        rejection: semi_action::GrantRejection,
    },
    /// 手上那張核准票是對**另一步**簽的（SPEC §9.7）。
    ApprovalWasForAnotherStep {
        mismatch: semi_action::ApprovalMismatch,
    },
}

impl RefusalReason {
    pub fn message(&self) -> String {
        match self {
            Self::ObserveHasNoHands => {
                "現在是 observe 級，她沒有手；要她動手得先把權限升到 suggest。".to_string()
            }
            Self::NeverInherited { class } => format!(
                "「{}」不隨任務授權繼承，每一次都要單獨核准——這一步沒有做。",
                class.name()
            ),
            Self::SemiActionNeedsGrantAndStepApproval => {
                "semi-action 需要結構化 grant 和顯示的那一步核准；不可走 suggest 隘口。".to_string()
            }
            Self::NotCoveredByGrant { rejection } => rejection.message().to_string(),
            Self::ApprovalWasForAnotherStep { mismatch } => mismatch.message(),
        }
    }
}

/// 一次 [`execute_with`] 的結局。**三種，不是兩種。**
///
/// 「她不肯做」和「她做了但失敗了」在 action log 上長得一樣的那一天，
/// 回放的人會把一次被擋下來的付款讀成一次失敗的付款。前者作業系統
/// 從頭到尾沒有被碰過，後者碰過了而且不知道碰到哪一步。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Refused { reason: RefusalReason },
    Failed { error: String },
    Done { detail: String },
}

/// 平台呼叫端提供實作者；測試只放 fake，不會真的開瀏覽器或視窗。
pub trait Executor {
    fn execute(&mut self, suggestion: &Suggestion) -> std::result::Result<String, String>;
}

/// 全部執行請求的唯一隘口。
///
/// [`Level`] 和 [`never_inherited_class`] 都在這裡被讀——不然它們就只是兩個
/// 寫出來沒有人看的型別，而這個 repo 有一整排那種東西的墓碑
/// （`system_events`、`secret_suspected`，都當過「寫進去然後沒人讀」的欄位
/// 好幾個月：不會報錯、測試全綠、文件照樣承諾使用者查得到）。
pub fn execute_with(
    level: Level,
    executor: &mut impl Executor,
    suggestion: &Suggestion,
) -> Outcome {
    match level {
        Level::Observe => {
            return Outcome::Refused {
                reason: RefusalReason::ObserveHasNoHands,
            };
        }
        Level::Suggest => {}
        Level::SemiAction => {
            return Outcome::Refused {
                reason: RefusalReason::SemiActionNeedsGrantAndStepApproval,
            };
        }
    }
    // 今天走不到：`suggest` 的三種動作都不在那五類裡。留著是因為下一個人
    // 加第四種動作時，`never_inherited_class` 會**編譯錯誤**逼他回答，
    // 而他答「是」的那一刻，這裡就自動擋下來了——不必他記得加一道檢查。
    if let Some(class) = never_inherited_class(suggestion) {
        return Outcome::Refused {
            reason: RefusalReason::NeverInherited { class },
        };
    }
    match executor.execute(suggestion) {
        Ok(detail) => Outcome::Done { detail },
        Err(error) => Outcome::Failed { error },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExecutionResult {
    Succeeded { detail: String },
    Failed { error: String },
}

/// 每列都重複完整 action 與時間，因此可脫離前一列單獨解讀（SPEC §9.3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ActionEvent {
    Proposed {
        at_ms: i64,
        action: ActionSnapshot,
    },
    Approved {
        at_ms: i64,
        action: ActionSnapshot,
    },
    /// 交給作業系統了。成功或失敗在 `result` 裡。
    Executed {
        at_ms: i64,
        action: ActionSnapshot,
        result: ExecutionResult,
    },
    /// **沒有**交給作業系統。不是 `Executed` 的一種——它連試都沒試。
    Refused {
        at_ms: i64,
        action: ActionSnapshot,
        reason: RefusalReason,
    },
    /// 一步做完後的畫面憑據；`None` 明確表示沒有取得，不表示驗證成功。
    StepFinished {
        at_ms: i64,
        step_number: u32,
        action: ActionSnapshot,
        evidence: Option<semi_action::ScreenEvidenceRef>,
    },
    /// 硬中止不回滾先前步驟，並記錄停在哪與誰喊停。
    Aborted {
        at_ms: i64,
        after_completed_steps: u32,
        by: semi_action::AbortActor,
    },
    Concluded {
        at_ms: i64,
        conclusion: semi_action::RunConclusionRecord,
    },
}

impl ActionEvent {
    /// 這一列發生在什麼時候。
    ///
    /// **沒有 `_` 萬用臂，而且不要替它加一個。** 這支是 [`ActionLog::forget_range`]
    /// 判斷「這列在不在他要忘掉的範圍裡」唯一的依據；一個新的事件種類如果沒有
    /// 在這裡被回答，正確的結果是編譯錯誤，不是安靜地回一個 0——那個 0 會讓那
    /// 一列永遠落在每一個範圍之外，於是它記的那串網址永遠忘不掉。
    pub fn at_ms(&self) -> i64 {
        match self {
            Self::Proposed { at_ms, .. }
            | Self::Approved { at_ms, .. }
            | Self::Executed { at_ms, .. }
            | Self::Refused { at_ms, .. }
            | Self::StepFinished { at_ms, .. }
            | Self::Aborted { at_ms, .. }
            | Self::Concluded { at_ms, .. } => *at_ms,
        }
    }
}

/// 一列讀不懂的 log。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableLine {
    /// 1-indexed，跟人看檔案時的行號一樣。
    pub line_no: usize,
    pub why: String,
}

/// 回放的結果。
///
/// **讀不懂的那幾列不會讓讀得懂的那幾列一起消失。** 一支 audit trail 如果
/// 中間一列壞掉就整份回 `Err`，那使用者問「她到底做了什麼」的時候會得到
/// 「不知道」，而其實有九列好好地躺在那裡。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Replay {
    pub events: Vec<ActionEvent>,
    /// 空的代表**每一列都讀得懂**，不是代表沒事發生。
    pub unreadable: Vec<UnreadableLine>,
}

pub struct ActionLog {
    path: PathBuf,
}

impl ActionLog {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::open(data_dir.join("action-log.jsonl"))
    }

    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &ActionEvent) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("建立 action log 目錄失敗：{}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("開啟 action log 失敗：{}", self.path.display()))?;
        serde_json::to_writer(&mut file, event).context("序列化 action log 失敗")?;
        file.write_all(b"\n").context("寫入 action log 失敗")?;
        file.sync_data().context("同步 action log 失敗")?;
        Ok(())
    }

    /// 讀回自己寫的那個檔案。
    ///
    /// 不收 path 參數：檔名只有 [`Self::in_data_dir`] 那一處寫死，讓每個
    /// 呼叫端各自再拼一次 `"action-log.jsonl"` 就是等著哪天兩邊拼得不一樣。
    pub fn replay(&self) -> Result<Replay> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            // 檔案還沒生出來 = 一次動作都沒有提出過。這是空的，不是壞的。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Replay::default()),
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("開啟 action log 失敗：{}", self.path.display()));
            }
        };
        let mut out = Replay::default();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line_no = index + 1;
            let line = match line {
                Ok(line) => line,
                Err(e) => {
                    out.unreadable.push(UnreadableLine {
                        line_no,
                        why: e.to_string(),
                    });
                    continue;
                }
            };
            match serde_json::from_str(&line) {
                Ok(event) => out.events.push(event),
                Err(e) => out.unreadable.push(UnreadableLine {
                    line_no,
                    why: e.to_string(),
                }),
            }
        }
        Ok(out)
    }

    /// 他按下「忘掉這一段」之後，這個檔案裡那一段的字也要不見。
    ///
    /// **這裡刪的是字本身，不是蓋一個旗標。** 這個 repo 有過「留著字的墓碑」
    /// 的前例：軟刪除把列標成已刪、內容原封不動留在磁碟上，而畫面上寫「已忘掉」。
    /// action log 是純文字 JSONL，裡面有完整的網址和檔案路徑——`sister.db` 那
    /// 一半清乾淨了而這個檔案沒有，等於那句「忘掉了」只對一半的磁碟成立。
    ///
    /// 範圍和 `Db::forget` 同一個慣例：`[from_ms, to_ms)`，左閉右開。
    ///
    /// **讀不懂的列一律刪掉。** 它問不出時間，所以沒有辦法判斷在不在範圍內；
    /// 而兩種錯法不對等——留著它可能留下他剛剛要求忘掉的那串網址（而且畫面上
    /// 讀不出來，沒有人會發現），刪掉它失去的是一列本來就顯示不出內容的字。
    /// 這跟排除規則「一律偏向多擋」是同一個取捨。刪了幾列會分開回報，因為
    /// 「忘掉了 3 列」和「忘掉了 3 列外加 1 列讀不懂的」不是同一句話。
    pub fn forget_range(&self, from_ms: i64, to_ms: i64) -> Result<ForgetReport> {
        let replay = self.replay()?;
        // 檔案不存在的時候 `replay()` 回一份空的，這裡就會是一份全 0 的報告，
        // 而且不會憑空把檔案生出來。
        if replay.events.is_empty() && replay.unreadable.is_empty() {
            return Ok(ForgetReport::default());
        }
        let mut report = ForgetReport {
            removed_unreadable: replay.unreadable.len() as u64,
            ..ForgetReport::default()
        };
        let mut kept = Vec::new();
        for event in &replay.events {
            if (from_ms..to_ms).contains(&event.at_ms()) {
                report.removed_in_range += 1;
            } else {
                kept.push(event);
            }
        }
        report.kept = kept.len() as u64;
        if report.removed_in_range == 0 && report.removed_unreadable == 0 {
            // 沒有東西要刪就不要重寫檔案——重寫一次就是多一次「寫到一半斷電」
            // 的機會，而這一趟本來什麼都不必做。
            return Ok(report);
        }
        // 先寫暫存檔再 rename。原地截斷再寫的話，中途斷電會留下一個被砍掉一半
        // 的 log；rename 在同一個檔案系統上是原子的，要嘛全新的、要嘛全舊的。
        let tmp = self.path.with_extension("jsonl.forgetting");
        {
            let mut file = File::create(&tmp)
                .with_context(|| format!("建立暫存 action log 失敗：{}", tmp.display()))?;
            for event in kept {
                serde_json::to_writer(&mut file, event).context("序列化 action log 失敗")?;
                file.write_all(b"\n").context("寫入 action log 失敗")?;
            }
            file.sync_data().context("同步 action log 失敗")?;
        }
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("換掉 action log 失敗：{}", self.path.display()))?;
        Ok(report)
    }
}

/// 從 action log 拿掉了幾列。
///
/// 三個數字分開放，不折成一個總數：「忘掉了 3 列」和「忘掉了 3 列、外加 1 列
/// 讀不懂的也一起刪了」是兩句不同的話，而合成一個 `4` 之後沒有人講得回來。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForgetReport {
    /// 時間落在範圍裡，被刪掉的列數。
    pub removed_in_range: u64,
    /// 解不開、問不出時間，因此一併刪掉的列數。
    pub removed_unreadable: u64,
    /// 留下來的列數。
    pub kept: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn click() -> UserButtonPress {
        UserButtonPress::from_button_click()
    }

    #[test]
    fn descriptions_name_the_concrete_action_and_target() {
        let url = Suggestion::open_url(click(), "https://example.com/jobs/42".into());
        let text = url.describe();
        assert!(text.contains("開啟網址"));
        assert!(text.contains("https://example.com/jobs/42"));
        assert!(!text.contains("處理這件事"));

        let file = Suggestion::open_file(click(), PathBuf::from("C:/work/report.txt"));
        let text = file.describe();
        assert!(text.contains("開啟檔案"));
        assert!(text.contains("C:/work/report.txt"));
        assert!(!text.contains("某個檔案"));

        let window = Suggestion::focus_window(click(), "Visual Studio Code".into());
        let text = window.describe();
        assert!(text.contains("聚焦視窗"));
        assert!(text.contains("Visual Studio Code"));
        assert!(!text.contains("某個視窗"));
    }

    /// 按鈕上寫的字，和按下去之後真的做的那件事，必須是同一句。
    #[test]
    fn the_button_and_the_thing_it_does_read_the_same_sentence() {
        let button =
            SuggestionButton::parse_json(r#"{"action":"open_file","path":"C:/work/report.txt"}"#)
                .unwrap();
        let on_the_button = button.describe();
        let done = button.press().describe();
        assert_eq!(on_the_button, done);
        assert!(done.contains("C:/work/report.txt"));
    }

    #[test]
    fn never_inherited_names_and_current_suggestions_are_pinned() {
        assert_eq!(
            NeverInherited::ALL.map(NeverInherited::name),
            ["送出", "發布", "付款", "刪除", "開 terminal"]
        );
        assert!(!is_never_inherited(&Suggestion::open_url(
            click(),
            "https://example.com".into()
        )));
        assert!(!is_never_inherited(&Suggestion::open_file(
            click(),
            PathBuf::from("notes.txt")
        )));
        assert!(!is_never_inherited(&Suggestion::focus_window(
            click(),
            "Editor".into()
        )));
    }

    /// 模型輸出解析得出來的是**按鈕**，不是動作。中間隔著一次人按下去。
    #[test]
    fn parsed_json_cannot_become_an_executable_suggestion_without_a_press() {
        let button =
            SuggestionButton::parse_json(r#"{"action":"open_url","url":"https://evil/x"}"#)
                .unwrap();
        // 多一個欄位就整個不收——半懂的指令不執行。
        assert!(
            SuggestionButton::parse_json(
                r#"{"action":"open_url","url":"https://evil/x","also_run":"rm -rf"}"#
            )
            .is_err()
        );
        let _needs_a_human = button.press();
    }

    struct Fake {
        calls: u32,
    }
    impl Executor for Fake {
        fn execute(&mut self, suggestion: &Suggestion) -> std::result::Result<String, String> {
            self.calls += 1;
            Ok(format!("fake: {}", suggestion.describe()))
        }
    }

    #[test]
    fn executor_is_replaceable_without_opening_anything() {
        let mut fake = Fake { calls: 0 };
        let suggestion = Suggestion::open_url(click(), "https://example.com".into());
        let Outcome::Done { detail } = execute_with(Level::Suggest, &mut fake, &suggestion) else {
            panic!("suggest 級的開網址要做得成");
        };
        assert!(detail.contains("fake:"));
        assert!(detail.contains("https://example.com"));
        assert!(!detail.contains("失敗"));
        assert_eq!(fake.calls, 1);
    }

    /// `observe` 級**物理上沒有手**，所以 executor 一次都不該被碰到。
    /// 只斷言「回了個 Refused」不夠：一個先做完再回 Refused 的實作也會過。
    #[test]
    fn observe_level_never_reaches_the_executor() {
        let mut fake = Fake { calls: 0 };
        let suggestion = Suggestion::open_url(click(), "https://example.com".into());
        let outcome = execute_with(Level::Observe, &mut fake, &suggestion);
        assert_eq!(fake.calls, 0, "observe 級不可以碰到 executor");
        let Outcome::Refused { reason } = outcome else {
            panic!("observe 級要拒絕");
        };
        assert_eq!(reason, RefusalReason::ObserveHasNoHands);
        let says = reason.message();
        assert!(says.contains("observe"));
        assert!(says.contains("沒有手"));
        // 「她不肯做」不可以講成「她做了但失敗了」。
        assert!(!says.contains("失敗"));
    }

    /// 拒絕、失敗、成功是**三句不一樣的話**。
    #[test]
    fn refused_failed_and_done_do_not_read_as_each_other() {
        let refused = RefusalReason::NeverInherited {
            class: NeverInherited::Pay,
        };
        let says = refused.message();
        assert!(says.contains("付款"));
        assert!(says.contains("單獨核准"));
        assert!(!says.contains("失敗"), "沒有試過的事不能講成失敗：{says}");
        assert!(!says.contains("完成"), "沒有試過的事不能講成完成：{says}");

        let observe = RefusalReason::ObserveHasNoHands.message();
        assert_ne!(observe, says);
    }

    #[test]
    fn action_log_replays_each_line_without_previous_context() {
        let proposed = ActionEvent::Proposed {
            at_ms: 10,
            action: ActionSnapshot::OpenUrl {
                url: "https://example.com".into(),
            },
        };
        let line = serde_json::to_string(&proposed).unwrap();
        assert!(line.contains("proposed"));
        assert!(line.contains("https://example.com"));
        assert!(!line.contains("approved"));
    }

    #[test]
    fn action_log_has_proposed_approved_failed_succeeded_and_refused_records() {
        let action = ActionSnapshot::FocusWindow {
            title: "Editor".into(),
        };
        let events = [
            ActionEvent::Proposed {
                at_ms: 1,
                action: action.clone(),
            },
            ActionEvent::Approved {
                at_ms: 2,
                action: action.clone(),
            },
            ActionEvent::Executed {
                at_ms: 3,
                action: action.clone(),
                result: ExecutionResult::Failed {
                    error: "找不到視窗".into(),
                },
            },
            ActionEvent::Executed {
                at_ms: 4,
                action: action.clone(),
                result: ExecutionResult::Succeeded {
                    detail: "已聚焦".into(),
                },
            },
            ActionEvent::Refused {
                at_ms: 5,
                action,
                reason: RefusalReason::ObserveHasNoHands,
            },
        ];
        let lines: Vec<String> = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        assert!(lines[0].contains("proposed"));
        assert!(!lines[0].contains("approved"));
        assert!(lines[1].contains("approved"));
        assert!(!lines[1].contains("succeeded"));
        assert!(lines[2].contains("failed"));
        assert!(!lines[2].contains("succeeded"));
        assert!(lines[3].contains("succeeded"));
        assert!(!lines[3].contains("failed"));
        // 被擋下來的那一列，回放的人不可以讀成一次失敗的嘗試。
        assert!(lines[4].contains("refused"));
        assert!(!lines[4].contains("failed"));
        assert!(!lines[4].contains("succeeded"));
        assert!(lines.iter().all(|line| line.contains("Editor")));
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sister-hands-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// 「忘掉那個下午」之後，那個下午的網址不可以還躺在磁碟上。
    ///
    /// 斷言故意打在**檔案的位元組**上，不是打在 `replay()` 的結果上。這個 repo
    /// 有過「留著字的墓碑」：軟刪除把列標成已刪，`replay()` 從此不回它，而內容
    /// 一個字都沒少地留在磁碟上。從 `replay()` 問「還在不在」，問的是讀取端肯
    /// 不肯給，不是那串字還在不在。
    #[test]
    fn forgetting_an_afternoon_takes_the_urls_out_of_the_file_not_just_out_of_the_replay() {
        let dir = tmp_dir("forget");
        let log = ActionLog::in_data_dir(&dir);
        let at = |ms: i64, url: &str| ActionEvent::Executed {
            at_ms: ms,
            action: ActionSnapshot::OpenUrl { url: url.into() },
            result: ExecutionResult::Succeeded {
                detail: "ok".into(),
            },
        };
        log.append(&at(1_000, "https://before.example")).unwrap();
        log.append(&at(5_000, "https://during.example")).unwrap();
        log.append(&at(9_000, "https://after.example")).unwrap();

        let report = log.forget_range(4_000, 8_000).unwrap();
        assert_eq!(
            report,
            ForgetReport {
                removed_in_range: 1,
                removed_unreadable: 0,
                kept: 2,
            },
        );

        let raw = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            !raw.contains("during.example"),
            "那個下午的網址還在檔案裡：{raw}",
        );
        assert!(
            raw.contains("before.example") && raw.contains("after.example"),
            "{raw}"
        );

        // 範圍是左閉右開，跟 `Db::forget` 同一個慣例：邊界上那一列屬於範圍。
        assert_eq!(log.forget_range(9_000, 9_001).unwrap().removed_in_range, 1);
        assert!(
            !std::fs::read_to_string(log.path())
                .unwrap()
                .contains("after.example")
        );

        // 沒有東西落在範圍裡的時候，不要動到留下來的那幾列。
        let untouched = std::fs::read_to_string(log.path()).unwrap();
        let report = log.forget_range(100_000, 200_000).unwrap();
        assert_eq!(
            report,
            ForgetReport {
                removed_in_range: 0,
                removed_unreadable: 0,
                kept: 1
            }
        );
        assert_eq!(std::fs::read_to_string(log.path()).unwrap(), untouched);
    }

    /// 讀不懂的那一列問不出時間，所以它一起走——而且要講出來它走了。
    #[test]
    fn a_line_we_cannot_read_cannot_prove_it_is_outside_the_range_so_it_goes_too() {
        let dir = tmp_dir("forget-bad");
        let log = ActionLog::in_data_dir(&dir);
        log.append(&ActionEvent::Executed {
            at_ms: 9_000,
            action: ActionSnapshot::OpenUrl {
                url: "https://keep.example".into(),
            },
            result: ExecutionResult::Succeeded {
                detail: "ok".into(),
            },
        })
        .unwrap();
        // 一列解不開、但裡面看得到一串網址的字。
        {
            use std::io::Write as _;
            let mut f = OpenOptions::new().append(true).open(log.path()).unwrap();
            writeln!(
                f,
                r#"{{"event":"from_a_future_version","url":"https://secret.example"}}"#
            )
            .unwrap();
        }
        let report = log.forget_range(0, 1_000).unwrap();
        assert_eq!(
            report,
            ForgetReport {
                removed_in_range: 0,
                removed_unreadable: 1,
                kept: 1,
            },
            "讀不懂的那一列要被算進去，而且不可以被算成 removed_in_range",
        );
        let raw = std::fs::read_to_string(log.path()).unwrap();
        assert!(!raw.contains("secret.example"), "{raw}");
        assert!(raw.contains("keep.example"), "{raw}");
    }

    /// 一次動作都沒有過的時候，忘掉一段時間不該生出一個空檔案。
    #[test]
    fn forgetting_when_she_never_did_anything_does_not_create_the_file() {
        let dir = tmp_dir("forget-empty");
        let log = ActionLog::in_data_dir(&dir);
        assert_eq!(
            log.forget_range(0, i64::MAX).unwrap(),
            ForgetReport::default()
        );
        assert!(!log.path().exists(), "憑空生出了一個 action log");
    }

    #[test]
    fn action_log_appends_jsonl_and_replays_all_rows() {
        let dir = tmp_dir("log");
        let log = ActionLog::in_data_dir(&dir);
        let action = ActionSnapshot::OpenFile {
            path: PathBuf::from("C:/work/report.txt"),
        };
        let proposed = ActionEvent::Proposed {
            at_ms: 100,
            action: action.clone(),
        };
        let executed = ActionEvent::Executed {
            at_ms: 120,
            action,
            result: ExecutionResult::Succeeded {
                detail: "作業系統已接受".into(),
            },
        };
        log.append(&proposed).unwrap();
        log.append(&executed).unwrap();

        let replay = log.replay().unwrap();
        assert_eq!(replay.events, vec![proposed, executed]);
        assert!(replay.unreadable.is_empty());
        let raw = std::fs::read_to_string(dir.join("action-log.jsonl")).unwrap();
        assert!(raw.contains("C:/work/report.txt"));
        assert!(raw.contains("succeeded"));
        assert!(!raw.contains("某個檔案"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 「一次都沒做過」和「日誌讀不出來」是兩件事。
    #[test]
    fn a_log_that_was_never_written_is_not_a_log_that_cannot_be_read() {
        let dir = tmp_dir("never");
        let never = ActionLog::in_data_dir(&dir).replay().unwrap();
        assert!(never.events.is_empty());
        assert!(
            never.unreadable.is_empty(),
            "還沒有檔案 = 沒做過任何事，不是有一列壞掉"
        );
    }

    /// 中間壞掉一列，不可以讓好的那幾列一起消失。
    #[test]
    fn one_broken_line_does_not_swallow_the_readable_ones() {
        let dir = tmp_dir("broken");
        let log = ActionLog::in_data_dir(&dir);
        log.append(&ActionEvent::Proposed {
            at_ms: 1,
            action: ActionSnapshot::OpenUrl {
                url: "https://a".into(),
            },
        })
        .unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap()
            .write_all(b"{ this is not json\n")
            .unwrap();
        log.append(&ActionEvent::Proposed {
            at_ms: 3,
            action: ActionSnapshot::OpenUrl {
                url: "https://c".into(),
            },
        })
        .unwrap();

        let replay = log.replay().unwrap();
        assert_eq!(replay.events.len(), 2, "讀得懂的兩列要還在");
        assert_eq!(replay.unreadable.len(), 1);
        assert_eq!(replay.unreadable[0].line_no, 2, "行號要指得到那一列");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
