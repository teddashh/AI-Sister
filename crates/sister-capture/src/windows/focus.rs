//! 前景視窗：她現在在看哪個程式、哪個視窗。
//!
//! 純 Win32，不碰 COM。這是整條管線最先跑的東西——排除規則要靠它決定
//! 這一刻該不該擷取，所以它必須快、必須不會卡住，而且抓不到就安靜地
//! 回傳空值（`FocusSnapshot::default()`），不是錯誤。

use anyhow::Result;
use sister_core::model::{FocusSnapshot, Millis};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

use crate::traits::FocusSource;

pub struct WindowsFocus;

impl FocusSource for WindowsFocus {
    fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
        Ok(foreground())
    }
}

/// 現在的前景視窗。任何一步失敗都退化成 `None`，不往上冒錯誤。
pub fn foreground() -> FocusSnapshot {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        // 沒有前景視窗是正常狀態：切換桌面、鎖定畫面的瞬間都會這樣
        return FocusSnapshot::default();
    }

    let (app_id, app_name) = match process_image_path(hwnd) {
        Some(path) => {
            let file = file_name(&path);
            let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s);
            // app_id 用小寫檔名（chrome.exe），排除規則比對的就是它
            (Some(file.to_ascii_lowercase()), Some(stem.to_string()))
        }
        None => (None, None),
    };

    FocusSnapshot {
        app_id,
        app_name,
        window_title: window_title(hwnd),
        // URL 由 UIA 另外補上（見 `url` 模組）；這一層不管瀏覽器
        url: None,
        pid: process_id(hwnd).map(|p| p as i64),
    }
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

    /// 前景視窗抓不到時必須安靜降級。錄製迴圈每秒呼叫一次，
    /// 讓它變成錯誤等於讓整個 session 因為切個桌面就死掉。
    #[test]
    fn foreground_never_panics() {
        let f = foreground();
        // 無頭 session 裡兩種結果都合法，重點是它有回來
        assert!(f.app_id.is_some() || f.app_id.is_none());
    }
}
