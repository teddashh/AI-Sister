//! 剪貼簿監看。
//!
//! 用 `GetClipboardSequenceNumber` 當水位：它不需要開啟剪貼簿、不會和別的
//! 程式搶鎖，每次呼叫幾乎免費。只有水位變了才真的去讀內容。
//!
//! 兩個水位相關的性質是隱私設計的一部分，不是最佳化：
//!
//! 1. **啟動時只對齊水位、不讀內容。** 程式開起來的前一秒使用者可能剛從
//!    密碼管理員複製了密碼。那份內容不屬於「她開始記錄之後發生的事」。
//! 2. **被排除時要推水位（[`WindowsClipboard::skip`]）。** 只是不讀的話，
//!    密碼會留在剪貼簿上，等她切回瀏覽器的下一個 tick 照樣被撈進資料庫——
//!    排除規則只是延後了洩漏，沒有擋掉它。
//!
//! `sequence`、owner、format 與 data 必須是同一份快照。先讀 safe owner、關掉
//! clipboard、再打開讀 data 會有 TOCTOU：中間若由被排除程式換掉內容，就會把
//! 被排除的 bytes 錯配給 safe owner。因此真正的 poll 只開一次 clipboard，在
//! 同一把鎖裡讀完，並於交出內容前重驗 sequence。

use std::num::NonZeroU32;

use anyhow::Result;
use sister_core::model::{ClipboardEvent, ClipboardKind, Millis};
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_HDROP, CF_UNICODETEXT};

use super::focus;
use crate::traits::{ClipboardSource, ClipboardWatermark};

pub struct WindowsClipboard {
    /// `None` 不等於 sequence 0；它明確表示從未建立過可信水位。
    /// Windows 文件規定 0 代表呼叫端無權讀取 sequence，不能拿來冒充基準線。
    last_seq: Option<NonZeroU32>,
}

impl Default for WindowsClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsClipboard {
    pub fn new() -> Self {
        Self { last_seq: None }
    }

    fn prime(&mut self) -> ClipboardWatermark {
        let Some(sequence) = locked_sequence_snapshot() else {
            return ClipboardWatermark::Unknown;
        };
        self.last_seq = Some(sequence);
        ClipboardWatermark::Established
    }
}

impl ClipboardSource for WindowsClipboard {
    fn poll(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
        // 這一次只是廉價的 changed gate，絕不拿它推水位。sequence == 0 是
        // 「目前問不到」，不是「剪貼簿從未變動」。
        let Some(observed) = clipboard_sequence() else {
            return Ok(None);
        };

        if self.last_seq.is_none() {
            // 啟動第一次只在成功拿到 clipboard lock 後建立基準，不讀舊內容。
            let _ = self.prime();
            return Ok(None);
        }
        if self.last_seq == Some(observed) {
            return Ok(None);
        }

        // 任一步失敗或前後 sequence 不一致，都不交出部分內容，也不推水位。
        // 下一拍會從同一個 last_seq 再嘗試。
        let Some(snapshot) = read_snapshot() else {
            return Ok(None);
        };
        if self.last_seq == Some(snapshot.sequence) {
            return Ok(None);
        }

        // 必須等完整、穩定的快照形成後才能提交水位。
        self.last_seq = Some(snapshot.sequence);
        let Some(kind) = snapshot.kind else {
            // 認不得的格式：連「複製過東西」都不記，因為我們無法描述它。
            return Ok(None);
        };

        Ok(Some(ClipboardEvent {
            ts,
            kind,
            byte_len: snapshot.text.as_deref().map_or(0, |text| text.len() as i64),
            text: snapshot.text,
            truncated: false,
            // 秘密偵測與截斷歸 recorder：政策在那裡，設定也在那裡。
            secret_suspected: false,
            // 問不到來源就保持 None；recorder 的 source gate 會 fail closed。
            source_app: snapshot.source_app,
        }))
    }

