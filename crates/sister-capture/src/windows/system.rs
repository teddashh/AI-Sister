//! Windows 鎖定狀態來源。
//!
//! Recorder 在任何 focus／clipboard／input／screen 之前先 poll 這裡。
//! 因此 WTS 問不出來不可以被壓成 `unlocked`：它會讓鎖屏當下的
//! 內容來源繼續被讀取。
//!
//! 這是同步 polling source，不是 Windows lifecycle event feed。它只能回報
//! 相鄰兩次成功樣本之間**觀察到的** lock/unlock 變化；兩次 poll 之間若完成
//! 一整輪 lock → unlock，這裡沒有證據可以補造那兩筆事件。

use anyhow::{Context, Result, bail};
use sister_core::model::Millis;
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK, WTS_SESSIONSTATE_UNLOCK, WTSActive, WTSFreeMemory,
    WTSINFOEXW, WTSQuerySessionInformationW, WTSSessionInfoEx,
};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::core::PWSTR;

use crate::traits::{
    SystemContentState, SystemObservation, SystemSource, SystemTransition, SystemTransitionKind,
};

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `GetVersionExW` can be compatibility-manifest virtualised. `RtlGetVersion`
    /// returns the real kernel version, which matters here because Windows 7 / Server
    /// 2008 R2 use the opposite numeric meaning for the WTS lock flags.
    fn RtlGetVersion(version: *mut OSVERSIONINFOW) -> i32;
}

fn windows_major_version() -> Result<u32> {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    let status = unsafe { RtlGetVersion(&mut version) };
    if status < 0 {
        bail!("RtlGetVersion failed with NTSTATUS 0x{:08x}", status as u32);
    }
    Ok(version.dwMajorVersion)
}

/// 把一筆完整 WTS Level 1 樣本分成已知狀態或不可觀測。
///
/// `SessionFlags` 單獨不夠：disconnected session 就算殘留 UNLOCK 也沒有可讀
/// 桌面。Windows 7 又把 LOCK/UNLOCK 數值定義反了，所以舊版一律 Unknown；
/// 目前 Release 1.0 的 Windows 合約從 Windows 10 起算。
fn classify_session(
    os_major: u32,
    session_state: i32,
    session_flags: i32,
) -> Option<SystemContentState> {
    if os_major < 10 || session_state != WTSActive.0 {
        return None;
    }

    match session_flags as u32 {
        WTS_SESSIONSTATE_UNLOCK => Some(SystemContentState::active()),
        WTS_SESSIONSTATE_LOCK => Some(SystemContentState::locked_awake()),
        _ => None,
    }
}

/// WTS 報告的當前 session 是否能讀使用者內容。
///
/// `Ok(None)` 是 API 成功但狀態不符合唯一允許的完整組合；`Err` 是根本沒讀到。
/// [`WindowsSystem::poll`] 將兩者都正規化成 fail-closed `Unknown`，但替後者留下
/// warning，避免診斷消失。
pub(crate) fn content_state() -> Result<Option<SystemContentState>> {
    let os_major = windows_major_version().context("read real Windows version")?;
    if os_major < 10 {
        return Ok(None);
    }

    let mut buffer = PWSTR::null();
    let mut bytes = 0u32;
    unsafe {
        WTSQuerySessionInformationW(
            None,
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buffer,
            &mut bytes,
        )
    }
    .context("WTSQuerySessionInformationW(WTSSessionInfoEx)")?;

    if buffer.is_null() {
        bail!("WTS session info returned a null buffer");
    }

    // Query 成功後這塊記憶體一定由 WTSFreeMemory 收回，包括
    // level／size 不合契約的路徑。
    let result = unsafe {
        if (bytes as usize) < std::mem::size_of::<WTSINFOEXW>() {
            Err(anyhow::anyhow!(
                "WTS session info was {bytes} bytes; need {}",
                std::mem::size_of::<WTSINFOEXW>()
            ))
        } else {
            let info = &*(buffer.0 as *const WTSINFOEXW);
            if info.Level != 1 {
                Err(anyhow::anyhow!(
                    "WTS session info level was {}; need 1",
                    info.Level
                ))
            } else {
                let level = info.Data.WTSInfoExLevel1;
                Ok(classify_session(
                    os_major,
                    level.SessionState.0,
                    level.SessionFlags,
                ))
            }
        }
    };
    unsafe { WTSFreeMemory(buffer.0.cast()) };
    result
}

/// 只將相鄰成功樣本之間已觀察到的 lock/unlock 轉換向上送。
///
/// Sleep/wake 需要原生 power notification；這個同步後端還沒有那個訊號，
/// 所以不用時鐘空洞猜，也不偽造事件。WTS query 仍有回應時，已知 lock 狀態
/// 是 `locked_awake`；query 失敗則整個狀態 Unknown。
pub struct WindowsSystem {
    previous: Option<SystemContentState>,
    next_sequence: u64,
}

impl WindowsSystem {
    pub fn new() -> Self {
        Self {
            previous: None,
            next_sequence: 1,
        }
    }

