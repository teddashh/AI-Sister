//! Windows 內建的 OCR（`Windows.Media.Ocr`）。
//!
//! ## 為什麼用系統內建的，而不是 PP-OCRv5
//!
//! 一開始的規劃（SPEC §14）是 PP-OCRv5 走 ONNX Runtime。實際去看相依樹之後
//! 改了主意，理由有三個，而第一個是決定性的：
//!
//! 1. **它會把一個 HTTP client 連進執行檔裡。** `oar-ocr` 的 `auto-download`
//!    會拉進 `ureq`，用來在執行期下載模型。PRIVACY.md 寫著「程式裡沒有任何
//!    對外連線的程式碼路徑，你可以自己驗證：整個 repo 搜不到 HTTP client」。
//!    那句話要嘛是真的，要嘛不是；為了省事讓它變成半真的，整份文件就不值錢了。
//! 2. **預算。** Phase 0 的驗收條件是 CPU < 3%、RAM < 400MB。ONNX Runtime
//!    加上兩個模型常駐，光是 arena 就吃掉大半個預算，而這是一個整天都在跑的
//!    背景程式。
//! 3. **體積。** 模型約 20MB，`onnxruntime.dll` 約 15MB，而且是外掛的 DLL
//!    （`ort` 的 `copy-dylibs`）——使用者要下載的就不再是一個檔案。
//!
//! 系統內建的 OCR 這三項全部是零：零相依、零模型位元組、零 DLL。
//! 現在的 `sister.exe` 仍然是**一個 5.4MB 的檔案**，沒有任何附屬檔案。
//!
//! 代價是精準度不如 PP-OCRv5，而且綁在 Windows 上。[`crate::traits::Ocr`]
//! 這個 trait 就是為了讓以後換引擎不必動到其他任何地方。
//!
//! ## 兩個已知的地雷
//!
//! **微軟官方文件說這個 API 需要 package identity**（見「WinRT APIs not
//! supported in desktop apps」列表，`Windows.Media.Ocr.OcrEngine` 明列其中），
//! 也就是理論上不支援沒有打包的 Win32 程式。實務上 ShareX、Text-Grab 這些
//! 沒打包的程式都在用，而且能用。這是「沒有文件保證但可行」的狀態，不是
//! 「不能用」——但既然沒有保證，CI 就必須真的在 Windows 上跑一次 OCR，
//! 用實測代替推測（見 `tests/windows_ocr.rs`）。
//!
//! **小圖不會報錯，會安靜地回傳零行。** 短邊低於約 40px 時 OCR 直接放棄，
//! 而且不給任何錯誤。我們擷取的是整個螢幕所以碰不到，但還是擋在前面，
//! 免得以後改成擷取單一視窗時撞上一個不會出聲的失敗。

use anyhow::{Context, Result, bail};
use sister_core::model::OcrBlock;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::core::HSTRING;

use crate::ocr_layout::{Word, assemble_line};
use crate::traits::{Ocr, RawFrame};

/// 這個引擎不回報信心度，所以我們也不假裝有。
///
/// 用 -1 而不是 1.0 是刻意的。假設以後有人寫了
/// `WHERE confidence > 0.5`：填 -1 的話所有文字會**一次全部消失**，
/// 那是會被立刻發現的方向；填 1.0 的話則是一個永遠沒有人會發現的謊。
/// 照 THREAT_MODEL 的原則，錯要往看得見的那邊錯。
pub const CONFIDENCE_UNKNOWN: f32 = -1.0;

/// 短邊低於這個值，Windows 的 OCR 會回傳零行而且不報錯。
///
/// 官方沒有文件，這個數字來自社群長期的回報（也和 Windows 8.1 前身 API
/// 文件裡寫死的 40～2600 對得上）。
const MIN_DIMENSION: u32 = 40;

/// 問不到 `MaxImageDimension` 時的保守退路。
///
/// 官方從來沒有公布過這個值，所以**不寫死**、一律在執行期問引擎；
/// 這裡只是問不到時的下限。
const FALLBACK_MAX_DIMENSION: u32 = 2600;

