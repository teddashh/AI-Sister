//! 輸入節奏：打了幾個字、點了幾下、滑鼠移動多遠。**不記按了什麼。**
//!
//! 這裡的保證不是靠「我們記得要過濾」，而是靠**根本不去看**：鍵盤 callback
//! 收到的 `lparam` 指向 `KBDLLHOOKSTRUCT`，裡面就是 `vkCode`。這份程式碼
//! 從頭到尾沒有解參考它。沒有讀取，就沒有可能外洩的路徑，也不需要任何人
//! 相信我們的過濾寫對了。
//!
//! # low-level hook 的鐵律
//!
//! `LowLevelHooksTimeout`（登錄檔，預設也是上限 1000ms）一旦超時，系統會
//! **靜默地把 hook 拆掉**——不會有錯誤、不會有事件，只是從此再也收不到輸入，
//! 而使用者只會覺得「她好像變笨了」。所以 callback 裡只做 atomic 加法：
//! 不配置記憶體、不上鎖、不寫 log、不碰資料庫。所有解讀都留到 `drain()`。
//!
//! 計數器是 process 全域的 static，因為 hook callback 是 `extern "system"`
//! 函式指標，沒有地方掛使用者資料。整個程序只會有一組 hook。

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::Relaxed};

use anyhow::Result;
use sister_core::model::{InputMetrics, Millis};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, HHOOK, MSG, MSLLHOOKSTRUCT,
    SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_XBUTTONDOWN,
};

use crate::traits::InputSource;

static KEYSTROKES: AtomicU64 = AtomicU64::new(0);
static CLICKS: AtomicU64 = AtomicU64::new(0);
static SCROLL: AtomicU64 = AtomicU64::new(0);
static MOUSE_PX: AtomicU64 = AtomicU64::new(0);
static BURSTS: AtomicU64 = AtomicU64::new(0);

static LAST_X: AtomicI32 = AtomicI32::new(0);
static LAST_Y: AtomicI32 = AtomicI32::new(0);
static HAVE_POS: AtomicBool = AtomicBool::new(false);

/// 最後一次輸入的 tick（`GetTickCount64` 的毫秒）。0 = 從來沒有過。
static LAST_INPUT_TICK: AtomicU64 = AtomicU64::new(0);
/// 最後一次按鍵的 tick，用來切「打字段落」。
static LAST_KEY_TICK: AtomicU64 = AtomicU64::new(0);
static HOOKS_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 中斷多久算是新的一段打字。
const BURST_GAP_MS: u64 = 2_000;

pub struct WindowsInput {
    window_start: Millis,
}

impl WindowsInput {
    /// 裝上 hook 並開始累積。失敗不致命——沒有節奏訊號比沒有記憶好。
    pub fn start(now: Millis) -> Self {
        install_hooks();
        Self { window_start: now }
    }

    pub fn hooks_active() -> bool {
        HOOKS_INSTALLED.load(Relaxed)
    }
}

impl InputSource for WindowsInput {
    fn drain(&mut self, ts: Millis) -> Result<Option<InputMetrics>> {
        let start = self.window_start;
        self.window_start = ts;

        let keystrokes = KEYSTROKES.swap(0, Relaxed) as i64;
        let clicks = CLICKS.swap(0, Relaxed) as i64;
        let scroll_ticks = SCROLL.swap(0, Relaxed) as i64;
        let mouse_px = MOUSE_PX.swap(0, Relaxed) as i64;
        let typing_bursts = BURSTS.swap(0, Relaxed) as i64;

        if keystrokes == 0 && clicks == 0 && scroll_ticks == 0 && mouse_px == 0 {
            // 完全沒動就不寫一列全 0 進資料庫——idle 是由「沒有紀錄」表達的
            return Ok(None);
        }

        let window_ms = (ts - start).max(0);
        Ok(Some(InputMetrics {
            ts_start: start,
            ts_end: ts,
            keystrokes,
            clicks,
            mouse_px,
            scroll_ticks,
            // 這一欄歸 recorder：只有它看得到焦點的變化
            window_switches: 0,
            idle_ms: idle_ms().min(window_ms),
            typing_bursts,
        }))
    }
}

/// 距離最後一次輸入過了多久。
pub fn idle_ms() -> i64 {
    let last = LAST_INPUT_TICK.load(Relaxed);
    if last == 0 {
        return 0;
    }
    (tick_now().saturating_sub(last)) as i64
}

