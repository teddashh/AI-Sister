//! 感官層的介面。
//!
//! 這裡是平台的唯一入口。Windows 的 WGC、macOS 的 ScreenCaptureKit、
//! 測試用的 replay，全部實作同一組 trait；上面的錄製迴圈與整個
//! `sister-core` 都看不見它們的差別。
//!
//! 每個來源都**允許失敗**且必須失敗得安靜——抓不到 URL 就是 `None`，
//! 不重試、不阻塞。感官層停下來的代價遠大於少一筆脈絡。

use anyhow::Result;
use sister_core::model::{ClipboardEvent, FocusSnapshot, InputMetrics, Millis, OcrBlock};

/// 平台交出來的一張原始畫面。
///
/// `rgba` 是 `Option`：replay 後端與 text-only 模式都不需要真的像素，
/// 但仍然需要一個 dhash 才能參與去重。因此 dhash 由後端負責算好，
/// 不是從 `rgba` 推導出來的。
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub ts: Millis,
    pub monitor: i32,
    pub width: u32,
    pub height: u32,
    /// RGBA8 像素（每像素 4 bytes）。`None` = 這個後端不提供影像。
    pub rgba: Option<Vec<u8>>,
    pub dhash: u64,
}

impl RawFrame {
    /// 由 RGBA 緩衝區建立，順便算好 dhash。
    pub fn from_rgba(ts: Millis, monitor: i32, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let dhash = sister_core::dedup::dhash_rgb(&rgba, width, height, 4);
        Self {
            ts,
            monitor,
            width,
            height,
            rgba: Some(rgba),
            dhash,
        }
    }

    pub fn pixel_count(&self) -> usize {
        (self.width as usize).saturating_mul(self.height as usize)
    }
}

/// 螢幕來源。
pub trait ScreenSource {
    /// 抓一張當下的畫面。回 `Ok(None)` 代表這一刻沒有可用畫面
    /// （螢幕鎖定、顯示器休眠），不是錯誤。
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>>;
}

/// 前景視窗來源。
pub trait FocusSource {
    fn snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot>;
}

/// 剪貼簿來源。回傳自上次呼叫以來的新事件。
pub trait ClipboardSource {
    fn poll(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>>;
}

/// 輸入動態來源。取走並清空目前累積的計數。
///
/// 實作者的鐵律：**永遠不記錄按鍵內容**，只記節奏與計數。
pub trait InputSource {
    fn drain(&mut self, ts: Millis) -> Result<Option<InputMetrics>>;
}

/// OCR 引擎。
pub trait Ocr {
    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>>;
}

/// 一個完整的平台後端：錄製迴圈唯一看得見的東西。
///
/// 刻意用扁平的方法而不是回傳 `&mut dyn XSource`：像 replay 這種
/// 五個來源共享同一份時間軸的後端，用 getter 會被 borrow checker 卡死。
/// 來源彼此獨立的平台（Windows、macOS）可以用 [`CompositeBackend`] 組起來。
pub trait Backend {
    /// 人類看得懂的後端名稱，寫進 sessions 表。
    fn name(&self) -> &str;
    fn grab_screen(&mut self, ts: Millis) -> Result<Option<RawFrame>>;
    fn focus_snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot>;
    fn poll_clipboard(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>>;
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputMetrics>>;
    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>>;
}

/// 把五個各自獨立的來源組成一個 [`Backend`]。
///
/// 平台層只要各自實作最小的 trait，缺的用 `Null*` 補齊即可——
/// 能力是逐項降級的，不是全有全無。
pub struct CompositeBackend<S, F, C, I, O> {
    pub name: String,
    pub screen: S,
    pub focus: F,
    pub clipboard: C,
    pub input: I,
    pub ocr: O,
}

impl<S, F, C, I, O> Backend for CompositeBackend<S, F, C, I, O>
where
    S: ScreenSource,
    F: FocusSource,
    C: ClipboardSource,
    I: InputSource,
    O: Ocr,
{
    fn name(&self) -> &str {
        &self.name
    }
    fn grab_screen(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.screen.grab(ts)
    }
    fn focus_snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot> {
        self.focus.snapshot(ts)
    }
    fn poll_clipboard(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
        self.clipboard.poll(ts)
    }
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputMetrics>> {
        self.input.drain(ts)
    }
    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>> {
        self.ocr.recognize(frame)
    }
}

// ---------- 什麼都不做的預設實作 ----------
//
// 平台能力是逐項降級的，不是全有全無：Linux 上沒有可靠的剪貼簿監聽
// 不該讓螢幕擷取也停擺。缺哪一項就插一個 Null 進去。

pub struct NullScreen;
impl ScreenSource for NullScreen {
    fn grab(&mut self, _ts: Millis) -> Result<Option<RawFrame>> {
        Ok(None)
    }
}

pub struct NullFocus;
impl FocusSource for NullFocus {
    fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
        Ok(FocusSnapshot::default())
    }
}

pub struct NullClipboard;
impl ClipboardSource for NullClipboard {
    fn poll(&mut self, _ts: Millis) -> Result<Option<ClipboardEvent>> {
        Ok(None)
    }
}

pub struct NullInput;
impl InputSource for NullInput {
    fn drain(&mut self, _ts: Millis) -> Result<Option<InputMetrics>> {
        Ok(None)
    }
}

/// 不做 OCR。text-only 以外的用途下，它代表「這台機器還沒有 OCR 引擎」。
pub struct NullOcr;
impl Ocr for NullOcr {
    fn recognize(&mut self, _frame: &RawFrame) -> Result<Vec<OcrBlock>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_frame_computes_its_own_hash() {
        let (w, h) = (16u32, 16u32);
        let build = |f: &dyn Fn(u32) -> u8| {
            let mut v = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..h {
                for x in 0..w {
                    let c = f(x);
                    v.extend_from_slice(&[c, c, c, 255]);
                }
            }
            v
        };

        // dHash 只在「左比右亮」時設位元。由暗到亮的畫面因此雜湊為 0，
        // 和純色一樣——這是演算法的性質，不是 bug。
        assert_eq!(RawFrame::from_rgba(0, 0, w, h, build(&|_| 128)).dhash, 0);
        let dark_to_light = build(&|x| if x < w / 2 { 0 } else { 255 });
        assert_eq!(RawFrame::from_rgba(0, 0, w, h, dark_to_light).dhash, 0);

        // 反過來（左亮右暗）就會有位元被設起來
        let light_to_dark = build(&|x| if x < w / 2 { 255 } else { 0 });
        assert_ne!(RawFrame::from_rgba(0, 0, w, h, light_to_dark).dhash, 0);
    }

    #[test]
    fn null_sources_are_silent_not_erroring() {
        // 降級必須是安靜的，不能變成錯誤往上冒
        assert!(NullScreen.grab(0).expect("no error").is_none());
        assert_eq!(
            NullFocus.snapshot(0).expect("no error"),
            FocusSnapshot::default()
        );
        assert!(NullClipboard.poll(0).expect("no error").is_none());
        assert!(NullInput.drain(0).expect("no error").is_none());
        let f = RawFrame {
            ts: 0,
            monitor: 0,
            width: 1,
            height: 1,
            rgba: None,
            dhash: 0,
        };
        assert!(NullOcr.recognize(&f).expect("no error").is_empty());
    }
}