    fn observe_sample(
        &mut self,
        ts: Millis,
        state: Option<SystemContentState>,
    ) -> Result<SystemObservation> {
        let Some(state) = state else {
            // 不可觀測空洞前後的差異不能冒充發生在復原這一刻的事件。
            // 已交出去的 sequence 則仍是事實，不能倒回 1 重用。
            self.previous = None;
            return Ok(SystemObservation::Unknown);
        };

        let kind = match (self.previous, state) {
            (Some(previous), current) if previous == current => None,
            (Some(previous), current)
                if previous.checked_applying(SystemTransitionKind::Lock) == Some(current) =>
            {
                Some(SystemTransitionKind::Lock)
            }
            (Some(previous), current)
                if previous.checked_applying(SystemTransitionKind::Unlock) == Some(current) =>
            {
                Some(SystemTransitionKind::Unlock)
            }
            // 第一個或 Unknown 後的第一個成功樣本只是 baseline。
            (None, _) => None,
            // 這個 source 目前只會產生 active／locked_awake；若型別日後長出
            // power 狀態而沒有對應事件，寧可拒絕也不能靜默改 state。
            (Some(previous), current) => bail!(
                "WTS state changed from {previous:?} to {current:?} without a valid transition"
            ),
        };

        let transitions = if let Some(kind) = kind {
            let sequence = self.next_sequence;
            self.next_sequence = sequence
                .checked_add(1)
                .context("Windows system transition sequence exhausted")?;
            vec![SystemTransition { sequence, ts, kind }]
        } else {
            Vec::new()
        };
        self.previous = Some(state);

        Ok(SystemObservation::Known { state, transitions })
    }
}

impl Default for WindowsSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSource for WindowsSystem {
    fn poll(&mut self, ts: Millis) -> Result<SystemObservation> {
        match content_state() {
            Ok(state) => self.observe_sample(ts, state),
            Err(error) => {
                tracing::warn!(error = %error, "Windows system state unavailable; capture remains fail closed");
                self.observe_sample(ts, None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::RemoteDesktop::{WTSDisconnected, WTSDown};

    fn known(observation: SystemObservation) -> (SystemContentState, Vec<SystemTransition>) {
        match observation {
            SystemObservation::Known { state, transitions } => (state, transitions),
            SystemObservation::Unknown => panic!("expected a known system observation"),
        }
    }

    #[test]
    fn only_active_unlock_allows_content() {
        assert_eq!(
            classify_session(10, WTSActive.0, WTS_SESSIONSTATE_UNLOCK as i32),
            Some(SystemContentState::active())
        );
        assert!(
            classify_session(10, WTSActive.0, WTS_SESSIONSTATE_UNLOCK as i32)
                .expect("known active state")
                .allows_content()
        );
    }

    #[test]
    fn active_lock_is_known_but_blocks_content() {
        let state = classify_session(10, WTSActive.0, WTS_SESSIONSTATE_LOCK as i32)
            .expect("active locked session is observable");
        assert_eq!(state, SystemContentState::locked_awake());
        assert!(!state.allows_content());
    }

    #[test]
    fn old_windows_disconnected_and_contradictory_samples_are_unknown() {
        // Windows 7 uses the opposite numeric meanings for these flags. The OS
        // version gate must win before either value can be interpreted.
        assert_eq!(
            classify_session(6, WTSActive.0, WTS_SESSIONSTATE_UNLOCK as i32),
            None
        );
        assert_eq!(
            classify_session(6, WTSActive.0, WTS_SESSIONSTATE_LOCK as i32),
            None
        );
        assert_eq!(
            classify_session(10, WTSDisconnected.0, WTS_SESSIONSTATE_UNLOCK as i32),
            None
        );
        assert_eq!(
            classify_session(10, WTSDown.0, WTS_SESSIONSTATE_LOCK as i32),
            None
        );
        assert_eq!(classify_session(10, WTSActive.0, -1), None);
    }

    #[test]
    fn first_sample_and_redundant_samples_do_not_invent_transitions() {
        let mut source = WindowsSystem::new();

        let (state, transitions) = known(
            source
                .observe_sample(100, Some(SystemContentState::active()))
                .unwrap(),
        );
        assert_eq!(state, SystemContentState::active());
        assert!(transitions.is_empty());

        let (_, transitions) = known(
            source
                .observe_sample(101, Some(SystemContentState::active()))
                .unwrap(),
        );
        assert!(transitions.is_empty());
    }

    #[test]
    fn observed_lock_unlock_transitions_have_exact_monotonic_sequences() {
        let mut source = WindowsSystem::new();
        source
            .observe_sample(100, Some(SystemContentState::active()))
            .unwrap();

        let (_, lock) = known(
            source
                .observe_sample(110, Some(SystemContentState::locked_awake()))
                .unwrap(),
        );
        assert_eq!(
            lock,
            vec![SystemTransition {
                sequence: 1,
                ts: 110,
                kind: SystemTransitionKind::Lock,
            }]
        );

        let (_, unlock) = known(
            source
                .observe_sample(120, Some(SystemContentState::active()))
                .unwrap(),
        );
        assert_eq!(
            unlock,
            vec![SystemTransition {
                sequence: 2,
                ts: 120,
                kind: SystemTransitionKind::Unlock,
            }]
        );
    }

    #[test]
    fn unknown_resets_state_baseline_without_reusing_sequence() {
        let mut source = WindowsSystem::new();
        source
            .observe_sample(100, Some(SystemContentState::active()))
            .unwrap();
        source
            .observe_sample(110, Some(SystemContentState::locked_awake()))
            .unwrap();

        assert_eq!(
            source.observe_sample(120, None).unwrap(),
            SystemObservation::Unknown
        );

        // Recovery does not prove that an unlock happened at 130.
        let (_, recovery) = known(
            source
                .observe_sample(130, Some(SystemContentState::active()))
                .unwrap(),
        );
        assert!(recovery.is_empty());

        let (_, next_lock) = known(
            source
                .observe_sample(140, Some(SystemContentState::locked_awake()))
                .unwrap(),
        );
        assert_eq!(next_lock.len(), 1);
        assert_eq!(next_lock[0].sequence, 2);
        assert_eq!(next_lock[0].kind, SystemTransitionKind::Lock);
    }
}