    fn skip(&mut self, _ts: Millis) -> Result<ClipboardWatermark> {
        // 被排除時不碰 owner／format／data，只在持有 clipboard lock 時重驗
        // nonzero sequence。建立失敗就明確回 Unknown，且保留舊水位。
        Ok(self.prime())
    }
}

struct ClipboardSnapshot {
    sequence: NonZeroU32,
    source_app: Option<String>,
    kind: Option<ClipboardKind>,
    text: Option<String>,
}

/// `OpenClipboard` 成功後保證所有 return path 都會關閉。
struct ClipboardLock;

impl ClipboardLock {
    fn open() -> Option<Self> {
        // 別重試、別等。搶不到就算了，我們一秒後還會再來。
        unsafe { OpenClipboard(None) }.ok()?;
        Some(Self)
    }
}

impl Drop for ClipboardLock {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

fn clipboard_sequence() -> Option<NonZeroU32> {
    NonZeroU32::new(unsafe { GetClipboardSequenceNumber() })
}

/// 只建立可信水位，不讀既有內容。前後兩次都在同一把 clipboard lock 內；
/// 任一個是 0 或中途改變都不算建立成功。
fn locked_sequence_snapshot() -> Option<NonZeroU32> {
    let _lock = ClipboardLock::open()?;
    let before = clipboard_sequence()?;
    let after = clipboard_sequence()?;
    (before == after).then_some(before)
}

/// sequence、owner、format 與 data 的原子觀測範圍。
fn read_snapshot() -> Option<ClipboardSnapshot> {
    let _lock = ClipboardLock::open()?;
    let before = clipboard_sequence()?;
    let source_app = clipboard_owner_app_locked();
    let kind = available_kind_locked();
    let text = match kind {
        Some(ClipboardKind::Text) => Some(read_text_locked()?),
        _ => None,
    };
    let after = clipboard_sequence()?;
    if before != after {
        return None;
    }

    Some(ClipboardSnapshot {
        sequence: before,
        source_app,
        kind,
        text,
    })
}

fn available_kind_locked() -> Option<ClipboardKind> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            Some(ClipboardKind::Text)
        } else if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() {
            Some(ClipboardKind::Files)
        } else if IsClipboardFormatAvailable(CF_BITMAP.0 as u32).is_ok()
            || IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok()
        {
            Some(ClipboardKind::Image)
        } else {
            None
        }
    }
}

/// 複製來源的程式名。呼叫端必須持有 clipboard lock，讓它和 data 屬於同一
/// 快照。來源視窗或程序問不到時維持 `None`，不可拿目前前景補值。
fn clipboard_owner_app_locked() -> Option<String> {
    let hwnd = unsafe { GetClipboardOwner() }.ok()?;
    if hwnd.0.is_null() {
        return None;
    }
    let path = focus::process_image_path(hwnd)?;
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    Some(file.to_ascii_lowercase())
}

/// 讀 `CF_UNICODETEXT`。呼叫端必須持有 clipboard lock。任何一步失敗都回
/// `None`，讓整份 snapshot 作廢；不能送出 `kind = text, text = None` 的半份事件。
fn read_text_locked() -> Option<String> {
    unsafe {
        let handle: HANDLE = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        if handle.0.is_null() {
            return None;
        }
        let global = HGLOBAL(handle.0);
        let allocation_bytes = GlobalSize(global);
        if allocation_bytes < std::mem::size_of::<u16>() {
            return None;
        }

        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            return None;
        }

        // GlobalSize 才是可以安全讀取的邊界。額外的 64Mi 字元上限避免惡意
        // clipboard 配置把一次 tick 變成長時間掃描；找不到 NUL 就整份拒絕。
        const MAX_CHARS: usize = 64 * 1024 * 1024;
        let chars = (allocation_bytes / std::mem::size_of::<u16>()).min(MAX_CHARS);
        let units = std::slice::from_raw_parts(ptr, chars);
        let text = units
            .iter()
            .position(|unit| *unit == 0)
            .map(|nul| String::from_utf16_lossy(&units[..nul]));

        let _ = GlobalUnlock(global);
        text
    }
}
