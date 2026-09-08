//! Windows installer 與產品行程之間的啟動交接。
//!
//! 兩邊各有一顆 process-lifetime named kernel object：installer 在修改 AI-Sister
//! payload／安裝登錄的 section 持有 mutex；每個產品行程則在碰 log、DB、WebView 或
//! 記憶／設定前開啟同一顆 manual-reset event，並保存 handle 到行程結束。產品採
//! 「查 installer → 建 product event → 再查 installer」，installer 則採
//! 「建 installer mutex → 查 product event」，所以帶協定的兩邊不論誰先走都不會一起
//! 進入產品狀態與 installer section。
//!
//! Windows loader 會在 Rust `main` 前先映射 exe，而且較舊的 binary 不認識這兩顆 object；
//! 因此這層不能冒充跨舊版的檔案替換已經原子化。NSIS 仍會 best-effort 掃描兩個舊 image，
//! 但那個 upstream current-user scanner 無法把所有列舉／token 失敗表成 Unknown。
//!
//! 這不是 single-instance lock，也不代表 recorder 正在跑。多個產品行程可以各自開啟同一
//! event；最後一個 handle 關閉後，kernel 才移除 object。

/// NSIS hook 與兩支產品執行檔共同使用的 installer-side object name。
pub const INSTALL_LIFECYCLE_MUTEX_NAME: &str = "Global\\com.ted-h.ai-sister-install-lifecycle-v1";

/// 所有帶協定的產品行程共同持有的 product-side object name。
pub const PRODUCT_LIFECYCLE_EVENT_NAME: &str = "Global\\com.ted-h.ai-sister-product-lifecycle-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerAdmission {
    /// 沒有 installer lifecycle owner；產品可以繼續下一段握手。
    Clear,
    /// 安裝或移除正在進行；這次啟動必須在任何產品狀態前結束。
    InProgress,
    /// 問不到 named object 是否存在；不能把沒量到冒充沒有 installer。
    Uncheckable,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectProbe {
    Missing,
    Present,
    Failed,
}

#[cfg(any(windows, test))]
const fn classify(probe: ObjectProbe) -> InstallerAdmission {
    match probe {
        ObjectProbe::Missing => InstallerAdmission::Clear,
        ObjectProbe::Present => InstallerAdmission::InProgress,
        ObjectProbe::Failed => InstallerAdmission::Uncheckable,
    }
}

/// 產品加入 lifecycle 成功後持有到行程結束的 handle。
///
/// 呼叫端不能提早 drop；否則 installer 會把仍在使用產品狀態的行程看成不存在。
pub struct ProductLifecycleGuard {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for ProductLifecycleGuard {
    fn drop(&mut self) {
        // SAFETY: handle 只由成功的 CreateEventW 產生，guard 擁有且只關閉一次。
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.handle) };
    }
}

/// 在任何產品狀態之前加入 product lifecycle。
///
/// 產品先查 installer、建立共用 event，再重查 installer。只有兩次都明確量到 mutex
/// 不存在才回傳 guard；存在或任何 native error 都 fail closed。
pub fn enter_product_lifecycle() -> Result<ProductLifecycleGuard, InstallerAdmission> {
    #[cfg(windows)]
    {
        windows_lifecycle::enter(INSTALL_LIFECYCLE_MUTEX_NAME, PRODUCT_LIFECYCLE_EVENT_NAME)
            .map(|handle| ProductLifecycleGuard { handle })
    }
    #[cfg(not(windows))]
    {
        Ok(ProductLifecycleGuard {})
    }
}

