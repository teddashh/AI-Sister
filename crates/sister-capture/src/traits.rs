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
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

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
        for px in rgba.as_chunks::<4>().0.iter().step_by(97) {
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
    /// 抓一張當下給 OCR／dHash 用的工作幀。回 `Ok(None)` 代表這一刻沒有
    /// 可用畫面（螢幕鎖定、顯示器休眠），不是錯誤。平台可以設安全尺寸上限；
    /// Windows 目前是長邊 4096px，低於上限才是原生解析度。
    ///
    /// 這張不可以先套用存檔用的 1568px 縮圖：12px 的字會掉到 7px，Windows
    /// OCR 會把 `Microsoft Teams` 讀成 `Micr099ftTeamsTr`——不報錯，只是讀錯。
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>>;
}

/// 前景視窗來源。
pub trait FocusSource {
    fn snapshot(&mut self, ts: Millis) -> Result<FocusSnapshot>;

    /// 見 [`Backend::url_capture`]。
    fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
        Default::default()
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

/// 錄製一幀時，OCR 管線實際採取的路徑。
///
/// 這裡不能只回一個 `Vec<OcrBlock>`：全幅讀、局部讀，以及根本沒有呼叫
/// 引擎卻沿用上一幀的文字，三種情況都可能交出同一組 blocks。把它們壓成
/// 同一個 Vec，收尾摘要就會把「沒跑」說成「跑了但很快」。
#[derive(Debug)]
pub enum OcrOutcome {
    Full {
        blocks: Vec<OcrBlock>,
        /// 局部路徑沒有把握，因而退回全幅。首張正常全幅是 false。
        fallback: bool,
    },
    Regions {
        /// 完整一幀的文字：變動區是新讀的，未變區來自已提交的上一幀。
        blocks: Vec<OcrBlock>,
        regions: NonZeroU64,
    },
    Reused {
        /// 像素沒有任何 RGB 變化，所以這些文字可直接沿用。
        blocks: Vec<OcrBlock>,
    },
}

impl OcrOutcome {
    pub(crate) fn blocks(&self) -> &[OcrBlock] {
        match self {
            Self::Full { blocks, .. } | Self::Regions { blocks, .. } | Self::Reused { blocks } => {
                blocks
            }
        }
    }

    pub fn into_blocks(self) -> Vec<OcrBlock> {
        match self {
            Self::Full { blocks, .. } | Self::Regions { blocks, .. } | Self::Reused { blocks } => {
                blocks
            }
        }
    }
}

/// 交給 OCR 實作嘗試的工作量。
///
/// `calls` 用 NonZero：有這個 struct 就代表 [`Ocr::recognize`] 真的被叫過，
/// 不能再讓 `calls = 0` 同時兼任「沒有量到」。實作仍可能在呼叫 OS 引擎前
/// 因缺語言、尺寸或 buffer 契約而拒絕；所以這不是 WinRT 邊界計數。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrWork {
    calls: NonZeroU64,
    elapsed: Duration,
    input_pixels: u64,
}

impl OcrWork {
    pub(crate) fn new(calls: NonZeroU64, elapsed: Duration, input_pixels: u64) -> Self {
        Self {
            calls,
            elapsed,
            input_pixels,
        }
    }

    pub fn calls(self) -> NonZeroU64 {
        self.calls
    }

    pub fn elapsed(self) -> Duration {
        self.elapsed
    }

    pub fn input_pixels(self) -> u64 {
        self.input_pixels
    }
}

/// 一次 recorder OCR 嘗試：結果和量測綁在同一個值上，避免接錯集合。
#[derive(Debug)]
pub struct OcrAttempt {
    pub outcome: Result<OcrOutcome>,
    /// `None` = 這是一個沒有 gate 的 raw OCR；不是「gate 花了 0 ms」。
    pub gate_elapsed: Option<Duration>,
    /// `None` = 這次沒有呼叫 OCR 實作；不是「呼叫 0 次」。
    pub work: Option<OcrWork>,
}

impl OcrAttempt {
    /// 把一個 raw OCR 引擎接成 recorder 管線。bench / doctor 仍直接呼叫 raw
    /// [`Ocr::recognize`]，所以不會不小心量到 changed-region gate。
    pub fn full(frame: &RawFrame, run: impl FnOnce() -> Result<Vec<OcrBlock>>) -> Self {
        let started = Instant::now();
        let result = run();
        let elapsed = started.elapsed();
        Self {
            outcome: result.map(|blocks| OcrOutcome::Full {
                blocks,
                fallback: false,
            }),
            gate_elapsed: None,
            work: Some(OcrWork::new(
                NonZeroU64::MIN,
                elapsed,
                u64::from(frame.width) * u64::from(frame.height),
            )),
        }
    }

    pub(crate) fn measured(
        outcome: Result<OcrOutcome>,
        gate_elapsed: Duration,
        work: Option<OcrWork>,
    ) -> Self {
        Self {
            outcome,
            gate_elapsed: Some(gate_elapsed),
            work,
        }
    }
}