/// OCR 在這台機器上到底能不能用、用哪個語言。
///
/// 這個型別存在的唯一理由是 `sister doctor` 要能誠實回答
/// 「她現在讀不讀得懂你螢幕上的中文」。
#[derive(Debug, Clone, Default)]
pub struct OcrStatus {
    /// 這台機器上裝了哪些 OCR 語言（BCP-47 tag）。
    pub available: Vec<String>,
    /// 實際會用的那一個。`None` = 完全沒有 OCR。
    pub chosen: Option<String>,
}

impl OcrStatus {
    pub fn probe(preferred: &[String]) -> Self {
        init_apartment();
        Self {
            available: available_languages(),
            chosen: pick_engine(preferred)
                .and_then(|e| recognizer_tag(&e))
                .or_else(|| {
                    // 引擎建得起來但問不出語言：仍然算可用，只是講不出是哪個
                    pick_engine(preferred).map(|_| "（未知）".to_string())
                }),
        }
    }

    /// 有 OCR，但**看起來讀不懂中文**。
    ///
    /// 這是這個模組最容易安靜出錯的地方：使用者的 Windows 沒裝中文 OCR
    /// 語言包時，引擎會退回英文，然後把滿螢幕的中文讀成一堆亂碼或空白。
    /// 錄製正常、`stats` 正常、資料庫也在長大——只有搜尋永遠是空的。
    ///
    /// 語言包可以在「設定 → 語言 → 選用功能 → 光學字元辨識」裝。
    pub fn cjk_gap(&self) -> Option<String> {
        let chosen = self.chosen.as_ref()?;
        if chosen.starts_with("zh") || chosen.starts_with("ja") || chosen.starts_with("ko") {
            return None;
        }
        Some(format!(
            "OCR 語言是 {chosen}，不是中文：螢幕上的中文會讀不出來，\
             搜尋中文將永遠是空的。已安裝的語言：{}。\
             請到「設定 → 語言 → 選用功能」加裝中文的光學字元辨識",
            if self.available.is_empty() {
                "（無）".to_string()
            } else {
                self.available.join("、")
            }
        ))
    }
}

/// Windows 內建 OCR。
///
/// 建構永遠不會失敗：沒有可用的語言時它就是一個建不出引擎的 `WindowsOcr`。
/// 錄製不該因為 OCR 缺席而停擺——畫面與脈絡還是值得留下來。
///
/// 但「不停擺」不等於「不出聲」。這裡踩過一次：建不出引擎時
/// `recognize()` 回 `Ok(Vec::new())`，上層讀起來就是「這張畫面上沒有字」，
/// 於是失敗計數是 0、摘要全綠、資料庫空的。現在它回 `Err`，由 recorder
/// 接住——畫面照樣保留，但 `last_ocr_error` 會說出原因。
pub struct WindowsOcr {
    engine: Option<OcrEngine>,
    max_dimension: u32,
    /// 交給呼叫端的緩衝區，避免每一幀都重新配置一張整個螢幕大小的記憶體。
    bgra: Vec<u8>,
}

impl WindowsOcr {
    pub fn new(preferred: &[String]) -> Self {
        init_apartment();
        let engine = pick_engine(preferred);
        let max_dimension = OcrEngine::MaxImageDimension()
            .ok()
            .filter(|d| *d > 0)
            .unwrap_or(FALLBACK_MAX_DIMENSION);
        Self {
            engine,
            max_dimension,
            bgra: Vec::new(),
        }
    }

    pub fn is_available(&self) -> bool {
        self.engine.is_some()
    }

    pub fn language(&self) -> Option<String> {
        recognizer_tag(self.engine.as_ref()?)
    }

    /// 引擎回報的單邊像素上限。畫面比它大就整張讀不了。
    pub fn max_dimension(&self) -> u32 {
        self.max_dimension
    }

