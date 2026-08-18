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
/// 有沒有**試過**裝 hook。和「裝成功了沒」是兩件事。
static HOOK_START_ATTEMPTED: AtomicBool = AtomicBool::new(false);
/// 試過之後，到底裝上了沒。
static HOOKS_OK: AtomicBool = AtomicBool::new(false);

/// hook 在這個程序裡的狀態。
///
/// 一定要是三態，不能是布林。原本用布林的時候，`doctor` 永遠回報
/// 「輸入 hook 沒裝上」——因為 `doctor` 根本不會去裝，而「沒去裝」和
/// 「裝了但失敗」被壓成了同一個 `false`。
///
/// 那條假警報的代價不是它自己：它和真正的警告（缺 UIA）並排印在同一個
/// 「目前失效的隱私保護」區塊裡。**一則永遠錯的警告會讓整個區塊被忽略**，
/// 包括旁邊那則是真的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    /// 還沒試過——`record` 以外的命令都是這個狀態，不是問題。
    NotStarted,
    Active,
    /// 試過而且失敗了。這個才需要吵。
    Failed,
}

/// 中斷多久算是新的一段打字。
const BURST_GAP_MS: u64 = 2_000;

pub struct WindowsInput {
    window_start: Millis,
    /// 一列涵蓋多久。見 `CaptureConfig::input_window_secs`。
    window_ms: i64,
}

impl WindowsInput {
    /// 裝上 hook 並開始累積。失敗不致命——沒有節奏訊號比沒有記憶好。
    pub fn start(now: Millis, window_secs: u64) -> Self {
        install_hooks();
        Self {
            window_start: now,
            window_ms: (window_secs as i64) * 1000,
        }
    }

    pub fn state() -> HookState {
        if !HOOK_START_ATTEMPTED.load(Relaxed) {
            HookState::NotStarted
        } else if HOOKS_OK.load(Relaxed) {
            HookState::Active
        } else {
            HookState::Failed
        }
    }

    pub fn hooks_active() -> bool {
        Self::state() == HookState::Active
    }
}

impl InputSource for WindowsInput {
    fn idle_ms(&mut self) -> Option<u64> {
        system_idle_ms()
    }

