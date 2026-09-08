//! Current-user Windows 登入啟動。
//!
//! `HKCU\\...\\Run` 是唯一真相；設定檔不另存一份布林值。這一層只管理自己的
//! Run value，不碰 Windows 的 `StartupApproved`（使用者仍可在「啟動應用程式」
//! 另外停用）。為了不讓 portable／診斷執行檔改到安裝版的值，寫入前還要以
//! uninstaller 的 `InstallLocation` 精確證明目前 exe 就在 current-user 安裝目錄。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LoginStartupState {
    #[cfg(windows)]
    Enabled,
    #[cfg(windows)]
    Disabled,
    #[cfg(windows)]
    Mismatch,
    #[cfg(windows)]
    Unreadable,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LoginStartupView {
    pub(crate) state: LoginStartupState,
    pub(crate) expected: Option<String>,
    pub(crate) actual: Option<String>,
    pub(crate) reason: Option<String>,
}

impl LoginStartupView {
    fn unsupported(expected: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            state: LoginStartupState::Unsupported,
            expected,
            actual: None,
            reason: Some(reason.into()),
        }
    }

    #[cfg(windows)]
    fn unreadable(expected: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            state: LoginStartupState::Unreadable,
            expected,
            actual: None,
            reason: Some(reason.into()),
        }
    }
}

/// 刻意不用裸 `bool` 穿過 command 邊界；它只代表這一個 HKCU Run 選項。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
pub(crate) struct LoginStartupEnabled(bool);

#[tauri::command]
pub(crate) fn login_startup_read() -> LoginStartupView {
    platform::read()
}

#[tauri::command]
pub(crate) fn login_startup_set(enabled: LoginStartupEnabled) -> Result<LoginStartupView, String> {
    platform::set(enabled.0)
}

#[cfg(not(windows))]
mod platform {
    use super::LoginStartupView;

    pub(super) fn read() -> LoginStartupView {
        unsupported()
    }

    pub(super) fn set(_enabled: bool) -> Result<LoginStartupView, String> {
        Err(unsupported()
            .reason
            .expect("unsupported platform always has a reason"))
    }

    fn unsupported() -> LoginStartupView {
        LoginStartupView::unsupported(
            None,
            "登入時啟動只支援 Windows current-user 安裝版；這個平台不會修改任何設定。",
        )
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::path::Path;
    use std::ptr::null_mut;

    use sister_shell::login_startup::{
        LoginCommand, RUN_VALUE_NAME, RegistrationState, RunValue, exact_install_location,
        exact_login_command, install_location_matches, registration_state,
    };
    use windows::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
        REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    use super::{LoginStartupState, LoginStartupView};

    const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const UNINSTALL_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AI-Sister";
    const INSTALL_LOCATION_VALUE: &str = "InstallLocation";
    const EXECUTABLE_NAME: &str = "sister-desktop.exe";
    const MAX_REGISTRY_VALUE_BYTES: u32 = 64 * 1024;
    const QUERY_RETRIES: usize = 3;

    #[derive(Debug)]
    struct InstalledContext {
        expected_command: LoginCommand,
    }

    impl InstalledContext {
        fn expected(&self) -> String {
            self.expected_command.as_str().to_owned()
        }
    }

    #[derive(Clone, Copy)]
    struct RegistryTarget<'a> {
        subkey: &'a str,
        value_name: &'a str,
    }

