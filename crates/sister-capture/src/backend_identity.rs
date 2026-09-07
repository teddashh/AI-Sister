//! 錄製 session 的受限來源身分。
//!
//! 這不是 [`crate::Backend::name`] 那種診斷名稱。Core 會把一個精確的
//! Windows platform 值當成「這一列網址經過 UIA focused-address-field gate」
//! 的來源票，所以這個型別與所有建構子都只留在 crate 內。一般呼叫端只能走
//! [`crate::Recorder::new`]；那條路不論 backend 名字為何，一律寫進 `untrusted/`
//! namespace。

/// 可寫進 `sessions.platform` 的 crate-private 身分。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackendIdentity(Kind);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Untrusted {
        os: String,
        label: String,
    },
    #[cfg(windows)]
    TrustedWindowsFocusedUrlV2,
}

impl BackendIdentity {
    /// 一般、replay 與測試後端的身分。
    ///
    /// OS 與 label 分開收，兩節都做 canonical encoding。這樣就算第三方把
    /// backend 取名為完整的 trusted platform，也只能得到
    /// `untrusted/<os>/<escaped label>`。
    pub(crate) fn untrusted(os: &str, label: &str) -> Self {
        Self(Kind::Untrusted {
            os: os.to_owned(),
            label: label.to_owned(),
        })
    }

    /// 只有真正的 Windows GDI + UIA focused-URL backend 能拿到這個入口。
    ///
    /// 函式雖然是 crate-visible，token 的欄位只在 `windows` 模組可見；外部
    /// backend／wrapper 既拿不到 token，也看不到這個 identity 型別。
    #[cfg(windows)]
    pub(crate) const fn trusted_windows(_: crate::windows::WindowsBackendToken) -> Self {
        Self(Kind::TrustedWindowsFocusedUrlV2)
    }

    pub(crate) fn session_platform(&self) -> String {
        match &self.0 {
            Kind::Untrusted { os, label } => {
                format!(
                    "untrusted/{}/{}",
                    encode_component(os),
                    encode_component(label)
                )
            }
            #[cfg(windows)]
            Kind::TrustedWindowsFocusedUrlV2 => {
                sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM.to_owned()
            }
        }
    }
}

/// 讓 provenance 的每一節只有一種切法；控制字元、斜線與 `%` 都不原樣寫入。
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut out, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_names_cannot_spell_the_windows_trust_value() {
        for label in [
            "windows-gdi-uia-focused-url-v1",
            "windows-gdi-uia-focused-url-v2",
            sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM,
            "replay",
            "../windows/windows-gdi-uia-focused-url-v1",
            "x/y\\z%0A",
        ] {
            let platform = BackendIdentity::untrusted("windows", label).session_platform();
            assert!(platform.starts_with("untrusted/windows/"), "{platform}");
            assert_ne!(
                platform,
                sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM,
                "untrusted label acquired focused-URL trust: {label:?}"
            );
        }
    }

    #[test]
    fn provenance_components_have_one_unambiguous_shape() {
        assert_eq!(
            BackendIdentity::untrusted("linux/dev", "a/b\\c%\n").session_platform(),
            "untrusted/linux%2Fdev/a%2Fb%5Cc%25%0A"
        );
    }
}
