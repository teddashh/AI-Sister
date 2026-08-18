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
pub mod screen;

use anyhow::Result;
use sister_core::config::Config;
use sister_core::now_ms;

use crate::traits::{Backend, CompositeBackend, NullOcr};

/// 這台機器上這個後端實際做得到什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub screen: bool,
    pub focus: bool,
    /// 瀏覽器網址（需要 UIA）。**沒有它，`excluded_urls` 整組規則不會生效。**
    pub url: bool,
    pub clipboard: bool,
    pub input: bool,
    pub ocr: bool,
}

impl Capabilities {
    pub fn current() -> Self {
        Self {
            screen: true,
            focus: true,
            // UIA 讀址欄還沒接（見 SPEC §5.2）
            url: false,
            clipboard: true,
            input: input::WindowsInput::hooks_active(),
            // Windows 的 OCR 走 PP-OCRv5/ONNX，和平台無關，另外接
            ocr: false,
        }
    }

    /// 因為能力缺席而**失效的隱私規則**。
    ///
    /// 和一般的功能缺口分開講：使用者可以接受「還不會 OCR」，但她必須知道
    /// 「你設定的網銀排除規則現在一條都不會生效」。這種事不能只寫在
    /// release note 裡。
    pub fn broken_privacy_rules(&self, config: &Config) -> Vec<String> {
        let mut out = Vec::new();
        if !self.url && !config.privacy.excluded_urls.is_empty() {
            out.push(format!(
                "沒有 UIA 網址擷取：{} 條 excluded_urls 規則（網銀、登入頁）\
                 目前不會生效，瀏覽器畫面只靠視窗標題規則過濾",
                config.privacy.excluded_urls.len()
            ));
        }
        if !self.input {
            out.push("輸入 hook 沒裝上：節奏訊號這個 session 會是空的".into());
        }
        out
    }
}

/// 組出 Windows 後端。
pub fn backend(config: &Config) -> Result<impl Backend + use<>> {
    enable_dpi_awareness();

    Ok(CompositeBackend {
        name: "windows-gdi".to_string(),
        screen: screen::WindowsScreen::new(config.capture.max_long_edge),
        focus: focus::WindowsFocus,
        clipboard: clipboard::WindowsClipboard::new(),
        input: input::WindowsInput::start(now_ms()),
        ocr: NullOcr,
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

    /// 缺 URL 擷取必須被說成隱私問題，不能只算功能缺口——
    /// 使用者設了網銀排除規則，她有權知道那些規則現在是空的。
    #[test]
    fn missing_url_capture_is_reported_as_a_privacy_gap() {
        let config = Config::default();
        let caps = Capabilities {
            url: false,
            input: true,
            ..Capabilities::current()
        };
        let broken = caps.broken_privacy_rules(&config);
        assert!(
            broken.iter().any(|w| w.contains("excluded_urls")),
            "沒有把失效的網址規則講出來：{broken:?}"
        );
    }

    #[test]
    fn a_fully_capable_backend_reports_nothing_broken() {
        let caps = Capabilities {
            screen: true,
            focus: true,
            url: true,
            clipboard: true,
            input: true,
            ocr: true,
        };
        assert!(caps.broken_privacy_rules(&Config::default()).is_empty());
    }
}
