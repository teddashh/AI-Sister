//! 第二次啟動的行程仲裁與視窗行為。
//!
//! Tauri 官方 single-instance plugin 仍是唯一的訊號接收者；Windows guard 只先補上官方
//! plugin 在「mutex 已建好、隱藏接收窗還沒建好」之間的競態。它的 Windows 實作全部放在
//! `cfg(windows)` 裡；收到訊號後的邏輯和 guard 決策仍是純 Rust，Linux 開發機可以
//! 直接用 `rustc --test` 執行。plugin callback 只拿得到這個 trait，不能藉機碰
//! recorder。

use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};

/// 只給 native CI 把「第一個 owner 拿到 guard，官方 receiver 還沒建」放大。
///
/// 兩個行程都會繼承這個環境變數，但只有**新建 mutex 的第一個 owner**會睡；
/// 等待轉交的 secondary 和接管 abandoned mutex 的行程都不會再睡一次。
pub(crate) const DIAGNOSTIC_PRIMARY_DELAY_ENV: &str = "AI_SISTER_DIAGNOSTIC_PRIMARY_DELAY_MS";
const MAX_DIAGNOSTIC_PRIMARY_DELAY_MS: u64 = 5_000;
const STARTUP_GUARD_SUFFIX: &str = "-startup-guard-v1";
const OFFICIAL_CLASS_SUFFIX: &str = "-sic";
const OFFICIAL_WINDOW_SUFFIX: &str = "-siw";

/// 官方 plugin 2.4.4 的 `WMCOPYDATA_SINGLE_INSTANCE_DATA`。這個數和 payload
/// 形狀必須一起跟上游；我們是把訊號送給官方 receiver，不是自己實作另一個協定。
const WMCOPYDATA_SINGLE_INSTANCE_DATA: usize = 1542;

fn diagnostic_primary_delay_ms(value: Option<&OsStr>) -> Option<u64> {
    let wanted = value?.to_str()?.parse::<u128>().ok()?;
    let capped = wanted.min(u128::from(MAX_DIAGNOSTIC_PRIMARY_DELAY_MS)) as u64;
    (capped != 0).then_some(capped)
}

fn startup_guard_mutex_name(identifier: &str) -> String {
    format!("Local\\{identifier}{STARTUP_GUARD_SUFFIX}")
}

fn official_receiver_names(identifier: &str) -> (String, String) {
    (
        format!("{identifier}{OFFICIAL_CLASS_SUFFIX}"),
        format!("{identifier}{OFFICIAL_WINDOW_SUFFIX}"),
    )
}

