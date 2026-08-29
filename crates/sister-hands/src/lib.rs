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
pub mod kill_switch;
pub mod platform;
pub mod replay_copy;
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
    /// 可開 URL／檔案／聚焦視窗，且**僅限使用者當場親手答應**。
    ///
    /// alpha.69 這裡寫的是「僅限使用者點按鈕觸發」，那時候只有字母人上那顆
    /// 按鈕走得到。alpha.70 的 `sister do` 在終端機上讀一個「好」也走這一級，
    /// 於是「按鈕」變成假話。守住的那件事沒有變——見 [`UserButtonPress`]。
    Suggest,
    /// 結構化任務授權 + 綁定具體內容的逐步核准。
    SemiAction,
}

/// **人當場親手答應**這件事的憑證。
///
/// 私有欄位使模型輸出、L0 文字和一般資料解析不能自己拼出這張票——這是它唯一
/// 的職責，而且從 alpha.69 到今天沒有變。
///
/// 變的是「親手答應」長什麼樣子：alpha.69 只有字母人上那顆 suggestion 按鈕的
/// click handler，所以這段話當時寫的是「只能由 UI 的按鈕鑄出」；alpha.70 的
/// `sister do` 在終端機上讀一個「好」，走的是同一個 [`Suggestion::press`]。
/// 兩個都是人，都不是模型——名單會再長，型別上的那道牆不會鬆。
#[derive(Debug, PartialEq, Eq)]
pub struct UserButtonPress(());

/// 這一步是憑什麼跑的。
///
/// 兩者都是**正當**的批准，差別在有沒有人在鍵盤前面。log 必須分得出來：
/// 這份 log 存在的理由就是回答「她憑什麼做這件事」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovedBy {
    /// 他當場按的。
    Press,
    /// 憑一張先前簽好的票自己跑的——沒有人在鍵盤前面。
    StandingGrant,
}

/// 一張票批准了某一步之後才拿得到的憑證。
/// 私有欄位、沒有公開建構子——唯一的來源是 `Grant::authorize_unattended`。
#[derive(Debug, PartialEq, Eq)]
pub struct GrantPermit(());

#[derive(Debug, PartialEq, Eq)]
pub struct SuggestionAuthorization(SuggestionAuthorizationKind);

#[derive(Debug, PartialEq, Eq)]
enum SuggestionAuthorizationKind {
    Press(UserButtonPress),
    StandingGrant(GrantPermit),
}

