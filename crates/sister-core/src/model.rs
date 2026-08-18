//! L0 訊號與 L1 事實的領域型別。
//!
//! 憲法（SPEC §0）：這一層的東西**全部由程式寫入**，不經 LLM、不可改寫。
//! 模型只能碰 L2/L3，而 L2/L3 在 Phase 4 之前根本不存在。

use serde::{Deserialize, Serialize};

/// Unix epoch 毫秒。系統內所有時間戳的唯一表示法。
pub type Millis = i64;

/// 現在時間（毫秒）。
pub fn now_ms() -> Millis {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as Millis)
        .unwrap_or(0)
}

/// 擷取當下的前景脈絡。附在 frame 上，也可獨立成為 focus 事件。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusSnapshot {
    /// 穩定識別碼：Windows 用 executable 名，macOS 用 bundle id。
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    /// 僅瀏覽器；抓不到就是 None（失敗容忍，不重試不阻塞）。
    pub url: Option<String>,
    pub pid: Option<i64>,
}

impl FocusSnapshot {
    /// 用於排除比對的小寫 app 識別字串。
    pub fn app_key(&self) -> String {
        self.app_id
            .as_deref()
            .or(self.app_name.as_deref())
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}

/// OCR 出來的一塊文字（一行或一個區域）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrBlock {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub confidence: f32,
}

/// 一張被保留下來的畫面。
///
/// 注意 `image`：`None` 代表 text-only 保留模式（第三張同意書關閉時的預設），
/// 此時 OCR 文字仍會落地，但畫面本身不存。
#[derive(Debug, Clone)]
pub struct FrameCapture {
    pub ts: Millis,
    pub monitor: i32,
    pub width: u32,
    pub height: u32,
    pub dhash: u64,
    pub image: Option<Vec<u8>>,
    pub image_ext: &'static str,
    pub ocr: Vec<OcrBlock>,
    pub focus: FocusSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusKind {
    Focus,
    TitleChange,
    UrlChange,
}

impl FocusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FocusKind::Focus => "focus",
            FocusKind::TitleChange => "title",
            FocusKind::UrlChange => "url",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusEvent {
    pub ts: Millis,
    pub kind: FocusKind,
    pub snapshot: FocusSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardKind {
    Text,
    Image,
    Files,
}

impl ClipboardKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClipboardKind::Text => "text",
            ClipboardKind::Image => "image",
            ClipboardKind::Files => "files",
        }
    }
}

/// 剪貼簿事件。
///
/// `text` 為 `None` 有兩種可能：非文字內容，或偵測到疑似秘密而**刻意不落地**
/// （SPEC §11.2：只記「複製了一個秘密」這件事，不記內容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardEvent {
    pub ts: Millis,
    pub kind: ClipboardKind,
    pub text: Option<String>,
    pub byte_len: i64,
    pub truncated: bool,
    pub secret_suspected: bool,
    pub source_app: Option<String>,
}

/// 輸入動態聚合值（預設每 10 秒一筆）。
///
/// **永遠不記按鍵內容**——這裡只有節奏與計數。這是「事後補不回來」清單
/// （SPEC §2.1）的主要載體：卡住、專注、焦慮都藏在這些數字裡，
/// 而它們幾乎零成本、純程式，不需要任何模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputMetrics {
    pub ts_start: Millis,
    pub ts_end: Millis,
    pub keystrokes: i64,
    pub clicks: i64,
    pub mouse_px: i64,
    pub scroll_ticks: i64,
    pub window_switches: i64,
    pub idle_ms: i64,
    pub typing_bursts: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemKind {
    Lock,
    Unlock,
    Sleep,
    Wake,
    CapturePaused,
    CaptureResumed,
    /// 因排除規則而略過一段擷取（SPEC §11.2，capture 當下就排除）。
    Excluded,
    SessionStart,
    SessionEnd,
}

impl SystemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SystemKind::Lock => "lock",
            SystemKind::Unlock => "unlock",
            SystemKind::Sleep => "sleep",
            SystemKind::Wake => "wake",
            SystemKind::CapturePaused => "pause",
            SystemKind::CaptureResumed => "resume",
            SystemKind::Excluded => "excluded",
            SystemKind::SessionStart => "session_start",
            SystemKind::SessionEnd => "session_end",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemEvent {
    pub ts: Millis,
    pub kind: SystemKind,
    pub detail: Option<String>,
}

