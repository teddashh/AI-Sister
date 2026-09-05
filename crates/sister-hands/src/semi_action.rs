//! `semi-action` 的平台無關授權、逐步核准與 audit 型別。

use crate::{
    ActionSnapshot, ApprovedBy, Executor, GrantPermit, NeverInherited, Outcome, Suggestion,
    never_inherited_class,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn grant_path(data_dir: &Path) -> PathBuf {
    data_dir.join("grant.json")
}

pub fn grant_tmp_path(data_dir: &Path) -> PathBuf {
    grant_path(data_dir).with_extension("json.tmp")
}

/// 授權書在磁碟上的**兩個**檔案。要刪、要帶走、要問「有沒有」的，都走這一個。
pub fn grant_files(data_dir: &Path) -> [PathBuf; 2] {
    [grant_path(data_dir), grant_tmp_path(data_dir)]
}

/// 把存著的授權書刪掉，回傳**真的被刪掉的那幾個路徑**。
///
/// **這支函式住在這裡，是因為有兩個執行檔要做同一件事。** CLI 的
/// `sister forget` 和字母人時間軸上的「忘掉這一段」刪的是同一個資料目錄；
/// 各寫一份的話，改天有人補了第三個檔案，只會補到其中一邊，而兩邊畫面上
/// 都照樣寫著「已經忘掉了」。`action-log.jsonl` 就是這樣漏了好幾版。
///
/// 「本來就沒有」不算失敗，回空的；真的刪不掉（權限、被鎖住）要往上丟——
/// 吞掉就變成一句沒查過的「已刪除」。
pub fn forget_saved_grant(data_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut gone = Vec::new();
    for path in grant_files(data_dir) {
        match std::fs::remove_file(&path) {
            Ok(()) => gone.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(gone)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Task(String);
impl Task {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct App(String);
impl App {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    OpenUrl,
    OpenFile,
    FocusWindow,
}
impl ActionKind {
    pub const fn of(action: &ActionSnapshot) -> Self {
        match action {
            ActionSnapshot::OpenUrl { .. } => Self::OpenUrl,
            ActionSnapshot::OpenFile { .. } => Self::OpenFile,
            ActionSnapshot::FocusWindow { .. } => Self::FocusWindow,
        }
    }
}

/// 授權書允許碰的 app。
///
/// **這一維約束的是證據，不是作業系統。** [`StepRequest::app`] 這個欄位
/// 型別上仍然是「呼叫端填進來的字」——這個 crate 管不到它從哪裡來。
/// 差別在呼叫端：alpha.70 起唯一的生產者（`sister do`）是從承諾卡的
/// `evidence_json` 一路回查到 `text_chunks.app_id` 算出來的，而算不出唯一
/// 一個 app 的時候它不會亂填一個名字，會是一個擋得住的三態值。
///
/// 仍然**不可以**把它讀成「作業系統只會讓這些 app 被打開」：
/// 「開啟 `https://…`」最後由哪一支程式接手，是 Windows 的檔案關聯決定的，
/// 不是我們。這一維說的是「這一步的字，是在哪一個 app 的畫面上看到的」。
///
/// （alpha.69 這段話寫的是「規劃者自己填的字，今天沒有一條誠實的路可以推回
/// 哪個 app」。alpha.70 就把那條路做出來了，於是這段註解變成假話——和
/// [`crate::execute_with`] 上面那一段是同一天、同一種失效。）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedApps(BTreeSet<App>);
impl AllowedApps {
    pub fn new(values: impl IntoIterator<Item = App>) -> Self {
        Self(values.into_iter().collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedActions(BTreeSet<ActionKind>);
impl AllowedActions {
    pub fn new(values: impl IntoIterator<Item = ActionKind>) -> Self {
        Self(values.into_iter().collect())
    }
}

/// 相對於發出時刻的期限。牆鐘倒退時不延長票，直接拒絕。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expiry {
    issued_at_ms: i64,
    valid_for_ms: u64,
}
impl Expiry {
    pub const fn after_issued(issued_at_ms: i64, valid_for_ms: u64) -> Self {
        Self {
            issued_at_ms,
            valid_for_ms,
        }
    }

    pub const fn issued_at_ms(self) -> i64 {
        self.issued_at_ms
    }

    pub const fn valid_for_ms(self) -> u64 {
        self.valid_for_ms
    }
}

/// 步數上限，**永遠 ≥ 1**。
///
/// `try_from` 不是裝飾：少了它，`derive(Deserialize)` 會讓一張讀回來的 grant
/// 帶著 `StepLimit(0)`，而那是 [`StepLimit::new`] 鑄不出來的東西。同一個型別
/// 有兩條進來的路、其中一條不檢查，就等於沒有檢查。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32")]
pub struct StepLimit(u32);
impl StepLimit {
    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}
impl TryFrom<u32> for StepLimit {
    type Error = &'static str;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value).ok_or("步數上限是 0 的授權書鑄不出來")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    task: Task,
    apps: AllowedApps,
    actions: AllowedActions,
    expiry: Expiry,
    step_limit: StepLimit,
}
impl Grant {
    pub fn new(
        task: Task,
        apps: AllowedApps,
        actions: AllowedActions,
        expiry: Expiry,
        step_limit: StepLimit,
    ) -> Self {
        Self {
            task,
            apps,
            actions,
            expiry,
            step_limit,
        }
    }
    pub const fn step_limit(&self) -> StepLimit {
        self.step_limit
    }
    pub const fn expiry(&self) -> Expiry {
        self.expiry
    }
    pub fn validate_expiry(&self, now_ms: i64) -> Result<(), GrantRejection> {
        if now_ms < self.expiry.issued_at_ms {
            return Err(GrantRejection::ExpiryClockWentBack);
        }
        let elapsed = u64::try_from(now_ms - self.expiry.issued_at_ms).unwrap_or(u64::MAX);
        if elapsed > self.expiry.valid_for_ms {
            return Err(GrantRejection::ExpiryElapsed);
        }
        Ok(())
    }
    /// 這張授權書讀成一句人話。
    ///
    /// **這是為了讓「他授權過什麼」進得了 action log。** 在這之前那份紀錄只
    /// 記得她做了什麼——同一串步驟在一張「只准 chrome、五分鐘、一步」和一張
    /// 「什麼都准、一整天、一百步」底下發生，log 上長得一模一樣。
    ///
    /// 五個維度全部要出現，一個都不能省：省掉的那一維在讀的人眼裡不是
    /// 「沒有限制」，是「這裡沒有這一維」——而那兩件事差得非常遠。
    pub fn describe(&self) -> String {
        // 空名單擋掉每一步。寫成「（沒有）」會被讀成「這一維空著＝不設限」，
        // 而它的意思剛好相反。
        let apps = if self.apps.0.is_empty() {
            "（一個都沒有授權，所以每一步都會被擋）".to_string()
        } else {
            self.apps
                .0
                .iter()
                .map(|app| app.0.as_str())
                .collect::<Vec<_>>()
                .join("、")
        };
        let actions = self
            .actions
            .0
            .iter()
            .map(|kind| match kind {
                ActionKind::OpenUrl => "open-url",
                ActionKind::OpenFile => "open-file",
                ActionKind::FocusWindow => "focus-window",
            })
            .collect::<Vec<_>>()
            .join("、");
        format!(
            "任務「{}」；app：{apps}；動作：{actions}；整張票在發出後 {} 毫秒內有效；每一輪各自最多 {} 步",
            self.task.0, self.expiry.valid_for_ms, self.step_limit.0
        )
    }
    pub fn covers(&self, step: &StepRequest, now_ms: i64) -> Result<(), GrantRejection> {
        if self.task != step.task {
            return Err(GrantRejection::Task);
        }
        if !self.apps.0.contains(&step.app) {
            return Err(GrantRejection::Apps);
        }
        if !self.actions.0.contains(&ActionKind::of(&step.action)) {
            return Err(GrantRejection::Actions);
        }
        self.validate_expiry(now_ms)
    }

    pub fn authorize_unattended(
        &self,
        step: &StepRequest,
        now_ms: i64,
    ) -> Result<(StepApproval, GrantPermit), GrantRejection> {
        self.covers(step, now_ms)?;
        Ok((
            StepApproval {
                shown: step.clone(),
                by: ApprovedBy::StandingGrant,
            },
            GrantPermit(()),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantRejection {
    Task,
    Apps,
    Actions,
    ExpiryElapsed,
    ExpiryClockWentBack,
}
impl GrantRejection {
    pub fn message(self) -> &'static str {
        match self {
            Self::Task => "task 維度拒絕：這不是授權的任務。",
            Self::Apps => "apps 維度拒絕：這個 app 不在授權內。",
            Self::Actions => "actions 維度拒絕：這類動作不在授權內。",
            Self::ExpiryElapsed => "expiry 維度拒絕：授權已過期。",
            Self::ExpiryClockWentBack => "expiry 維度拒絕：時鐘倒退，期限無法驗證。",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRequest {
    task: Task,
    app: App,
    action: ActionSnapshot,
}
impl StepRequest {
    pub fn new(task: Task, app: App, action: ActionSnapshot) -> Self {
        Self { task, app, action }
    }
    pub fn action(&self) -> &ActionSnapshot {
        &self.action
    }
    pub const fn separate_approval_required(&self) -> Option<NeverInherited> {
        match &self.action {
            ActionSnapshot::OpenUrl { .. } => None,
            ActionSnapshot::OpenFile { .. } => None,
            ActionSnapshot::FocusWindow { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeparateApproval {
    Required(NeverInherited),
}
pub const fn separate_approval_for_class(class: NeverInherited) -> SeparateApproval {
    SeparateApproval::Required(class)
}

/// UI 顯示過的具體一步。只有消耗它才能鑄出核准票。
pub struct PresentedStep(StepRequest);
/// 這一段只在編譯期釘住這兩個型別沒有 `Serialize` 或 `DeserializeOwned`。
///
/// 兩個 blanket impl 各自涵蓋「所有 `Serialize` 的型別」和「所有 `DeserializeOwned`
/// 的型別」。那四行具名 impl 只有在 `StepApproval` 和 `PresentedStep` 這兩個型別
/// **都不是**那樣的時候才不衝突。任何人幫它們加上 `Serialize` 或
/// `DeserializeOwned`，這裡當場 E0119。四個方向當下都實測過。
/// 它不能證明「批准不能落地後重播」：`StepRequest` 本身可序列化，公開的
/// `PresentedStep::new(...).approve()` 仍能從落地後重讀的 request 產生批准。
///
/// `#[allow(dead_code)]`：這兩個 trait 沒有人呼叫是**故意的**——它們的用途是佔住
/// coherence，不是被呼叫。少了這一行，`clippy -D warnings` 會紅。
#[allow(dead_code)]
mod approval_stays_in_memory {
    trait NotSerialize {}
    impl<T: serde::Serialize> NotSerialize for T {}
    impl NotSerialize for super::StepApproval {}
    impl NotSerialize for super::PresentedStep {}

    trait NotDeserialize {}
    impl<T: serde::de::DeserializeOwned> NotDeserialize for T {}
    impl NotDeserialize for super::StepApproval {}
    impl NotDeserialize for super::PresentedStep {}
}

impl PresentedStep {
    pub fn new(step: StepRequest) -> Self {
        Self(step)
    }
    pub fn approve(self) -> StepApproval {
        StepApproval {
            shown: self.0,
            by: ApprovedBy::Press,
        }
    }
}

// 刻意不 derive `Serialize`/`Deserialize`。這個型別層的否定性質由上面
// `approval_stays_in_memory` 那段 coherence 衝突釘住：
// 一旦 `StepApproval` 拿到 `Serialize`，它就同時被 blanket impl 和具名 impl 涵蓋，
// 編譯直接紅（E0119）。
pub struct StepApproval {
    shown: StepRequest,
    by: ApprovedBy,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalMismatch {
    shown: ActionSnapshot,
    requested: ActionSnapshot,
}
impl ApprovalMismatch {
    /// 畫面上顯示過的那一步、以及真的要送出去的那一步，兩者不同。
    pub const fn between(shown: ActionSnapshot, requested: ActionSnapshot) -> Self {
        Self { shown, requested }
    }
    pub fn message(&self) -> String {
        format!(
            "核准只對顯示的那一步有效；畫面是「{}」，請求卻是「{}」，所以拒絕這次請求。",
            self.shown.describe(),
            self.requested.describe()
        )
    }
}
impl StepApproval {
    pub const fn by(&self) -> ApprovedBy {
        self.by
    }

    pub fn authorizes(&self, requested: &StepRequest) -> Result<(), ApprovalMismatch> {
        if self.shown == *requested {
            Ok(())
        } else {
            Err(ApprovalMismatch {
                shown: self.shown.action.clone(),
                requested: requested.action.clone(),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunConclusion {
    Completed,
    StepLimitReached {
        completed_steps: u32,
        limit: StepLimit,
    },
    Aborted {
        after_completed_steps: u32,
        by: AbortActor,
    },
}
impl RunConclusion {
    pub fn message(self) -> String {
        match self {
            // **不是「任務做完了」。** 他可以每一步都說不要，那一輪照樣走到底；
            // 走到底講的是「沒有東西再問了」，不是「你要的事情辦好了」。
            // 也不要在這句話裡提上限——那是 `StepLimitReached` 要講的事，
            // 兩句話各自只講自己那一件，才分得開（見底下那條測試）。
            Self::Completed => "這一輪的步驟都問完了。".into(),
            Self::StepLimitReached {
                completed_steps,
                limit,
            } => format!(
                "已做 {completed_steps} 步並停在步數上限 {}；不代表任務完成。",
                limit.get()
            ),
            Self::Aborted {
                after_completed_steps,
                by,
            } => format!(
                "已中止；先前完成 {after_completed_steps} 步，不回滾。中止者：{}。",
                by.name()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortActor {
    User,
    System,
    /// 有人在別的地方拔了手（tray 按鈕、`sister hands stop`）。
    HandsPulled,
}
impl AbortActor {
    const fn name(self) -> &'static str {
        match self {
            Self::User => "使用者",
            Self::System => "系統",
            Self::HandsPulled => "外部拔手開關",
        }
    }
}

// `ScreenEvidenceRef(String)` 本來站在這裡，被 `StepEvidence` 取代之後整份
// 拿掉了——不是為了少幾行，是因為它會騙人：一個叫「畫面憑據」、就住在真的
// 那一個隔壁、`pub` 而且零個呼叫端的型別，下一個人讀到會以為那才是接線的
// 地方，然後把新的東西接到一條死路上。它的名字是它唯一還在說的話，而那句
// 話是假的。

/// 做完之後那張畫面上記著的東西。欄位對應 `frames` 那一列。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenAfter {
    pub url: Option<String>,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenField {
    Url,
    WindowTitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cannot_tell", rename_all = "snake_case")]
pub enum CannotTell {
    /// 那張畫面上這一欄是空的——她那一刻沒能記下任何東西。
    ///
    /// 和 [`Self::ScreenUrlUnreadable`] 分開，是因為使用者能做的事不一樣：
    /// 這一格是「沒探到」，那一格是「探到了，但那不是一個網址」。r29 以前
    /// 兩者共用一句「這台機器沒有記下那張畫面的網址」，而後者根本記下了。
    NothingOnScreen { field: ScreenField },
    /// 那張畫面的網址欄**有值**，但抽不出可以比的網站名。
    ///
    /// 走得到：`about:blank`、`file:///C:/x.pdf`、UIA 探到半截的字串。
    /// 只有網址會落到這一格——視窗標題是用「有沒有含這幾個字」比的，
    /// 任何非空的標題都比得動，所以沒有「有值但讀不懂的標題」這種東西。
    ScreenUrlUnreadable,
    /// 這一步的目標本身是空的：`FocusWindow` 給了空白標題，或
    /// `OpenFile` 的路徑切不出檔名。
    NothingInTheAsk,
    /// 這一步要開的是網址，字也在，但她這一版抽不出可以比的網站名。
    ///
    /// 已知會走到這裡的是**中文／非 ASCII 網域**（`https://例え.jp/`）：
    /// `target_policy::validate_url` 收它，`segment::looks_like_host` 不收。
    /// 和 [`Self::NothingInTheAsk`] 分開是因為那句話會讀成「你沒有給目標」，
    /// 而使用者明明給了。
    AskUrlUnreadable,
    /// 這一列是 alpha.95 以前寫的；那幾版根本沒有比過。
    ///
    /// **這一格沒有產品寫入端。** 它只從 `StepEvidence::After::target` 的
    /// `#[serde(default)]` 長出來——舊紀錄的 JSON 裡沒有 `target` 這個欄位。
    /// `target_on_screen()` 一格都到不了這裡（它只回 `Matched` /
    /// `Mismatched`，或 `CannotTell` 的其他四格）。
    ///
    /// 所以：**誰在產品碼裡寫下 `TargetOnScreen::default()`，誰就讓上面那句
    /// 話變成假話**——一列今天寫的紀錄會對使用者說「這是舊版寫的」。要表達
    /// 「查了、但沒東西可比」請用 `NothingOnScreen` / `NothingInTheAsk`。
    NotChecked,
}

/// 做完之後那張畫面，和這一步「該變成的樣子」對不對得上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target_on_screen", rename_all = "snake_case")]
pub enum TargetOnScreen {
    /// `wanted` 和 `saw` 兩個都留著，因為**這兩格對得上的定義不一樣**：
    /// 網址是整個網站名相等（正規化過 `www.` 之後），視窗標題是「標題裡
    /// 含有這幾個字」。少了 `wanted`，句子只能寫成「畫面的標題是 X」，
    /// 那對一個子字串比對是過度宣稱——標題可能是「登入 — 健保存摺」。
    Matched {
        field: ScreenField,
        saw: String,
        wanted: String,
    },
    Mismatched {
        field: ScreenField,
        saw: String,
        wanted: String,
    },
    CannotTell {
        why: CannotTell,
    },
}

impl Default for TargetOnScreen {
    fn default() -> Self {
        Self::CannotTell {
            why: CannotTell::NotChecked,
        }
    }
}

/// 一步做完後實際查到的畫面狀態。
///
/// `ActionEvent::StepFinished::evidence` 外面的 `Option` 留給舊版紀錄：`None`
/// 代表那一版根本沒有查。新版查過以後一定寫入這裡其中一格，連「沒有」也不拿
/// `None` 代替。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepEvidence {
    After {
        frame_id: i64,
        frame_at_ms: i64,
        has_image: bool,
        /// 從動作結束到挑中這一張畫面為止，她等了多久。
        ///
        /// 0 代表「第一眼就看到了」。這個數字讓「對不上」那句話說得出
        /// **它找了多久**——一次快照對不上和兩秒內每一張都對不上，是兩件
        /// 不同的事，而使用者要靠它決定要不要自己去看一眼。
        ///
        /// `#[serde(default)]`：alpha.95 以前的那些列沒有這個鍵，補 0。
        /// 那些列同時也沒有 `target`，所以句子走的是「這一列是舊版寫的」
        /// 那一格，讀不到這個 0。
        #[serde(default)]
        waited_ms: u64,
        #[serde(default)]
        target: TargetOnScreen,
    },
    Before {
        frame_id: i64,
        frame_at_ms: i64,
        earlier_by_ms: i64,
        has_image: bool,
        #[serde(default)]
        wait: StepWait,
    },
    NotRecording {
        reason: NotRecordingReason,
    },
    NoFrameNearby {
        #[serde(default)]
        wait: StepWait,
    },
}

/// 這一步之後有沒有等下一張畫面，以及沒等的話是為什麼。
///
/// **「我沒有等」和「她沒在錄」是兩件事。** 中間有一版用一個 `waited_ms: u64`
/// 表示，0 就印「她當時沒在錄」——而 0 底下有六種 presence，其中 `Stalled`
/// 和 `Unreadable` 的正確答案是「說不準」（見 `NotRecordingReason::message`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "wait", rename_all = "snake_case")]
pub enum StepWait {
    /// 等了這麼久，還是沒有等到動作之後的畫面。
    Waited { ms: u64 },
    /// 沒有等，理由是這個。`NotRecordingReason` 已經是這個問題的字彙表，
    /// 借用它就不會有第二套說法。
    DidNotWait { because: NotRecordingReason },
    /// 這一列是 alpha.80 以前寫的；那幾版一秒都沒等過。presence 是否有被記下來
    /// 要由外層 evidence 變體回答，不能在共用的等待訊息裡猜。
    NotRecorded,
}

#[allow(clippy::derivable_impls)]
impl Default for StepWait {
    fn default() -> Self {
        Self::NotRecorded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum NotRecordingReason {
    NeverStarted,
    Stopped { at_ms: Option<i64> },
    Thinking { until_ms: i64 },
    Stalled { at_ms: i64 },
    Booting,
    Unreadable,
}

impl StepEvidence {
    pub fn message(&self) -> String {
        // **時刻走 `replay_copy::at`，不要自己插一個 `ts:`。** 那個前綴在這個
        // crate 裡是有意思的：它代表「這個數字對不出時刻」。硬寫成 `ts:{ms}`
        // 的話，一個好好的時戳會長得跟一個轉不出來的時戳一模一樣，而這份報告
        // 存在的理由就是讓人看得出每一步發生在哪一刻。
        let at = crate::replay_copy::at;
        match self {
            Self::After {
                frame_id,
                frame_at_ms,
                has_image,
                waited_ms,
                target,
            } => {
                let frame = if *has_image {
                    format!(
                        "做完之後的畫面憑據是 frame #{frame_id}（{}），圖在",
                        at(*frame_at_ms)
                    )
                } else {
                    format!(
                        "做完之後有 frame #{frame_id}（{}）這一列，但沒有截圖；紀錄在，圖不在",
                        at(*frame_at_ms)
                    )
                };
                // 十句。每一句都要講一件另外九句沒講的事——
                // `each_of_the_ten_endings_says_a_thing_the_others_do_not`
                // 會把每一句的招牌詞拿去掃另外九句，撞到就紅。
                //
                // 對得上的兩句都要**講清楚比的是什麼**：網址比的是網站名
                // （不是哪一頁），標題比的是「裡面有沒有這幾個字」。r29 早先
                // 寫成「而且⋯就在 X 上。」，讀起來像「這一步成功了」，而她
                // 看到的其實可能是同一個網站的登入牆。
                let ending = match target {
                    TargetOnScreen::Matched { field: ScreenField::Url, saw, .. } =>
                        format!("，那張畫面的網址也在 {saw} 上——她比的是網站，不是你停在哪一頁。"),
                    TargetOnScreen::Matched { field: ScreenField::WindowTitle, saw, wanted } =>
                        format!("，那張畫面的視窗標題「{saw}」裡有「{wanted}」——她比的是標題含不含這幾個字。"),
                    TargetOnScreen::Mismatched { field: ScreenField::Url, saw, wanted } =>
                        format!("，但那張畫面的網址在 {saw} 上，不是你要開的 {wanted}——這一步有沒有真的做到，她沒有把握。"),
                    TargetOnScreen::Mismatched { field: ScreenField::WindowTitle, saw, wanted } =>
                        format!("，但那張畫面的視窗標題是「{saw}」，裡面沒有「{wanted}」——這一步有沒有真的做到，她沒有把握。"),
                    TargetOnScreen::CannotTell { why: CannotTell::NothingOnScreen { field: ScreenField::Url } } =>
                        "。這台機器沒有探到那張畫面的網址欄，所以這只證明畫面變了，不證明變成你要的樣子。".to_string(),
                    TargetOnScreen::CannotTell { why: CannotTell::NothingOnScreen { field: ScreenField::WindowTitle } } =>
                        "。這台機器沒有探到那張畫面的視窗標題，所以這只證明畫面變了，不證明變成你要的樣子。".to_string(),
                    TargetOnScreen::CannotTell { why: CannotTell::ScreenUrlUnreadable } =>
                        "。那張畫面的網址欄有記到東西，但那不是一個看得出網站的網址，所以沒得比。".to_string(),
                    TargetOnScreen::CannotTell { why: CannotTell::NothingInTheAsk } =>
                        "。這一步本身沒有給出可以拿去跟畫面比的目標，所以這只證明畫面變了，不證明變成你要的樣子。".to_string(),
                    TargetOnScreen::CannotTell { why: CannotTell::AskUrlUnreadable } =>
                        "。你要開的網址她認得、也開了，但她這一版看不懂那個網域（例如中文網域），所以沒法跟畫面比。".to_string(),
                    TargetOnScreen::CannotTell { why: CannotTell::NotChecked } =>
                        "。這一列是舊版寫的，那幾版沒有比對過畫面上真的變成什麼。".to_string(),
                };
                // 只有在「沒等到他要的樣子」的時候才補這一句，而且只在真的
                // 等過的時候。對得上那一格不補：那會變成「等了 0 毫秒」這種
                // 沒有人需要知道的雜訊。
                let waited = match target {
                    TargetOnScreen::Matched { .. } => String::new(),
                    _ if *waited_ms == 0 => String::new(),
                    _ => format!(
                        "（她在這一步之後盯了 {waited_ms} 毫秒，看的是這段時間裡最新的那一張。）"
                    ),
                };
                format!("{frame}{ending}{waited}")
            }
            Self::Before {
                frame_id,
                frame_at_ms,
                earlier_by_ms,
                has_image,
                wait,
            } => {
                let frame = if *has_image {
                    format!(
                        "只有動作前 {earlier_by_ms} 毫秒的 frame #{frame_id}（{}）；這不是做完之後的畫面，圖在，只能證明她按下去時先前看見什麼。",
                        at(*frame_at_ms)
                    )
                } else {
                    format!(
                        "只有動作前 {earlier_by_ms} 毫秒的 frame #{frame_id}（{}）；這不是做完之後的畫面，而且沒有截圖；紀錄在，圖不在。",
                        at(*frame_at_ms)
                    )
                };
                let presence = matches!(wait, StepWait::NotRecorded)
                    .then_some("而她當時在不在錄沒有記。")
                    .unwrap_or_default();
                format!("{frame}{}{presence}", wait.message())
            }
            Self::NotRecording { reason } => reason.message(),
            Self::NoFrameNearby { wait } => format!(
                "她當時正在錄，但這一步前後的時間窗內一張 frame 都沒有；{}",
                wait.message()
            ),
        }
    }
}

impl StepWait {
    fn message(self) -> String {
        match self {
            Self::Waited { ms } => format!("等了 {ms} 毫秒還是沒有等到動作之後的畫面。"),
            Self::DidNotWait { because } => because.message(),
            Self::NotRecorded => "這一列是舊版寫的；那幾版不等下一張圖。".into(),
        }
    }
}

impl NotRecordingReason {
    pub(crate) fn message(self) -> String {
        // 同一條規矩，`StepEvidence::message` 那裡寫過一次
        // （「`ts:` 代表這個數字對不出時刻」）。**那一輪只修到直接的那幾臂，
        // 沒修到這裡**——而 `StepEvidence::NotRecording` 整個轉手給這個函式，
        // 於是同一句謊話在一次委派之外原封不動地活了下來。
        let at = crate::replay_copy::at;
        match self {
            Self::NeverStarted => {
                "這一步做完時她從來沒有開始錄，所以不會有這一步前後的新畫面憑據。".into()
            }
            Self::Stopped {
                at_ms: Some(stopped_at),
            } => {
                format!(
                    "這一步做完時她已經在 {} 收工了，所以不會有這一步前後的新畫面憑據。",
                    at(stopped_at)
                )
            }
            Self::Stopped { at_ms: None } => {
                "這一步做完時她已經收工了，但紀錄裡沒有留下時刻，所以不會有這一步前後的新畫面憑據。"
                    .into()
            }
            Self::Thinking { until_ms } => format!(
                "這一步做完時錄製已停，只剩解釋層在把最後一段想完（估到 {}）；再等下去也不會有新的畫面憑據。",
                at(until_ms)
            ),
            Self::Stalled { at_ms } => format!(
                "這一步做完時她從 {} 起就沒有回報過心跳（當掉了？）；有沒有新的畫面憑據說不準。",
                at(at_ms)
            ),
            Self::Booting => {
                "這一步做完時她才剛起來，還沒開始錄；再等一下可能就有新的畫面憑據。".into()
            }
            Self::Unreadable => {
                "這一步做完時錄製狀態那個檔案讀不懂；有沒有新的畫面憑據說不準。".into()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "conclusion", rename_all = "snake_case")]
pub enum RunConclusionRecord {
    Completed {
        /// 這一輪**問到你面前**幾步。
        ///
        /// 少了這個數字，「問了三步、三步都做完」和「一步都沒問到你」在紀錄裡
        /// 是同一列 `{"conclusion":"completed"}`——而後者的意思是她挑不出事情做，
        /// 讀起來卻像她把事情做完了。alpha.70 在螢幕上分開了這兩件事，
        /// 磁碟上那一半漏掉了，於是 `sister hands log` 隔一週再看回去又合起來。
        ///
        /// **這裡的兩種空是不同的兩件事，不可以合併：**
        /// `None` ＝ 這一列是還沒記這個數字的版本寫的（欄位根本不存在），
        /// `Some(0)` ＝ 記過了，數出來就是零。
        #[serde(default)]
        asked: Option<u32>,
        /// 那 N 步是誰決定的。`None` 只代表舊版沒有記。
        #[serde(default)]
        decided_by: Option<ApprovedBy>,
    },
    StepLimitReached {
        completed_steps: u32,
        limit: u32,
    },
}

impl RunConclusionRecord {
    /// 和 [`RunConclusion::message`] 講的是同一句話——這裡把記錄轉回結局再問它，
    /// 不另外抄一份文案。
    ///
    /// 例外是 `Completed`：那一層身上沒有「問了幾步」這個數字，講不出這一句。
    pub fn message(self) -> String {
        match self {
            // 「這一輪的步驟都問完了」在問了零步的時候是一句真話，而它讀起來是
            // 「都處理完了」。為什麼是零有三種答案，這一列裡沒有那個資訊——
            // 所以只講數字，不替它猜理由。猜錯的理由比沒有理由更糟。
            Self::Completed {
                asked: Some(0),
                decided_by: Some(ApprovedBy::Press),
            } => "這一輪走完了，但一步都沒有問到你。為什麼是零，要看當時螢幕上那一句；\
                 紀錄裡只有這個數字。"
                .to_string(),
            Self::Completed {
                asked: Some(n),
                decided_by: Some(ApprovedBy::Press),
            } => {
                format!(
                    "{}（問到你面前 {n} 步）",
                    RunConclusion::Completed.message()
                )
            }
            Self::Completed {
                asked: Some(0),
                decided_by: Some(ApprovedBy::StandingGrant),
            } => "這一輪憑先前簽好的票自己跑完了，但沒有東西可做。".to_string(),
            Self::Completed {
                asked: Some(n),
                decided_by: Some(ApprovedBy::StandingGrant),
            } => format!("這一輪憑先前簽好的票自己決定了 {n} 步；當時沒有人在鍵盤前面。"),
            Self::Completed {
                asked: None,
                decided_by: Some(ApprovedBy::Press),
            } => format!(
                "{}（知道是他當場按的，但這一列沒有記決定了幾步）",
                RunConclusion::Completed.message()
            ),
            Self::Completed {
                asked: None,
                decided_by: Some(ApprovedBy::StandingGrant),
            } => "這一輪走完了；知道是憑先前簽好的票自己決定，但這一列沒有記決定了幾步，當時沒有人在鍵盤前面。".to_string(),
            Self::Completed {
                asked,
                decided_by: None,
            } => format!(
                "{}（這一列沒有記是誰決定的；記下的決定步數：{}）",
                RunConclusion::Completed.message(),
                asked.map_or_else(|| "沒有記".to_string(), |n| n.to_string())
            ),
            Self::StepLimitReached {
                completed_steps,
                limit,
            } => match StepLimit::new(limit) {
                Some(limit) => RunConclusion::StepLimitReached {
                    completed_steps,
                    limit,
                }
                .message(),
                // 上限 0 的 grant 鑄不出來，所以這一列是被改過或寫壞的。
                // 不要替它編一句「停在步數上限 0」——那會像是真的發生過。
                None => format!(
                    "這一列記著已做 {completed_steps} 步、上限 0；上限 0 的授權書鑄不出來，所以這一列壞了，不解讀。"
                ),
            },
        }
    }
}

/// 一張 grant 的步數與中止狀態。中止及達上限後都不再接受下一步。
pub struct SemiActionRun {
    grant: Grant,
    completed_steps: u32,
    aborted_by: Option<AbortActor>,
}
impl SemiActionRun {
    pub fn new(grant: Grant) -> Self {
        Self {
            grant,
            completed_steps: 0,
            aborted_by: None,
        }
    }
    pub fn grant(&self) -> &Grant {
        &self.grant
    }
    pub fn may_start_step(&self) -> Result<(), RunConclusion> {
        if let Some(by) = self.aborted_by {
            return Err(RunConclusion::Aborted {
                after_completed_steps: self.completed_steps,
                by,
            });
        }
        if self.completed_steps >= self.grant.step_limit.get() {
            return Err(RunConclusion::StepLimitReached {
                completed_steps: self.completed_steps,
                limit: self.grant.step_limit,
            });
        }
        Ok(())
    }
    pub fn finish_step(
        &mut self,
        at_ms: i64,
        action: ActionSnapshot,
        evidence: Option<StepEvidence>,
    ) -> Result<crate::ActionEvent, RunConclusion> {
        self.may_start_step()?;
        self.completed_steps += 1;
        Ok(crate::ActionEvent::StepFinished {
            at_ms,
            step_number: self.completed_steps,
            action,
            evidence,
        })
    }
    pub fn abort(&mut self, at_ms: i64, by: AbortActor) -> crate::ActionEvent {
        self.aborted_by = Some(by);
        crate::ActionEvent::Aborted {
            at_ms,
            after_completed_steps: self.completed_steps,
            by,
        }
    }
}

/// grant、步級核准與 executor 的唯一 semi-action 隘口。
///
/// **這裡擋下來的每一種都是 [`Outcome::Refused`]，不是 [`Outcome::Failed`]。**
/// `Failed` 的意思是作業系統碰過了、而且不知道碰到哪一步；授權不通過的時候
/// executor 從頭到尾沒有被呼叫。折成同一種的那一天，回放的人會把一次被擋下來
/// 的付款讀成一次失敗的付款——[`Outcome`] 自己的文件就是這樣寫的。
pub fn execute_approved_step(
    grant: &Grant,
    now_ms: i64,
    approval: StepApproval,
    step: &StepRequest,
    executor: &mut impl Executor,
    suggestion: &Suggestion,
) -> Outcome {
    if let Err(rejection) = grant.covers(step, now_ms) {
        return Outcome::Refused {
            reason: crate::RefusalReason::NotCoveredByGrant { rejection },
        };
    }
    if let Err(mismatch) = approval.authorizes(step) {
        return Outcome::Refused {
            reason: crate::RefusalReason::ApprovalWasForAnotherStep { mismatch },
        };
    }
    // 核准的那一步、和真的要遞給 executor 的那一步，也可能不是同一件事。
    // 用同一個型別講同一句話：畫面是 A，送出去的是 B。
    let handed = suggestion.snapshot();
    if handed != *step.action() {
        return Outcome::Refused {
            reason: crate::RefusalReason::ApprovalWasForAnotherStep {
                mismatch: ApprovalMismatch::between(step.action().clone(), handed),
            },
        };
    }
    // 兩個 enum、兩個 match：`Suggestion` 帶著按鈕憑證、`ActionSnapshot` 不帶，
    // 所以它們沒辦法共用同一份 match。兩邊都問，不挑一邊問——挑一邊的那天，
    // 沒被挑到的那一份就變成一條寫在那裡但沒有人讀的規則。
    let class = step
        .separate_approval_required()
        .or_else(|| never_inherited_class(suggestion));
    if let Some(reason) = never_inherited_refusal(approval.by(), class) {
        return Outcome::Refused { reason };
    }
    if let crate::Attached::No { since_ms } = executor.hands_attached() {
        return Outcome::Refused {
            reason: crate::RefusalReason::HandsPulled { since_ms },
        };
    }
    match executor.execute(suggestion) {
        Ok(detail) => Outcome::Done { detail },
        Err(error) => Outcome::Failed { error },
    }
}

/// 永不繼承的那五類，**兩種批准來源都擋**——不一樣的只有拒絕的理由。
///
/// 這裡一度寫成「他當場按了就放行」，那是一次真的退步：`execute_with` 那半
/// （`lib.rs:416`）從以前到現在都是無條件擋的，semi-action 這半若放行，同一個
/// `NeverInherited::Pay` 走兩個隘口就會得到相反的答案，而且那一邊沒有任何東西
/// 會提醒你。這一類的名字是「不隨任務授權繼承」，不是「按一下就好」；要放寬
/// 它得先改 SPEC §9.2，不是在加 provenance 的時候順手改掉。
fn never_inherited_refusal(
    approved_by: ApprovedBy,
    class: Option<NeverInherited>,
) -> Option<crate::RefusalReason> {
    let class = class?;
    Some(match approved_by {
        // 沒有人在鍵盤前面，所以話要講得更明白：這一類靠票是跑不動的。
        ApprovedBy::StandingGrant => crate::RefusalReason::NeedsLivePress { class },
        ApprovedBy::Press => crate::RefusalReason::NeverInherited { class },
    })
}

#[cfg(test)]
mod provenance_tests {
    use super::*;

    #[test]
    fn never_inherited_is_refused_for_both_sources_only_the_reason_differs() {
        assert_eq!(
            never_inherited_refusal(ApprovedBy::StandingGrant, Some(NeverInherited::Pay)),
            Some(crate::RefusalReason::NeedsLivePress {
                class: NeverInherited::Pay
            })
        );
        // **不是 `None`。** 當場按了仍然擋——只是理由回到既有的那一個。
        assert_eq!(
            never_inherited_refusal(ApprovedBy::Press, Some(NeverInherited::Pay)),
            Some(crate::RefusalReason::NeverInherited {
                class: NeverInherited::Pay
            })
        );
    }

    #[test]
    fn an_ordinary_class_free_step_is_not_refused_by_this_gate() {
        assert_eq!(
            never_inherited_refusal(ApprovedBy::StandingGrant, None),
            None
        );
        assert_eq!(never_inherited_refusal(ApprovedBy::Press, None), None);
    }

    /// `ts:` 在這個 crate 裡是「這個數字對不出時刻」的記號（見
    /// `replay_copy::at`）。畫面證據曾經無條件印 `ts:{ms}`，於是一個好好的
    /// 時戳和一個轉不出來的時戳在報告上長得一模一樣。
    ///
    /// **這一條要走完每一個帶時刻的變體，包含轉手出去的那幾個。** alpha.76
    /// 修的時候只列了 `After` 和 `Before` 兩個直接的臂，而 `NotRecording`
    /// 整個委派給 `NotRecordingReason::message`——那裡三個 `ts:` 原封不動，
    /// 測試全綠。少列一個變體和沒有這條測試，在那一版是同一個結果。
    #[test]
    fn step_evidence_prints_a_real_time_not_the_cannot_convert_marker() {
        let ms = 1_756_200_004_400;
        let carrying_a_real_timestamp = [
            StepEvidence::After {
                waited_ms: 0,
                frame_id: 7,
                frame_at_ms: ms,
                has_image: true,
                target: Default::default(),
            },
            StepEvidence::Before {
                frame_id: 7,
                frame_at_ms: ms,
                earlier_by_ms: 300,
                has_image: false,
                wait: StepWait::Waited { ms: 2_000 },
            },
            StepEvidence::NotRecording {
                reason: NotRecordingReason::Stopped { at_ms: Some(ms) },
            },
            StepEvidence::NotRecording {
                reason: NotRecordingReason::Thinking { until_ms: ms },
            },
            StepEvidence::NotRecording {
                reason: NotRecordingReason::Stalled { at_ms: ms },
            },
        ];
        for evidence in carrying_a_real_timestamp {
            let message = evidence.message();
            assert!(
                message.contains(&crate::replay_copy::at(ms)),
                "要印得出時刻：{message}"
            );
            assert!(
                !message.contains("ts:"),
                "這個時戳轉得出來，不該掛著對不出時刻的記號：{message}"
            );
        }
    }

    /// 上面那條只證明得了「列到的那幾個沒問題」。這一條守的是**列表本身**：
    /// 帶時刻的變體多一個而測試沒跟著多一個的話，這裡會紅。
    #[test]
    fn every_not_recording_reason_that_carries_a_timestamp_is_covered_above() {
        let ms = 1_756_200_004_400;
        let all = [
            NotRecordingReason::NeverStarted,
            NotRecordingReason::Stopped { at_ms: Some(ms) },
            NotRecordingReason::Stopped { at_ms: None },
            NotRecordingReason::Thinking { until_ms: ms },
            NotRecordingReason::Stalled { at_ms: ms },
            NotRecordingReason::Booting,
            NotRecordingReason::Unreadable,
        ];
        let with_a_time = all
            .iter()
            .filter(|reason| reason.message().contains(&crate::replay_copy::at(ms)))
            .count();
        assert_eq!(
            with_a_time, 3,
            "帶時刻的 NotRecordingReason 變成 {with_a_time} 個了；\
             上面那條的清單要跟著補，否則新的那個又會偷偷印 ts:"
        );
        for reason in all {
            assert!(!reason.message().contains("ts:"), "{}", reason.message());
        }
    }
}