    const RUN_TARGET: RegistryTarget<'static> = RegistryTarget {
        subkey: RUN_SUBKEY,
        value_name: RUN_VALUE_NAME,
    };
    const INSTALL_TARGET: RegistryTarget<'static> = RegistryTarget {
        subkey: UNINSTALL_SUBKEY,
        value_name: INSTALL_LOCATION_VALUE,
    };

    #[derive(Debug, PartialEq, Eq)]
    enum RegistryValue {
        Missing,
        String(String),
        Other { reason: String },
    }

    struct OwnedKey(HKEY);

    impl Drop for OwnedKey {
        fn drop(&mut self) {
            // SAFETY: `OwnedKey` is constructed only after an API returned a valid owned handle.
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub(super) fn read() -> LoginStartupView {
        match installed_context() {
            Ok(context) => read_registered(&context),
            Err(view) => view,
        }
    }

    pub(super) fn set(enabled: bool) -> Result<LoginStartupView, String> {
        let executable =
            std::env::current_exe().map_err(|error| format!("無法取得目前執行檔路徑：{error}"))?;
        set_at(enabled, &executable, INSTALL_TARGET, RUN_TARGET)
    }

    fn set_at(
        enabled: bool,
        executable: &Path,
        install_target: RegistryTarget<'_>,
        run_target: RegistryTarget<'_>,
    ) -> Result<LoginStartupView, String> {
        let context = match installed_context_at(executable, install_target) {
            Ok(context) => context,
            Err(view) => {
                return Err(view
                    .reason
                    .unwrap_or_else(|| "這份執行檔不支援登入啟動設定。".to_owned()));
            }
        };
        let mutation = if enabled {
            write_string(run_target, context.expected_command.as_str())
        } else {
            delete_value(run_target)
        };
        if let Err(reason) = mutation {
            return Err(format!("無法更新目前使用者的登入啟動登錄值：{reason}"));
        }

        // 寫完一定讀回；「API 回成功」不能冒充「目前狀態已符合要求」。
        desired_readback(enabled, read_registered_at(&context, run_target))
    }

    fn installed_context() -> Result<InstalledContext, LoginStartupView> {
        let executable = std::env::current_exe().map_err(|error| {
            LoginStartupView::unreadable(None, format!("無法取得目前執行檔路徑：{error}"))
        })?;
        installed_context_at(&executable, INSTALL_TARGET)
    }

    fn installed_context_at(
        executable: &Path,
        install_target: RegistryTarget<'_>,
    ) -> Result<InstalledContext, LoginStartupView> {
        if executable.file_name() != Some(OsStr::new(EXECUTABLE_NAME)) {
            return Err(LoginStartupView::unsupported(
                None,
                "目前執行檔不是安裝版 sister-desktop.exe；不會修改安裝版的登入啟動值。",
            ));
        }
        let executable_text = executable.to_str().ok_or_else(|| {
            LoginStartupView::unreadable(None, "目前執行檔路徑不是有效的 Unicode。")
        })?;
        let expected_command = exact_login_command(executable_text).map_err(|error| {
            LoginStartupView::unreadable(
                None,
                format!("目前執行檔不能形成 Windows Run 命令：{error}"),
            )
        })?;
        let expected_text = Some(expected_command.as_str().to_owned());
        let directory = executable
            .parent()
            .and_then(|path| path.to_str())
            .ok_or_else(|| {
                LoginStartupView::unreadable(
                    expected_text.clone(),
                    "目前執行檔沒有可讀的 Unicode parent directory。",
                )
            })?;
        let expected_location = exact_install_location(directory).map_err(|error| {
            LoginStartupView::unreadable(
                expected_text.clone(),
                format!("目前安裝目錄不能形成 InstallLocation：{error}"),
            )
        })?;

        match query_value(install_target) {
            Ok(RegistryValue::String(actual))
                if install_location_matches(&expected_location, &actual) =>
            {
                Ok(InstalledContext { expected_command })
            }
            Ok(RegistryValue::Missing) => Err(LoginStartupView::unsupported(
                expected_text,
                "找不到精確對應目前執行檔的 current-user 安裝資訊；portable／診斷版不修改登入啟動。",
            )),
            Ok(RegistryValue::String(_)) => Err(LoginStartupView::unsupported(
                expected_text,
                "current-user InstallLocation 不精確對應目前執行檔；不會修改另一份安裝的登入啟動值。",
            )),
            Ok(RegistryValue::Other { reason }) => Err(LoginStartupView::unreadable(
                expected_text,
                format!("current-user InstallLocation 不是可驗證的字串：{reason}"),
            )),
            Err(reason) => Err(LoginStartupView::unreadable(
                expected_text,
                format!("無法讀取 current-user InstallLocation：{reason}"),
            )),
        }
    }

    fn read_registered(context: &InstalledContext) -> LoginStartupView {
        read_registered_at(context, RUN_TARGET)
    }

    fn read_registered_at(
        context: &InstalledContext,
        target: RegistryTarget<'_>,
    ) -> LoginStartupView {
        let expected = context.expected();
        match query_value(target) {
            Ok(RegistryValue::Missing) => LoginStartupView {
                state: LoginStartupState::Disabled,
                expected: Some(expected),
                actual: None,
                reason: None,
            },
            Ok(RegistryValue::String(actual)) => {
                let state = match registration_state(
                    &context.expected_command,
                    RunValue::String(actual.as_str()),
                ) {
                    RegistrationState::Enabled => LoginStartupState::Enabled,
                    RegistrationState::Disabled => LoginStartupState::Disabled,
                    RegistrationState::Mismatch => LoginStartupState::Mismatch,
                };
                LoginStartupView {
                    state,
                    expected: Some(expected),
                    actual: Some(actual),
                    reason: (state == LoginStartupState::Mismatch)
                        .then(|| "HKCU Run 內已有 AI-Sister，但不是這一版的精確命令。".to_owned()),
                }
            }
            Ok(RegistryValue::Other { reason }) => {
                debug_assert_eq!(
                    registration_state(&context.expected_command, RunValue::Other),
                    RegistrationState::Mismatch
                );
                LoginStartupView {
                    state: LoginStartupState::Mismatch,
                    expected: Some(expected),
                    actual: None,
                    reason: Some(format!(
                        "HKCU Run 內已有 AI-Sister，但不是可核對的 REG_SZ：{reason}"
                    )),
                }
            }
            Err(reason) => LoginStartupView::unreadable(
                Some(expected),
                format!("無法讀取目前使用者的 HKCU Run 值：{reason}"),
            ),
        }
    }

    fn desired_readback(
        enabled: bool,
        readback: LoginStartupView,
    ) -> Result<LoginStartupView, String> {
        let desired = if enabled {
            LoginStartupState::Enabled
        } else {
            LoginStartupState::Disabled
        };
        if readback.state == desired {
            return Ok(readback);
        }

        let reason = readback
            .reason
            .as_deref()
            .unwrap_or("讀回值不符合剛才要求的狀態");
        Err(format!(
            "登入啟動寫入後讀回為 {}，不是要求的 {}：{reason}",
            state_name(readback.state),
            state_name(desired)
        ))
    }

    fn state_name(state: LoginStartupState) -> &'static str {
        match state {
            LoginStartupState::Enabled => "enabled",
            LoginStartupState::Disabled => "disabled",
            LoginStartupState::Mismatch => "mismatch",
            LoginStartupState::Unreadable => "unreadable",
            LoginStartupState::Unsupported => "unsupported",
        }
    }

    fn query_value(target: RegistryTarget<'_>) -> Result<RegistryValue, String> {
        let key = match open_key(target.subkey, KEY_QUERY_VALUE)? {
            Some(key) => key,
            None => return Ok(RegistryValue::Missing),
        };
        let name = wide(target.value_name);
        let mut value_type = REG_VALUE_TYPE(0);
        let mut length = 0u32;
        // SAFETY: the key is valid for this scope; the name is NUL-terminated; the size/type
        // pointers reference initialized local storage. No data buffer is supplied on this pass.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut value_type),
                None,
                Some(&mut length),
            )
        };
        if is_missing(status) {
            return Ok(RegistryValue::Missing);
        }
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return Err(win32_error("RegQueryValueExW(size)", status));
        }

        for _ in 0..QUERY_RETRIES {
            if length > MAX_REGISTRY_VALUE_BYTES {
                return Err(format!(
                    "登錄值宣告 {length} bytes，超過安全讀取上限 {MAX_REGISTRY_VALUE_BYTES}"
                ));
            }
            let mut bytes = vec![0u8; length as usize];
            let mut actual_length = length;
            // SAFETY: the buffer has `length` bytes, and `actual_length` advertises exactly that
            // capacity. A zero-length value intentionally passes no data pointer.
            let status = unsafe {
                RegQueryValueExW(
                    key.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    Some(&mut value_type),
                    if bytes.is_empty() {
                        None
                    } else {
                        Some(bytes.as_mut_ptr())
                    },
                    Some(&mut actual_length),
                )
            };
            if is_missing(status) {
                return Ok(RegistryValue::Missing);
            }
            if status == ERROR_MORE_DATA {
                length = actual_length;
                continue;
            }
            if status != ERROR_SUCCESS {
                return Err(win32_error("RegQueryValueExW(data)", status));
            }
            if actual_length > length {
                return Err("RegQueryValueExW 回報的資料長度超過提供的 buffer".to_owned());
            }
            bytes.truncate(actual_length as usize);
            return Ok(decode_value(value_type, &bytes));
        }

        Err("登錄值在讀取時持續改變，三次仍無法取得一致內容".to_owned())
    }

    fn decode_value(value_type: REG_VALUE_TYPE, bytes: &[u8]) -> RegistryValue {
        if value_type != REG_SZ {
            return RegistryValue::Other {
                reason: format!("registry type {}，預期 REG_SZ", value_type.0),
            };
        }
        if bytes.len() < 2 || bytes.len() % 2 != 0 {
            return RegistryValue::Other {
                reason: "REG_SZ 長度不是含結尾 NUL 的 UTF-16".to_owned(),
            };
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
            return RegistryValue::Other {
                reason: "REG_SZ 沒有且只有一個結尾 NUL".to_owned(),
            };
        }
        match String::from_utf16(&units[..units.len() - 1]) {
            Ok(value) => RegistryValue::String(value),
            Err(_) => RegistryValue::Other {
                reason: "REG_SZ 含有無效 UTF-16".to_owned(),
            },
        }
    }

    fn open_key(subkey: &str, access: REG_SAM_FLAGS) -> Result<Option<OwnedKey>, String> {
        let subkey = wide(subkey);
        let mut key = HKEY(null_mut());
        // SAFETY: the subkey is NUL-terminated and `key` points to writable local storage.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                access,
                &mut key,
            )
        };
        if is_missing(status) {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(win32_error("RegOpenKeyExW", status));
        }
        Ok(Some(OwnedKey(key)))
    }

    fn create_key(subkey: &str, access: REG_SAM_FLAGS) -> Result<OwnedKey, String> {
        let subkey = wide(subkey);
        let mut key = HKEY(null_mut());
        // SAFETY: both strings are valid NUL-terminated pointers for the call, and `key` points
        // to writable local storage. No security attributes are supplied.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(win32_error("RegCreateKeyExW", status));
        }
        Ok(OwnedKey(key))
    }

    fn write_string(target: RegistryTarget<'_>, value: &str) -> Result<(), String> {
        let units: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let mut bytes = Vec::with_capacity(units.len() * 2);
        for unit in units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        write_raw(target, REG_SZ, &bytes)
    }

    fn write_raw(
        target: RegistryTarget<'_>,
        value_type: REG_VALUE_TYPE,
        bytes: &[u8],
    ) -> Result<(), String> {
        let key = create_key(target.subkey, KEY_SET_VALUE | KEY_QUERY_VALUE)?;
        let name = wide(target.value_name);
        // SAFETY: the key is writable, the name is NUL-terminated, and `bytes` stays alive for
        // the complete call.
        let status =
            unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, value_type, Some(bytes)) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(win32_error("RegSetValueExW", status))
        }
    }

    fn delete_value(target: RegistryTarget<'_>) -> Result<(), String> {
        let Some(key) = open_key(target.subkey, KEY_SET_VALUE)? else {
            return Ok(());
        };
        let name = wide(target.value_name);
        // SAFETY: the key is writable and the value name is NUL-terminated.
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
        if status == ERROR_SUCCESS || is_missing(status) {
            Ok(())
        } else {
            Err(win32_error("RegDeleteValueW", status))
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn is_missing(status: WIN32_ERROR) -> bool {
        status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND
    }

    fn win32_error(operation: &str, status: WIN32_ERROR) -> String {
        format!("{operation} failed with Win32 error {}", status.0)
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        use windows::Win32::System::Registry::{REG_DWORD, RegDeleteKeyW};

        use super::*;

        static NEXT_TEST_KEY: AtomicU64 = AtomicU64::new(0);

        struct TestTarget {
            subkey: String,
        }

        impl TestTarget {
            fn new() -> Self {
                let suffix = NEXT_TEST_KEY.fetch_add(1, Ordering::Relaxed);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Windows clock is after Unix epoch")
                    .as_nanos();
                Self {
                    subkey: format!(
                        r"Software\ted-h\AI-Sister\Tests\login-startup-{}-{now}-{suffix}",
                        std::process::id()
                    ),
                }
            }

            fn target<'a>(&'a self, value_name: &'a str) -> RegistryTarget<'a> {
                RegistryTarget {
                    subkey: &self.subkey,
                    value_name,
                }
            }
        }

        impl Drop for TestTarget {
            fn drop(&mut self) {
                let _ = delete_value(self.target(RUN_VALUE_NAME));
                let _ = delete_value(self.target(INSTALL_LOCATION_VALUE));
                let subkey = wide(&self.subkey);
                // SAFETY: this deletes only the unique, empty HKCU leaf created by this test.
                unsafe {
                    let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()));
                }
            }
        }

        #[test]
        fn hkcu_test_subkey_round_trip_covers_missing_string_mismatch_and_other_type() {
            let fixture = TestTarget::new();
            let target = fixture.target(RUN_VALUE_NAME);
            let context = InstalledContext {
                expected_command: exact_login_command(r"C:\AI-Sister\sister-desktop.exe").unwrap(),
            };
            let expected = context.expected_command.as_str();

            assert_eq!(query_value(target).unwrap(), RegistryValue::Missing);
            let readback = read_registered_at(&context, target);
            assert_eq!(readback.state, LoginStartupState::Disabled);
            assert!(desired_readback(false, readback.clone()).is_ok());
            assert!(desired_readback(true, readback).is_err());

            write_string(target, expected).unwrap();
            assert_eq!(
                query_value(target).unwrap(),
                RegistryValue::String(expected.to_owned())
            );
            let readback = read_registered_at(&context, target);
            assert_eq!(readback.state, LoginStartupState::Enabled);
            assert!(desired_readback(true, readback.clone()).is_ok());
            assert!(desired_readback(false, readback).is_err());

            write_string(target, r#""C:\old\sister-desktop.exe" --ai-sister-login"#).unwrap();
            assert!(matches!(
                query_value(target).unwrap(),
                RegistryValue::String(actual) if actual != expected
            ));
            let readback = read_registered_at(&context, target);
            assert_eq!(readback.state, LoginStartupState::Mismatch);
            assert!(desired_readback(true, readback.clone()).is_err());
            assert!(desired_readback(false, readback).is_err());

            write_raw(target, REG_DWORD, &1u32.to_le_bytes()).unwrap();
            assert!(matches!(
                query_value(target).unwrap(),
                RegistryValue::Other { reason } if reason.contains("REG_SZ")
            ));
            assert_eq!(
                read_registered_at(&context, target).state,
                LoginStartupState::Mismatch
            );

            delete_value(target).unwrap();
            assert_eq!(query_value(target).unwrap(), RegistryValue::Missing);
        }

        #[test]
        fn only_exact_install_location_can_mutate_the_test_run_value() {
            let fixture = TestTarget::new();
            let install_target = fixture.target(INSTALL_LOCATION_VALUE);
            let run_target = fixture.target(RUN_VALUE_NAME);
            let executable = Path::new(r"C:\AI-Sister\sister-desktop.exe");

            let error = set_at(true, executable, install_target, run_target).unwrap_err();
            assert!(error.contains("安裝資訊"));
            assert_eq!(query_value(run_target).unwrap(), RegistryValue::Missing);

            write_string(install_target, r"C:\AI-Sister").unwrap();
            let error = set_at(true, executable, install_target, run_target).unwrap_err();
            assert!(error.contains("不精確對應"));
            assert_eq!(query_value(run_target).unwrap(), RegistryValue::Missing);

            write_raw(install_target, REG_DWORD, &1u32.to_le_bytes()).unwrap();
            let error = set_at(true, executable, install_target, run_target).unwrap_err();
            assert!(error.contains("不是可驗證的字串"));
            assert_eq!(query_value(run_target).unwrap(), RegistryValue::Missing);

            write_string(install_target, r#""C:\AI-Sister""#).unwrap();
            let enabled = set_at(true, executable, install_target, run_target).unwrap();
            assert_eq!(enabled.state, LoginStartupState::Enabled);
            assert_eq!(
                query_value(run_target).unwrap(),
                RegistryValue::String(
                    r#""C:\AI-Sister\sister-desktop.exe" --ai-sister-login"#.to_owned()
                )
            );

            let disabled = set_at(false, executable, install_target, run_target).unwrap();
            assert_eq!(disabled.state, LoginStartupState::Disabled);
            assert_eq!(query_value(run_target).unwrap(), RegistryValue::Missing);
        }

        #[test]
        fn a_set_resolves_only_for_the_one_state_it_requested() {
            let states = [
                LoginStartupState::Enabled,
                LoginStartupState::Disabled,
                LoginStartupState::Mismatch,
                LoginStartupState::Unreadable,
                LoginStartupState::Unsupported,
            ];
            for enabled in [false, true] {
                let wanted = if enabled {
                    LoginStartupState::Enabled
                } else {
                    LoginStartupState::Disabled
                };
                for state in states {
                    let readback = LoginStartupView {
                        state,
                        expected: Some("expected".to_owned()),
                        actual: None,
                        reason: Some("fixture".to_owned()),
                    };
                    assert_eq!(
                        desired_readback(enabled, readback).is_ok(),
                        state == wanted,
                        "wanted {}, read back {}",
                        state_name(wanted),
                        state_name(state)
                    );
                }
            }
        }
    }
}
