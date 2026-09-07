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

use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU64,
    Ordering::{AcqRel, Acquire, Relaxed, Release},
};

use anyhow::Result;
use sister_core::model::{
    HookHealth, InputListening, InputMetrics, InputTick, Millis, classify_quiet_window,
};
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

/// 最高位是「不准新 callback 進入」，其餘位數現在已進入的 callback。
///
/// 這兩個狀態必須放在**同一個** atomic：若分成 `enabled` 與
/// `in_flight`，callback 可以在讀到 enabled 之後被掛起，suspend 看到
/// `in_flight == 0` 就清除並返回；等 resume 重開後，那個舊 callback
/// 才繼續加一，隱私洞裡的計數就穿過了邊界。
///
/// 用單一 CAS 後，線性化點就是 callback 的 reader-count CAS 與
/// suspend 的 disabled-bit `fetch_or` 誰先成功：前者先就會被 wait 等完，
/// 後者先就讓 callback 當場放棄。兩者之間沒有漏縫。
const INPUT_DISABLED: u64 = 1 << 63;
const INPUT_IN_FLIGHT_MASK: u64 = !INPUT_DISABLED;
static INPUT_GATE: AtomicU64 = AtomicU64::new(0);

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
        let mut input = Self {
            window_start: now,
            window_ms: (window_secs as i64) * 1000,
        };
        // 一個 process 只有一組 static callback 狀態；若同行程重開
        // recorder，不能繼承上一場未滿視窗的計數或 suspended 狀態。
        input.restart_accumulation(now);
        input
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

    /// 丟掉現在所有尚未交付的輸入狀態，並從 `ts` 重開視窗。
    ///
    /// 除了會出現在 `InputMetrics` 的五個計數，連 idle、打字段落與
    /// 滑鼠位置的 baseline 也必須清。否則隱私洞裡的最後一個事件雖然
    /// 沒有直接出列，還是會透過「下一鍵是不是新 burst」或跨洞滑鼠距離
    /// 影響恢復後的第一列。
    fn discard_accumulated(&mut self, ts: Millis) {
        KEYSTROKES.store(0, Relaxed);
        CLICKS.store(0, Relaxed);
        SCROLL.store(0, Relaxed);
        MOUSE_PX.store(0, Relaxed);
        BURSTS.store(0, Relaxed);

        LAST_INPUT_TICK.store(0, Relaxed);
        LAST_KEY_TICK.store(0, Relaxed);
        HAVE_POS.store(false, Relaxed);
        LAST_X.store(0, Relaxed);
        LAST_Y.store(0, Relaxed);
        self.window_start = ts;
    }

    /// 關上入口，等所有已取得 reader 票的 callback 離開，再清空。
    fn suspend_accumulation(&mut self, ts: Millis) {
        INPUT_GATE.fetch_or(INPUT_DISABLED, AcqRel);
        while INPUT_GATE.load(Acquire) & INPUT_IN_FLIGHT_MASK != 0 {
            // callback 只做 atomic 運算，yield 是避免 recorder 執行緒在
            // 少見的排程撞期裡把它餓死。
            std::thread::yield_now();
        }
        self.discard_accumulated(ts);
    }

    /// 仍在 disabled 時清掉 gap 尾端，然後以一次 release store 開關。
    fn resume_accumulation(&mut self, ts: Millis) {
        if INPUT_GATE.load(Acquire) & INPUT_DISABLED == 0 {
            return; // 重複 resume 是 no-op，不可清掉恢復後的新輸入
        }
        debug_assert_eq!(INPUT_GATE.load(Acquire) & INPUT_IN_FLIGHT_MASK, 0);
        self.discard_accumulated(ts);
        INPUT_GATE.store(0, Release);
    }

    fn restart_accumulation(&mut self, ts: Millis) {
        self.suspend_accumulation(ts);
        self.resume_accumulation(ts);
    }
}

/// callback 持有這張票時，suspend 一定會等它 Drop 才清計數。
/// 零大小、不配置、不上鎖，符合 low-level hook 的時間限制。
struct InputCallbackGuard;