#[cfg(windows)]
mod windows_lifecycle {
    use super::{InstallerAdmission, ObjectProbe, classify};
    use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE};
    #[cfg(test)]
    use windows::Win32::System::Threading::OpenEventW;
    use windows::Win32::System::Threading::{
        CreateEventW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
    };
    use windows::core::{HRESULT, PCWSTR};

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn probe_mutex(name: &str) -> ObjectProbe {
        let wide = wide(name);
        // SAFETY: `wide` 有結尾 NUL 且在呼叫期間存活；只要求 synchronize 權限，
        // 不取得／釋放 mutex ownership，也不改 installer 的生命週期。
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr())) } {
            Ok(handle) => close_probe_handle(handle),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
                ObjectProbe::Missing
            }
            Err(_) => ObjectProbe::Failed,
        }
    }

    #[cfg(test)]
    pub(super) fn probe_event(name: &str) -> ObjectProbe {
        let wide = wide(name);
        // SAFETY: same NUL-terminated lifetime argument as probe_mutex; this only opens a handle.
        match unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr())) } {
            Ok(handle) => close_probe_handle(handle),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
                ObjectProbe::Missing
            }
            Err(_) => ObjectProbe::Failed,
        }
    }

    fn close_probe_handle(handle: HANDLE) -> ObjectProbe {
        // SAFETY: handle 是 OpenMutexW／OpenEventW 的成功回傳值，且只關閉一次。
        if unsafe { CloseHandle(handle) }.is_ok() {
            ObjectProbe::Present
        } else {
            ObjectProbe::Failed
        }
    }

    fn create_product_event(name: &str) -> Result<HANDLE, InstallerAdmission> {
        let wide = wide(name);
        // SAFETY: `wide` 有結尾 NUL 且在呼叫期間存活。manual-reset event 不會被 signal；
        // 只用 kernel object 的 handle lifetime 表示至少一個產品行程仍在。
        unsafe { CreateEventW(None, true, false, PCWSTR(wide.as_ptr())) }
            .map_err(|_| InstallerAdmission::Uncheckable)
    }

    pub(super) fn enter(
        installer_name: &str,
        product_name: &str,
    ) -> Result<HANDLE, InstallerAdmission> {
        match classify(probe_mutex(installer_name)) {
            InstallerAdmission::Clear => {}
            blocked => return Err(blocked),
        }

        let product_handle = create_product_event(product_name)?;
        match classify(probe_mutex(installer_name)) {
            InstallerAdmission::Clear => Ok(product_handle),
            blocked => {
                // SAFETY: product_handle 是本函式剛取得且尚未交給 guard 的唯一 handle。
                let _ = unsafe { CloseHandle(product_handle) };
                Err(blocked)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_measured_missing_object_allows_the_next_step() {
        assert_eq!(classify(ObjectProbe::Missing), InstallerAdmission::Clear);
        assert_eq!(
            classify(ObjectProbe::Present),
            InstallerAdmission::InProgress
        );
        assert_eq!(
            classify(ObjectProbe::Failed),
            InstallerAdmission::Uncheckable
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_has_no_nsis_lifecycle() {
        assert!(enter_product_lifecycle().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn native_product_event_lives_exactly_as_long_as_the_guard() {
        let suffix = format!("{}", std::process::id());
        let installer_name = format!("Local\\ai-sister-installer-test-{suffix}");
        let product_name = format!("Local\\ai-sister-product-test-{suffix}");

        assert_eq!(
            windows_lifecycle::probe_event(&product_name),
            ObjectProbe::Missing
        );
        let handle = windows_lifecycle::enter(&installer_name, &product_name)
            .expect("enter product lifecycle");
        assert_eq!(
            windows_lifecycle::probe_event(&product_name),
            ObjectProbe::Present
        );
        // SAFETY: handle is owned by this test and closed exactly once.
        unsafe { windows::Win32::Foundation::CloseHandle(handle) }
            .expect("close product lifecycle event");
        assert_eq!(
            windows_lifecycle::probe_event(&product_name),
            ObjectProbe::Missing
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_installer_mutex_blocks_product_before_it_creates_an_event() {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::core::PCWSTR;

        let suffix = format!("{}", std::process::id());
        let installer_name = format!("Local\\ai-sister-installer-block-test-{suffix}");
        let product_name = format!("Local\\ai-sister-product-block-test-{suffix}");
        let wide = installer_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: unique test name is NUL-terminated and lives through the call.
        let installer_handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }
            .expect("create installer lifecycle test mutex");

        assert!(matches!(
            windows_lifecycle::enter(&installer_name, &product_name),
            Err(InstallerAdmission::InProgress)
        ));
        assert_eq!(
            windows_lifecycle::probe_event(&product_name),
            ObjectProbe::Missing
        );
        // SAFETY: handle is the successful CreateMutexW result and is closed exactly once.
        unsafe { CloseHandle(installer_handle) }.expect("close installer lifecycle test mutex");
    }
}
