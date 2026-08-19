//! Windows 擷取後端。
//!
//! 五個來源各自獨立實作最小的 trait，再用 [`CompositeBackend`] 組起來。
//! 能力是逐項降級的：OCR 還沒接上不代表焦點與剪貼簿不能先跑。
//!
//! 目前**還沒有**的東西，以及它們各自的代價，都在 [`Capabilities`] 裡誠實
//! 列出來，由 `sister doctor` 直接顯示給使用者看。缺功能不可怕，
//! 缺功能而使用者以為有才可怕。

pub mod clipboard;
pub mod focus;
pub mod input;
pub mod ocr;
pub mod screen;
pub mod uia;

use anyhow::Result;
use sister_core::config::Config;
use sister_core::now_ms;

use crate::traits::{Backend, CompositeBackend};

/// 這台機器上這個後端實際做得到什麼。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    // `screen` / `focus` / `clipboard` 這三個欄位曾經在這裡，永遠是 `true`，
    // 而且沒有任何一行程式碼讀過它們。那正是這個專案在對付的東西：一個
    // 回報 ✓ 但什麼都沒驗證的能力欄位。真的要知道前景讀不讀得到，
    // `doctor` 現在會當場問一次——見那裡的 `focus_probe`。
    /// 瀏覽器網址（需要 UIA）。**沒有它，`excluded_urls` 整組規則不會生效。**
    pub url: bool,
    /// 輸入 hook 的三態。**不能是布林**：`doctor` 不會去裝 hook，而
    /// 「沒去裝」不等於「裝失敗」——把兩者壓成 false 會產生一則永遠錯的
    /// 警告，然後整個警告區塊都會被使用者學會忽略。
    pub input: input::HookState,
    pub ocr: bool,
    /// OCR 實際挑中的語言，以及這台機器上裝了哪些。
    ///
    /// 「有沒有 OCR」是個布林，但「讀不讀得懂中文」不是——所以兩件事分開存。
    pub ocr_language: Option<String>,
    pub ocr_languages_available: Vec<String>,
}

impl Capabilities {
    pub fn current(config: &Config) -> Self {
        let ocr = ocr::OcrStatus::probe(&config.capture.ocr_languages);
        Self {
            // 「UIA 建得起來」而已。真的讀不讀得到位址列要在有瀏覽器開著
            // 的時候才知道——那件事由 `doctor` 的實測那一段回答，不是這裡。
            url: uia::Uia::probe(),
            input: input::WindowsInput::state(),
            ocr: ocr.chosen.is_some(),
            ocr_language: ocr.chosen,
            ocr_languages_available: ocr.available,
        }
    }

    /// 寫給另一個行程看的那一份。見 [`sister_core::capabilities`]。
    ///
    /// 只帶原始事實過去，不帶結論——設定頁要拿它和**現在**的規則清單重算，
    /// 不然「上一場錄製開始時他還沒寫那條規則」會讓那一頁永遠沉默。
    ///
    /// 這一份是**開機探測**：`url_capture`、`browser_ticks`、`url_reads` 三個
    /// 都留在預設值，因為這一刻還沒有任何一場錄製發生過。錄製途中由
    /// `record` 迴圈反覆蓋掉（見 `sister_core::capabilities::write`），
    /// 那才是這個檔案不再凍在開機那一刻的地方。
    pub fn report(&self) -> sister_core::capabilities::Report {
        sister_core::capabilities::Report {
            at: now_ms(),
            url: self.url,
            // 只有「試過而且失敗」才算失效。還沒試過不是問題，`record` 起來時
            // 才會裝——在那之前吵，吵的是一件還沒發生的事。
            input_hook_failed: self.input == input::HookState::Failed,
            ..Default::default()
        }
    }

    /// 因為能力缺席而**失效的隱私規則**。
    ///
    /// 和一般的功能缺口分開講：使用者可以接受「還不會 OCR」，但她必須知道
    /// 「你設定的網銀排除規則現在一條都不會生效」。這種事不能只寫在
    /// release note 裡。
    ///
    /// 判斷本身住在 [`sister_core::capabilities::Report`]：設定頁那個行程
    /// 沒有這裡的相依，卻要講出一模一樣的那句話。同一份判斷兩個地方寫，
    /// 遲早會變成兩句不一樣的話——而使用者會相信比較好聽的那一句。
    pub fn broken_privacy_rules(
        &self,
        privacy: &sister_core::config::PrivacyConfig,
    ) -> Vec<sister_core::capabilities::Broken> {
        self.report().broken_privacy_rules(privacy)
    }