    fn drain(&mut self, ts: Millis) -> Result<Option<InputMetrics>> {
        // 視窗還沒滿就先繼續累積。**這個 early return 一定要在 swap 之前**：
        // 先把計數器清掉再判斷要不要出一列，等於把那一段的輸入丟掉。
        //
        // 這一段原本不存在，於是 recorder 每個 tick（400ms）叫一次就寫一列，
        // 而 `input_window_secs = 10` 從來沒有人讀。打字時是一秒 2.5 列，
        // 不是十秒一列——DATA_INVENTORY 上那句「預設每 10 秒一列」曾經是錯的。
        //
        // 時鐘往回跳的話 `ts - window_start` 會變負。夾成 0 的話它永遠小於
        // 視窗長度，而 `window_start` 只在出了一列之後才前進——於是**打字
        // 節奏這一路訊號從此完全停止**，而錄製摘要上一個字都不會提。
        // 往回跳就把視窗接到現在重開，最多損失一段沒出完的計數。
        if ts < self.window_start {
            self.window_start = ts;
            return Ok(None);
        }
        if ts - self.window_start < self.window_ms {
            return Ok(None);
        }

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

/// 距離最後一次輸入過了多久——**問作業系統**，不看我們自己的 hook。
///
/// 和下面那個 `idle_ms()` 的差別很重要：那個依賴 hook 裝得起來，而擷取
/// 迴圈拿這個數字去決定「要不要碰螢幕」。hook 沒裝上的時候，那個會永遠
/// 回 0（＝剛剛才有人動過）——對計數欄位來說是保守的好答案，對省電來說
/// 卻是「永遠不省」。這一個不需要 hook，一次系統呼叫，不配置記憶體。
fn system_idle_ms() -> Option<u64> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: cbSize 填對了，info 是我們自己的堆疊變數
    if unsafe { GetLastInputInfo(&mut info) }.ok().is_err() {
        return None; // 答不出來就說答不出來，不要回一個會讓她閉眼的數字
    }
    // dwTime 和 GetTickCount 同源，都是 32-bit 回繞的毫秒數
    Some((tick_now() as u32).wrapping_sub(info.dwTime) as u64)
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
    if HOOK_START_ATTEMPTED.swap(true, Relaxed) {
        return; // 一個程序一組就夠。裝兩次會讓每個按鍵被數兩下。
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("sister-input-hooks".into())
        .spawn(move || unsafe {
            let kb: Option<HHOOK> = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), None, 0).ok();
            let ms: Option<HHOOK> = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0).ok();

            let ok = kb.is_some() || ms.is_some();
            HOOKS_OK.store(ok, Relaxed);
            // 先報告結果再進訊息迴圈，否則呼叫端要等到迴圈結束才問得到
            let _ = tx.send(());

            if !ok {
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
        .is_ok();

    // hook 必須裝在有訊息迴圈的那條執行緒上，但呼叫端在下一行就要能誠實回答
    // 「裝上了沒」——所以在這裡等它回報。等不到就當作失敗，不要樂觀假設。
    if !spawned
        || rx
            .recv_timeout(std::time::Duration::from_millis(1_000))
            .is_err()
    {
        HOOKS_OK.store(false, Relaxed);
    }
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
        // window_ms = 0 讓視窗這一關不參與判定，這條測的是「安靜」本身
        let mut input = WindowsInput {
            window_start: 0,
            window_ms: 0,
        };
        assert!(input.drain(1000).expect("no error").is_none());
    }

    /// **視窗沒滿之前不出列，而且累積的輸入不可以被丟掉。**
    ///
    /// `input_window_secs` 本來是個沒有人讀的設定：recorder 每 400ms 叫一次
    /// drain，它就每 400ms 寫一列，打字時一秒 2.5 列——而設定檔上寫著 10 秒、
    /// DATA_INVENTORY 上也寫著「預設每 10 秒一列」。
    ///
    /// 這條同時盯著那個 early return 的**位置**：擺到 swap 後面的話，視窗
    /// 沒滿的那幾次會把計數器清掉，於是使用者打的字被靜靜地扔了。
    #[test]
    fn input_is_accumulated_until_the_window_closes_not_dropped() {
        KEYSTROKES.store(0, Relaxed);
        CLICKS.store(0, Relaxed);
        SCROLL.store(0, Relaxed);
        MOUSE_PX.store(0, Relaxed);
        BURSTS.store(0, Relaxed);

        let mut input = WindowsInput {
            window_start: 0,
            window_ms: 10_000,
        };

        // 視窗中間打了字，但還沒滿 10 秒
        KEYSTROKES.store(5, Relaxed);
        assert!(
            input.drain(400).expect("no error").is_none(),
            "視窗沒滿不該出列"
        );
        KEYSTROKES.store(9, Relaxed); // 又多打了幾個
        assert!(input.drain(800).expect("no error").is_none());

        // 視窗滿了，這時候才出一列，而且要含全部的按鍵數
        let m = input
            .drain(10_000)
            .expect("no error")
            .expect("視窗滿了就該出列");
        assert_eq!(m.keystrokes, 9, "視窗中間累積的輸入不能被丟掉");
        assert_eq!((m.ts_start, m.ts_end), (0, 10_000));
    }

    /// 時鐘往回跳不可以讓節奏訊號從此停止。
    ///
    /// `ts - window_start` 變負、夾成 0，於是永遠小於視窗長度；而
    /// `window_start` 只在出了一列之後才前進。症狀是 `input_metrics`
    /// 從此一列都不再寫，而且沒有任何地方會講。
    #[test]
    fn a_clock_that_jumps_backwards_does_not_silence_the_rhythm_forever() {
        KEYSTROKES.store(5, Relaxed);

        let mut input = WindowsInput {
            window_start: 1_000_000,
            window_ms: 10_000,
        };
        // 退了一小時。這一次不出列是對的（視窗重開），但基準必須跟著退。
        assert!(input.drain(1_000_000 - 3_600_000).unwrap().is_none());

        // 從新基準往後過了一整個視窗，就該出列了。
        let m = input.drain(1_000_000 - 3_600_000 + 10_000).unwrap();
        assert!(m.is_some(), "時鐘往回跳之後節奏訊號就再也沒出現過");
    }

    #[test]
    fn drain_takes_the_counters_and_resets_them() {
        KEYSTROKES.store(7, Relaxed);
        CLICKS.store(2, Relaxed);
        SCROLL.store(3, Relaxed);
        MOUSE_PX.store(450, Relaxed);
        BURSTS.store(1, Relaxed);

        let mut input = WindowsInput {
            window_start: 1000,
            window_ms: 10_000,
        };
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
            window_ms: 1_000,
        };
        let m = input.drain(6_000).expect("no error").expect("some metrics");
        assert!(m.idle_ms <= 1_000, "idle {} > window", m.idle_ms);
    }
}