impl InputCallbackGuard {
    fn enter() -> Option<Self> {
        let mut gate = INPUT_GATE.load(Acquire);
        loop {
            if gate & INPUT_DISABLED != 0 {
                return None;
            }
            let in_flight = gate & INPUT_IN_FLIGHT_MASK;
            if in_flight == INPUT_IN_FLIGHT_MASK {
                return None; // 不可能到達；溢位時仍以放棄為安全答案
            }
            match INPUT_GATE.compare_exchange_weak(gate, gate + 1, AcqRel, Acquire) {
                Ok(_) => return Some(Self),
                Err(actual) => gate = actual,
            }
        }
    }
}

impl Drop for InputCallbackGuard {
    fn drop(&mut self) {
        INPUT_GATE.fetch_sub(1, Release);
    }
}

impl InputSource for WindowsInput {
    fn idle_ms(&mut self) -> Option<u64> {
        system_idle_ms()
    }

    fn drain(&mut self, ts: Millis) -> Result<Option<InputTick>> {
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
            // 完全沒動仍不寫一列全 0 的 input_metrics；改由 input_health
            // 分清楚「作業系統證實沒人動」和「我們沒有在聽」。
            let hook = match Self::state() {
                HookState::NotStarted => HookHealth::NotStarted,
                HookState::Active => HookHealth::Active,
                HookState::Failed => HookHealth::Failed,
            };
            return Ok(Some(InputTick {
                ts_start: start,
                ts_end: ts,
                metrics: None,
                listening: classify_quiet_window(hook, system_idle_ms(), ts - start),
            }));
        }