impl UserButtonPress {
    fn from_button_click() -> Self {
        Self(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Suggestion {
    OpenUrl {
        url: String,
        authorization: SuggestionAuthorization,
    },
    OpenFile {
        path: PathBuf,
        authorization: SuggestionAuthorization,
    },
    FocusWindow {
        title: String,
        authorization: SuggestionAuthorization,
    },
}

impl Suggestion {
    pub fn open_url(pressed: UserButtonPress, url: String) -> Self {
        Self::OpenUrl {
            url,
            authorization: SuggestionAuthorization(SuggestionAuthorizationKind::Press(pressed)),
        }
    }

    pub fn open_file(pressed: UserButtonPress, path: PathBuf) -> Self {
        Self::OpenFile {
            path,
            authorization: SuggestionAuthorization(SuggestionAuthorizationKind::Press(pressed)),
        }
    }

    pub fn focus_window(pressed: UserButtonPress, title: String) -> Self {
        Self::FocusWindow {
            title,
            authorization: SuggestionAuthorization(SuggestionAuthorizationKind::Press(pressed)),
        }
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

    /// 取出按鈕即將核准的死資料；這不會放寬執行權限。
    ///
    /// [`ActionSnapshot`] 不帶 [`UserButtonPress`]，本來就是公開且任何呼叫端都能
    /// 建構的 enum；公開的 [`Self::describe`] 也一直由同一份 snapshot 算出來。
    /// 真正鑄出按壓憑證、讓動作有機會通過執行隘口的仍然只有 [`Self::press`]。
    pub fn snapshot(&self) -> ActionSnapshot {
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

    pub fn take_up(self, permit: GrantPermit) -> Suggestion {
        let authorization =
            SuggestionAuthorization(SuggestionAuthorizationKind::StandingGrant(permit));
        match self.0 {
            SuggestionDraft::OpenUrl { url } => Suggestion::OpenUrl { url, authorization },
            SuggestionDraft::OpenFile { path } => Suggestion::OpenFile {
                path,
                authorization,
            },
            SuggestionDraft::FocusWindow { title } => Suggestion::FocusWindow {
                title,
                authorization,
            },
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
    /// 他被問了，他說不要。**這不是她拒絕，是他拒絕**——但結果一樣是
    /// 根本沒有交給作業系統，所以它屬於 `Refused` 而不是 `Failed`。
    ///
    /// 少了這一種的話，一次「他說不要」在 action log 上會是一片空白，
    /// 而空白讀起來是「她從來沒問過這一步」。
    UserDeclinedThisStep,
    /// `observe` 級物理上沒有手。
    ObserveHasNoHands,
    /// 落在永不繼承的五類裡，要單獨核准（SPEC §9.2）。
    NeverInherited { class: NeverInherited },
    /// 這一類不能憑 standing grant 無人執行，必須有人當場按。
    NeedsLivePress { class: NeverInherited },
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
    /// 手被拔掉了（`hands.stop`）——這一步沒有交給作業系統。
    HandsPulled { since_ms: Option<i64> },
}

impl RefusalReason {
    pub fn message(&self) -> String {
        match self {
            Self::UserDeclinedThisStep => "你說不要，所以這一步沒有做。".to_string(),
            Self::ObserveHasNoHands => {
                "現在是 observe 級，她沒有手；要她動手得先把權限升到 suggest。".to_string()
            }
            Self::NeverInherited { class } => format!(
                "「{}」不隨任務授權繼承，每一次都要單獨核准——這一步沒有做。",
                class.name()
            ),
            Self::NeedsLivePress { class } => format!(
                "「{}」這一類不能靠票自己跑，要他當場按——這一步沒有做。",
                class.name()
            ),
            Self::SemiActionNeedsGrantAndStepApproval => {
                "semi-action 需要結構化 grant 和顯示的那一步核准；不可走 suggest 隘口。".to_string()
            }
            Self::NotCoveredByGrant { rejection } => rejection.message().to_string(),
            Self::ApprovalWasForAnotherStep { mismatch } => mismatch.message(),
            Self::HandsPulled { since_ms } => match since_ms {
                Some(since_ms) => format!(
                    "手從 {} 起被拔掉了，所以這一步沒有交給作業系統。要接回去請跑 `sister hands resume`。",
                    replay_copy::at(*since_ms)
                ),
                None => {
                    "手被拔掉了，所以這一步沒有交給作業系統。要接回去請跑 `sister hands resume`。"
                        .to_string()
                }
            },
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

    /// 手還在不在。
    ///
    /// 沒有預設實作是刻意的：每一個 executor 都必須明講自己有沒有接開關。
    fn hands_attached(&self) -> Attached;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attached {
    Yes,
    /// `since_ms` 是拔的時間；`None` 代表開關在但時戳讀不出來。
    No {
        since_ms: Option<i64>,
    },
}

/// **suggest 那條路**的唯一隘口：字母人上那顆按鈕按下去，走這裡。
///
/// **這不是全部執行請求的唯一隘口。** semi-action 有它自己那一個
/// ([`semi_action::execute_approved_step`])，而底下 [`Level::SemiAction`] 那一臂
/// 就是在講這件事——那條路要一張結構化的 grant 和逐步核准，混進這裡等於用
/// 一顆按鈕換掉整張授權書。alpha.69 寫下這段註解的時候 semi-action 還沒有
/// 呼叫端，「唯一」讀起來是真的；alpha.70 的 `sister do` 一接上去它就變成假話，
/// 而註解裡的假話正是這個 repo 最常見的那一種——每一行都是真的，湊起來在說謊。
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
    if let Attached::No { since_ms } = executor.hands_attached() {
        return Outcome::Refused {
            reason: RefusalReason::HandsPulled { since_ms },
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
    /// 一輪的開頭：**這一輪是在哪一張授權書底下發生的**。
    ///
    /// 少了這一列，紀錄上只剩「她做了什麼」——他授權過什麼是唯一沒有被記下來
    /// 的那一半。同一串步驟在一張「只准 chrome、五分鐘、一步」和一張「什麼都
    /// 准、一整天、一百步」底下發生，翻回去看是一模一樣的。
    ///
    /// 它同時是**一輪的界線**。在這之前，兩次 `sister do` 的步驟在檔案裡直接
    /// 接在一起，沒有任何東西說得出「這一步和上一步不是同一輪的事」。
    Granted {
        at_ms: i64,
        grant: semi_action::Grant,
    },
    Proposed {
        at_ms: i64,
        action: ActionSnapshot,
    },
    Approved {
        at_ms: i64,
        action: ActionSnapshot,
        /// `None` 只代表舊版本沒有記批准來源，不代表沒有人按。
        #[serde(default)]
        by: Option<ApprovedBy>,
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
    /// 一步做完後查到的畫面狀態；`None` 只代表舊版根本沒有查。
    StepFinished {
        at_ms: i64,
        step_number: u32,
        action: ActionSnapshot,
        evidence: Option<semi_action::StepEvidence>,
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
            Self::Granted { at_ms, .. }
            | Self::Proposed { at_ms, .. }
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
    /// 那段時間裡她動過幾次手——**不刪任何東西**。
    ///
    /// 給「你確定要忘掉嗎」那一頁用的。它和 [`Self::forget_range`] 共用
    /// [`LineVerdict`]，所以預覽上的數字和按下去真的消失的列數是同一個。
    ///
    /// 只數解得開而且落在範圍裡的列。解不開的那幾列 `forget_range` 也會刪掉，
    /// 但它們不是「她動過的手」——把一列壞掉的字算進「你那個下午做了 3 件事」
    /// 裡面，那個 3 就變成一個沒有人答得出來的數字。
    pub fn count_in_range(&self, from_ms: i64, to_ms: i64) -> Result<u64> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("開啟 action log 失敗：{}", self.path.display()));
            }
        };
        let mut n = 0;
        for line in BufReader::new(file).lines() {
            let line =
                line.with_context(|| format!("讀 action log 失敗：{}", self.path.display()))?;
            if matches!(LineVerdict::of(&line, from_ms, to_ms), LineVerdict::InRange) {
                n += 1;
            }
        }
        Ok(n)
    }

    /// **留下來的那幾列一個位元組都不會變。** 這裡不走 [`Self::replay`]——那支
    /// 回的是解析過的 [`ActionEvent`]，把它們重新序列化寫回去，等於用「這一版
    /// 認得的欄位」去重寫每一列。`ActionEvent` 沒有 `deny_unknown_fields`，所以
    /// 一列由新版寫下、帶著這一版還不認得的欄位的紀錄，會在存檔的那一刻安靜地
    /// 掉一半——而他要求刪掉的是**另一段時間**。忘掉星期二不可以順手改寫星期一。
    /// 所以留下來的列照原字串搬過去，只有時間拿去比對。
    pub fn forget_range(&self, from_ms: i64, to_ms: i64) -> Result<ForgetReport> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            // 檔案還沒生出來 = 一次動作都沒有提出過。回一份全 0 的報告，而且
            // 不要憑空把檔案生出來。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ForgetReport::default());
            }
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("開啟 action log 失敗：{}", self.path.display()));
            }
        };
        let mut report = ForgetReport::default();
        let mut kept: Vec<String> = Vec::new();
        for line in BufReader::new(file).lines() {
            let line =
                line.with_context(|| format!("讀 action log 失敗：{}", self.path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            match LineVerdict::of(&line, from_ms, to_ms) {
                LineVerdict::InRange => report.removed_in_range += 1,
                LineVerdict::Outside => kept.push(line),
                LineVerdict::Unreadable => report.removed_unreadable += 1,
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
            for line in &kept {
                file.write_all(line.as_bytes())
                    .and_then(|()| file.write_all(b"\n"))
                    .context("寫入 action log 失敗")?;
            }
            file.sync_data().context("同步 action log 失敗")?;
        }
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("換掉 action log 失敗：{}", self.path.display()))?;
        Ok(report)
    }
}

/// 一列相對於「要忘掉的那段時間」是什麼身分。
///
/// **這是唯一一份判斷。** [`ActionLog::forget_range`] 和
/// [`ActionLog::count_in_range`] 必須對同一列給同一個答案——預覽答應一個數字、
/// 按下去刪掉另一個數字，是這個 repo 一路在修的那件事。兩支各寫一次
/// `serde_json::from_str` 加一次範圍比對，就是等著哪天兩邊的邊界寫得不一樣。
enum LineVerdict {
    /// 解得開，而且時間落在 `[from_ms, to_ms)` 裡。
    InRange,
    /// 解得開，時間在範圍外。**原字不動留下來。**
    Outside,
    /// 解不開 ⇒ 問不出時間 ⇒ 證明不了自己在範圍外。
    Unreadable,
}

impl LineVerdict {
    fn of(line: &str, from_ms: i64, to_ms: i64) -> Self {
        match serde_json::from_str::<ActionEvent>(line) {
            Ok(event) if (from_ms..to_ms).contains(&event.at_ms()) => Self::InRange,
            Ok(_) => Self::Outside,
            Err(_) => Self::Unreadable,
        }
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

        fn hands_attached(&self) -> Attached {
            Attached::Yes
        }
    }

    struct PulledFake {
        executed: Vec<ActionSnapshot>,
    }
    impl Executor for PulledFake {
        fn execute(&mut self, suggestion: &Suggestion) -> std::result::Result<String, String> {
            self.executed.push(suggestion.snapshot());
            Ok("不該執行".into())
        }

        fn hands_attached(&self) -> Attached {
            Attached::No {
                since_ms: Some(1234),
            }
        }
    }

    #[test]
    fn suggest_choke_point_refuses_pulled_hands_without_executing() {
        let mut fake = PulledFake { executed: vec![] };
        let suggestion = Suggestion::open_url(click(), "https://example.com".into());
        assert_eq!(
            execute_with(Level::Suggest, &mut fake, &suggestion),
            Outcome::Refused {
                reason: RefusalReason::HandsPulled {
                    since_ms: Some(1234)
                }
            }
        );
        assert!(fake.executed.is_empty());
    }

    #[test]
    fn pulled_hands_copy_never_claims_execution_failed() {
        let message = RefusalReason::HandsPulled { since_ms: None }.message();
        assert!(message.contains("手被拔掉"));
        assert!(message.contains("沒有交給作業系統"));
        assert!(message.contains("sister hands resume"));
        assert!(!message.contains("失敗"));

        let dated = RefusalReason::HandsPulled {
            since_ms: Some(1_700_000_000_000),
        }
        .message();
        assert!(dated.contains("2023"), "{dated}");
        assert!(
            !dated
                .as_bytes()
                .windows(10)
                .any(|w| w.iter().all(u8::is_ascii_digit)),
            "{dated}"
        );
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
    fn step_evidence_uses_kind_inside_the_evidence_field_and_legacy_null_still_reads() {
        let action = ActionSnapshot::OpenUrl {
            url: "https://example.com".into(),
        };
        let event = ActionEvent::StepFinished {
            at_ms: 10,
            step_number: 1,
            action: action.clone(),
            evidence: Some(semi_action::StepEvidence::After {
                frame_id: 4,
                frame_at_ms: 11,
                has_image: true,
            }),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["evidence"]["kind"], "after");
        assert!(value["evidence"].get("evidence").is_none(), "{value}");

        let legacy = serde_json::json!({
            "event": "step_finished",
            "at_ms": 10,
            "step_number": 1,
            "action": action,
            "evidence": null
        });
        let read: ActionEvent = serde_json::from_value(legacy).unwrap();
        assert!(matches!(
            read,
            ActionEvent::StepFinished { evidence: None, .. }
        ));
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
                by: Some(ApprovedBy::Press),
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

    /// 預覽答應的數字，和按下去真的消失的列數，必須是同一個。
    ///
    /// 「你確定要忘掉嗎」那一頁上寫幾件，就要真的走掉幾件。這一條把同一份
    /// 檔案先問一次 `count_in_range`、再真的 `forget_range`，兩個數字對起來。
    #[test]
    fn what_the_preview_promises_is_what_actually_disappears() {
        let dir = tmp_dir("forget-preview");
        let log = ActionLog::in_data_dir(&dir);
        for ms in [500, 1_500, 2_500, 9_000] {
            log.append(&ActionEvent::Executed {
                at_ms: ms,
                action: ActionSnapshot::OpenUrl {
                    url: format!("https://x{ms}.example"),
                },
                result: ExecutionResult::Succeeded {
                    detail: "ok".into(),
                },
            })
            .unwrap();
        }
        // 讀不懂的那一列會被刪掉，但**不算**在「她動過的手」裡。
        {
            use std::io::Write as _;
            let mut f = OpenOptions::new().append(true).open(log.path()).unwrap();
            writeln!(f, "{{壞掉的").unwrap();
        }

        let promised = log.count_in_range(1_000, 3_000).unwrap();
        assert_eq!(promised, 2, "1_500 和 2_500 這兩列");
        let report = log.forget_range(1_000, 3_000).unwrap();
        assert_eq!(
            report.removed_in_range, promised,
            "預覽說 {promised} 件，實際走掉 {} 件",
            report.removed_in_range,
        );
        assert_eq!(report.removed_unreadable, 1, "壞掉那列也走了，只是不算手");
        assert_eq!(report.kept, 2);
        // 刪完再問一次，範圍裡就沒有東西了。
        assert_eq!(log.count_in_range(1_000, 3_000).unwrap(), 0);
    }

    /// 忘掉星期二，不可以順手改寫星期一。
    ///
    /// 留下來的列如果是重新序列化寫回去的，一列由新版寫下、帶著這一版還不認得
    /// 的欄位的紀錄就會安靜地掉一半——而他要求刪掉的是另一段時間。這裡塞一列
    /// 「未來版本」的紀錄：它解得開（`ActionEvent` 沒有 `deny_unknown_fields`），
    /// 所以不會被當成讀不懂的那一種，時間也在範圍外，因此它必須**原字不動**。
    #[test]
    fn a_row_outside_the_range_survives_byte_for_byte() {
        let dir = tmp_dir("forget-verbatim");
        let log = ActionLog::in_data_dir(&dir);
        log.append(&ActionEvent::Executed {
            at_ms: 1_000,
            action: ActionSnapshot::OpenUrl {
                url: "https://gone.example".into(),
            },
            result: ExecutionResult::Succeeded {
                detail: "ok".into(),
            },
        })
        .unwrap();
        let from_the_future = r#"{"event":"approved","at_ms":9000,"action":{"action":"open_url","url":"https://kept.example"},"why":"未來版本才有的欄位"}"#;
        {
            use std::io::Write as _;
            let mut f = OpenOptions::new().append(true).open(log.path()).unwrap();
            writeln!(f, "{from_the_future}").unwrap();
        }

        let report = log.forget_range(0, 5_000).unwrap();
        assert_eq!(
            report,
            ForgetReport {
                removed_in_range: 1,
                removed_unreadable: 0,
                kept: 1,
            },
            "那一列解得開而且在範圍外，不可以被算成讀不懂的",
        );
        assert_eq!(
            std::fs::read_to_string(log.path()).unwrap(),
            format!("{from_the_future}\n"),
            "留下來的那一列被重寫了——這一版不認得的欄位掉了",
        );
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