/// 感官層送出的訊號。平台後端只產生這個 enum，其餘全系統與平台無關。
#[derive(Debug, Clone)]
pub enum Signal {
    Frame(Box<FrameCapture>),
    Focus(FocusEvent),
    Clipboard(ClipboardEvent),
    Input(InputMetrics),
    System(SystemEvent),
}

impl Signal {
    pub fn ts(&self) -> Millis {
        match self {
            Signal::Frame(f) => f.ts,
            Signal::Focus(f) => f.ts,
            Signal::Clipboard(c) => c.ts,
            Signal::Input(i) => i.ts_start,
            Signal::System(s) => s.ts,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Signal::Frame(_) => "frame",
            Signal::Focus(_) => "focus",
            Signal::Clipboard(_) => "clipboard",
            Signal::Input(_) => "input",
            Signal::System(_) => "system",
        }
    }
}

/// 文字進入索引時的來源分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Ocr,
    Clipboard,
    WindowTitle,
    Url,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Ocr => "ocr",
            SourceKind::Clipboard => "clipboard",
            SourceKind::WindowTitle => "window_title",
            SourceKind::Url => "url",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        Some(match s {
            "ocr" => SourceKind::Ocr,
            "clipboard" => SourceKind::Clipboard,
            "window_title" => SourceKind::WindowTitle,
            "url" => SourceKind::Url,
            _ => return None,
        })
    }
}

/// 統一文字層的一筆紀錄（FTS 的 external content）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub id: i64,
    pub ts: Millis,
    pub source_kind: SourceKind,
    pub source_id: Option<i64>,
    pub frame_id: Option<i64>,
    pub app_id: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
    pub text: String,
}

/// 檢索結果：一段命中的文字 + 它的出處。
///
/// 「每一句話都查得到出處」（SPEC §0.5）在資料層的落點就是這個結構——
/// 任何回答都必須能還原成一組 `SearchHit`。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub chunk_id: i64,
    pub ts: Millis,
    pub source_kind: SourceKind,
    pub frame_id: Option<i64>,
    pub app_id: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
    pub text: String,
    /// 命中片段（FTS snippet），含 `[` `]` 標記。
    pub snippet: String,
    pub score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_plausible() {
        // 2026-01-01 < now < 2100-01-01，抓到時鐘完全壞掉的情況。
        let t = now_ms();
        assert!(t > 1_767_225_600_000, "now_ms too small: {t}");
        assert!(t < 4_102_444_800_000, "now_ms too large: {t}");
    }

    #[test]
    fn app_key_prefers_id_and_lowercases() {
        let f = FocusSnapshot {
            app_id: Some("Chrome.exe".into()),
            app_name: Some("Google Chrome".into()),
            ..Default::default()
        };
        assert_eq!(f.app_key(), "chrome.exe");

        let f2 = FocusSnapshot {
            app_id: None,
            app_name: Some("KeePassXC".into()),
            ..Default::default()
        };
        assert_eq!(f2.app_key(), "keepassxc");

        assert_eq!(FocusSnapshot::default().app_key(), "");
    }

    #[test]
    fn signal_ts_and_label_agree() {
        let s = Signal::System(SystemEvent {
            ts: 42,
            kind: SystemKind::Lock,
            detail: None,
        });
        assert_eq!(s.ts(), 42);
        assert_eq!(s.label(), "system");

        let c = Signal::Clipboard(ClipboardEvent {
            ts: 7,
            kind: ClipboardKind::Text,
            text: None,
            byte_len: 0,
            truncated: false,
            secret_suspected: true,
            source_app: None,
        });
        assert_eq!(c.ts(), 7);
        assert_eq!(c.label(), "clipboard");
    }

    #[test]
    fn source_kind_roundtrips() {
        for k in [
            SourceKind::Ocr,
            SourceKind::Clipboard,
            SourceKind::WindowTitle,
            SourceKind::Url,
        ] {
            assert_eq!(SourceKind::from_str_kind(k.as_str()), Some(k));
        }
        assert_eq!(SourceKind::from_str_kind("nope"), None);
    }
}
