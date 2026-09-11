//! 感官層的介面。
//!
//! 這裡是平台的唯一入口。Windows 的 WGC、macOS 的 ScreenCaptureKit、
//! 測試用的 replay，全部實作同一組 trait；上面的錄製迴圈與整個
//! `sister-core` 都看不見它們的差別。
//!
//! 每個來源都允許失敗，但安全前提失敗不能冒充正常值：system／privacy
//! context 不知道時明確回 `Unknown`，recorder 會在任何內容來源前 fail closed。
//! 不重試、不阻塞；能力缺口另由報告說清楚。

use anyhow::Result;
use sister_core::model::{ClipboardEvent, InputTick, Millis, OcrBlock, PrivacyContext};
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

/// 一次和 privacy gate 前景身份綁定的畫面擷取。
#[derive(Debug, Clone)]
pub enum ScreenCapture {
    Frame(RawFrame),
    /// 畫面來源當下沒有可用 frame；這不代表 OS lock/sleep。
    Unavailable,
    /// 擷取前或後的前景已不是 privacy gate 核准的那個。
    PrivacyChanged,
}

/// Privacy 問答核准的 native 前景身份。
///
/// 內部欄位不公開；只能拿整個 permit 交回同一個 backend 重驗。
/// Windows 同時綁 exact HWND、PID、核准世代與完整 privacy observation；
/// replay 綁 timeline step generation；第三方 backend 可建立一個只供自己
/// 比對的 opaque generation。`kind` 不公開，讓 replay／第三方／測試名字
/// 都不能造出 Windows production permit。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePermit {
    kind: CapturePermitKind,
    native_window: u64,
    process_id: Option<u32>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePermitKind {
    BackendLocal,
    Replay,
    #[cfg(windows)]
    Windows,
    #[cfg(test)]
    Test,
}

impl CapturePermit {
    /// 建立只在同一個 backend 內有意義的 opaque permit。
    ///
    /// 外部 backend 在 privacy observation 核准時保存這個完整值，之後於
    /// `is_current` 重新量完自己的前景狀態，再與收到的值比較。這不攜帶
    /// Windows 身分、也不會讓 [`crate::Recorder::new`] 寫出 trusted session
    /// provenance；它只解決「這份核准還是不是同一個 backend 世代」。
    pub const fn backend_local(generation: u64) -> Self {
        Self {
            kind: CapturePermitKind::BackendLocal,
            native_window: generation,
            process_id: None,
            generation,
        }
    }

    pub(crate) const fn replay(generation: u64) -> Self {
        Self {
            kind: CapturePermitKind::Replay,
            native_window: generation,
            process_id: None,
            generation,
        }
    }

    #[cfg(test)]
    pub(crate) const fn test(generation: u64) -> Self {
        Self {
            kind: CapturePermitKind::Test,
            native_window: generation,
            process_id: None,
            generation,
        }
    }

    #[cfg(windows)]
    pub(crate) const fn windows(
        _: crate::windows::WindowsBackendToken,
        native_window: u64,
        process_id: u32,
        generation: u64,
    ) -> Self {
        Self {
            kind: CapturePermitKind::Windows,
            native_window,
            process_id: Some(process_id),
            generation,
        }
    }

    #[cfg(windows)]
    pub(crate) const fn windows_parts(self) -> Option<(u64, u32, u64)> {
        match (self.kind, self.process_id) {
            (CapturePermitKind::Windows, Some(process_id)) => {
                Some((self.native_window, process_id, self.generation))
            }
            _ => None,
        }
    }
}

/// Focus source 的一次完整回答。Unknown 沒有 permit，所以不可能
/// 在後面意外被當成已核准前景。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyObservation {
    Known {
        context: PrivacyContext,
        permit: CapturePermit,
    },
    Unknown,
}

impl PrivacyObservation {
    pub fn known(context: PrivacyContext, permit: CapturePermit) -> Self {
        debug_assert!(matches!(context, PrivacyContext::Known { .. }));
        Self::Known { context, permit }
    }
}

/// Clipboard bytes 已讀到 RAM 後，前景 permit 仍然成立才能交給 recorder。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardCapture {
    Event(Option<ClipboardEvent>),
    ContextChanged,
}

