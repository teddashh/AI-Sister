//! `semi-action` 的平台無關授權、逐步核准與 audit 型別。

use crate::{ActionSnapshot, Executor, NeverInherited, Outcome, Suggestion, never_inherited_class};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
/// **這一維約束的是提出請求的那一方自己填的字，不是動作真正會打開什麼。**
/// [`ActionKind::of`] 是從 [`ActionSnapshot`] 算出來的，[`StepRequest::app`]
/// 不是——今天沒有一條誠實的路可以從「開啟 `https://…`」或「開啟 `a.txt`」
/// 推回哪一個 app 會接手（那是 Windows 的檔案關聯決定的，不是我們）。
/// 在有那條路之前，把它讀成「規劃者宣告它要碰哪些 app」，不要讀成
/// 「作業系統只會讓這些 app 被打開」。
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
        if now_ms < self.expiry.issued_at_ms {
            return Err(GrantRejection::ExpiryClockWentBack);
        }
        let elapsed = u64::try_from(now_ms - self.expiry.issued_at_ms).unwrap_or(u64::MAX);
        if elapsed > self.expiry.valid_for_ms {
            return Err(GrantRejection::ExpiryElapsed);
        }
        Ok(())
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
impl PresentedStep {
    pub fn new(step: StepRequest) -> Self {
        Self(step)
    }
    pub fn approve(self) -> StepApproval {
        StepApproval { shown: self.0 }
    }
}

pub struct StepApproval {
    shown: StepRequest,
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
}
impl AbortActor {
    const fn name(self) -> &'static str {
        match self {
            Self::User => "使用者",
            Self::System => "系統",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenEvidenceRef(String);
impl ScreenEvidenceRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "conclusion", rename_all = "snake_case")]
pub enum RunConclusionRecord {
    Completed,
    StepLimitReached { completed_steps: u32, limit: u32 },
}

impl RunConclusionRecord {
    /// 和 [`RunConclusion::message`] 講的是同一句話——這裡把記錄轉回結局再問它，
    /// 不另外抄一份文案。
    pub fn message(self) -> String {
        match self {
            Self::Completed => RunConclusion::Completed.message(),
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
        evidence: Option<ScreenEvidenceRef>,
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
    if let Some(class) = class {
        return Outcome::Refused {
            reason: crate::RefusalReason::NeverInherited { class },
        };
    }
    match executor.execute(suggestion) {
        Ok(detail) => Outcome::Done { detail },
        Err(error) => Outcome::Failed { error },
    }
}
