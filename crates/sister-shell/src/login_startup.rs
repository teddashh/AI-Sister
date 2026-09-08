//! Windows 登入啟動的純政策。
//!
//! 真正碰 HKCU 的那一半留在 desktop；這裡只回答不需要 Windows 才答得出的事：
//! 哪個 argv 是背景登入、要寫入的命令到底長什麼樣，以及讀回哪一種值才能算
//! 「這一版已登錄」。放在 root workspace 才能讓 Linux 的日常測試真的跑到。

use std::ffi::OsStr;

/// HKCU Run 的 value name。必須和 Tauri `productName` 相同，stock uninstaller 才會
/// 在真正移除（而不是 `/UPDATE`）時清掉同一筆。
pub const RUN_VALUE_NAME: &str = "AI-Sister";

/// Windows 登入項傳給 desktop 的唯一參數。
pub const LOGIN_ARGUMENT: &str = "--ai-sister-login";

/// Microsoft 對 Run value command line 公開的長度上限，以 UTF-16 code unit 計。
pub const RUN_COMMAND_MAX_UTF16: usize = 260;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchIntent {
    Interactive,
    Login,
}

/// 傳入的是 executable 之後的 argv；只認 exact、case-sensitive 的完整參數。
pub fn launch_intent<I, S>(arguments: I) -> LaunchIntent
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if arguments
        .into_iter()
        .any(|argument| argument.as_ref() == OsStr::new(LOGIN_ARGUMENT))
    {
        LaunchIntent::Login
    } else {
        LaunchIntent::Interactive
    }
}

/// 和 InstallLocation 分開的 newtype：兩者底層都是 String，接反仍應是編譯錯誤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginCommand(String);

impl LoginCommand {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallLocation(String);

impl InstallLocation {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    EmptyExecutable,
    QuoteInExecutable,
    TooLong { utf16: usize, maximum: usize },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyExecutable => f.write_str("執行檔路徑是空的"),
            Self::QuoteInExecutable => f.write_str("執行檔路徑含有雙引號"),
            Self::TooLong { utf16, maximum } => write!(
                f,
                "登入啟動命令有 {utf16} 個 UTF-16 code units，超過 Windows Run 上限 {maximum}"
            ),
        }
    }
}

impl std::error::Error for CommandError {}

/// 產生唯一獲准寫進 HKCU Run 的命令。
///
/// executable 一律加引號；「目前路徑沒有空白」不是可以省略引號的理由，因為搬到
/// 另一個 current-user install root 後，同一筆設定仍必須有相同語意。
pub fn exact_login_command(executable: &str) -> Result<LoginCommand, CommandError> {
    if executable.is_empty() {
        return Err(CommandError::EmptyExecutable);
    }
    if executable.contains('"') {
        return Err(CommandError::QuoteInExecutable);
    }
    let command = format!("\"{executable}\" {LOGIN_ARGUMENT}");
    let utf16 = command.encode_utf16().count();
    if utf16 > RUN_COMMAND_MAX_UTF16 {
        return Err(CommandError::TooLong {
            utf16,
            maximum: RUN_COMMAND_MAX_UTF16,
        });
    }
    Ok(LoginCommand(command))
}

/// Tauri current-user NSIS 寫入的 InstallLocation 本身帶雙引號。
pub fn exact_install_location(directory: &str) -> Result<InstallLocation, CommandError> {
    if directory.is_empty() {
        return Err(CommandError::EmptyExecutable);
    }
    if directory.contains('"') {
        return Err(CommandError::QuoteInExecutable);
    }
    Ok(InstallLocation(format!("\"{directory}\"")))
}

/// 只有 uninstall metadata 精確指向目前 exe 的 parent，才把這支程式當安裝版。
pub fn install_location_matches(expected: &InstallLocation, actual: &str) -> bool {
    actual == expected.as_str()
}

/// 已成功讀到的 Run value。讀取失敗與「不是安裝版」在 native 層各自保留，不能
/// 塞進 Missing；這裡只分類確實讀到的三種形狀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunValue<'a> {
    Missing,
    String(&'a str),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    Enabled,
    Disabled,
    Mismatch,
}