/// Clipboard source 是否真的建立了 nonzero/可信的事件水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardWatermark {
    Established,
    Unknown,
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
    /// 這一拍能不能安全地讀內容，以及已知的前景脈絡。
    ///
    /// 回傳型別刻意沒有 `Default`：平台層問不出來就只能送
    /// [`PrivacyContext::Unknown`]，不能把全空 snapshot 當成安全。
    fn context(&mut self, ts: Millis) -> Result<PrivacyObservation>;

    /// 剛才 privacy gate 核准的 native 前景還是不是現在這一個。
    ///
    /// 這個比對不讀內容；Windows 只重讀 HWND + PID。問不到是
    /// error/false，呼叫端都必須在持久化 clipboard/frame 前丟掉。
    fn is_current(&mut self, permit: CapturePermit) -> Result<bool>;

    /// 見 [`Backend::url_capture`]。
    fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
        Default::default()
    }
}

/// 原生作業系統會送出的四種生命週期轉換。
///
/// 平台後端只拿得到這個受限 enum，所以它不可能偽造
/// `CapturePaused` / `CaptureResumed` / `MasterStopEngaged` /
/// `MasterStopReleased` / `Excluded` / session marker 這些只能由 recorder
/// 自己建立的稽核事件。**這份清單是舉例，規則是「不在這四格裡的都偽造不出來」**——
/// alpha.118 加了全停那兩格，而這段話原本只點名三種，讀起來像是被想過的就那幾種。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemTransitionKind {
    Lock,
    Unlock,
    Sleep,
    Wake,
}