/// dHash 說「近似重複」之後，stateful OCR gate 的第二個答案。
///
/// 9×8 dHash 會刻意吞掉很小的像素變化；單一新字也可能在門檻內。raw OCR
/// 沒有第二層證據，維持重複即可。changed-region gate 則可以只試那個 crop：
/// 結構驗過就把這幀升格成新畫面，驗不過仍沿用 dHash，不准為游標閃爍之類
/// 的小變化退回全幅 OCR。
#[derive(Debug)]
pub enum DhashRecheck {
    Duplicate {
        /// `None` = 這個 OCR backend 沒有第二層 gate。
        gate_elapsed: Option<Duration>,
        /// gate 可能試過 crop 才決定不升格；那些成本不能從摘要消失。
        work: Option<OcrWork>,
        /// `Some` = 真的試過這麼多個 region，但結構證據不足，沒有採用。
        rejected_regions: Option<NonZeroU64>,
        /// gate 或 raw OCR 真的執行失敗；維持 dHash 重複，但摘要不能把它
        /// 混成普通的結構拒絕而保持沉默。
        error: Option<anyhow::Error>,
    },
    /// 小變化有足夠證據，不能再讓 dHash 把它當成重複。
    Changed(OcrAttempt),
}

impl DhashRecheck {
    fn unchanged_without_gate() -> Self {
        Self::Duplicate {
            gate_elapsed: None,
            work: None,
            rejected_regions: None,
            error: None,
        }
    }
}

/// 只供 recorder 使用的 OCR 管線。
///
/// raw [`Ocr`] 自動得到「每幀全幅」的接法；changed-region wrapper 刻意只
/// 實作這個 trait，因此型別上就不可能被 `sister bench` 當成 raw engine。
pub trait RecordingOcr {
    fn recognize_frame(&mut self, frame: &RawFrame) -> OcrAttempt;

    /// dHash 已判成近似重複時，只有握有完整像素 baseline 的 gate 能推翻它。
    fn recheck_dhash_duplicate(&mut self, _frame: &RawFrame) -> DhashRecheck {
        DhashRecheck::unchanged_without_gate()
    }

    /// 只有 frame 與文字真的寫進 DB 後才提交 OCR baseline。
    fn commit_frame(&mut self, _frame: &RawFrame) {}

    /// DB 寫入失敗；剛才算出的 pending baseline 不成立。
    fn discard_frame(&mut self, _frame: &RawFrame) {}

    /// 暫停或隱私排除期間沒有看畫面，跨過這個洞後必須重新全幅讀一次。
    fn reset(&mut self) {}
}

impl<T: Ocr> RecordingOcr for T {
    fn recognize_frame(&mut self, frame: &RawFrame) -> OcrAttempt {
        OcrAttempt::full(frame, || self.recognize(frame))
    }
}

/// OCR 引擎拒絕尺寸過大的圖片。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrImageTooLarge;

impl std::fmt::Display for OcrImageTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("圖片超過 OCR 引擎尺寸上限")
    }
}

impl std::error::Error for OcrImageTooLarge {}

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
    /// 見 [`ClipboardSource::skip`]。排除期間必須呼叫。
    fn skip_clipboard(&mut self, ts: Millis) {
        let _ = ts;
    }
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputMetrics>>;
    /// 見 [`InputSource::idle_ms`]。
    fn idle_ms(&mut self) -> Option<u64> {
        None
    }
    fn recognize(&mut self, frame: &RawFrame) -> OcrAttempt;
    fn recheck_ocr_dhash_duplicate(&mut self, _frame: &RawFrame) -> DhashRecheck {
        DhashRecheck::unchanged_without_gate()
    }
    fn commit_ocr_frame(&mut self, _frame: &RawFrame) {}
    fn discard_ocr_frame(&mut self, _frame: &RawFrame) {}
    fn reset_ocr(&mut self) {}

    /// 這段錄製中途**壞掉**的能力，原始事實。
    ///
    /// `sister doctor` 只看得到開機那一瞬間。但能力是會在半路上掉的：UIA
    /// 卡三次之後就永久投降，而它一投降，`excluded_urls` 整組規則從那一刻起
    /// 一條都不生效——`doctor` 當時是綠的，摘要也是綠的，只有網銀從那之後
    /// 全被錄了進去。那是這個專案最不能接受的失效方式（THREAT_MODEL
    /// 「安靜地不生效」）。
    ///
    /// **回布林不回句子。** 以前這裡回的是寫好的警告字串，於是同一個判斷在
    /// 這裡和 `capabilities::Report::broken_privacy_rules` 各寫了一份，而那兩
    /// 份是給不同畫面看的（終端機／設定頁）。同一件事兩句話，遲早會走散，
    /// 而使用者會相信比較好聽的那一句。現在句子只有一個出處，這裡只送事實。
    ///
    /// 預設全 `false`：沒有 UIA 的平台不會半路掉這兩樣。
    fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
        Default::default()
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
    O: RecordingOcr,
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
    fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
        // 目前只有 focus 那一支會半路掉能力（UIA）。其他來源要嘛一開始就
        // 不在，要嘛一直都在，那些由 `Capabilities` 在開機時講完。
        self.focus.url_capture()
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
    fn recognize(&mut self, frame: &RawFrame) -> OcrAttempt {
        self.ocr.recognize_frame(frame)
    }
    fn recheck_ocr_dhash_duplicate(&mut self, frame: &RawFrame) -> DhashRecheck {
        self.ocr.recheck_dhash_duplicate(frame)
    }
    fn commit_ocr_frame(&mut self, frame: &RawFrame) {
        self.ocr.commit_frame(frame)
    }
    fn discard_ocr_frame(&mut self, frame: &RawFrame) {
        self.ocr.discard_frame(frame)
    }
    fn reset_ocr(&mut self) {
        self.ocr.reset()
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
            fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
                sister_core::capabilities::UrlCapture {
                    gave_up: true,
                    password_check_broken: false,
                }
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
        assert!(Backend::url_capture(&backend).gave_up);

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
        assert_eq!(
            Backend::url_capture(&healthy),
            sister_core::capabilities::UrlCapture::default()
        );
    }
}