    /// 看起來在運作、實際上不會有結果的地方。
    ///
    /// 和 [`Self::broken_privacy_rules`] 分開：那個講的是「她記了不該記的」，
    /// 這個講的是「她其實什麼都沒記住，但你不會發現」。兩者都是安靜的失敗，
    /// 但補救方式完全不同，混在一起講只會兩邊都被忽略。
    pub fn silently_degraded(&self, config: &Config) -> Vec<String> {
        let mut out = Vec::new();
        if config.capture.ocr {
            if !self.ocr {
                out.push(
                    "這台機器沒有任何 OCR 語言：畫面會被記下來，但上面的字\
                     一個都不會進資料庫，搜尋永遠是空的"
                        .into(),
                );
            } else if let Some(gap) = (ocr::OcrStatus {
                available: self.ocr_languages_available.clone(),
                chosen: self.ocr_language.clone(),
            })
            .cjk_gap()
            {
                out.push(gap);
            }
        }
        out
    }
}

/// 組出 Windows 後端。
pub fn backend(config: &Config) -> Result<impl Backend + use<>> {
    enable_dpi_awareness();

    Ok(CompositeBackend {
        name: "windows-gdi".to_string(),
        screen: screen::WindowsScreen::new(),
        focus: focus::WindowsFocus::new(),
        clipboard: clipboard::WindowsClipboard::new(),
        input: input::WindowsInput::start(now_ms(), config.capture.input_window_secs),
        ocr: ocr::WindowsOcr::new(&config.capture.ocr_languages),
    })
}

/// 宣告自己認得 per-monitor DPI。
///
/// 不做這件事的話，在高 DPI 螢幕上 Windows 會餵給我們一張被系統放大過的
/// 模糊點陣圖，而不是原生像素。那張圖 OCR 幾乎讀不出字——縮放後的字緣
/// 全是插值出來的灰階。這一行直接決定文字品質。
fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    // 已經設過（或 manifest 裡設過）會失敗，那是正常的
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一台什麼都做得到的機器。測試從這裡出發，只改要測的那一項，
    /// 這樣斷言就不會受跑測試的那台機器裝了什麼影響。
    fn fully_capable() -> Capabilities {
        Capabilities {
            url: true,
            input: input::HookState::Active,
            ocr: true,
            ocr_language: Some("zh-Hant-TW".into()),
            ocr_languages_available: vec!["zh-Hant-TW".into()],
        }
    }

    /// 缺 URL 擷取必須被說成隱私問題，不能只算功能缺口——
    /// 使用者設了網銀排除規則，她有權知道那些規則現在是空的。
    #[test]
    fn missing_url_capture_is_reported_as_a_privacy_gap() {
        let caps = Capabilities {
            url: false,
            ..fully_capable()
        };
        let broken = caps.broken_privacy_rules(&Config::default().privacy);
        assert!(
            broken.iter().any(|w| w.message.contains("excluded_urls")),
            "沒有把失效的網址規則講出來：{broken:?}"
        );
    }

    #[test]
    fn a_fully_capable_backend_reports_nothing_broken() {
        let config = Config::default();
        assert!(
            fully_capable()
                .broken_privacy_rules(&config.privacy)
                .is_empty()
        );
        assert!(fully_capable().silently_degraded(&config).is_empty());
    }

    /// 沒有 OCR 的話，錄製看起來一切正常但搜尋永遠是空的。
    /// 這件事必須有人講出來。
    #[test]
    fn missing_ocr_is_reported_as_a_silent_failure() {
        let caps = Capabilities {
            ocr: false,
            ocr_language: None,
            ocr_languages_available: vec![],
            ..fully_capable()
        };
        let warnings = caps.silently_degraded(&Config::default());
        assert!(
            warnings.iter().any(|w| w.contains("搜尋")),
            "沒有講出「搜尋會是空的」這個實際後果：{warnings:?}"
        );
    }

    /// 有 OCR 但只讀得懂英文，比完全沒有 OCR 更陰險：
    /// 英文介面讀得到，中文內容讀不到，看起來就只是「她沒記到那件事」。
    #[test]
    fn an_english_only_ocr_engine_is_reported_as_a_gap() {
        let caps = Capabilities {
            ocr: true,
            ocr_language: Some("en-US".into()),
            ocr_languages_available: vec!["en-US".into()],
            ..fully_capable()
        };
        let warnings = caps.silently_degraded(&Config::default());
        assert!(
            warnings.iter().any(|w| w.contains("中文")),
            "英文引擎讀不懂中文這件事沒有被講出來：{warnings:?}"
        );
    }

    /// 使用者自己把 OCR 關掉時，不該再嘮叨語言的事。
    #[test]
    fn disabling_ocr_on_purpose_is_not_a_warning() {
        let mut config = Config::default();
        config.capture.ocr = false;
        let caps = Capabilities {
            ocr: false,
            ocr_language: None,
            ocr_languages_available: vec![],
            ..fully_capable()
        };
        assert!(caps.silently_degraded(&config).is_empty());
    }
}