impl SystemTransitionKind {
    pub(crate) const fn core_kind(self) -> sister_core::model::SystemKind {
        match self {
            Self::Lock => sister_core::model::SystemKind::Lock,
            Self::Unlock => sister_core::model::SystemKind::Unlock,
            Self::Sleep => sister_core::model::SystemKind::Sleep,
            Self::Wake => sister_core::model::SystemKind::Wake,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemTransition {
    /// 來源在這次 recorder 生命週期內單調遞增的序號。
    /// 時戳可以相同，但 sequence 不可重送或倒退。
    pub sequence: u64,
    pub ts: Millis,
    pub kind: SystemTransitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLockState {
    Unlocked,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPowerState {
    Awake,
    Sleeping,
}

/// 這一拍的 OS 內容狀態。Lock 與 power 是正交的兩維；
/// `Wake` 只能改 power，不能把仍然 locked 的 session 假裝成可讀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemContentState {
    lock: SystemLockState,
    power: SystemPowerState,
}

impl SystemContentState {
    pub const fn new(lock: SystemLockState, power: SystemPowerState) -> Self {
        Self { lock, power }
    }

    pub const fn active() -> Self {
        Self::new(SystemLockState::Unlocked, SystemPowerState::Awake)
    }

    pub const fn locked_awake() -> Self {
        Self::new(SystemLockState::Locked, SystemPowerState::Awake)
    }

    pub const fn lock(self) -> SystemLockState {
        self.lock
    }

    pub const fn power(self) -> SystemPowerState {
        self.power
    }

    pub const fn allows_content(self) -> bool {
        matches!(self.lock, SystemLockState::Unlocked)
            && matches!(self.power, SystemPowerState::Awake)
    }

    pub const fn applying(self, transition: SystemTransitionKind) -> Self {
        match transition {
            SystemTransitionKind::Lock => Self::new(SystemLockState::Locked, self.power),
            SystemTransitionKind::Unlock => Self::new(SystemLockState::Unlocked, self.power),
            SystemTransitionKind::Sleep => Self::new(self.lock, SystemPowerState::Sleeping),
            SystemTransitionKind::Wake => Self::new(self.lock, SystemPowerState::Awake),
        }
    }

    /// 把一個真正改變對應維度的 event 套上去。
    /// 同狀態的 `Unlock` / `Wake` 等重複宣告不是 transition。
    pub fn checked_applying(self, transition: SystemTransitionKind) -> Option<Self> {
        let next = self.applying(transition);
        if self.lock == next.lock && self.power == next.power {
            None
        } else {
            Some(next)
        }
    }
}

/// 系統狀態來源的一次觀察。
///
/// `Unknown` 和「已知可用、沒有新事件」是不同變體，而且沒有
/// `Default`。不知道時 recorder 必須在 focus／clipboard／screen
/// 之前停下。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemObservation {
    Known {
        state: SystemContentState,
        transitions: Vec<SystemTransition>,
    },
    Unknown,
}

impl SystemObservation {
    pub fn active() -> Self {
        Self::Known {
            state: SystemContentState::active(),
            transitions: Vec::new(),
        }
    }
}

/// 鎖定／睡眠等原生系統狀態的來源。
pub trait SystemSource {
    fn poll(&mut self, ts: Millis) -> Result<SystemObservation>;
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
    fn skip(&mut self, ts: Millis) -> Result<ClipboardWatermark> {
        let _ = ts;
        Ok(ClipboardWatermark::Unknown)
    }
}

/// 輸入動態來源。取走並清空目前累積的計數。
///
/// 實作者的鐵律：**永遠不記錄按鍵內容**，只記節奏與計數。
pub trait InputSource {
    fn drain(&mut self, ts: Millis) -> Result<Option<InputTick>>;

    /// 停止累積輸入，放棄到 `ts` 為止的節奏，並將來源維持在停止態。
    ///
    /// 使用者暫停，或 OS 鎖定、休眠、狀態 Unknown 期間必須呼叫這個，
    /// 不能只是不呼叫 [`drain`](Self::drain)。輸入 hook 是持續累加的；
    /// 沒有主動停掉的話，空洞期間的計數會在恢復後第一次 drain
    /// 才被寫進資料庫。一般 app/URL 排除仍保留不含內容的節奏，不使用這個邊界。
    /// 重複呼叫必須安全，不得在這裡自動恢復累積。
    fn suspend(&mut self, ts: Millis) -> Result<()>;

    /// 放棄停止期間到 `ts` 為止的所有輸入，然後重開統計視窗。
    ///
    /// 呼叫端只能在所有重疊的 pause/system gap 都關閉後呼叫；
    /// 在那之前的 [`drain`](Self::drain) 不得把來源偷偷打開。
    /// 重複呼叫必須是 no-op，不得清掉恢復後的新輸入。
    fn resume(&mut self, ts: Millis) -> Result<()>;

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
///
/// Backend 只能提供診斷名稱，不能自行聲稱 session provenance。這個
/// compile-fail regression 盯住那條曾經公開的轉授 seam；若以後又加回
/// `identity()`，doc test 會由預期失敗變成意外成功：
///
/// ```compile_fail
/// use sister_capture::Backend;
/// fn steal_identity<B: Backend>(backend: &B) {
///     let _ = backend.identity();
/// }
/// ```
///
/// 第三方 backend 仍能建立自己的 Known observation；permit 是 opaque，
/// 實作者只保存並以整值比較，不必也不能讀它的欄位：
///
/// ```
/// use anyhow::Result;
/// use sister_capture::{CapturePermit, FocusSource, PrivacyObservation};
/// use sister_core::model::{
///     BrowserUrlState, FocusSnapshot, Millis, PrivacyContext, SensitiveFieldState,
/// };
///
/// struct ExternalFocus { approved: Option<CapturePermit> }
/// impl FocusSource for ExternalFocus {
///     fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
///         let permit = CapturePermit::backend_local(7);
///         self.approved = Some(permit);
///         Ok(PrivacyObservation::known(
///             PrivacyContext::known(
///                 FocusSnapshot::default(),
///                 SensitiveFieldState::Clear,
///                 BrowserUrlState::NotApplicable,
///             ),
///             permit,
///         ))
///     }
///     fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
///         Ok(self.approved == Some(permit))
///     }
/// }
/// ```
pub trait Backend {
    /// 人類看得懂的後端名稱，只供診斷顯示。
    fn name(&self) -> &str;
    fn poll_system(&mut self, ts: Millis) -> Result<SystemObservation>;
    fn grab_screen(&mut self, ts: Millis, permit: CapturePermit) -> Result<ScreenCapture>;
    fn privacy_context(&mut self, ts: Millis) -> Result<PrivacyObservation>;
    fn capture_permit_is_current(&mut self, permit: CapturePermit) -> Result<bool>;
    fn poll_clipboard(&mut self, ts: Millis, permit: CapturePermit) -> Result<ClipboardCapture>;
    /// 見 [`ClipboardSource::skip`]。排除期間必須呼叫。
    fn skip_clipboard(&mut self, ts: Millis) -> Result<ClipboardWatermark> {
        let _ = ts;
        Ok(ClipboardWatermark::Unknown)
    }
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputTick>>;
    /// 見 [`InputSource::suspend`]。輸入來源要持續停止，不是單次清除。
    fn suspend_input(&mut self, ts: Millis) -> Result<()>;
    /// 見 [`InputSource::resume`]。只能在所有重疊 gap 都已關閉後呼叫。
    fn resume_input(&mut self, ts: Millis) -> Result<()>;
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
    /// 卡三次之後就永久投降；privacy context 從那刻起 Unknown，recorder 安全停讀。
    /// 若摘要仍是綠的，使用者只看到一段無由來的記憶空洞，所以這個能力缺口仍要
    /// 沿後端接出去。
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

/// 把六個各自獨立的來源組成一個 [`Backend`]。
///
/// 平台層只要各自實作最小的 trait，缺的用 `Null*` 補齊即可——
/// 能力是逐項降級的，不是全有全無。
pub struct CompositeBackend<Y, S, F, C, I, O> {
    pub name: String,
    pub system: Y,
    pub screen: S,
    pub focus: F,
    pub clipboard: C,
    pub input: I,
    pub ocr: O,
}

impl<Y, S, F, C, I, O> Backend for CompositeBackend<Y, S, F, C, I, O>
where
    Y: SystemSource,
    S: ScreenSource,
    F: FocusSource,
    C: ClipboardSource,
    I: InputSource,
    O: RecordingOcr,
{
    fn name(&self) -> &str {
        &self.name
    }
    fn poll_system(&mut self, ts: Millis) -> Result<SystemObservation> {
        self.system.poll(ts)
    }
    fn grab_screen(&mut self, ts: Millis, permit: CapturePermit) -> Result<ScreenCapture> {
        if !self.focus.is_current(permit)? {
            return Ok(ScreenCapture::PrivacyChanged);
        }
        let frame = self.screen.grab(ts)?;
        if !self.focus.is_current(permit)? {
            return Ok(ScreenCapture::PrivacyChanged);
        }
        Ok(match frame {
            Some(frame) => ScreenCapture::Frame(frame),
            None => ScreenCapture::Unavailable,
        })
    }
    fn privacy_context(&mut self, ts: Millis) -> Result<PrivacyObservation> {
        self.focus.context(ts)
    }
    fn capture_permit_is_current(&mut self, permit: CapturePermit) -> Result<bool> {
        self.focus.is_current(permit)
    }
    fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
        // 目前只有 focus 那一支會半路掉能力（UIA）。其他來源要嘛一開始就
        // 不在，要嘛一直都在，那些由 `Capabilities` 在開機時講完。
        self.focus.url_capture()
    }
    fn poll_clipboard(&mut self, ts: Millis, permit: CapturePermit) -> Result<ClipboardCapture> {
        if !self.focus.is_current(permit)? {
            return Ok(ClipboardCapture::ContextChanged);
        }
        let event = self.clipboard.poll(ts)?;
        if !self.focus.is_current(permit)? {
            let _ = self.clipboard.skip(ts);
            return Ok(ClipboardCapture::ContextChanged);
        }
        Ok(ClipboardCapture::Event(event))
    }
    fn skip_clipboard(&mut self, ts: Millis) -> Result<ClipboardWatermark> {
        self.clipboard.skip(ts)
    }
    fn drain_input(&mut self, ts: Millis) -> Result<Option<InputTick>> {
        self.input.drain(ts)
    }
    fn suspend_input(&mut self, ts: Millis) -> Result<()> {
        self.input.suspend(ts)
    }
    fn resume_input(&mut self, ts: Millis) -> Result<()> {
        self.input.resume(ts)
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
    fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
        Ok(PrivacyObservation::Unknown)
    }

    fn is_current(&mut self, _permit: CapturePermit) -> Result<bool> {
        Ok(false)
    }
}

/// 沒有原生系統狀態來源。這不是「系統可用」，而是不知道。
pub struct NullSystem;
impl SystemSource for NullSystem {
    fn poll(&mut self, _ts: Millis) -> Result<SystemObservation> {
        Ok(SystemObservation::Unknown)
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
    fn drain(&mut self, _ts: Millis) -> Result<Option<InputTick>> {
        // 和隔壁三個 Null 一樣：降級是安靜的。回一列 `unknown` 的話，每個
        // tick 都會多一列**零長度**（`ts_start == ts_end`）的 input_health
        // ——`input_health_covering` 用的是半開區間（`ts_end > ?1`），對
        // `ts_start == ts_end` 恆假，所以那種列**一個讀得到的地方都沒有**；
        // 而 `forget` 的重疊刪除和 `prune` 照樣要走過它。純成本。
        Ok(None)
    }

    fn suspend(&mut self, _ts: Millis) -> Result<()> {
        Ok(())
    }

    fn resume(&mut self, _ts: Millis) -> Result<()> {
        Ok(())
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
    fn backend_local_permits_never_equal_internal_authority_kinds() {
        let local = CapturePermit::backend_local(7);
        assert_ne!(local, CapturePermit::replay(7));
        assert_ne!(local, CapturePermit::test(7));
    }

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
            NullFocus.context(0).expect("no error"),
            PrivacyObservation::Unknown,
            "缺 focus source 是沒量到，不是已知安全"
        );
        assert_eq!(
            NullSystem.poll(0).expect("no error"),
            SystemObservation::Unknown,
            "缺 system source 也不可以冒充 active"
        );
        assert!(NullClipboard.poll(0).expect("no error").is_none());
        assert!(NullInput.drain(0).expect("no error").is_none());
        NullInput.suspend(0).expect("no error");
        NullInput.resume(0).expect("no error");
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

    #[test]
    fn composite_keeps_input_suspended_until_an_explicit_resume() {
        struct BufferedInput {
            pending: u64,
            enabled: bool,
            suspended_at: Option<Millis>,
            resumed_at: Option<Millis>,
        }

        impl InputSource for BufferedInput {
            fn drain(&mut self, _ts: Millis) -> Result<Option<InputTick>> {
                assert_eq!(self.pending, 0, "suspend 後不得還有舊計數可以 drain");
                Ok(None)
            }

            fn suspend(&mut self, ts: Millis) -> Result<()> {
                self.pending = 0;
                self.enabled = false;
                self.suspended_at = Some(ts);
                Ok(())
            }

            fn resume(&mut self, ts: Millis) -> Result<()> {
                if !self.enabled {
                    self.pending = 0;
                    self.enabled = true;
                    self.resumed_at = Some(ts);
                }
                Ok(())
            }
        }

        let mut backend = CompositeBackend {
            name: "test".into(),
            system: NullSystem,
            screen: NullScreen,
            focus: NullFocus,
            clipboard: NullClipboard,
            input: BufferedInput {
                pending: 9,
                enabled: true,
                suspended_at: None,
                resumed_at: None,
            },
            ocr: NullOcr,
        };

        Backend::suspend_input(&mut backend, 4_000).expect("suspend input");
        assert_eq!(backend.input.suspended_at, Some(4_000));
        assert!(!backend.input.enabled);
        assert!(
            Backend::drain_input(&mut backend, 14_000)
                .expect("drain")
                .is_none()
        );

        Backend::resume_input(&mut backend, 15_000).expect("resume input");
        assert!(backend.input.enabled);
        assert_eq!(backend.input.resumed_at, Some(15_000));
    }

    /// 半路上掉的能力要走得出後端這一層。
    ///
    /// 這條線存在的理由很具體：UIA 卡三次之後會永久投降，recorder 雖然
    /// fail closed，開機時的 `doctor` 卻仍可能是綠的。這個測試盯著能力缺口
    /// 有接到收工報告，避免安全停讀變成無從解釋的空洞。
    #[test]
    fn a_capability_lost_mid_run_makes_it_out_of_the_backend() {
        struct Flaky;
        impl FocusSource for Flaky {
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                Ok(PrivacyObservation::Unknown)
            }
            fn is_current(&mut self, _permit: CapturePermit) -> Result<bool> {
                Ok(false)
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
            system: NullSystem,
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
            system: NullSystem,
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