pub fn registration_state(expected: &LoginCommand, actual: RunValue<'_>) -> RegistrationState {
    match actual {
        RunValue::Missing => RegistrationState::Disabled,
        RunValue::String(actual) if actual == expected.as_str() => RegistrationState::Enabled,
        RunValue::String(_) | RunValue::Other => RegistrationState::Mismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_argument_is_exact_not_a_prefix_or_a_case_fold() {
        assert_eq!(launch_intent([] as [&str; 0]), LaunchIntent::Interactive);
        assert_eq!(launch_intent([LOGIN_ARGUMENT]), LaunchIntent::Login);
        assert_eq!(
            launch_intent(["--ai-sister-login-later"]),
            LaunchIntent::Interactive
        );
        assert_eq!(
            launch_intent(["--AI-SISTER-LOGIN"]),
            LaunchIntent::Interactive
        );
        assert_eq!(
            launch_intent(["--unrelated", LOGIN_ARGUMENT]),
            LaunchIntent::Login
        );
    }

    #[test]
    fn run_command_always_quotes_the_exact_executable_and_adds_one_argument() {
        let command =
            exact_login_command(r"C:\Users\王小明\App Data\Local\AI-Sister\sister-desktop.exe")
                .expect("command");
        assert_eq!(
            command.as_str(),
            r#""C:\Users\王小明\App Data\Local\AI-Sister\sister-desktop.exe" --ai-sister-login"#
        );
        assert!(exact_login_command("").is_err());
        assert!(exact_login_command(r#"C:\bad"name.exe"#).is_err());
    }

    #[test]
    fn run_command_checks_the_documented_utf16_limit_not_utf8_bytes() {
        let overhead = format!("\"\" {LOGIN_ARGUMENT}").encode_utf16().count();
        let fits = "界".repeat(RUN_COMMAND_MAX_UTF16 - overhead);
        let exact = exact_login_command(&fits).expect("exact boundary fits");
        assert_eq!(exact.as_str().encode_utf16().count(), RUN_COMMAND_MAX_UTF16);

        let too_long = format!("{fits}界");
        assert!(matches!(
            exact_login_command(&too_long),
            Err(CommandError::TooLong {
                utf16,
                maximum: RUN_COMMAND_MAX_UTF16
            }) if utf16 == RUN_COMMAND_MAX_UTF16 + 1
        ));
    }

    #[test]
    fn only_the_exact_quoted_installer_location_counts_as_installed() {
        let expected =
            exact_install_location(r"C:\Users\ted\AppData\Local\AI-Sister").expect("location");
        assert!(install_location_matches(
            &expected,
            r#""C:\Users\ted\AppData\Local\AI-Sister""#
        ));
        assert!(!install_location_matches(
            &expected,
            r"C:\Users\ted\AppData\Local\AI-Sister"
        ));
        assert!(!install_location_matches(
            &expected,
            r#""C:\elsewhere\AI-Sister""#
        ));
    }

    #[test]
    fn absent_exact_and_every_other_readable_value_stay_distinct() {
        let expected = exact_login_command(r"C:\AI-Sister\sister-desktop.exe").unwrap();
        assert_eq!(
            registration_state(&expected, RunValue::Missing),
            RegistrationState::Disabled
        );
        assert_eq!(
            registration_state(&expected, RunValue::String(expected.as_str())),
            RegistrationState::Enabled
        );
        assert_eq!(
            registration_state(
                &expected,
                RunValue::String(r#""C:\old\sister-desktop.exe" --ai-sister-login"#)
            ),
            RegistrationState::Mismatch
        );
        assert_eq!(
            registration_state(
                &expected,
                RunValue::String(r#""C:\AI-Sister\sister-desktop.exe""#)
            ),
            RegistrationState::Mismatch
        );
        assert_eq!(
            registration_state(&expected, RunValue::Other),
            RegistrationState::Mismatch
        );
    }
}
