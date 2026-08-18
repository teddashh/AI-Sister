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

use anyhow::Result;
use sister_core::model::{ClipboardEvent, ClipboardKind, Millis};
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_HDROP, CF_UNICODETEXT};

use super::focus;
use crate::traits::ClipboardSource;

pub struct WindowsClipboard {
    last_seq: u32,
    primed: bool,
}

impl Default for WindowsClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsClipboard {
    pub fn new() -> Self {
        Self {
            last_seq: 0,
            primed: false,
        }
    }

    /// 水位有沒有前進。`None` = 沒有新東西。
    fn advanced(&mut self) -> Option<u32> {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if !self.primed {
            // 開機第一次只對齊，不讀。她按 Ctrl-C 的那一刻我們還沒在看。
            self.primed = true;
            self.last_seq = seq;
            return None;
        }
        (seq != self.last_seq).then(|| {
            self.last_seq = seq;
            seq
        })
    }
}

impl ClipboardSource for WindowsClipboard {
    fn poll(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
        if self.advanced().is_none() {
            return Ok(None);
        }

        let kind = match available_kind() {
            Some(k) => k,
            // 認不得的格式：連「複製過東西」都不記，因為我們無法描述它
            None => return Ok(None),
        };

        let source_app = clipboard_owner_app();
        let text = if kind == ClipboardKind::Text {
            read_text()
        } else {
            None
        };

        Ok(Some(ClipboardEvent {
            ts,
            kind,
            byte_len: text.as_deref().map_or(0, |t| t.len() as i64),
            text,
            truncated: false,
            // 秘密偵測與截斷歸 recorder：政策在那裡，設定也在那裡
            secret_suspected: false,
            source_app,
        }))
    }

    fn skip(&mut self, _ts: Millis) {
        // 推水位但不讀。少了這一步，排除規則只是把洩漏延後到下一個 tick。
        let _ = self.advanced();
    }
}

fn available_kind() -> Option<ClipboardKind> {
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

/// 複製來源的程式名。用來回答「這串東西是從哪裡來的」。
fn clipboard_owner_app() -> Option<String> {
    let hwnd = unsafe { GetClipboardOwner() }.ok()?;
    if hwnd.0.is_null() {
        return None;
    }
    let path = focus::process_image_path(hwnd)?;
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    Some(file.to_ascii_lowercase())
}

/// 讀 `CF_UNICODETEXT`。任何一步失敗都回 `None`——剪貼簿隨時可能被別的
/// 程式鎖住，那不是錯誤，等下一次就好。
fn read_text() -> Option<String> {
    unsafe {
        // 別重試、別等。搶不到就算了，我們一秒後還會再來。
        OpenClipboard(None).ok()?;
        let out = read_text_locked();
        let _ = CloseClipboard();
        out
    }
}

unsafe fn read_text_locked() -> Option<String> {
    unsafe {
        let handle: HANDLE = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        if handle.0.is_null() {
            return None;
        }
        let global = HGLOBAL(handle.0);
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            return None;
        }

        // 自己走到 NUL：GlobalSize 回的是配置大小，不一定等於字串長度
        let mut len = 0usize;
        // 上限是為了防一份壞掉的緩衝區把我們帶進沒有盡頭的迴圈。
        // 64Mi 個 UTF-16 字元遠超過任何真實的複製貼上。
        const MAX_CHARS: usize = 64 * 1024 * 1024;
        while len < MAX_CHARS && *ptr.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));

        let _ = GlobalUnlock(global);
        (!text.is_empty()).then_some(text)
    }
}