fn official_signal_payload(cwd: &str, args: &[String]) -> Vec<u8> {
    format!("{cwd}|{}\0", args.join("|")).into_bytes()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardMutexProbe {
    Acquired,
    StillOwned,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupGuardAction {
    ForwardToPrimary,
    BecomePrimary,
    Retry,
    FailClosed,
}

/// 仲裁只有四種結果；receiver 在 deadline 上出現仍然要轉交，mutex 在
/// deadline 上放開仍然要接管。只有 owner 還在、receiver 也始終沒出現才失敗關閉。
fn startup_guard_action(
    receiver_ready: bool,
    mutex: GuardMutexProbe,
    deadline_elapsed: bool,
) -> StartupGuardAction {
    if receiver_ready {
        StartupGuardAction::ForwardToPrimary
    } else {
        match mutex {
            GuardMutexProbe::Acquired => StartupGuardAction::BecomePrimary,
            GuardMutexProbe::StillOwned if !deadline_elapsed => StartupGuardAction::Retry,
            GuardMutexProbe::StillOwned | GuardMutexProbe::Failed => StartupGuardAction::FailClosed,
        }
    }
}

#[cfg(windows)]
mod windows_guard {
    use super::{
        DIAGNOSTIC_PRIMARY_DELAY_ENV, GuardMutexProbe, StartupGuardAction,
        WMCOPYDATA_SINGLE_INSTANCE_DATA, diagnostic_primary_delay_ms, official_receiver_names,
        official_signal_payload, startup_guard_action, startup_guard_mutex_name,
    };
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use tauri::Manager;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WAIT_ABANDONED,
        WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
    };
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW, WM_COPYDATA,
    };
    use windows::core::PCWSTR;

    const RECEIVER_WAIT_LIMIT: Duration = Duration::from_secs(20);
    const RECEIVER_POLL_MS: u32 = 50;
    const COPYDATA_TIMEOUT_MS: u32 = 15_000;
    const FAIL_CLOSED_EXIT_CODE: i32 = 70;

    /// 把 guard handle 留在 Tauri state，所有 logging、setup、recorder 與視窗狀態都收掉以前
    /// 這把 mutex 都仍由 primary main thread 擁有。
    ///
    /// 不在 `Drop` 裡 `ReleaseMutex`：mutex 只能由取得它的 thread 釋放，而 Tauri state
    /// 沒有「一定在哪個 thread drop」的型別保證。process 結束時 Windows 會關掉 handle；
    /// owner thread 離開時（正常收尾的最後一刻或異常終止），正在等的 secondary
    /// 會收到 `WAIT_ABANDONED` 並正式取得它。
    struct StartupGuardOwner {
        _handle: isize,
    }

    enum Arbitration {
        Primary(HANDLE),
        Forwarded,
        FailedClosed(&'static str),
    }

    pub(crate) fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
        tauri::plugin::Builder::new("startup-single-instance-guard")
            .setup(|app, _api| {
                let identifier = app.config().identifier.clone();
                match arbitrate(&identifier) {
                    Arbitration::Primary(handle) => {
                        if !app.manage(StartupGuardOwner {
                            _handle: handle.0 as isize,
                        }) {
                            // 同一個 app 出現兩份 guard state 不是可以猜的狀態。這裡仍然
                            // 擁有 mutex，直接結束就不會讓它變成第二個 primary。
                            fail_closed(app, Some(handle), "guard state 重複註冊");
                        }
                    }
                    Arbitration::Forwarded => exit_secondary(app, None, 0),
                    Arbitration::FailedClosed(reason) => {
                        fail_closed(app, None, reason);
                    }
                }
                Ok(())
            })
            .build()
    }

    fn arbitrate(identifier: &str) -> Arbitration {
        // `Local\\` 讓它只在目前 Windows session 裡仲裁，不會讓另一個
        // interactive session 的 AI-Sister 被這一邊擋住。
        let guard_name = encode_wide(&startup_guard_mutex_name(identifier));
        // SAFETY: `guard_name` 有結尾 NUL，在這個呼叫期間一直存活；沒有自訂
        // SECURITY_ATTRIBUTES。
        let handle = match unsafe { CreateMutexW(None, true, PCWSTR(guard_name.as_ptr())) } {
            Ok(handle) => handle,
            Err(_) => return Arbitration::FailedClosed("guard mutex 建立失敗"),
        };
        // GetLastError 一定要緊接 CreateMutexW；中間任何 Win32 呼叫都可能蓋掉答案。
        let already_existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

        if !already_existed {
            if let Some(delay_ms) = diagnostic_primary_delay_ms(
                std::env::var_os(DIAGNOSTIC_PRIMARY_DELAY_ENV).as_deref(),
            ) {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            return Arbitration::Primary(handle);
        }

        wait_for_receiver_or_owner(identifier, handle)
    }

    fn wait_for_receiver_or_owner(identifier: &str, handle: HANDLE) -> Arbitration {
        let (class_name, window_name) = official_receiver_names(identifier);
        let class_name = encode_wide(&class_name);
        let window_name = encode_wide(&window_name);
        let deadline = Instant::now() + RECEIVER_WAIT_LIMIT;

        loop {
            // SAFETY: 兩個 buffer 都有結尾 NUL，呼叫期間存活。FindWindowW 只借用它們。
            let receiver =
                unsafe { FindWindowW(PCWSTR(class_name.as_ptr()), PCWSTR(window_name.as_ptr())) }
                    .ok();

            if startup_guard_action(
                receiver.is_some(),
                GuardMutexProbe::StillOwned,
                Instant::now() >= deadline,
            ) == StartupGuardAction::ForwardToPrimary
            {
                let delivered = receiver.map(forward_official_signal).unwrap_or(false);
                if delivered {
                    // secondary 只有在官方 WndProc 回答 1 以後才回報成功。
                    // 這個 handle 沒有 ownership，關掉後再結束。
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    return Arbitration::Forwarded;
                }

                // receiver 可能在 FindWindowW 之後正好離開。若它的 owner 也已離開，
                // 我們還能接管；若 mutex 還被握著，訊息是否已被處理無法確定，
                // 不重送、不啟動另一份。
                let probe = probe_mutex(handle, 0);
                return match startup_guard_action(false, probe, true) {
                    StartupGuardAction::BecomePrimary => Arbitration::Primary(handle),
                    _ => {
                        unsafe {
                            let _ = CloseHandle(handle);
                        }
                        Arbitration::FailedClosed("receiver 轉交失敗")
                    }
                };
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait_ms = if remaining.is_zero() {
                0
            } else {
                remaining.as_millis().min(u128::from(RECEIVER_POLL_MS)) as u32
            };
            let probe = probe_mutex(handle, wait_ms);
            match startup_guard_action(false, probe, remaining.is_zero()) {
                StartupGuardAction::BecomePrimary => return Arbitration::Primary(handle),
                StartupGuardAction::Retry => continue,
                StartupGuardAction::FailClosed => {
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    return Arbitration::FailedClosed(if probe == GuardMutexProbe::Failed {
                        "guard mutex 等待失敗"
                    } else {
                        "primary 在等待上限內沒有建好 receiver"
                    });
                }
                StartupGuardAction::ForwardToPrimary => unreachable!("receiver 此刻未就緒"),
            }
        }
    }

    fn probe_mutex(handle: HANDLE, timeout_ms: u32) -> GuardMutexProbe {
        // SAFETY: handle 是這個行程的 CreateMutexW 成功回傳值，只在這個函式之後
        // 的成功轉交、失敗關閉或 process 收尾時才關掉。
        match unsafe { WaitForSingleObject(handle, timeout_ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => GuardMutexProbe::Acquired,
            WAIT_TIMEOUT => GuardMutexProbe::StillOwned,
            _ => GuardMutexProbe::Failed,
        }
    }

    fn forward_official_signal(receiver: windows::Win32::Foundation::HWND) -> bool {
        // 這些輸入是 tauri-plugin-single-instance 2.4.4 Windows sender 的 payload 協定。
        let cwd = std::env::current_dir().unwrap_or_default();
        let cwd = cwd.to_str().unwrap_or_default();
        let args = std::env::args().collect::<Vec<String>>();
        let bytes = official_signal_payload(cwd, &args);
        let Ok(byte_count) = u32::try_from(bytes.len()) else {
            return false;
        };
        let copy_data = COPYDATASTRUCT {
            dwData: WMCOPYDATA_SINGLE_INSTANCE_DATA,
            cbData: byte_count,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        let mut receiver_result = 0usize;
        // SAFETY: WM_COPYDATA 在同步呼叫返回前才借用 `copy_data` 和 `bytes`；
        // SendMessageTimeoutW 又把原本無上限的 SendMessageW 限在 15 秒。
        let sent = unsafe {
            SendMessageTimeoutW(
                receiver,
                WM_COPYDATA,
                WPARAM(0),
                LPARAM((&copy_data as *const COPYDATASTRUCT) as isize),
                // 不加 SMTO_ABORTIFHUNG：native race gate 故意讓 primary 在 message
                // loop 前停 5 秒，那正好是 Windows 可能開始把 thread 視為 hung
                // 的邊界。這裡已有硬性 15 秒 timeout，不會變回無上限等待。
                SMTO_BLOCK | SMTO_ERRORONEXIT,
                COPYDATA_TIMEOUT_MS,
                Some(&mut receiver_result),
            )
        };
        sent.0 != 0 && receiver_result == 1
    }

    fn fail_closed<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        owned_handle: Option<HANDLE>,
        reason: &'static str,
    ) -> ! {
        eprintln!("AI-Sister single-instance guard fail-closed: {reason}");
        exit_secondary(app, owned_handle, FAIL_CLOSED_EXIT_CODE)
    }

    fn exit_secondary<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        handle: Option<HANDLE>,
        code: i32,
    ) -> ! {
        if let Some(handle) = handle {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
        app.cleanup_before_exit();
        std::process::exit(code)
    }

    fn encode_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(windows)]
pub(crate) use windows_guard::plugin as startup_guard_plugin;

/// 第二次啟動對已存在視窗做到了哪一步。
///
/// `Missing` 不是 `ShowFailed`：前者在 Tauri 的 setup 還沒建出設定檔裡的視窗時
/// 可以發生，等 setup 跑到那裡再補一次就好；後者則是視窗已經存在、但原生
/// `show` 真的失敗了。兩個狀況不能合成一句「沒打開」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExistingInstanceReveal {
    Revealed,
    Missing,
    ShowFailed(String),
    FocusFailed(String),
}

/// 第二次啟動唯一獲准做的兩件事。
///
/// 這個 trait 故意沒有 recorder、Shell 或 app-exit 方法：第二次啟動不是「結束再
/// 重開」，不能借系統匣的 quit 路徑（那條路會停止 recorder）。
pub(crate) trait RevealWindow {
    fn reveal_show(&self) -> Result<(), String>;
    fn reveal_focus(&self) -> Result<(), String>;
}

fn reveal_existing_instance<W: RevealWindow>(window: Option<&W>) -> ExistingInstanceReveal {
    let Some(window) = window else {
        return ExistingInstanceReveal::Missing;
    };
    if let Err(error) = window.reveal_show() {
        return ExistingInstanceReveal::ShowFailed(error);
    }
    if let Err(error) = window.reveal_focus() {
        return ExistingInstanceReveal::FocusFailed(error);
    }
    ExistingInstanceReveal::Revealed
}

/// plugin 的訊號可能比 Tauri 的 `Ready` 早到。那時候不是把「找不到視窗」當成
/// 已經處理，而是留一個位元給 setup；第一個 instance 本來就會繼續完成開機。
pub(crate) fn reveal_or_defer<W: RevealWindow>(
    pending: &AtomicBool,
    window: Option<&W>,
) -> ExistingInstanceReveal {
    let outcome = reveal_existing_instance(window);
    if outcome == ExistingInstanceReveal::Missing {
        pending.store(true, Ordering::Release);
    }
    outcome
}

pub(crate) fn take_deferred_reveal<W: RevealWindow>(
    pending: &AtomicBool,
    window: &W,
) -> Option<ExistingInstanceReveal> {
    pending
        .swap(false, Ordering::AcqRel)
        .then(|| reveal_existing_instance(Some(window)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn diagnostic_delay_has_a_hard_cap() {
        assert_eq!(
            DIAGNOSTIC_PRIMARY_DELAY_ENV,
            "AI_SISTER_DIAGNOSTIC_PRIMARY_DELAY_MS"
        );
        assert_eq!(diagnostic_primary_delay_ms(None), None);
        assert_eq!(diagnostic_primary_delay_ms(Some(OsStr::new("0"))), None);
        assert_eq!(
            diagnostic_primary_delay_ms(Some(OsStr::new("not-a-number"))),
            None
        );
        assert_eq!(
            diagnostic_primary_delay_ms(Some(OsStr::new("1250"))),
            Some(1_250)
        );
        assert_eq!(
            diagnostic_primary_delay_ms(Some(OsStr::new("999999999999999999999999"))),
            Some(MAX_DIAGNOSTIC_PRIMARY_DELAY_MS)
        );
    }

    #[test]
    fn guard_and_official_receiver_names_follow_the_configured_identifier() {
        let identifier = "com.ted-h.ai-sister";
        assert_eq!(
            startup_guard_mutex_name(identifier),
            "Local\\com.ted-h.ai-sister-startup-guard-v1"
        );
        assert_eq!(
            official_receiver_names(identifier),
            (
                "com.ted-h.ai-sister-sic".to_string(),
                "com.ted-h.ai-sister-siw".to_string()
            )
        );
    }

    #[test]
    fn forwarded_signal_uses_the_official_copydata_payload_shape() {
        assert_eq!(WMCOPYDATA_SINGLE_INSTANCE_DATA, 1542);
        assert_eq!(
            official_signal_payload(
                r"C:\work\AI-Sister",
                &["sister-desktop.exe".into(), "--example".into()]
            ),
            b"C:\\work\\AI-Sister|sister-desktop.exe|--example\0"
        );
    }

    #[test]
    fn guard_only_forwards_takes_ownership_retries_or_fails_closed() {
        assert_eq!(
            startup_guard_action(true, GuardMutexProbe::StillOwned, true),
            StartupGuardAction::ForwardToPrimary
        );
        assert_eq!(
            startup_guard_action(false, GuardMutexProbe::Acquired, true),
            StartupGuardAction::BecomePrimary
        );
        assert_eq!(
            startup_guard_action(false, GuardMutexProbe::StillOwned, false),
            StartupGuardAction::Retry
        );
        assert_eq!(
            startup_guard_action(false, GuardMutexProbe::StillOwned, true),
            StartupGuardAction::FailClosed
        );
        assert_eq!(
            startup_guard_action(false, GuardMutexProbe::Failed, false),
            StartupGuardAction::FailClosed
        );
    }

    struct FakeWindow {
        calls: RefCell<Vec<&'static str>>,
        show: Result<(), &'static str>,
        focus: Result<(), &'static str>,
    }

    impl FakeWindow {
        fn succeeds() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                show: Ok(()),
                focus: Ok(()),
            }
        }
    }

    impl RevealWindow for FakeWindow {
        fn reveal_show(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("show");
            self.show.map_err(str::to_string)
        }

        fn reveal_focus(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("focus");
            self.focus.map_err(str::to_string)
        }
    }

    #[test]
    fn a_second_launch_shows_before_it_focuses_the_existing_pet() {
        let window = FakeWindow::succeeds();
        assert_eq!(
            reveal_existing_instance(Some(&window)),
            ExistingInstanceReveal::Revealed
        );
        assert_eq!(*window.calls.borrow(), ["show", "focus"]);
    }

    #[test]
    fn an_early_second_launch_is_deferred_until_the_pet_exists() {
        let pending = AtomicBool::new(false);
        let absent: Option<&FakeWindow> = None;
        assert_eq!(
            reveal_or_defer(&pending, absent),
            ExistingInstanceReveal::Missing
        );
        assert!(pending.load(Ordering::Acquire));

        let window = FakeWindow::succeeds();
        assert_eq!(
            take_deferred_reveal(&pending, &window),
            Some(ExistingInstanceReveal::Revealed)
        );
        assert!(!pending.load(Ordering::Acquire));
        assert_eq!(*window.calls.borrow(), ["show", "focus"]);
        assert_eq!(take_deferred_reveal(&pending, &window), None);
    }

    #[test]
    fn show_and_focus_failures_are_not_reported_as_a_revealed_window() {
        let show_fails = FakeWindow {
            calls: RefCell::new(Vec::new()),
            show: Err("show broke"),
            focus: Ok(()),
        };
        assert_eq!(
            reveal_existing_instance(Some(&show_fails)),
            ExistingInstanceReveal::ShowFailed("show broke".into())
        );
        assert_eq!(*show_fails.calls.borrow(), ["show"]);

        let focus_fails = FakeWindow {
            calls: RefCell::new(Vec::new()),
            show: Ok(()),
            focus: Err("focus broke"),
        };
        assert_eq!(
            reveal_existing_instance(Some(&focus_fails)),
            ExistingInstanceReveal::FocusFailed("focus broke".into())
        );
        assert_eq!(*focus_fails.calls.borrow(), ["show", "focus"]);
    }
}