fn tick_now() -> u64 {
    // GetTickCount64 不配置、不會失敗，在 hook callback 裡呼叫是安全的
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

/// 開一條專屬執行緒裝 hook 並跑訊息迴圈。
///
/// low-level hook 的事件是送到**安裝它的那條執行緒**的訊息佇列，所以那條
/// 執行緒必須一直在抽訊息。錄製迴圈自己在忙別的事，不能兼任。
fn install_hooks() {
    if HOOKS_INSTALLED.swap(true, Relaxed) {
        return; // 一個程序一組就夠
    }

    std::thread::Builder::new()
        .name("sister-input-hooks".into())
        .spawn(|| unsafe {
            let kb: Option<HHOOK> = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), None, 0).ok();
            let ms: Option<HHOOK> = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0).ok();

            if kb.is_none() && ms.is_none() {
                HOOKS_INSTALLED.store(false, Relaxed);
                tracing::warn!("輸入 hook 裝不上，節奏訊號這個 session 不會有");
                return;
            }

            let mut msg = MSG::default();
            // GetMessageW 回 0 是 WM_QUIT、回 -1 是錯誤，兩種都該收工
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}

/// 鍵盤 hook。**只加一，不看按了什麼。**
///
/// `lparam` 指向 `KBDLLHOOKSTRUCT`（含 `vkCode`）。這裡刻意不解參考它：
/// 按鍵內容從來沒有進入過這個程序的記憶體，這比任何過濾都可靠。
unsafe extern "system" fn kb_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && (wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize)
    {
        let now = tick_now();
        KEYSTROKES.fetch_add(1, Relaxed);
        LAST_INPUT_TICK.store(now, Relaxed);

        let prev = LAST_KEY_TICK.swap(now, Relaxed);
        if prev == 0 || now.saturating_sub(prev) > BURST_GAP_MS {
            BURSTS.fetch_add(1, Relaxed);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 滑鼠 hook。讀座標（位置不是內容），不讀其它任何東西。
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        LAST_INPUT_TICK.store(tick_now(), Relaxed);
        match wparam.0 as u32 {
            WM_MOUSEMOVE => unsafe {
                let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
                let (x, y) = (info.pt.x, info.pt.y);
                if HAVE_POS.swap(true, Relaxed) {
                    let dx = (x - LAST_X.load(Relaxed)) as f64;
                    let dy = (y - LAST_Y.load(Relaxed)) as f64;
                    MOUSE_PX.fetch_add(dx.hypot(dy) as u64, Relaxed);
                }
                LAST_X.store(x, Relaxed);
                LAST_Y.store(y, Relaxed);
            },
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                CLICKS.fetch_add(1, Relaxed);
            }
            WM_MOUSEWHEEL => {
                SCROLL.fetch_add(1, Relaxed);
            }
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 沒有任何輸入時不該寫一列全 0 的紀錄：idle 是用「沒有列」表達的，
    /// 一秒一列空紀錄會把資料庫塞滿沒有資訊的東西。
    #[test]
    fn silence_produces_no_row() {
        KEYSTROKES.store(0, Relaxed);
        CLICKS.store(0, Relaxed);
        SCROLL.store(0, Relaxed);
        MOUSE_PX.store(0, Relaxed);
        let mut input = WindowsInput { window_start: 0 };
        assert!(input.drain(1000).expect("no error").is_none());
    }

    #[test]
    fn drain_takes_the_counters_and_resets_them() {
        KEYSTROKES.store(7, Relaxed);
        CLICKS.store(2, Relaxed);
        SCROLL.store(3, Relaxed);
        MOUSE_PX.store(450, Relaxed);
        BURSTS.store(1, Relaxed);

        let mut input = WindowsInput { window_start: 1000 };
        let m = input
            .drain(11_000)
            .expect("no error")
            .expect("some metrics");
        assert_eq!((m.keystrokes, m.clicks, m.scroll_ticks), (7, 2, 3));
        assert_eq!(m.mouse_px, 450);
        assert_eq!((m.ts_start, m.ts_end), (1000, 11_000));
        // window_switches 歸 recorder，這裡一定是 0
        assert_eq!(m.window_switches, 0);

        // 取走就要歸零，不然下一個視窗會重複計算同一批輸入
        assert!(input.drain(21_000).expect("no error").is_none());
    }

    #[test]
    fn idle_never_exceeds_the_window() {
        // idle 比視窗還長是沒有意義的：這個視窗才十秒，不可能閒置一小時
        LAST_INPUT_TICK.store(0, Relaxed);
        KEYSTROKES.store(1, Relaxed);
        let mut input = WindowsInput {
            window_start: 5_000,
        };
        let m = input.drain(6_000).expect("no error").expect("some metrics");
        assert!(m.idle_ms <= 1_000, "idle {} > window", m.idle_ms);
    }
}
