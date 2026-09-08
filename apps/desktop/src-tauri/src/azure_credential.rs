//! Azure Speech key 的唯一保存邊界。
//!
//! 設定檔、WebView 回條與 log 都只看得到 [`CredentialState`]，永遠拿不到 key。
//! 真正朗讀的 native command 才能短暫讀出 blob；離開 scope 時會把那份記憶清零。

const TARGET: &str = "ted-h/AI-Sister/AzureSpeech/v1";
const USERNAME: &str = "Azure Speech subscription key";
// 和 sister-tts 的 request validator 同一個上限；Credential Manager 不能收下一串
// transport 之後一定拒絕的 key，否則 present 和 ready 會變成假話。
const MAX_KEY_BYTES: usize = sister_tts::MAX_SUBSCRIPTION_KEY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialState {
    Present,
    Missing,
    Unreadable,
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Azure Speech key 是空的")]
    Empty,
    #[error("Azure Speech key 超過 {MAX_KEY_BYTES} bytes")]
    TooLong,
    #[error("Azure Speech key 含空白、控制字元或非 ASCII 字元")]
    InvalidCharacters,
    #[cfg_attr(windows, allow(dead_code))]
    #[error("這個平台沒有 Windows Credential Manager，沒有保存 Azure Speech key")]
    Unsupported,
    #[cfg(windows)]
    #[error("Windows Credential Manager 操作失敗（{0:#010x}）")]
    Windows(i32),
    #[error("Windows Credential Manager 裡的 Azure Speech key 不是可用的 UTF-8")]
    InvalidStoredValue,
}

pub type Result<T> = std::result::Result<T, Error>;

/// 只接受一行可見 ASCII。Azure key 不應含空白；拒絕而不是悄悄 trim，畫面才不會
/// 說「保存成功」但實際保存的是另一串 bytes。
fn validate_key(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Err(Error::Empty);
    }
    if bytes.len() > MAX_KEY_BYTES {
        return Err(Error::TooLong);
    }
    if bytes
        .iter()
        .any(|byte| !byte.is_ascii_graphic() || byte.is_ascii_whitespace())
    {
        return Err(Error::InvalidCharacters);
    }
    Ok(())
}

/// 朗讀 command 唯一會拿到的 secret。`Debug` 故意只印型別，不印長度或內容。
pub struct SecretKey(Vec<u8>);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretKey([REDACTED])")
    }
}

impl SecretKey {
    pub fn as_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.0).map_err(|_| Error::InvalidStoredValue)
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[cfg(windows)]
mod platform {
    use super::{Error, Result, SecretKey, TARGET, USERNAME, validate_key};
    use std::ptr;
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use windows::core::{HRESULT, PCWSTR, PWSTR};

    struct ReadGuard(*mut CREDENTIALW);

    impl Drop for ReadGuard {
        fn drop(&mut self) {
            // SAFETY: CredReadW 成功時回傳的 buffer 必須恰好由 CredFree 釋放。
            unsafe { CredFree(self.0.cast()) };
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn map_windows(error: windows::core::Error) -> Error {
        Error::Windows(error.code().0)
    }

    fn is_missing(error: &windows::core::Error) -> bool {
        error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0)
    }

    pub fn read() -> Result<Option<SecretKey>> {
        let target = wide(TARGET);
        let mut raw = ptr::null_mut();
        // SAFETY: target 是 NUL 結尾；raw 由 API 寫入，成功後交給 ReadGuard。
        if let Err(error) =
            unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut raw) }
        {
            return if is_missing(&error) {
                Ok(None)
            } else {
                Err(map_windows(error))
            };
        }
        if raw.is_null() {
            return Err(Error::InvalidStoredValue);
        }
        let _guard = ReadGuard(raw);
        // SAFETY: raw 由 CredReadW 成功填入，guard 仍持有 buffer。
        let credential = unsafe { &*raw };
        let length = credential.CredentialBlobSize as usize;
        if length == 0 || credential.CredentialBlob.is_null() {
            return Err(Error::InvalidStoredValue);
        }
        // SAFETY: Windows 保證 CredentialBlob 指向 CredentialBlobSize bytes。
        let bytes = unsafe { std::slice::from_raw_parts(credential.CredentialBlob, length) };
        validate_key(bytes)?;
        Ok(Some(SecretKey(bytes.to_vec())))
    }

    pub fn write(mut key: String) -> Result<()> {
        let mut blob = std::mem::take(&mut key).into_bytes();
        let result = (|| {
            validate_key(&blob)?;
            let mut target = wide(TARGET);
            let mut username = wide(USERNAME);
            let credential = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: PWSTR(username.as_mut_ptr()),
                ..Default::default()
            };
            // SAFETY: credential 指到的 target/username/blob 在呼叫期間都活著；API
            // 會複製內容，不保留 caller pointer。
            unsafe { CredWriteW(&credential, 0) }.map_err(map_windows)
        })();
        blob.fill(0);
        result
    }

    pub fn delete() -> Result<()> {
        let target = wide(TARGET);
        // SAFETY: target 是 NUL 結尾且只在呼叫期間借用。
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if is_missing(&error) => Ok(()),
            Err(error) => Err(map_windows(error)),
        }
    }
}

#[cfg(windows)]
pub use platform::{delete, read, write};

#[cfg(not(windows))]
pub fn read() -> Result<Option<SecretKey>> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn write(mut key: String) -> Result<()> {
    // 不支援也把 IPC 交來的那份配置清掉；不能因 early return 把 secret 留到 String drop。
    // SAFETY: 只原位覆寫 bytes，不改 length/capacity，且覆寫後不再以 UTF-8 讀取。
    unsafe { key.as_mut_vec() }.fill(0);
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn delete() -> Result<()> {
    Err(Error::Unsupported)
}

pub fn state() -> CredentialState {
    match read() {
        Ok(Some(_)) => CredentialState::Present,
        Ok(None) => CredentialState::Missing,
        Err(Error::Unsupported) => CredentialState::Unsupported,
        Err(_) => CredentialState::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation_never_silently_changes_bytes() {
        assert!(validate_key(b"0123456789abcdef0123456789abcdef").is_ok());
        assert!(matches!(validate_key(b""), Err(Error::Empty)));
        assert!(matches!(
            validate_key(b" key"),
            Err(Error::InvalidCharacters)
        ));
        assert!(matches!(
            validate_key(b"key\n"),
            Err(Error::InvalidCharacters)
        ));
        assert!(matches!(
            validate_key("金鑰".as_bytes()),
            Err(Error::InvalidCharacters)
        ));
        assert!(matches!(
            validate_key(&vec![b'a'; MAX_KEY_BYTES + 1]),
            Err(Error::TooLong)
        ));
    }

    #[test]
    fn secret_debug_never_contains_the_key() {
        let secret = SecretKey(b"do-not-print-me".to_vec());
        let debug = format!("{secret:?}");
        assert_eq!(debug, "SecretKey([REDACTED])");
        assert!(!debug.contains("do-not-print-me"));
    }
}