    /// 拿一張內建的圖真的辨識一次，回傳讀到的每一行。
    ///
    /// 為什麼要有這個：`doctor` 原本只會說「OCR 語言 zh-Hant-TW ✓」，
    /// 那句話的意思其實只是「引擎建得起來」。實測發現引擎建得起來、語言
    /// 也對，錄了一分鐘卻一個字都沒有進資料庫——**「能建立」和「能讀字」
    /// 是兩件事，而我們只驗了前者。**
    ///
    /// 所以改成不宣稱、直接示範：讀一張答案已知的圖給使用者看。
    /// 圖是編進執行檔裡的（約 16KB），不需要外部檔案，也就不會因為
    /// 少一個檔案而變成另一種安靜的失敗。
    pub fn self_test(&mut self) -> Result<Vec<String>> {
        const IMAGE: &[u8] = include_bytes!("../../assets/ocr-selftest-zh.png");

        if self.engine.is_none() {
            bail!("沒有可用的 OCR 語言");
        }
        let img = image::load_from_memory(IMAGE)
            .context("內建的自我測試圖解不開")?
            .to_rgba8();
        let (w, h) = img.dimensions();
        let frame = RawFrame::from_rgba(0, 0, w, h, img.into_raw());
        Ok(self
            .recognize(&frame)?
            .into_iter()
            .map(|b| b.text)
            .collect())
    }

    /// 自我測試該讀到的東西。給呼叫端判斷「讀到了但讀錯」用。
    pub const SELF_TEST_EXPECTS: &'static [&'static str] = &["本期應繳金額", "客服專線"];
}

impl Ocr for WindowsOcr {
    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>> {
        // 這兩個以前都回 `Ok(Vec::new())`，理由是「錄製不該因為 OCR 缺席而
        // 停擺」——那個目標是對的，但作法錯了：回空的等於告訴上層「這張畫面
        // 上沒有字」，於是 `ocr_failures` 是 0、`last_ocr_error` 是 None、
        // 摘要一片綠，而資料庫裡一個字都沒有。錄製照樣不會停（recorder 會
        // 接住這個錯、保留畫面），差別只在於它**說得出來**。
        // 呼叫端的約定先驗（跟這台機器裝了什麼無關），環境的問題後驗。
        let Some(rgba) = frame.rgba.as_deref() else {
            // 擷取端交出一張沒有像素的畫面。那不是「畫面上沒有字」，是 bug。
            bail!("這一幀沒有像素資料（{}x{}）", frame.width, frame.height);
        };
        let Some(engine) = self.engine.as_ref() else {
            bail!(
                "OCR 引擎沒有建起來——這台機器上沒有可用的 OCR 語言包。\
                 到「設定 → 語言 → 選用功能」加裝光學字元辨識，再跑 `sister doctor` 確認"
            );
        };
        let (w, h) = (frame.width, frame.height);

        let needed = (w as usize) * (h as usize) * 4;
        if needed == 0 || rgba.len() < needed {
            bail!(
                "影像緩衝區和宣告的尺寸對不起來：{w}x{h} 需要 {needed} bytes，只有 {}",
                rgba.len()
            );
        }
        // 太小的圖引擎會安靜地回傳零行，不會報錯——擋在這裡至少不會被誤會成
        // 「這張畫面上沒有字」
        if w.min(h) < MIN_DIMENSION {
            return Ok(Vec::new());
        }
        if w.max(h) > self.max_dimension {
            bail!(
                "畫面 {w}x{h} 超過引擎上限 {}；請調小 capture.max_long_edge",
                self.max_dimension
            );
        }

        // RGBA → BGRA。順便把 alpha 壓成不透明：來源若留下未定義的 alpha，
        // 在 premultiplied 的解讀下整張圖會被當成全透明，OCR 就什麼都讀不到。
        self.bgra.clear();
        self.bgra.reserve(needed);
        for px in rgba[..needed].chunks_exact(4) {
            self.bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }

        // 底下每一步都掛了 context。理由不是好看：這支程式跑在別人的機器上，
        // 我摸不到。一句「OCR 失敗」等於沒說，而「哪一步失敗」是使用者
        // 貼一行回來就能定位的東西。
        let buffer =
            CryptographicBuffer::CreateFromByteArray(&self.bgra).context("CreateFromByteArray")?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            w as i32,
            h as i32,
            BitmapAlphaMode::Premultiplied,
        )
        .with_context(|| format!("CreateCopyWithAlphaFromBuffer {w}x{h}"))?;