        let window_ms = (ts - start).max(0);
        let metrics = InputMetrics {
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
        };
        Ok(Some(InputTick {
            ts_start: start,
            ts_end: ts,
            metrics: Some(metrics),
            listening: InputListening::Unknown,
        }))
    }

    fn suspend(&mut self, ts: Millis) -> Result<()> {
        self.suspend_accumulation(ts);
        Ok(())
    }

    fn resume(&mut self, ts: Millis) -> Result<()> {
        self.resume_accumulation(ts);
        Ok(())
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
        if let Some(_guard) = InputCallbackGuard::enter() {
            let now = tick_now();
            KEYSTROKES.fetch_add(1, Relaxed);
            LAST_INPUT_TICK.store(now, Relaxed);

            let prev = LAST_KEY_TICK.swap(now, Relaxed);
            if prev == 0 || now.saturating_sub(prev) > BURST_GAP_MS {
                BURSTS.fetch_add(1, Relaxed);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 滑鼠 hook。讀座標（位置不是內容），不讀其它任何東西。
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(_guard) = InputCallbackGuard::enter() {
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
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// 底下每一條都在動同一組**行程層級**的計數器（`KEYSTROKES` 那幾個
    /// static），而 `cargo test` 預設是多執行緒。
    ///
    /// 兩條同時跑的時候，一條的 `drain` 會用 `swap(0)` 把另一條剛 `store`
    /// 進去的數字取走；被害的那條看到的是「視窗滿了卻沒出列」，而 `drain`
    /// 在計數器全 0 時本來就回 `None`。錯誤訊息會指著一個根本沒壞的
    /// early return。
    ///
    /// 這在 CI 上真的發生過，而且它擋掉的不只是一次測試——Release 那一步接
    /// 在測試後面，所以那一版的 exe 直接沒有產出。一條每 n 次紅一次的測試，
    /// 最後的下場是被人習慣性地重跑，而它哪天講了真話也不會有人相信。
    ///
    /// 這是測試的問題，不是 `drain` 的問題：那幾個 static 是 Win32 hook 的
    /// callback 唯一能寫進去的地方（回呼函式沒有 `self`），改成可注入的話，
    /// 被測的就不再是真的跑在機器上的那條路了。
    static COUNTERS: Mutex<()> = Mutex::new(());

    /// 中毒了照樣往下走：前一條 panic 過的意思是它已經報告過自己了，不需要
    /// 讓後面每一條都跟著死一次、還死在一個看不懂的地方。
    fn exclusive() -> MutexGuard<'static, ()> {
        COUNTERS.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **`HookState` 翻成 `HookHealth` 的那三臂，每一臂都要是對的。**
    ///
    /// 隔壁那條只走得到 `NotStarted`（這個測試 binary 從來沒有裝過 hook），
    /// 於是 `Active` 和 `Failed` 兩臂在整套測試裡是死碼——而把
    /// `HookState::Failed => HookHealth::Active` 這樣改一個字，
    /// 「hook 裝失敗 ＋ 使用者真的離開電腦」就會退回**有把握地**說
    /// 「鍵盤和滑鼠一下都沒有動」，正是這一版宣稱修掉的那句謊。
    /// `ops.rs` 那條原始碼形狀測試守的是 `match` 問誰，守不到每一臂算出什麼。
    ///
    /// 視窗長度刻意用 0，好讓判定變成確定的而不是看機器閒置幾秒：
    /// `idle >= 0` 恆真，所以 `Active` 一定不是 `NotListening`
    /// （作業系統答不出來的話是 `Unknown`，同樣不是 `NotListening`），
    /// 而 `NotStarted` / `Failed` 根本不看作業系統。
    #[test]
    fn every_hook_state_maps_to_its_own_listening_verdict() {
        let _lock = exclusive();
        let saved = (HOOK_START_ATTEMPTED.load(Relaxed), HOOKS_OK.load(Relaxed));

        for (attempted, ok, deaf) in [
            (false, false, true), // NotStarted
            (true, false, true),  // Failed
            (true, true, false),  // Active
        ] {
            HOOK_START_ATTEMPTED.store(attempted, Relaxed);
            HOOKS_OK.store(ok, Relaxed);
            KEYSTROKES.store(0, Relaxed);
            CLICKS.store(0, Relaxed);
            SCROLL.store(0, Relaxed);
            MOUSE_PX.store(0, Relaxed);

            let mut input = WindowsInput {
                window_start: 1000,
                window_ms: 0,
            };
            let tick = input
                .drain(1000)
                .expect("no error")
                .expect("安靜的視窗要出一個 tick");
            let state = Self_state_name(attempted, ok);
            if deaf {
                assert_eq!(
                    tick.listening,
                    InputListening::NotListening,
                    "{state} 的時候不准說成「沒人碰」"
                );
            } else {
                assert_ne!(
                    tick.listening,
                    InputListening::NotListening,
                    "{state} 而且作業系統說整段沒人碰，卻講成「我沒在聽」"
                );
            }
        }

        HOOK_START_ATTEMPTED.store(saved.0, Relaxed);
        HOOKS_OK.store(saved.1, Relaxed);
    }

    /// 只是給上面那條的斷言訊息用的名字。
    #[allow(non_snake_case)]
    fn Self_state_name(attempted: bool, ok: bool) -> &'static str {
        match (attempted, ok) {
            (false, _) => "hook 還沒去裝",
            (true, false) => "hook 裝失敗",
            (true, true) => "hook 裝起來了",
        }
    }

    /// 安靜的視窗仍然不寫一列全 0 的 `input_metrics`——那張表上的 idle 是用
    /// 「沒有列」表達的，一秒一列空紀錄會把資料庫塞滿沒有資訊的東西。
    ///
    /// 但「安靜」本身要留下痕跡，所以改成出一個 `metrics: None` 的 tick，由
    /// recorder 寫進 `input_health`：好分清楚「作業系統證實沒人碰」和「我們
    /// 的 hook 根本沒在聽」。alpha.97 以前這兩件事是同一個沉默——她講的是
    /// 誠實的那一句（「我分不出是哪一種」），代價是**有把握的那一句在真實
    /// 錄製上永遠印不出來**（它要的是一列全 0 的紀錄，而這裡從不寫）；而讓
    /// 它印得出來的便宜作法（把沉默直接當成沒人動）會製造一句更糟的謊。
    ///
    /// 這條也釘住那條鐵律：這個測試程序從來沒有裝過 hook（`install_hooks`
    /// 的唯一入口 `WindowsInput::start` 沒有任何測試會走），所以不管作業系統
    /// 怎麼回答，都**不准**說成「沒人碰」。
    #[test]
    fn a_silent_window_yields_a_health_tick_not_an_empty_metrics_row() {
        let _lock = exclusive();
        KEYSTROKES.store(0, Relaxed);
        CLICKS.store(0, Relaxed);
        SCROLL.store(0, Relaxed);
        MOUSE_PX.store(0, Relaxed);
        // window_ms = 0 讓視窗這一關不參與判定，這條測的是「安靜」本身
        let mut input = WindowsInput {
            window_start: 0,
            window_ms: 0,
        };

        let tick = input
            .drain(1000)
            .expect("no error")
            .expect("安靜的視窗也要出一個 tick，不然「我沒在聽」沒有人記");
        assert!(
            tick.metrics.is_none(),
            "安靜的視窗不可以寫一列全 0 的 input_metrics"
        );
        assert_eq!((tick.ts_start, tick.ts_end), (0, 1000));
        assert_eq!(
            tick.listening,
            InputListening::NotListening,
            "hook 沒裝上的時候，不管作業系統說什麼都不准講成「沒人碰」"
        );
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
        let _lock = exclusive();
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
        let tick = input
            .drain(10_000)
            .expect("no error")
            .expect("視窗滿了就該出列");
        let m = tick.metrics.expect("metrics");
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
        let _lock = exclusive();
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
        let _lock = exclusive();
        KEYSTROKES.store(7, Relaxed);
        CLICKS.store(2, Relaxed);
        SCROLL.store(3, Relaxed);
        MOUSE_PX.store(450, Relaxed);
        BURSTS.store(1, Relaxed);

        let mut input = WindowsInput {
            window_start: 1000,
            window_ms: 10_000,
        };
        let tick = input
            .drain(11_000)
            .expect("no error")
            .expect("some metrics");
        let m = tick.metrics.expect("metrics");
        assert_eq!((m.keystrokes, m.clicks, m.scroll_ticks), (7, 2, 3));
        assert_eq!(m.mouse_px, 450);
        assert_eq!((m.ts_start, m.ts_end), (1000, 11_000));
        // window_switches 歸 recorder，這裡一定是 0
        assert_eq!(m.window_switches, 0);

        // 取走就要歸零，不然下一個視窗會重複計算同一批輸入
        let quiet = input
            .drain(21_000)
            .expect("no error")
            .expect("quiet window");
        assert_eq!(quiet.metrics, None);
        // 這個測試沒有裝 hook（`install_hooks` 的唯一入口 `WindowsInput::start`
        // 整個 crate 的測試都不會走），所以 `state()` 一定是 `NotStarted`，
        // 而 `classify_quiet_window` 對那一態**不看作業系統怎麼回答**——這條
        // 斷言因此是確定的，不是碰運氣。
        //
        // 但它守不住「hook 狀態寫死」那一刀：寫死成 `Active` 之後，紅不紅
        // 就取決於跑測試那台機器剛好閒置了幾秒（這個視窗是 10 秒）。那一刀
        // 的守衛是 `ops.rs` 那條原始碼形狀測試，不是這裡。
        assert_eq!(
            quiet.listening,
            InputListening::NotListening,
            "沒裝 hook 的安靜視窗不可以被講成「沒人碰」"
        );
    }

    #[test]
    fn suspended_input_and_its_baselines_cannot_leak_into_the_next_window() {
        let _lock = exclusive();
        INPUT_GATE.store(0, Release);
        KEYSTROKES.store(7, Relaxed);
        CLICKS.store(2, Relaxed);
        SCROLL.store(3, Relaxed);
        MOUSE_PX.store(450, Relaxed);
        BURSTS.store(4, Relaxed);
        LAST_INPUT_TICK.store(123, Relaxed);
        LAST_KEY_TICK.store(456, Relaxed);
        HAVE_POS.store(true, Relaxed);
        LAST_X.store(800, Relaxed);
        LAST_Y.store(600, Relaxed);

        let mut input = WindowsInput {
            window_start: 1_000,
            window_ms: 10_000,
        };
        input.suspend(6_000).expect("suspend privacy gap");

        assert_ne!(INPUT_GATE.load(Acquire) & INPUT_DISABLED, 0);
        assert_eq!(KEYSTROKES.load(Relaxed), 0);
        assert_eq!(CLICKS.load(Relaxed), 0);
        assert_eq!(SCROLL.load(Relaxed), 0);
        assert_eq!(MOUSE_PX.load(Relaxed), 0);
        assert_eq!(BURSTS.load(Relaxed), 0);
        assert_eq!(LAST_INPUT_TICK.load(Relaxed), 0);
        assert_eq!(LAST_KEY_TICK.load(Relaxed), 0);
        assert!(!HAVE_POS.load(Relaxed));
        assert_eq!((LAST_X.load(Relaxed), LAST_Y.load(Relaxed)), (0, 0));
        assert!(
            InputCallbackGuard::enter().is_none(),
            "gap 內的 callback 必須在計數前就被擋掉"
        );

        // 排除後又發生的一次輸入仍要留下，但它的視窗和計數都
        // 只能從 gap 邊界開始，不得把前面的 7/2/3/450 帶回來。
        input.resume(6_000).expect("resume after privacy gap");
        {
            let _guard = InputCallbackGuard::enter().expect("callbacks enabled after resume");
            KEYSTROKES.fetch_add(1, Relaxed);
            BURSTS.fetch_add(1, Relaxed);
        }
        // 重複 resume 不能把上面這個新事件清掉。
        input.resume(7_000).expect("idempotent resume");
        let tick = input.drain(16_000).expect("input").expect("post-gap input");
        let metrics = tick.metrics.expect("metrics");
        assert_eq!((metrics.ts_start, metrics.ts_end), (6_000, 16_000));
        assert_eq!(metrics.keystrokes, 1);
        assert_eq!(metrics.clicks, 0);
        assert_eq!(metrics.scroll_ticks, 0);
        assert_eq!(metrics.mouse_px, 0);
        assert_eq!(metrics.typing_bursts, 1);
    }

    #[test]
    fn suspend_waits_for_a_callback_that_crossed_the_linearization_point() {
        let _lock = exclusive();
        INPUT_GATE.store(0, Release);
        let guard = InputCallbackGuard::enter().expect("callback enters while enabled");
        assert_eq!(INPUT_GATE.load(Acquire) & INPUT_IN_FLIGHT_MASK, 1);

        let handle = std::thread::spawn(|| {
            let mut input = WindowsInput {
                window_start: 0,
                window_ms: 10_000,
            };
            input.suspend(4_000).expect("suspend");
            input
        });

        // 等 suspend 把 disabled bit 設上。只要舊 callback 還持有 reader 票，
        // 它就不可以返回並清除計數。
        while INPUT_GATE.load(Acquire) & INPUT_DISABLED == 0 {
            std::thread::yield_now();
        }
        assert_eq!(INPUT_GATE.load(Acquire) & INPUT_IN_FLIGHT_MASK, 1);
        assert!(!handle.is_finished(), "suspend 不可越過 in-flight callback");

        drop(guard);
        let mut input = handle.join().expect("suspend thread");
        assert_eq!(INPUT_GATE.load(Acquire), INPUT_DISABLED);
        assert!(InputCallbackGuard::enter().is_none());

        input.resume(4_000).expect("resume");
        assert_eq!(INPUT_GATE.load(Acquire), 0);
        drop(InputCallbackGuard::enter().expect("callback enters after resume"));
    }

    #[test]
    fn idle_never_exceeds_the_window() {
        let _lock = exclusive();
        // idle 比視窗還長是沒有意義的：這個視窗才十秒，不可能閒置一小時
        LAST_INPUT_TICK.store(0, Relaxed);
        KEYSTROKES.store(1, Relaxed);
        let mut input = WindowsInput {
            window_start: 5_000,
            window_ms: 1_000,
        };
        let tick = input.drain(6_000).expect("no error").expect("some metrics");
        let m = tick.metrics.expect("metrics");
        assert!(m.idle_ms <= 1_000, "idle {} > window", m.idle_ms);
    }
}
