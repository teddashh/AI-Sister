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
    /// 鍵盤焦點在密碼欄上。**這一欄不落地**，它只活到排除判定為止。
    ///
    /// 用 `bool` 而不是 `Option<bool>` 是刻意的：「不知道」要在**來源那一端**
    /// 就決定成 `true`（見 `sister_capture::windows::uia::Reading::should_skip_frame`），
    /// 而不是一路傳下來讓每個看到它的人各自決定一次要不要保守。
    /// 那種設計遲早會有一個地方選錯，而且沒有人會發現。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub password_field: bool,
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
    /// 每一種。`session_marks_sql` 從這裡長出來，所以少列一種就是少扣一種。
    /// 有 `all_lists_every_kind` 這條測試釘住（新增一種會讓它編不過）。
    pub const ALL: [SystemKind; 9] = [
        SystemKind::Lock,
        SystemKind::Unlock,
        SystemKind::Sleep,
        SystemKind::Wake,
        SystemKind::CapturePaused,
        SystemKind::CaptureResumed,
        SystemKind::Excluded,
        SystemKind::SessionStart,
        SystemKind::SessionEnd,
    ];

    /// **這一列講的是那場錄製本身，不是他那天發生了什麼。**
    ///
    /// `Recorder::new` 開場寫一列 `SessionStart`、`finish` 收尾寫一列
    /// `SessionEnd`。兩列都不是她「記下來的東西」——它們是那個容器上的標籤，
    /// 和 `sessions` 那一列是同一種東西。所以凡是在問「她記下來的還剩不剩」
    /// 的地方都要把它們扣掉。其餘七種（鎖定、睡眠、暫停、被規則擋掉……）講
    /// 的是他那天真的發生過的事，要算。
    ///
    /// 少了這個區分會出兩件事，而兩件都出過：
    ///
    /// - `forget` 清光一整天之後，她**下一次開始錄**寫的那一列 `SessionStart`
    ///   就足以讓 `DbStats::nothing_recorded_left()` 翻成 false，於是
    ///   `Emptiness` 讓開 `Erased`、接到最寬的 `Fresh`——`facts` 說「她還沒錄
    ///   過」、`doctor` 說「還沒有任何內容」、`stats` 上那個 ⚠ 整個不見。他一
    ///   秒前才親手刪掉的一整天，被一列開場標籤否認了。
    /// - `retention::delete_empty_sessions` 永遠找不到一場空的錄製：`finish`
    ///   **先**寫 `SessionEnd` **再**呼叫 `end_session`，於是它自己剛剛寫的那
    ///   一列讓那一場「不空」。那道清掃在產品裡從來沒有刪掉過任何一列，而
    ///   `forget` 那句「等她收工的時候那一列就會跟著走」是照著它寫的。
    ///
    /// `match` 不寫 `_`：多一種 `SystemKind` 就編不過，而不是安靜地被歸成
    /// 「她記下來的東西」。
    pub const fn is_session_mark(self) -> bool {
        match self {
            SystemKind::SessionStart | SystemKind::SessionEnd => true,
            SystemKind::Lock
            | SystemKind::Unlock
            | SystemKind::Sleep
            | SystemKind::Wake
            | SystemKind::CapturePaused
            | SystemKind::CaptureResumed
            | SystemKind::Excluded => false,
        }
    }

    /// 給 SQL 用的 `IN (…)` 內容，從 [`Self::is_session_mark`] 長出來。
    ///
    /// 手寫一份字串常數的話，`as_str` 改一個字就會有一支 SQL 安靜地不再命中
    /// 任何列——症狀是上面那兩句謊話回來，而沒有任何東西編不過。
    pub fn session_marks_sql() -> String {
        let marks: Vec<_> = Self::ALL
            .iter()
            .filter(|k| k.is_session_mark())
            .map(|k| format!("'{}'", k.as_str()))
            .collect();
        format!("({})", marks.join(","))
    }

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

/// 一場錄製為什麼結束。
///
/// 「她停了」和「她**為什麼**停了」是兩個問題，而以前只答得出第一個：
/// `session_end` 的 `detail` 一律是 `None`，於是「你按了停止」「時間到了」
/// 「同意書被撤回」在磁碟上長得一模一樣。三件事的下一步完全不同——第一件
/// 什麼都不用做，第三件是「她從現在起什麼都不會記」。
///
/// 沒有 `Crashed`：當掉的那一場**寫不了任何東西**。它的樣子是
/// `sessions.ended_at` 留在 `NULL`（見 [`crate::db::Db::last_session`]），
/// 而一個由當掉的行程自己宣告的「我當掉了」本來就不存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// `--duration` 跑完了。
    Duration,
    /// 有人按了停止（字母人上的那一顆、系統匣、或 `sister stop`）。
    Requested,
    /// Ctrl-C。
    Interrupted,
    /// 第一張同意書在半路上被撤回。
    ConsentRevoked,
}

impl EndReason {
    /// 存進 `system_events.detail` 的字串。
    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::Duration => "duration",
            EndReason::Requested => "requested",
            EndReason::Interrupted => "interrupted",
            EndReason::ConsentRevoked => "consent-revoked",
        }
    }

    /// 講給人聽的那一句。存的是上面那組字串而不是這一句：文案會改，而改了
    /// 之後舊的紀錄不該變成讀不懂的東西。
    pub fn describe(text: &str) -> &str {
        match text {
            "duration" => "錄滿了你指定的時間",
            "requested" => "你按了停止",
            "interrupted" => "在終端機按了 Ctrl-C",
            "consent-revoked" => "第一張同意書被撤回",
            other => other,
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

    /// `ALL` 少列一種，`session_marks_sql` 就會少扣一種——而那是一支照樣跑得
    /// 完的 SQL，沒有任何東西會編不過。
    ///
    /// 底下那個 `match` 沒有 `_`：**多一種 `SystemKind`，這條測試就編不過**，
    /// 於是加變體的人一定會走到這裡。這是這個檔案裡唯一一個把「別忘了」變成
    /// 編譯錯誤的辦法（`DbStats` 那邊用的是解構的 E0027，同一招）。
    #[test]
    fn all_lists_every_kind() {
        fn index(k: SystemKind) -> usize {
            match k {
                SystemKind::Lock => 0,
                SystemKind::Unlock => 1,
                SystemKind::Sleep => 2,
                SystemKind::Wake => 3,
                SystemKind::CapturePaused => 4,
                SystemKind::CaptureResumed => 5,
                SystemKind::Excluded => 6,
                SystemKind::SessionStart => 7,
                SystemKind::SessionEnd => 8,
            }
        }
        for (i, k) in SystemKind::ALL.iter().enumerate() {
            assert_eq!(index(*k), i, "ALL 的第 {i} 個是 {k:?}，順序或內容對不上");
        }
    }

    /// SQL 那一串是從 `as_str` 長出來的，不是另外手寫的一份。
    #[test]
    fn session_marks_sql_is_built_from_as_str() {
        let sql = SystemKind::session_marks_sql();
        for k in SystemKind::ALL {
            let quoted = format!("'{}'", k.as_str());
            assert_eq!(
                sql.contains(&quoted),
                k.is_session_mark(),
                "{k:?} 在 {sql} 裡的有無，和 is_session_mark 對不上",
            );
        }
        // 「他那天發生了什麼」那幾種一個都不可以在裡面——`Excluded` 尤其：
        // 那一列是「她看到了、而且照規則沒記」，是這份紀錄裡最需要留下來的
        // 一種證據。
        assert!(!sql.contains("'excluded'"), "{sql}");
    }
}
