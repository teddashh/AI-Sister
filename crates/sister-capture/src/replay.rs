//! Replay 後端：從腳本檔重播一段工作階段。
//!
//! 這**不是**測試替身。它是 SPEC §12 replay 評測的地基：同一段時間軸
//! 可以反覆餵給不同版本的斷句器、去重門檻、gatekeeper，得到可比較的
//! 數字。沒有它，「這次改動有沒有變好」就只能靠感覺。
//!
//! 順帶的好處是核心得以在無頭 Linux 上完整測試——開發機沒有螢幕，
//! 但錄製迴圈的每一條分支都跑得到。
//!
//! 腳本是 JSON：
//! ```json
//! {
//!   "name": "bill-lookup",
//!   "privacy_context": "clear",
//!   "system_state": "active",
//!   "steps": [
//!     { "at_ms": 0,
//!       "app": "chrome.exe", "title": "中華電信 帳單", "url": "https://bill.cht.com.tw",
//!       "text": ["本期應繳 NT$13,450", "客服 0800-080-123"],
//!       "clipboard": "0800-080-123",
//!       "keystrokes": 12, "clicks": 2 }
//!   ]
//! }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;

use sister_core::model::{
    BrowserUrlState, ClipboardEvent, ClipboardKind, FocusSnapshot, InputMetrics, Millis, OcrBlock,
    PrivacyContext, SensitiveFieldState,
};

use crate::traits::{
    Backend, CapturePermit, ClipboardCapture, ClipboardWatermark, PrivacyObservation, RawFrame,
    ScreenCapture, SystemContentState, SystemLockState, SystemObservation, SystemPowerState,
    SystemTransition, SystemTransitionKind,
};

/// Replay 必須在腳本上明說 privacy context 的前提。
///
/// 沒有 `Default`；舊腳本缺這欄會解析失敗，而不是被當成
/// `Clear`。Replay 是給定且可公開的合成語料，`Clear` 是腳本作者
/// 對整份語料做的明確聲明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPrivacyContext {
    Clear,
    Focused,
    SensitiveUnknown,
    Unknown,
}

/// Replay 起點的 OS 狀態。同樣沒有 `Default`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaySystemState {
    Active,
    Locked,
    Sleeping,
    LockedSleeping,
    Unknown,
}

impl ReplaySystemState {
    fn content(self) -> Option<SystemContentState> {
        Some(match self {
            Self::Active => SystemContentState::active(),
            Self::Locked => SystemContentState::locked_awake(),
            Self::Sleeping => {
                SystemContentState::new(SystemLockState::Unlocked, SystemPowerState::Sleeping)
            }
            Self::LockedSleeping => {
                SystemContentState::new(SystemLockState::Locked, SystemPowerState::Sleeping)
            }
            Self::Unknown => return None,
        })
    }
}

/// 時間軸上的一步。缺省的欄位代表「這一刻這個感官沒有新東西」。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Step {
    /// 相對於腳本起點的毫秒數。
    pub at_ms: Millis,
    pub app: Option<String>,
    pub app_name: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    /// 螢幕上的文字。同時擔任 OCR 的輸出。
    pub text: Vec<String>,
    /// 明確指定 dhash。省略時由 `text` 推導，因此「同樣的文字 = 同一個畫面」，
    /// 去重邏輯不需要真的像素就能被測到。
    pub dhash: Option<u64>,
    /// 這一步的 screen source 沒有 frame。它不是 OS lock/sleep；
    /// 真的 lifecycle 狀態只能用 `system_event` / `system_state` 表達。
    pub no_screen: bool,
    pub clipboard: Option<String>,
    pub clipboard_source_app: Option<String>,
    pub keystrokes: i64,
    pub clicks: i64,
    pub mouse_px: i64,
    pub scroll_ticks: i64,
    /// 這個腳本時點真正發生的 OS 轉換。型別上只能是四種原生事件。
    pub system_event: Option<SystemTransitionKind>,
    /// 在 Unknown 之後明確重建的無-event baseline。不可和 `system_event`
    /// 寫在同一 step，避免作者偽造「剛好在這一刻發生」的 audit。
    pub system_state: Option<ReplaySystemState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default)]
    pub name: String,
    pub privacy_context: ReplayPrivacyContext,
    pub system_state: ReplaySystemState,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl Scenario {
    pub fn from_json(json: &str) -> Result<Self> {
        let scenario: Self = serde_json::from_str(json).context("parse replay scenario")?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read scenario {}", path.display()))?;
        Self::from_json(&raw)
    }

    /// 腳本總長度（毫秒）。
    pub fn duration_ms(&self) -> Millis {
        self.steps.last().map_or(0, |s| s.at_ms)
    }

    fn validate(&self) -> Result<()> {
        let mut state = self.system_state.content();
        let mut previous_at = None;
        let mut group_at = None;
        let mut group_has_baseline = false;
        let mut group_has_event = false;
        for step in &self.steps {
            anyhow::ensure!(
                step.at_ms >= 0,
                "replay step timestamp cannot be negative: {}",
                step.at_ms
            );
            if let Some(previous) = previous_at {
                anyhow::ensure!(
                    step.at_ms >= previous,
                    "replay steps are out of order: {} follows {previous}",
                    step.at_ms
                );
            }
            previous_at = Some(step.at_ms);

            if group_at != Some(step.at_ms) {
                group_at = Some(step.at_ms);
                group_has_baseline = false;
                group_has_event = false;
            }
            if step.system_state.is_some() && step.system_event.is_some() {
                anyhow::bail!(
                    "replay step at {} cannot combine system_state baseline with system_event",
                    step.at_ms
                );
            }
            if step.system_state.is_some() {
                anyhow::ensure!(
                    !group_has_event,
                    "replay timestamp {} cannot combine a system_state baseline with a system_event",
                    step.at_ms
                );
                group_has_baseline = true;
            }
            if step.system_event.is_some() {
                anyhow::ensure!(
                    !group_has_baseline,
                    "replay timestamp {} cannot combine a system_state baseline with a system_event",
                    step.at_ms
                );
                group_has_event = true;
            }
            if let Some(baseline) = step.system_state {
                if baseline.content().is_some() {
                    anyhow::ensure!(
                        state.is_none(),
                        "replay known system_state baseline at {} is only valid after unknown; use system_event for a known state change",
                        step.at_ms
                    );
                }
                state = baseline.content();
            }
            if let Some(event) = step.system_event {
                let Some(current) = state else {
                    anyhow::bail!(
                        "replay system_event at {} needs a known system_state baseline",
                        step.at_ms
                    );
                };
                state = Some(current.checked_applying(event).with_context(|| {
                    format!(
                        "replay system_event {event:?} at {} does not change its state dimension",
                        step.at_ms
                    )
                })?);
            }
        }
        Ok(())
    }
}