        // `join()` 會阻塞而且不會 pump 訊息。錄製迴圈這條執行緒是 MTA
        // （見 `init_apartment`），所以阻塞是安全的；放到 STA 上會卡死訊息迴圈。
        let result = engine
            .RecognizeAsync(&bitmap)
            .context("RecognizeAsync")?
            .join()
            .context("等 RecognizeAsync 的結果")?;

        let lines = result.Lines().context("OcrResult::Lines")?;
        let mut blocks = Vec::with_capacity(lines.Size().unwrap_or(0) as usize);
        for line in &lines {
            let mut words = Vec::new();
            for word in &line.Words().context("OcrLine::Words")? {
                let r = word.BoundingRect().context("OcrWord::BoundingRect")?;
                words.push(Word {
                    text: word.Text().context("OcrWord::Text")?.to_string(),
                    x: r.X,
                    y: r.Y,
                    w: r.Width,
                    h: r.Height,
                });
            }
            // 這裡刻意不用 `line.Text()`：它是用空白把詞接起來的，
            // 中文被逐字切開時會變成「本 期 應 繳」。理由見 `crate::ocr_layout`。
            if let Some(l) = assemble_line(&words) {
                blocks.push(OcrBlock {
                    text: l.text,
                    x: l.x,
                    y: l.y,
                    w: l.w,
                    h: l.h,
                    confidence: CONFIDENCE_UNKNOWN,
                });
            }
        }
        Ok(blocks)
    }
}

/// 依偏好順序挑一個裝得起來的引擎。
///
/// `IsLanguageSupported` 是真正的閘門。`TryCreateFromLanguage` 在語言不支援時
/// 回傳的是 null，在 Rust 這邊會變成一個 `Err`——但那個 `Err` 的 HRESULT
/// 讀起來是 0（成功）。所以**不能**去判斷它的錯誤碼，只能看成敗。
fn pick_engine(preferred: &[String]) -> Option<OcrEngine> {
    for tag in preferred {
        let Ok(lang) = Language::CreateLanguage(&HSTRING::from(tag.as_str())) else {
            continue;
        };
        if !OcrEngine::IsLanguageSupported(&lang).unwrap_or(false) {
            continue;
        }
        if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
            return Some(engine);
        }
    }
    // 偏好清單一個都沒裝：退回系統自己的選擇，好過完全沒有 OCR。
    // 這件事會透過 `OcrStatus::cjk_gap` 被講出來，不會靜靜地發生。
    OcrEngine::TryCreateFromUserProfileLanguages().ok()
}

fn recognizer_tag(engine: &OcrEngine) -> Option<String> {
    Some(
        engine
            .RecognizerLanguage()
            .ok()?
            .LanguageTag()
            .ok()?
            .to_string(),
    )
}

fn available_languages() -> Vec<String> {
    let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() else {
        return Vec::new();
    };
    (&langs)
        .into_iter()
        .filter_map(|l| Some(l.LanguageTag().ok()?.to_string()))
        .collect()
}

/// 讓這條執行緒進入 MTA。
///
/// `OcrEngine` 是 agile 的（`ThreadingModel.Both`），所以哪個 apartment 都能用；
/// 真正要緊的是我們會用 `join()` 阻塞等結果，而在 STA 上阻塞會卡死訊息迴圈。
///
/// `RPC_E_CHANGED_MODE` 代表這條執行緒早就被別人初始化成 STA 了，那不是錯誤，
/// 是既成事實——照樣可以用，只是我們沒得選。
fn init_apartment() {
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
}
