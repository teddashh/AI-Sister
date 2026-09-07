//! 前景視窗：她現在在看哪個程式、哪個視窗、哪個網址。
//!
//! 視窗那一半是純 Win32，不碰 COM，快到可以每個 tick 跑一次。這是整條
//! 管線最先跑的東西——排除規則要靠它決定這一刻該不該擷取，所以它必須快、
//! 必須不會卡住。抓不到時回傳 [`PrivacyContext::Unknown`]，
//! 讓 recorder 在任何內容來源之前 fail closed；不再把空 snapshot
//! 當成「確定安全」。
//!
//! 網址那一半（以及「焦點是不是在密碼欄上」）只能靠 UIA，而 UIA 會卡。
//! 那一整包風險關在 [`crate::windows::uia`] 裡的一條可拋棄執行緒中，
//! 這裡只負責問與不問：
//!
//! - **網址**只對瀏覽器問；可快取位址列 element，但每拍重讀 Value
//! - **密碼欄**每個 tick 都問，但那只是一次呼叫，不走樹

use anyhow::Result;
use sister_core::model::{FocusSnapshot, Millis, PrivacyContext, SensitiveFieldState};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

use crate::traits::{CapturePermit, FocusSource, PrivacyObservation};

use crate::browsers::is_browser;

pub struct WindowsFocus {
    uia: crate::windows::uia::Uia,
    next_generation: u64,
    approved: Option<Approved>,
}

#[derive(Clone)]
struct Approved {
    native_window: u64,
    process_id: u32,
    generation: u64,
    context: PrivacyContext,
}

impl Default for WindowsFocus {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsFocus {
    pub fn new() -> Self {
        Self {
            uia: crate::windows::uia::Uia::new(),
            next_generation: 0,
            approved: None,
        }
    }

    /// UIA 還活著嗎。`false` = privacy context 不可用，recorder 會 fail closed。
    pub fn url_capture_alive(&self) -> bool {
        self.uia.is_alive()
    }

    /// 讀一份完整、綁 exact HWND/PID 的 privacy observation。慢 UIA 回來後
    /// 再讀一次 Win32 identity；不同就把整份答案作廢。
    fn observe_current(&mut self) -> Option<(u64, u32, PrivacyContext)> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return None;
        }
        let pid = process_id(hwnd)?;
        let snapshot = foreground_for(hwnd, pid)?;
        let browser = is_browser(&snapshot.app_key());
        let reading = self.uia.read(hwnd, browser)?;

        let after = unsafe { GetForegroundWindow() };
        if after != hwnd || process_id(after) != Some(pid) {
            return None;
        }

        let sensitive_field = match reading.password_focused {
            Some(false) => SensitiveFieldState::Clear,
            Some(true) => SensitiveFieldState::Focused,
            None => SensitiveFieldState::Unknown,
        };
        Some((
            hwnd.0 as isize as u64,
            pid,
            PrivacyContext::known(snapshot, sensitive_field, reading.browser_url),
        ))
    }
}

impl FocusSource for WindowsFocus {
    fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
        let Some((native_window, process_id, context)) = self.observe_current() else {
            self.approved = None;
            return Ok(PrivacyObservation::Unknown);
        };
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Windows capture permit generation exhausted"))?;
        let generation = self.next_generation;
        self.approved = Some(Approved {
            native_window,
            process_id,
            generation,
            context: context.clone(),
        });
        let permit = CapturePermit::windows(
            super::backend_token(),
            native_window,
            process_id,
            generation,
        );
        Ok(PrivacyObservation::known(context, permit))
    }

    fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
        let Some((native_window, process_id, generation)) = permit.windows_parts() else {
            return Ok(false);
        };
        let Some(approved) = self.approved.clone() else {
            return Ok(false);
        };
        if (
            approved.native_window,
            approved.process_id,
            approved.generation,
        ) != (native_window, process_id, generation)
        {
            return Ok(false);
        }
        let Some((now_window, now_pid, now_context)) = self.observe_current() else {
            return Ok(false);
        };
        Ok(now_window == native_window && now_pid == process_id && now_context == approved.context)
    }

    fn url_capture(&self) -> sister_core::capabilities::UrlCapture {
        sister_core::capabilities::UrlCapture {
            gave_up: !self.uia.is_alive(),
            // 兩個都送過去，句子那邊自己決定要不要蓋掉其中一則——整個 UIA
            // 都沒了的時候，「密碼欄問不出來」是它的後果不是另一件事，而
            // 「兩則裡哪一則該閉嘴」是一個判斷，判斷只住在一個地方。
            password_check_broken: self.uia.password_check_broken(),
        }
    }
}

/// 現在的前景視窗。`None` 是不知道，呼叫端不得放行內容。
pub fn foreground() -> Option<FocusSnapshot> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let pid = process_id(hwnd)?;
    foreground_for(hwnd, pid)
}

fn foreground_for(hwnd: HWND, pid: u32) -> Option<FocusSnapshot> {
    let path = process_image_path_for_pid(pid)?;
    let file = file_name(&path);
    let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s);

    Some(FocusSnapshot {
        app_id: Some(file.to_ascii_lowercase()),
        app_name: Some(stem.to_string()),
        // 視窗標題抓不到時仍可依 app 規則擋掉；空標題本身不被寫入。
        window_title: window_title(hwnd),
        // 網址與敏感欄由 UIA 在 `context()` 裡補上。這個函式刻意只碰
        // Win32：它是排除判定的第一手資料，不能因為 COM 卡住而跟著卡住。
        url: None,
        pid: Some(pid as i64),
    })
}

pub fn window_title(hwnd: HWND) -> Option<String> {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }
    // +1 給結尾的 NUL
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if n <= 0 {
        return None;
    }
    let title = String::from_utf16_lossy(&buf[..n as usize]);
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

pub fn process_id(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// 視窗所屬程序的執行檔完整路徑。
///
/// 用 `PROCESS_QUERY_LIMITED_INFORMATION` 而不是 `PROCESS_QUERY_INFORMATION`：
/// 前者對高完整性等級的程序也拿得到，而且我們只要一個檔名，
/// 沒有理由要求超過必要的權限。
pub fn process_image_path(hwnd: HWND) -> Option<String> {
    let pid = process_id(hwnd)?;
    process_image_path_for_pid(pid)
}

fn process_image_path_for_pid(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();

        // 不能提早 return：handle 一定要關，否則每秒漏一個
        let _ = CloseHandle(handle);

        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_takes_the_last_segment() {
        assert_eq!(
            file_name(r"C:\Program Files\Google\chrome.exe"),
            "chrome.exe"
        );
        assert_eq!(file_name("chrome.exe"), "chrome.exe");
        assert_eq!(file_name(r"C:\a/b\c.exe"), "c.exe");
    }

    /// 無頭 session 或正在切換桌面時仍必須有限時間內回來。
    /// 返回 `None` 的安全後果由 `PrivacyContext::Unknown` 測試釘住。
    #[test]
    fn foreground_returns_without_panicking() {
        let _ = foreground();
    }
}
