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

    /// 這張畫面上最暗與最亮的像素（灰階近似）。`None` = 沒有像素。
    ///
    /// 存在的理由是要分辨兩件長得一模一樣的事：**「OCR 讀不出字」** 與
    /// **「這張圖上本來就沒有字」**。一張擷取失敗而全黑的畫面，在尺寸、
    /// 位元組數、甚至 dhash 上都跟正常畫面沒有明顯差別，而 OCR 對它的
    /// 回答同樣是「零行」——於是兩種病因在報告裡完全無法區分。
    ///
    /// `span.0 == span.1` 就代表整張圖是同一個顏色，那不是一張畫面，
    /// 是一次失敗的擷取。
    pub fn luma_span(&self) -> Option<(u8, u8)> {
        let rgba = self.rgba.as_deref()?;
        // 取樣就夠了：要回答的是「有沒有內容」，不是精確的直方圖。
        // 質數步長避免和螢幕上的規則圖樣（格線、掃描線）共振。
        let (mut lo, mut hi) = (255u8, 0u8);
        for px in rgba.chunks_exact(4).step_by(97) {
            // 近似的亮度：整數權重，不需要浮點數
            let y = ((px[0] as u32 * 77 + px[1] as u32 * 150 + px[2] as u32 * 29) >> 8) as u8;
            lo = lo.min(y);
            hi = hi.max(y);
        }
        (lo <= hi).then_some((lo, hi))
    }
}

/// 螢幕來源。
pub trait ScreenSource {
    /// 抓一張當下的畫面，**完整解析度**。回 `Ok(None)` 代表這一刻沒有
    /// 可用畫面（螢幕鎖定、顯示器休眠），不是錯誤。
    ///
    /// 這張是要拿去 OCR 與存檔的，所以像素必須是原生的：縮過的圖上，
    /// 12px 的字會掉到 7px，Windows 的 OCR 就會把 `Microsoft Teams`
    /// 讀成 `Micr099ftTeamsTr`——不報錯、不失敗，只是讀錯。
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>>;

    /// 便宜地探一張，**只保證 dhash 能用**。
    ///
    /// 一天裡絕大多數的 tick 畫面根本沒變，而它們全都要付一次抓圖的錢。
    /// 實測 120 個 tick 裡有 103 個是重複的——那 86% 完整解析度的搬運，
    /// 從頭到尾只是為了算出一個 64-bit 的雜湊然後把整張圖丟掉。
    ///
    /// 回傳的像素可能小到不能 OCR、不能存檔，呼叫端只能看 `dhash`。
    /// 這是安全的：dhash 本來就會把任何尺寸的圖 box-average 成 9×8，
    /// 所以探測圖與全圖算出來的雜湊一致（`hamming == 0`）。
    ///
    /// 預設轉呼 [`grab`](Self::grab)：沒有便宜路徑的來源（replay、測試）
    /// 行為完全不變。
    fn probe(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.grab(ts)
    }
}

/// 前景視窗來源。
pub trait FocusSource {
    fn snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot>;

