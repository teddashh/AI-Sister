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
use std::path::Path;

use sister_core::model::{
    ClipboardEvent, ClipboardKind, FocusSnapshot, InputMetrics, Millis, OcrBlock,
};

use crate::traits::{Backend, RawFrame};

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
    /// 這一步螢幕不可用（鎖屏、休眠）。
    pub no_screen: bool,
    pub clipboard: Option<String>,
    pub clipboard_source_app: Option<String>,
    pub keystrokes: i64,
    pub clicks: i64,
    pub mouse_px: i64,
    pub scroll_ticks: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub steps: Vec<Step>,
}

impl Scenario {
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("parse replay scenario")
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
    /// 已經吐過剪貼簿事件的 step 索引，避免同一步重複觸發。
    clipboard_emitted: Option<usize>,
    input_since: Millis,
    input_acc: InputMetrics,
}

impl ReplayBackend {
    pub fn new(scenario: Scenario) -> Self {
        Self::with_origin(scenario, 0)
    }

    /// 指定腳本零點對應的真實時間。
    pub fn with_origin(scenario: Scenario, origin: Millis) -> Self {
        Self {
            scenario,
            origin,
            cursor: 0,
            current: Step::default(),
            clipboard_emitted: None,
            input_since: origin,
            input_acc: InputMetrics::default(),
        }
    }

    /// 腳本零點的真實時間。
    pub fn origin(&self) -> Millis {
        self.origin
    }

    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    /// 把時間推進到 `ts`，套用所有已到期的 step。
    fn advance(&mut self, ts: Millis) {
        while self.cursor < self.scenario.steps.len()
            && self.origin + self.scenario.steps[self.cursor].at_ms <= ts
        {
            let step = self.scenario.steps[self.cursor].clone();
            self.input_acc.keystrokes += step.keystrokes;
            self.input_acc.clicks += step.clicks;
            self.input_acc.mouse_px += step.mouse_px;
            self.input_acc.scroll_ticks += step.scroll_ticks;
            // 第一次觀察到某個 app 不算「切換」——那是取得焦點，不是換窗
            if self.current.app.is_some() && step.app.is_some() && step.app != self.current.app {
                self.input_acc.window_switches += 1;
            }
            self.current = step;
            self.cursor += 1;
        }
    }

    /// 時間軸是否已經播完。
    ///
    /// 只看時間、不看游標：否則答案會取決於呼叫者先前問過什麼，
    /// 這種依賴呼叫順序的 API 遲早會被誤用。
    pub fn is_finished(&self, ts: Millis) -> bool {
        ts >= self.origin + self.scenario.duration_ms()
    }
}

impl Backend for ReplayBackend {
    fn name(&self) -> &str {
        "replay"
    }