/// 由文字內容導出的穩定 64-bit hash（FNV-1a）。
///
/// 用 FNV 而不是 `DefaultHasher`：後者不保證跨版本穩定，而腳本要能
/// 在幾個月後產生一模一樣的結果，否則 replay 評測就失去意義。
fn text_hash(lines: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for line in lines {
        for b in line.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h ^= b'\n' as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// 依時間軸重播的後端。
///
/// 由外部驅動時間：呼叫端給定 `ts`，後端回答「在這個時刻，感官看到什麼」。
/// 因此 replay 是完全確定性的，不依賴真實時鐘。
pub struct ReplayBackend {
    scenario: Scenario,
    /// 腳本時間零點對應的真實 epoch 毫秒。
    ///
    /// 腳本裡寫的是相對時間（第 0 秒、第 5 秒），但資料庫記的必須是絕對時間。
    /// 把兩者的接縫放在這裡，腳本本身才能保持可攜與確定性。
    origin: Millis,
    /// 下一個尚未消費的 step。
    cursor: usize,
    /// 目前生效的 step（游標已經越過它）。
    current: Step,
    /// 已到期、尚未交給 recorder 的最新剪貼簿變更。
    ///
    /// 文字與來源必須從同一個 step 一起保存：其他感官的 step 會繼續推進
    /// `current`，但不能因此抹掉這次 copy，或把它錯配給後來的 app。
    pending_clipboard: Option<PendingClipboard>,
    input_since: Millis,
    input_acc: InputMetrics,
    /// `false` 時 timeline 仍往前播，但輸入事件從源頭放棄。
    /// suspend 和 resume 分開，才能覆蓋整段 pause/system gap。
    input_enabled: bool,
    privacy_context: ReplayPrivacyContext,
    system_content: Option<SystemContentState>,
    /// 尚未交給 recorder 的 observation。Baseline 與 Unknown 都是 barrier，
    /// 不能和前後 transition 折成同一個最終狀態。
    queued_system: VecDeque<SystemObservation>,
    /// 同一段已知狀態中、尚未封成 observation 的連續 transition。
    unflushed_transitions: Vec<SystemTransition>,
    next_system_sequence: u64,
}

#[derive(Debug, Clone)]
struct PendingClipboard {
    text: String,
    source_app: Option<String>,
}

impl ReplayBackend {
    pub fn new(scenario: Scenario) -> Self {
        Self::with_origin(scenario, 0)
    }

    /// 指定腳本零點對應的真實時間。
    pub fn with_origin(scenario: Scenario, origin: Millis) -> Self {
        let privacy_context = scenario.privacy_context;
        let system_content = scenario.system_state.content();
        let mut queued_system = VecDeque::new();
        queued_system.push_back(match system_content {
            Some(state) => SystemObservation::Known {
                state,
                transitions: Vec::new(),
            },
            None => SystemObservation::Unknown,
        });
        Self {
            scenario,
            origin,
            cursor: 0,
            current: Step::default(),
            pending_clipboard: None,
            input_since: origin,
            input_acc: InputMetrics::default(),
            input_enabled: true,
            privacy_context,
            system_content,
            queued_system,
            unflushed_transitions: Vec::new(),
            next_system_sequence: 1,
        }
    }

    /// 腳本零點的真實時間。
    pub fn origin(&self) -> Millis {
        self.origin
    }

    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    fn current_permit(&self) -> CapturePermit {
        // `cursor` 只在 timeline step 跨過時變，比對的是「privacy 問答後
        // 還是不是同一個腳本世代」，不猜 native window。
        CapturePermit::replay(self.cursor as u64)
    }

    fn flush_system_transitions(&mut self) {
        if self.unflushed_transitions.is_empty() {
            return;
        }
        let state = self
            .system_content
            .expect("validated replay transition always has a known resulting state");
        self.queued_system.push_back(SystemObservation::Known {
            state,
            transitions: std::mem::take(&mut self.unflushed_transitions),
        });
    }

    /// 把時間推進到 `ts`，套用所有已到期的 step。
    fn advance(&mut self, ts: Millis) {
        while self.cursor < self.scenario.steps.len()
            && self.origin + self.scenario.steps[self.cursor].at_ms <= ts
        {
            let step = self.scenario.steps[self.cursor].clone();
            if self.input_enabled {
                self.input_acc.keystrokes += step.keystrokes;
                self.input_acc.clicks += step.clicks;
                self.input_acc.mouse_px += step.mouse_px;
                self.input_acc.scroll_ticks += step.scroll_ticks;
                // 第一次觀察到某個 app 不算「切換」——那是取得焦點，不是換窗
                if self.current.app.is_some() && step.app.is_some() && step.app != self.current.app
                {
                    self.input_acc.window_switches += 1;
                }
            }
            if let Some(baseline) = step.system_state {
                // Baseline/Unknown 是一條不可跨越的邊界；先把前一段 transition
                // 封起來，再把 baseline 自己排成獨立 observation。
                self.flush_system_transitions();
                self.system_content = baseline.content();
                self.queued_system.push_back(match self.system_content {
                    Some(state) => SystemObservation::Known {
                        state,
                        transitions: Vec::new(),
                    },
                    None => SystemObservation::Unknown,
                });
            }
            if let Some(kind) = step.system_event {
                self.system_content = self.system_content.map(|state| state.applying(kind));
                self.unflushed_transitions.push(SystemTransition {
                    sequence: self.next_system_sequence,
                    ts: self.origin + step.at_ms,
                    kind,
                });
                self.next_system_sequence += 1;
            }
            if let Some(text) = step.clipboard.clone() {
                self.pending_clipboard = Some(PendingClipboard {
                    text,
                    source_app: step.clipboard_source_app.clone(),
                });
            }
            self.current = step;
            self.cursor += 1;
        }
        self.flush_system_transitions();
    }

    /// 時間軸是否已經播完。
    ///
    /// 只看時間、不看游標：否則答案會取決於呼叫者先前問過什麼，
    /// 這種依賴呼叫順序的 API 遲早會被誤用。
    pub fn is_finished(&self, ts: Millis) -> bool {
        ts >= self.origin + self.scenario.duration_ms()
    }

    /// 時間軸到尾端後，是否仍有被 Unknown/baseline 隔開、尚待 recorder
    /// 逐一驗證的系統 observation。
    pub fn has_pending_system_observations(&self) -> bool {
        !self.queued_system.is_empty() || !self.unflushed_transitions.is_empty()
    }
}

impl Backend for ReplayBackend {
    fn name(&self) -> &str {
        "replay"
    }

    fn poll_system(&mut self, ts: Millis) -> Result<SystemObservation> {
        self.advance(ts);
        if let Some(observation) = self.queued_system.pop_front() {
            return Ok(observation);
        }
        let Some(state) = self.system_content else {
            return Ok(SystemObservation::Unknown);
        };
        Ok(SystemObservation::Known {
            state,
            transitions: Vec::new(),
        })
    }

    fn grab_screen(&mut self, ts: Millis, permit: CapturePermit) -> Result<ScreenCapture> {
        self.advance(ts);
        if permit != self.current_permit() {
            return Ok(ScreenCapture::PrivacyChanged);
        }
        if self.current.no_screen {
            return Ok(ScreenCapture::Unavailable);
        }
        Ok(ScreenCapture::Frame(RawFrame {
            ts,
            monitor: 0,
            width: 1920,
            height: 1080,
            rgba: None,
            dhash: self
                .current
                .dhash
                .unwrap_or_else(|| text_hash(&self.current.text)),
        }))
    }

    fn privacy_context(&mut self, ts: Millis) -> Result<PrivacyObservation> {
        self.advance(ts);
        let focus = FocusSnapshot {
            app_id: self.current.app.clone(),
            app_name: self
                .current
                .app_name
                .clone()
                .or_else(|| self.current.app.clone()),
            window_title: self.current.title.clone(),
            url: self.current.url.clone(),
            pid: None,
        };
        let browser_url = if crate::browsers::is_browser(&focus.app_key()) {
            focus
                .url
                .clone()
                .map_or(BrowserUrlState::Unknown, BrowserUrlState::Known)
        } else {
            BrowserUrlState::NotApplicable
        };
        Ok(match self.privacy_context {
            ReplayPrivacyContext::Clear => PrivacyObservation::known(
                PrivacyContext::known(focus, SensitiveFieldState::Clear, browser_url),
                self.current_permit(),
            ),
            ReplayPrivacyContext::Focused => PrivacyObservation::known(
                PrivacyContext::known(focus, SensitiveFieldState::Focused, browser_url),
                self.current_permit(),
            ),
            ReplayPrivacyContext::SensitiveUnknown => PrivacyObservation::known(
                PrivacyContext::known(focus, SensitiveFieldState::Unknown, browser_url),
                self.current_permit(),
            ),
            ReplayPrivacyContext::Unknown => PrivacyObservation::Unknown,
        })
    }

    fn capture_permit_is_current(&mut self, permit: CapturePermit) -> Result<bool> {
        Ok(permit == self.current_permit())
    }

    fn poll_clipboard(&mut self, ts: Millis, permit: CapturePermit) -> Result<ClipboardCapture> {
        self.advance(ts);
        if permit != self.current_permit() {
            return Ok(ClipboardCapture::ContextChanged);
        }
        let Some(pending) = self.pending_clipboard.take() else {
            return Ok(ClipboardCapture::Event(None));
        };
        Ok(ClipboardCapture::Event(Some(ClipboardEvent {
            ts,
            kind: ClipboardKind::Text,
            byte_len: pending.text.len() as i64,
            text: Some(pending.text),
            truncated: false,
            secret_suspected: false,
            source_app: pending.source_app,
        })))
    }

    fn skip_clipboard(&mut self, ts: Millis) -> Result<ClipboardWatermark> {
        self.advance(ts);
        self.pending_clipboard = None;
        Ok(ClipboardWatermark::Established)
    }

    fn drain_input(&mut self, ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
        self.advance(ts);
        if !self.input_enabled {
            // 呼叫端在 gap 裡誤呼 drain 也不能偷開來源，更不能把
            // 暫停前未滿視窗的資料交出去。
            self.input_acc = InputMetrics::default();
            self.input_since = ts;
            return Ok(None);
        }
        if self.input_acc == InputMetrics::default() {
            // 回 `None`，不是一列 `unknown`。**重播沒有作業系統可以問**：
            // 沒有 hook、沒有 `GetLastInputInfo`，`Unknown` 那一列講的和
            // 「沒有列」是同一件事（`human_motion` 兩種都走 `NotMeasured`），
            // 但代價不一樣——重播沒有視窗批次，每個 tick 都會寫一列，一分鐘
            // 一百多列，而那一百多列一個讀得到的地方都沒有。
            //
            // （`input_health` 帶著 `session_id`，所以這裡曾經還有第二個理由：
            // 那些列會讓 `delete_empty_sessions` 從此清不掉重播出來的 session。
            // 那條已經在 `retention::content_only` 修掉了——整張表都不算內容，
            // 而且會跟著那一場一起走。留在這裡當紀錄，別再拿它當理由。）
            self.input_since = ts;
            return Ok(None);
        }
        let mut m = std::mem::take(&mut self.input_acc);
        m.ts_start = self.input_since;
        m.ts_end = ts;
        self.input_since = ts;
        Ok(Some(sister_core::model::InputTick {
            ts_start: m.ts_start,
            ts_end: m.ts_end,
            metrics: Some(m),
            listening: sister_core::model::InputListening::Unknown,
        }))
    }

    fn suspend_input(&mut self, ts: Millis) -> Result<()> {
        // 先關閉再 advance：剛好落在 pause/lock 邊界的 step 也屬於
        // gap，不能短暫加進去再依賴後續呼叫剛好清掉。
        self.input_enabled = false;
        self.advance(ts);
        self.input_acc = InputMetrics::default();
        self.input_since = ts;
        Ok(())
    }

    fn resume_input(&mut self, ts: Millis) -> Result<()> {
        if self.input_enabled {
            return Ok(());
        }
        // 仍在 disabled 時把 gap 尾端播完並清掉；只有這兩步完成後
        // 才打開，所以 `at_ms == ts` 的輸入不會穿過邊界。
        self.advance(ts);
        self.input_acc = InputMetrics::default();
        self.input_since = ts;
        self.input_enabled = true;
        Ok(())
    }

    fn recognize(&mut self, frame: &RawFrame) -> crate::traits::OcrAttempt {
        crate::traits::OcrAttempt::full(frame, || {
            // 腳本裡的文字就是 OCR 的結果；幾何資訊給一個規律的假版面即可
            Ok(self
                .current
                .text
                .iter()
                .enumerate()
                .map(|(i, t)| OcrBlock {
                    text: t.clone(),
                    x: 40,
                    y: 60 + i as i32 * 28,
                    w: 800,
                    h: 24,
                    confidence: 0.99,
                })
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus_at(backend: &mut ReplayBackend, ts: Millis) -> FocusSnapshot {
        match backend.privacy_context(ts).expect("privacy context") {
            PrivacyObservation::Known {
                context:
                    PrivacyContext::Known {
                        focus,
                        sensitive_field: SensitiveFieldState::Clear,
                        ..
                    },
                ..
            } => focus,
            other => panic!("scenario declared clear context, got {other:?}"),
        }
    }

    fn permit_at(backend: &mut ReplayBackend, ts: Millis) -> CapturePermit {
        match backend.privacy_context(ts).expect("privacy context") {
            PrivacyObservation::Known { permit, .. } => permit,
            other => panic!("scenario did not yield a permit: {other:?}"),
        }
    }

    fn frame_at(backend: &mut ReplayBackend, ts: Millis) -> RawFrame {
        let permit = permit_at(backend, ts);
        match backend.grab_screen(ts, permit).expect("grab") {
            ScreenCapture::Frame(frame) => frame,
            other => panic!("expected frame, got {other:?}"),
        }
    }

    fn scenario() -> Scenario {
        Scenario::from_json(
            r#"{
              "name": "bill",
              "privacy_context": "clear",
              "system_state": "active",
              "steps": [
                { "at_ms": 0, "app": "chrome.exe", "title": "中華電信 帳單",
                  "url": "https://bill.cht.com.tw",
                  "text": ["本期應繳 NT$13,450", "客服 0800-080-123"],
                  "keystrokes": 12, "clicks": 2 },
                { "at_ms": 5000, "app": "chrome.exe", "title": "中華電信 帳單",
                  "url": "https://bill.cht.com.tw",
                  "text": ["本期應繳 NT$13,450", "客服 0800-080-123"] },
                { "at_ms": 9000, "app": "code.exe", "title": "db.rs",
                  "text": ["fn insert_frame()"], "clipboard": "0800-080-123",
                  "keystrokes": 40 },
                { "at_ms": 12000, "no_screen": true }
              ]
            }"#,
        )
        .expect("parse scenario")
    }

    #[test]
    fn scenario_parses_and_reports_duration() {
        let s = scenario();
        assert_eq!(s.name, "bill");
        assert_eq!(s.steps.len(), 4);
        assert_eq!(s.duration_ms(), 12_000);
    }

    #[test]
    fn unknown_fields_are_rejected_so_typos_do_not_pass_silently() {
        let r = Scenario::from_json(
            r#"{"name":"x","privacy_context":"clear","system_state":"active","steps":[{"at_ms":0,"txt":["oops"]}]}"#,
        );
        assert!(r.is_err(), "a misspelled field must fail loudly");
    }

    #[test]
    fn safety_assumptions_are_required_instead_of_defaulting_to_safe() {
        for json in [
            r#"{"name":"x","system_state":"active","steps":[]}"#,
            r#"{"name":"x","privacy_context":"clear","steps":[]}"#,
        ] {
            assert!(
                Scenario::from_json(json).is_err(),
                "缺少 privacy/system 安全前提時不准默認放行：{json}"
            );
        }
    }

    #[test]
    fn replay_privacy_states_keep_unknown_separate_from_clear() {
        for (declared, expected) in [
            (
                ReplayPrivacyContext::Clear,
                PrivacyContext::known(
                    FocusSnapshot::default(),
                    SensitiveFieldState::Clear,
                    BrowserUrlState::NotApplicable,
                ),
            ),
            (
                ReplayPrivacyContext::Focused,
                PrivacyContext::known(
                    FocusSnapshot::default(),
                    SensitiveFieldState::Focused,
                    BrowserUrlState::NotApplicable,
                ),
            ),
            (
                ReplayPrivacyContext::SensitiveUnknown,
                PrivacyContext::known(
                    FocusSnapshot::default(),
                    SensitiveFieldState::Unknown,
                    BrowserUrlState::NotApplicable,
                ),
            ),
            (ReplayPrivacyContext::Unknown, PrivacyContext::Unknown),
        ] {
            let mut backend = ReplayBackend::new(Scenario {
                name: "privacy-state".into(),
                privacy_context: declared,
                system_state: ReplaySystemState::Active,
                steps: Vec::new(),
            });
            let actual = backend.privacy_context(0).expect("privacy");
            match (actual, expected) {
                (PrivacyObservation::Known { context, .. }, expected) => {
                    assert_eq!(context, expected)
                }
                (PrivacyObservation::Unknown, PrivacyContext::Unknown) => {}
                (actual, expected) => panic!("privacy mismatch: {actual:?} != {expected:?}"),
            }
        }
    }

    #[test]
    fn replay_system_unknown_and_true_transitions_are_not_collapsed_into_active() {
        let mut unknown = ReplayBackend::new(Scenario {
            name: "unknown-system".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Unknown,
            steps: Vec::new(),
        });
        assert_eq!(
            unknown.poll_system(0).expect("system"),
            SystemObservation::Unknown
        );

        let mut backend = ReplayBackend::new(Scenario {
            name: "system-transitions".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![
                Step {
                    at_ms: 1_000,
                    system_event: Some(SystemTransitionKind::Lock),
                    ..Default::default()
                },
                Step {
                    at_ms: 2_000,
                    system_event: Some(SystemTransitionKind::Unlock),
                    ..Default::default()
                },
            ],
        });
        assert_eq!(
            backend.poll_system(2_000).expect("initial baseline"),
            SystemObservation::active(),
            "即使第一個 coarse poll 已跨過事件，也要先交付無 event baseline"
        );
        assert_eq!(
            backend.poll_system(2_000).expect("system"),
            SystemObservation::Known {
                state: SystemContentState::active(),
                transitions: vec![
                    SystemTransition {
                        sequence: 1,
                        ts: 1_000,
                        kind: SystemTransitionKind::Lock,
                    },
                    SystemTransition {
                        sequence: 2,
                        ts: 2_000,
                        kind: SystemTransitionKind::Unlock,
                    },
                ],
            }
        );
        assert_eq!(
            backend.poll_system(2_001).expect("system"),
            SystemObservation::active(),
            "同一個原生 transition 只交付一次"
        );
    }

    #[test]
    fn event_at_zero_is_queued_behind_the_initial_baseline() {
        let scenario = Scenario::from_json(
            r#"{
              "privacy_context":"clear",
              "system_state":"active",
              "steps":[{"at_ms":0,"system_event":"lock"}]
            }"#,
        )
        .expect("valid scenario");
        let mut backend = ReplayBackend::new(scenario);

        assert_eq!(
            backend.poll_system(0).expect("baseline"),
            SystemObservation::active()
        );
        assert_eq!(
            backend.poll_system(0).expect("transition"),
            SystemObservation::Known {
                state: SystemContentState::locked_awake(),
                transitions: vec![SystemTransition {
                    sequence: 1,
                    ts: 0,
                    kind: SystemTransitionKind::Lock,
                }],
            }
        );
        assert!(!backend.has_pending_system_observations());
    }

    #[test]
    fn coarse_poll_preserves_unknown_recovery_and_transition_barriers() {
        let scenario = Scenario::from_json(
            r#"{
              "privacy_context":"clear",
              "system_state":"active",
              "steps":[
                {"at_ms":1000,"system_state":"unknown"},
                {"at_ms":2000,"system_state":"active"},
                {"at_ms":3000,"system_event":"lock"}
              ]
            }"#,
        )
        .expect("valid scenario");
        let mut backend = ReplayBackend::new(scenario);
        assert_eq!(
            backend.poll_system(0).expect("initial baseline"),
            SystemObservation::active()
        );

        assert_eq!(
            backend.poll_system(3_000).expect("unknown barrier"),
            SystemObservation::Unknown
        );
        assert_eq!(
            backend.poll_system(3_000).expect("recovery baseline"),
            SystemObservation::active()
        );
        assert_eq!(
            backend
                .poll_system(3_000)
                .expect("post-recovery transition"),
            SystemObservation::Known {
                state: SystemContentState::locked_awake(),
                transitions: vec![SystemTransition {
                    sequence: 1,
                    ts: 3_000,
                    kind: SystemTransitionKind::Lock,
                }],
            }
        );
        assert!(!backend.has_pending_system_observations());
    }

    #[test]
    fn malformed_system_timelines_are_rejected_before_replay() {
        let invalid = [
            (
                r#"{"privacy_context":"clear","system_state":"active","steps":[{"at_ms":-1}]}"#,
                "negative",
            ),
            (
                r#"{"privacy_context":"clear","system_state":"active","steps":[{"at_ms":2},{"at_ms":1}]}"#,
                "out of order",
            ),
            (
                r#"{"privacy_context":"clear","system_state":"unknown","steps":[{"at_ms":1,"system_state":"active"},{"at_ms":1,"system_event":"lock"}]}"#,
                "cannot combine",
            ),
            (
                r#"{"privacy_context":"clear","system_state":"active","steps":[{"at_ms":1,"system_state":"locked"}]}"#,
                "only valid after unknown",
            ),
        ];

        for (json, expected) in invalid {
            let error = Scenario::from_json(json).expect_err("timeline must be rejected");
            assert!(
                format!("{error:#}").contains(expected),
                "expected {expected:?} in {error:#}"
            );
        }
    }

    #[test]
    fn identical_text_yields_identical_hash_so_dedup_works_without_pixels() {
        let mut b = ReplayBackend::new(scenario());
        let a = frame_at(&mut b, 0);
        let c = frame_at(&mut b, 5000);
        assert_eq!(a.dhash, c.dhash, "same screen text must dedup");

        let d = frame_at(&mut b, 9000);
        assert_ne!(c.dhash, d.dhash, "different text must not dedup");
    }

    #[test]
    fn text_hash_is_stable_across_runs() {
        // replay 評測的前提：同一份腳本永遠得到同一個結果
        let lines = vec![
            "本期應繳 NT$13,450".to_string(),
            "客服 0800-080-123".to_string(),
        ];
        assert_eq!(text_hash(&lines), text_hash(&lines));
        assert_ne!(text_hash(&lines), text_hash(&lines[..1]));
        assert_eq!(text_hash(&[]), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn focus_follows_the_timeline() {
        let mut b = ReplayBackend::new(scenario());
        let f = focus_at(&mut b, 0);
        assert_eq!(f.app_id.as_deref(), Some("chrome.exe"));
        assert_eq!(f.url.as_deref(), Some("https://bill.cht.com.tw"));

        let f = focus_at(&mut b, 9000);
        assert_eq!(f.app_id.as_deref(), Some("code.exe"));
        assert_eq!(f.url, None, "the editor has no URL");
    }

    #[test]
    fn time_only_moves_forward_and_skipped_steps_still_apply() {
        // 錄製迴圈的 tick 可能比腳本粗，中間的 step 不能被漏掉
        let mut b = ReplayBackend::new(scenario());
        let f = focus_at(&mut b, 9500);
        assert_eq!(f.app_id.as_deref(), Some("code.exe"));

        let tick = b.drain_input(9500).expect("input").expect("some input");
        let m = tick.metrics.expect("metrics");
        assert_eq!(
            m.keystrokes, 52,
            "keystrokes from all elapsed steps accumulate"
        );
        assert_eq!(m.window_switches, 1);
    }

    #[test]
    fn clipboard_fires_once_per_step() {
        let mut b = ReplayBackend::new(scenario());
        let permit = permit_at(&mut b, 0);
        assert_eq!(
            b.poll_clipboard(0, permit).expect("poll"),
            ClipboardCapture::Event(None)
        );
        let permit = permit_at(&mut b, 9000);
        let e = match b.poll_clipboard(9000, permit).expect("poll") {
            ClipboardCapture::Event(Some(event)) => event,
            other => panic!("expected clipboard event, got {other:?}"),
        };
        assert_eq!(e.text.as_deref(), Some("0800-080-123"));
        let permit = permit_at(&mut b, 9_100);
        assert_eq!(
            b.poll_clipboard(9_100, permit).expect("poll"),
            ClipboardCapture::Event(None),
            "must not repeat"
        );
    }

    #[test]
    fn clipboard_change_survives_unrelated_steps_until_a_coarse_poll() {
        let scenario = Scenario::from_json(
            r#"{
              "privacy_context":"clear",
              "system_state":"active",
              "steps":[
                {"at_ms":1000,"app":"terminal.exe","clipboard":"first copy",
                 "clipboard_source_app":"terminal.exe"},
                {"at_ms":2000,"app":"code.exe","text":["unrelated screen change"]}
              ]
            }"#,
        )
        .expect("scenario");
        let mut backend = ReplayBackend::new(scenario);

        let permit = permit_at(&mut backend, 2_000);
        let event = match backend.poll_clipboard(2_000, permit).expect("coarse poll") {
            ClipboardCapture::Event(Some(event)) => event,
            other => panic!("expected pending clipboard event, got {other:?}"),
        };
        assert_eq!(event.text.as_deref(), Some("first copy"));
        assert_eq!(
            event.source_app.as_deref(),
            Some("terminal.exe"),
            "source app must stay paired with the copy step"
        );

        let permit = permit_at(&mut backend, 2_100);
        assert_eq!(
            backend.poll_clipboard(2_100, permit).expect("second poll"),
            ClipboardCapture::Event(None),
            "a pending copy is emitted exactly once"
        );
    }

    #[test]
    fn coarse_clipboard_poll_keeps_only_the_latest_copy_and_its_source() {
        let scenario = Scenario::from_json(
            r#"{
              "privacy_context":"clear",
              "system_state":"active",
              "steps":[
                {"at_ms":1000,"clipboard":"old copy",
                 "clipboard_source_app":"terminal.exe"},
                {"at_ms":2000,"clipboard":"latest copy",
                 "clipboard_source_app":"code.exe"},
                {"at_ms":3000,"text":["unrelated screen change"]}
              ]
            }"#,
        )
        .expect("scenario");
        let mut backend = ReplayBackend::new(scenario);

        let permit = permit_at(&mut backend, 3_000);
        let event = match backend.poll_clipboard(3_000, permit).expect("coarse poll") {
            ClipboardCapture::Event(Some(event)) => event,
            other => panic!("expected latest clipboard event, got {other:?}"),
        };
        assert_eq!(event.text.as_deref(), Some("latest copy"));
        assert_eq!(event.source_app.as_deref(), Some("code.exe"));

        let permit = permit_at(&mut backend, 3_001);
        assert_eq!(
            backend.poll_clipboard(3_001, permit).expect("second poll"),
            ClipboardCapture::Event(None),
            "the superseded copy must not reappear"
        );
    }

    #[test]
    fn privacy_or_system_gap_skip_clears_pending_clipboard_change() {
        let scenario = Scenario::from_json(
            r#"{
              "privacy_context":"clear",
              "system_state":"active",
              "steps":[
                {"at_ms":1000,"clipboard":"inside the gap",
                 "clipboard_source_app":"private.exe"},
                {"at_ms":2000,"system_event":"lock"}
              ]
            }"#,
        )
        .expect("scenario");
        let mut backend = ReplayBackend::new(scenario);

        assert_eq!(
            backend.skip_clipboard(2_000).expect("establish watermark"),
            ClipboardWatermark::Established
        );
        let permit = permit_at(&mut backend, 2_000);
        assert_eq!(
            backend
                .poll_clipboard(2_000, permit)
                .expect("poll after gap"),
            ClipboardCapture::Event(None),
            "clipboard content observed inside a privacy/system gap must stay discarded"
        );
    }

    #[test]
    fn input_drains_to_empty() {
        let mut b = ReplayBackend::new(scenario());
        let tick = b.drain_input(1000).expect("input").expect("some");
        let m = tick.metrics.expect("metrics");
        assert_eq!(m.keystrokes, 12);
        assert_eq!(m.clicks, 2);
        assert!(
            b.drain_input(2000).expect("input").is_none(),
            "重播沒有作業系統可以問，安靜的時候不該寫 input_health"
        );
    }

    #[test]
    fn suspended_input_cannot_be_drained_after_the_gap() {
        let mut b = ReplayBackend::new(scenario());

        // 0ms 的 12 次按鍵與 2 次點擊都屬於排除洞；skip 必須先
        // advance 到邊界再清掉，不能讓它們在恢復後才被 drain。
        b.suspend_input(5_000).expect("suspend input gap");
        assert!(
            b.drain_input(5_000).expect("input").is_none(),
            "排除期間的輸入不可以在邊界之後被取回"
        );

        b.resume_input(5_000).expect("resume input");

        let tick = b
            .drain_input(9_000)
            .expect("input")
            .expect("恢復後的輸入應該存在");
        let metrics = tick.metrics.expect("metrics");
        assert_eq!((metrics.ts_start, metrics.ts_end), (5_000, 9_000));
        assert_eq!(metrics.keystrokes, 40, "不能混入排除期間的 12 次");
        assert_eq!(metrics.clicks, 0, "不能混入排除期間的 2 次");
    }

    #[test]
    fn replay_drops_every_step_while_suspended_and_resume_is_idempotent() {
        let scenario = Scenario::from_json(
            r#"{
              "privacy_context":"clear",
              "system_state":"active",
              "steps":[
                {"at_ms":1000,"app":"private.exe","keystrokes":11,"clicks":3},
                {"at_ms":2000,"app":"private.exe","keystrokes":13},
                {"at_ms":3000,"app":"code.exe","keystrokes":5}
              ]
            }"#,
        )
        .expect("scenario");
        let mut backend = ReplayBackend::new(scenario);

        backend.suspend_input(500).expect("suspend");
        // 其他 source 仍會推進共用 timeline；這兩步的輸入仍必須在源頭被丟掉。
        let _ = backend.privacy_context(2_000).expect("advance timeline");
        assert!(backend.drain_input(2_000).expect("drain").is_none());

        backend.resume_input(2_000).expect("resume");
        backend
            .resume_input(2_500)
            .expect("repeated resume is a no-op");
        let tick = backend
            .drain_input(3_000)
            .expect("drain")
            .expect("post-resume input");
        let metrics = tick.metrics.expect("metrics");
        assert_eq!((metrics.ts_start, metrics.ts_end), (2_000, 3_000));
        assert_eq!(metrics.keystrokes, 5);
        assert_eq!(metrics.clicks, 0);
        assert_eq!(
            metrics.window_switches, 1,
            "恢復後的真實 app switch 仍要計數"
        );
    }

    #[test]
    fn origin_shifts_the_whole_timeline_into_real_time() {
        // 腳本寫相對時間，資料庫記絕對時間——接縫只有這一處
        let origin = 1_786_924_800_000; // 2026-08-17T00:00:00Z
        let mut b = ReplayBackend::with_origin(scenario(), origin);

        // 還沒到零點，什麼都還沒生效
        assert_eq!(focus_at(&mut b, origin - 1).app_id, None);

        let f = focus_at(&mut b, origin);
        assert_eq!(f.app_id.as_deref(), Some("chrome.exe"));

        let f = focus_at(&mut b, origin + 9000);
        assert_eq!(f.app_id.as_deref(), Some("code.exe"));

        assert!(!b.is_finished(origin + 11_999));
        assert!(b.is_finished(origin + 12_000));
    }

    #[test]
    fn unavailable_screen_source_is_absent_not_an_error() {
        let mut b = ReplayBackend::new(scenario());
        let permit = permit_at(&mut b, 12_000);
        assert!(matches!(
            b.grab_screen(12_000, permit).expect("grab"),
            ScreenCapture::Unavailable
        ));
        assert!(b.is_finished(12_000));
    }

    #[test]
    fn ocr_returns_the_scripted_text_with_plausible_geometry() {
        let mut b = ReplayBackend::new(scenario());
        let f = frame_at(&mut b, 0);
        let blocks = b.recognize(&f).outcome.expect("ocr").into_blocks();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "本期應繳 NT$13,450");
        assert!(blocks[1].y > blocks[0].y, "lines must not overlap");
    }
}
