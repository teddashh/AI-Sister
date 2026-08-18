//! 前景視窗：她現在在看哪個程式、哪個視窗、哪個網址。
//!
//! 視窗那一半是純 Win32，不碰 COM，快到可以每個 tick 跑一次。這是整條
//! 管線最先跑的東西——排除規則要靠它決定這一刻該不該擷取，所以它必須快、
//! 必須不會卡住，而且抓不到就安靜地回傳空值（`FocusSnapshot::default()`），
//! 不是錯誤。
//!
//! 網址那一半（以及「焦點是不是在密碼欄上」）只能靠 UIA，而 UIA 會卡。
//! 那一整包風險關在 [`crate::windows::uia`] 裡的一條可拋棄執行緒中，
//! 這裡只負責問與不問：
//!
//! - **網址**只對瀏覽器問，而且只在 `(hwnd, 標題)` 變了才走一次樹
//! - **密碼欄**每個 tick 都問，但那只是一次呼叫，不走樹

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

/// 會叫 UIA 的瀏覽器。
///
/// 比對的是 `app_id`（小寫的執行檔名），用**子字串**——`chrome` 同時涵蓋
/// `chrome.exe` 與各種 Chromium 分支的命名，而漏掉一個瀏覽器的代價是
/// 那個瀏覽器的網銀規則整組失效（THREAT_MODEL 安靜失效 #2 就是這個形狀）。
const BROWSERS: &[&str] = &[
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "vivaldi",
    "opera",
    "chromium",
    "arc",
    "zen",
    "librewolf",
    "waterfox",
    "floorp",
    "thorium",
    "iexplore",
];

pub struct WindowsFocus {
    uia: crate::windows::uia::Uia,
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
        }
    }

    /// UIA 還活著嗎。`false` = `excluded_urls` 整組規則目前不生效。
    pub fn url_capture_alive(&self) -> bool {
        self.uia.is_alive()
    }
}

impl FocusSource for WindowsFocus {
    fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
        let mut snapshot = foreground();

        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return Ok(snapshot);
        }
        // **不是瀏覽器就完全不碰 UIA。** 這不只是省時間：每一次 UIA 呼叫
        // 都是一次可能卡住、而且叫不回來的跨程序往返，對一個整天在跑的
        // 背景程式來說，「一天裡絕大多數時間根本沒有這個風險」本身就是
        // 一項功能。代價是非瀏覽器的密碼欄我們看不到（DATA_INVENTORY
        // 「已知缺口」有記）。
        let app = snapshot.app_key();
        if !BROWSERS.iter().any(|b| app.contains(b)) {
            return Ok(snapshot);
        }

        let title = snapshot.window_title.clone().unwrap_or_default();
        if let Some(reading) = self.uia.read(hwnd, &title) {
            // 「不知道焦點在不在密碼欄上」→ 擋掉這一幀，那是對的。
            // 但**每一次都不知道**就不是保守了，那是「她在瀏覽器裡什麼都
            // 記不住」，而原因藏在沒有人會看的地方。`password_check_broken`
            // 是那條線：踩到之後改成「宣告做不到」而不是「安靜地全擋」。
            snapshot.password_field =
                reading.should_skip_frame() && !self.uia.password_check_broken();
            snapshot.url = reading.url;
        }
        // `None` = UIA 在這台機器上不能用。那是一個能力缺口，由
        // `Capabilities` 一次講清楚，不是在這裡每秒擋掉一幀——
        // 那只會讓她什麼都記不住，而且原因藏在一個沒有人看的地方。
        Ok(snapshot)
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
        // 網址與密碼欄由 UIA 在 `snapshot()` 裡補上。這個函式刻意只碰
        // Win32：它是排除判定的第一手資料，不能因為 COM 卡住而跟著卡住。
        url: None,
        pid: process_id(hwnd).map(|p| p as i64),
        password_field: false,
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