    fn grab_screen(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.advance(ts);
        if self.current.no_screen {
            return Ok(None);
        }
        Ok(Some(RawFrame {
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

    fn focus_snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot> {
        self.advance(ts);
        Ok(FocusSnapshot {
            app_id: self.current.app.clone(),
            app_name: self
                .current
                .app_name
                .clone()
                .or_else(|| self.current.app.clone()),
            window_title: self.current.title.clone(),
            url: self.current.url.clone(),
            pid: None,
        })
    }

    fn poll_clipboard(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
        self.advance(ts);
        let step_idx = self.cursor.saturating_sub(1);
        if self.clipboard_emitted == Some(step_idx) {
            return Ok(None);
        }
        let Some(text) = self.current.clipboard.clone() else {
            return Ok(None);
        };
        self.clipboard_emitted = Some(step_idx);
        Ok(Some(ClipboardEvent {
            ts,
            kind: ClipboardKind::Text,
            byte_len: text.len() as i64,
            text: Some(text),
            truncated: false,
            secret_suspected: false,
            source_app: self
                .current
                .clipboard_source_app
                .clone()
                .or_else(|| self.current.app.clone()),
        }))
    }

    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputMetrics>> {
        self.advance(ts);
        if self.input_acc == InputMetrics::default() {
            self.input_since = ts;
            return Ok(None);
        }
        let mut m = std::mem::take(&mut self.input_acc);
        m.ts_start = self.input_since;
        m.ts_end = ts;
        self.input_since = ts;
        Ok(Some(m))
    }

    fn recognize(&mut self, _frame: &RawFrame) -> Result<Vec<OcrBlock>> {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario() -> Scenario {
        Scenario::from_json(
            r#"{
              "name": "bill",
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
        let r = Scenario::from_json(r#"{"name":"x","steps":[{"at_ms":0,"txt":["oops"]}]}"#);
        assert!(r.is_err(), "a misspelled field must fail loudly");
    }

    #[test]
    fn identical_text_yields_identical_hash_so_dedup_works_without_pixels() {
        let mut b = ReplayBackend::new(scenario());
        let a = b.grab_screen(0).expect("grab").expect("frame");
        let c = b.grab_screen(5000).expect("grab").expect("frame");
        assert_eq!(a.dhash, c.dhash, "same screen text must dedup");

        let d = b.grab_screen(9000).expect("grab").expect("frame");
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
        let f = b.focus_snapshot(0).expect("focus");
        assert_eq!(f.app_id.as_deref(), Some("chrome.exe"));
        assert_eq!(f.url.as_deref(), Some("https://bill.cht.com.tw"));

        let f = b.focus_snapshot(9000).expect("focus");
        assert_eq!(f.app_id.as_deref(), Some("code.exe"));
        assert_eq!(f.url, None, "the editor has no URL");
    }

    #[test]
    fn time_only_moves_forward_and_skipped_steps_still_apply() {
        // 錄製迴圈的 tick 可能比腳本粗，中間的 step 不能被漏掉
        let mut b = ReplayBackend::new(scenario());
        let f = b.focus_snapshot(9500).expect("focus");
        assert_eq!(f.app_id.as_deref(), Some("code.exe"));

        let m = b.drain_input(9500).expect("input").expect("some input");
        assert_eq!(
            m.keystrokes, 52,
            "keystrokes from all elapsed steps accumulate"
        );
        assert_eq!(m.window_switches, 1);
    }

    #[test]
    fn clipboard_fires_once_per_step() {
        let mut b = ReplayBackend::new(scenario());
        assert!(b.poll_clipboard(0).expect("poll").is_none());
        let e = b
            .poll_clipboard(9000)
            .expect("poll")
            .expect("clipboard event");
        assert_eq!(e.text.as_deref(), Some("0800-080-123"));
        assert!(
            b.poll_clipboard(9100).expect("poll").is_none(),
            "must not repeat"
        );
    }

    #[test]
    fn input_drains_to_empty() {
        let mut b = ReplayBackend::new(scenario());
        let m = b.drain_input(1000).expect("input").expect("some");
        assert_eq!(m.keystrokes, 12);
        assert_eq!(m.clicks, 2);
        assert!(
            b.drain_input(2000).expect("input").is_none(),
            "drained means empty"
        );
    }

    #[test]
    fn origin_shifts_the_whole_timeline_into_real_time() {
        // 腳本寫相對時間，資料庫記絕對時間——接縫只有這一處
        let origin = 1_786_924_800_000; // 2026-08-17T00:00:00Z
        let mut b = ReplayBackend::with_origin(scenario(), origin);

        // 還沒到零點，什麼都還沒生效
        assert_eq!(b.focus_snapshot(origin - 1).expect("focus").app_id, None);

        let f = b.focus_snapshot(origin).expect("focus");
        assert_eq!(f.app_id.as_deref(), Some("chrome.exe"));

        let f = b.focus_snapshot(origin + 9000).expect("focus");
        assert_eq!(f.app_id.as_deref(), Some("code.exe"));

        assert!(!b.is_finished(origin + 11_999));
        assert!(b.is_finished(origin + 12_000));
    }

    #[test]
    fn locked_screen_is_absent_not_an_error() {
        let mut b = ReplayBackend::new(scenario());
        assert!(b.grab_screen(12_000).expect("grab").is_none());
        assert!(b.is_finished(12_000));
    }

    #[test]
    fn ocr_returns_the_scripted_text_with_plausible_geometry() {
        let mut b = ReplayBackend::new(scenario());
        let f = b.grab_screen(0).expect("grab").expect("frame");
        let blocks = b.recognize(&f).expect("ocr");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "本期應繳 NT$13,450");
        assert!(blocks[1].y > blocks[0].y, "lines must not overlap");
    }
}