    /// 見 [`Backend::degradations`]。
    fn degradations(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 剪貼簿來源。回傳自上次呼叫以來的新事件。
pub trait ClipboardSource {
    fn poll(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>>;

    /// 放棄這一刻的剪貼簿內容，但把「看到哪裡了」推到現在。
    ///
    /// 排除期間必須呼叫這個，不能只是不呼叫 [`poll`](Self::poll)。差別在於
    /// 以水位（sequence number）判斷新舊的來源：她在密碼管理員裡複製的密碼
    /// 會留在剪貼簿上，只要沒有人把水位推過去，等她切回瀏覽器的下一個 tick，
    /// 那份內容照樣會被讀進資料庫——排除規則只是延後了洩漏，沒有擋掉它。
    ///
    /// 預設是 no-op：以事件時間為準的來源（replay）本來就沒有這個問題。
    fn skip(&mut self, ts: Millis) {
        let _ = ts;
    }
}

/// 輸入動態來源。取走並清空目前累積的計數。
///
/// 實作者的鐵律：**永遠不記錄按鍵內容**，只記節奏與計數。
pub trait InputSource {
    fn drain(&mut self, ts: Millis) -> Result<Option<InputMetrics>>;

    /// 距離使用者最後一次碰鍵盤滑鼠過了多久。`None` = 這個平台答不出來。
    ///
    /// 這是整個 tick 裡唯一**不用碰螢幕就能問到的變化訊號**，而且便宜到
    /// 可以每次都問。沒有人動過任何東西，畫面就多半沒變——「多半」是關鍵
    /// 字，所以呼叫端不准無限相信它（見 `Recorder` 的 `MAX_BLIND_MS`）。
    fn idle_ms(&mut self) -> Option<u64> {
        None
    }
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
    /// 見 [`ScreenSource::probe`]。
    fn probe_screen(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.grab_screen(ts)
    }
    fn focus_snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot>;
    fn poll_clipboard(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>>;
    /// 見 [`ClipboardSource::skip`]。排除期間必須呼叫。
    fn skip_clipboard(&mut self, ts: Millis) {
        let _ = ts;
    }
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputMetrics>>;
    /// 見 [`InputSource::idle_ms`]。
    fn idle_ms(&mut self) -> Option<u64> {
        None
    }
    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>>;

    /// 這段錄製中途**壞掉**的能力，一句一則人話。
    ///
    /// `sister doctor` 只看得到開機那一瞬間。但能力是會在半路上掉的：UIA
    /// 卡三次之後就永久投降，而它一投降，`excluded_urls` 整組規則從那一刻起
    /// 一條都不生效——`doctor` 當時是綠的，摘要也是綠的，只有網銀從那之後
    /// 全被錄了進去。那是這個專案最不能接受的失效方式（THREAT_MODEL
    /// 「安靜地不生效」）。
    ///
    /// 所以收工前問一次。預設空的：沒有東西掉，就不要製造雜訊——一則
    /// 恆真的警告會讓整個警告區塊被學會忽略。
    fn degradations(&self) -> Vec<String> {
        Vec::new()
    }
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
    fn probe_screen(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.screen.probe(ts)
    }
    fn focus_snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot> {
        self.focus.snapshot(ts)
    }
    fn degradations(&self) -> Vec<String> {
        // 目前只有 focus 那一支會半路掉能力（UIA）。其他來源要嘛一開始就
        // 不在，要嘛一直都在，那些由 `Capabilities` 在開機時講完。
        self.focus.degradations()
    }
    fn poll_clipboard(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
        self.clipboard.poll(ts)
    }
    fn skip_clipboard(&mut self, ts: Millis) {
        self.clipboard.skip(ts)
    }
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputMetrics>> {
        self.input.drain(ts)
    }
    fn idle_ms(&mut self) -> Option<u64> {
        self.input.idle_ms()
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

    /// 一次失敗的擷取（整張同色）必須跟一張沒有字的畫面分得出來。
    ///
    /// 上面那條測試剛好示範了為什麼不能靠 dhash：由暗到亮的漸層和純色
    /// 一樣雜湊成 0。dhash 是設計來判斷「變了沒」的，不是「有沒有內容」。
    #[test]
    fn a_blank_capture_is_distinguishable_from_a_real_screen() {
        let (w, h) = (64u32, 64u32);
        let n = (w * h) as usize;

        let black = RawFrame::from_rgba(0, 0, w, h, vec![0u8; n * 4]);
        let (lo, hi) = black.luma_span().expect("有像素");
        assert_eq!(lo, hi, "全黑的擷取必須是單一亮度");

        let mut pixels = Vec::with_capacity(n * 4);
        for i in 0..n {
            let c = (i % 251) as u8;
            pixels.extend_from_slice(&[c, c, c, 255]);
        }
        let (lo, hi) = RawFrame::from_rgba(0, 0, w, h, pixels)
            .luma_span()
            .expect("有像素");
        assert!(hi - lo > 32, "有內容的畫面亮度該有範圍，實際 {lo}–{hi}");

        // 沒有像素的幀（replay、text-only）不該假裝答得出來
        assert!(
            RawFrame {
                ts: 0,
                monitor: 0,
                width: 4,
                height: 4,
                rgba: None,
                dhash: 0,
            }
            .luma_span()
            .is_none()
        );
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

    /// 半路上掉的能力要走得出後端這一層。
    ///
    /// 這條線存在的理由很具體：UIA 卡三次之後會永久投降，而它一投降，
    /// `excluded_urls` 從那一刻起一條都不生效。開機時的 `doctor` 是綠的、
    /// 錄製摘要也是綠的——除非有人在收工時問一句。這個測試就是在盯著
    /// 那句問話還在不在，別哪天重構把它接丟了。
    #[test]
    fn a_capability_lost_mid_run_makes_it_out_of_the_backend() {
        struct Flaky;
        impl FocusSource for Flaky {
            fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
                Ok(FocusSnapshot::default())
            }
            fn degradations(&self) -> Vec<String> {
                vec!["UIA 投降了".into()]
            }
        }

        let backend = CompositeBackend {
            name: "test".into(),
            screen: NullScreen,
            focus: Flaky,
            clipboard: NullClipboard,
            input: NullInput,
            ocr: NullOcr,
        };
        assert_eq!(Backend::degradations(&backend), vec!["UIA 投降了"]);

        // 而沒掉東西的時候必須完全安靜：一則恆真的警告會讓整個警告區塊
        // 被學會忽略，包括旁邊那則是真的
        let healthy = CompositeBackend {
            name: "test".into(),
            screen: NullScreen,
            focus: NullFocus,
            clipboard: NullClipboard,
            input: NullInput,
            ocr: NullOcr,
        };
        assert!(Backend::degradations(&healthy).is_empty());
    }
}
