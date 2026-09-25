//! A158 驗收（Claude 在看到 delegate 的 diff 之前寫的）：閘門層。
//! 她自己的三個身分在任何設定下都回 `OwnWindow`；近似名字不是她；舊規則不變。

use sister_core::config::{Config, Exclusion, OWN_APP_KEYS, OWN_WINDOW_REASON, PrivacyConfig};
use sister_core::model::{BrowserUrlState, FocusSnapshot, PrivacyContext, SensitiveFieldState};

fn ctx(
    app_id: Option<&str>,
    app_name: Option<&str>,
    sensitive: SensitiveFieldState,
) -> PrivacyContext {
    PrivacyContext::known(
        FocusSnapshot {
            app_id: app_id.map(str::to_owned),
            app_name: app_name.map(str::to_owned),
            window_title: Some("AI-Sister".into()),
            ..Default::default()
        },
        sensitive,
        BrowserUrlState::NotApplicable,
    )
}

fn no_rules() -> PrivacyConfig {
    let mut p = Config::default().privacy;
    p.excluded_apps.clear();
    p.excluded_urls.clear();
    p.excluded_titles.clear();
    p.pause_on_screenshare = false;
    p
}

#[test]
fn every_identity_is_her_own_window_under_default_and_empty_rules() {
    assert_eq!(OWN_APP_KEYS.len(), 3, "前提：三個平台各一個身分");
    for key in OWN_APP_KEYS {
        for (label, privacy) in [
            ("預設", Config::default().privacy),
            ("規則全清空", no_rules()),
        ] {
            assert_eq!(
                privacy.check(&ctx(Some(key), None, SensitiveFieldState::Clear)),
                Exclusion::OwnWindow,
                "{label}：{key} 應該是她自己的視窗"
            );
        }
    }
}

#[test]
fn the_identity_is_matched_on_app_key_whatever_the_case() {
    let p = no_rules();
    assert_eq!(
        p.check(&ctx(
            Some("Sister-Desktop.EXE"),
            None,
            SensitiveFieldState::Clear
        )),
        Exclusion::OwnWindow
    );
    // app_id 讀不到時 app_key 退回 app_name。
    assert_eq!(
        p.check(&ctx(
            None,
            Some("Sister-Desktop"),
            SensitiveFieldState::Clear
        )),
        Exclusion::OwnWindow
    );
    assert_eq!(
        p.check(&ctx(
            Some("COM.TED-H.AI-SISTER"),
            None,
            SensitiveFieldState::Clear
        )),
        Exclusion::OwnWindow
    );
}

#[test]
fn near_names_are_not_her() {
    let p = no_rules();
    let near = [
        "sister.exe",
        "my-sister-desktop.exe",
        "sister-desktop-helper.exe",
        "sister-desktop.exe.bak",
        "ai-sister",
        "ai-sister.exe",
        "com.ted-h.ai-sister.helper",
        "com.ted-h",
    ];
    for name in near {
        assert_eq!(
            p.check(&ctx(Some(name), None, SensitiveFieldState::Clear)),
            Exclusion::Allowed,
            "{name} 不是她，規則全清空時應該照錄"
        );
    }
}

#[test]
fn her_window_wins_over_the_sensitive_field_states_but_not_over_unknown_context() {
    let p = Config::default().privacy;
    for state in [SensitiveFieldState::Focused, SensitiveFieldState::Unknown] {
        assert_eq!(
            p.check(&ctx(Some("sister-desktop.exe"), None, state)),
            Exclusion::OwnWindow,
            "{state:?}"
        );
    }
    assert_eq!(
        p.check(&PrivacyContext::Unknown),
        Exclusion::Blocked("privacy context unavailable".to_string())
    );
}

#[test]
fn own_window_is_blocked_for_every_caller_that_only_reads_reason() {
    assert!(Exclusion::OwnWindow.is_blocked());
    assert_eq!(Exclusion::OwnWindow.reason(), Some(OWN_WINDOW_REASON));
    assert!(!OWN_WINDOW_REASON.is_empty());
}

#[test]
fn clipboard_copied_from_her_window_is_hers_and_the_old_clipboard_rules_hold() {
    let p = no_rules();
    for src in [
        "sister-desktop.exe",
        "  SISTER-DESKTOP  ",
        "com.ted-h.ai-sister",
    ] {
        assert_eq!(
            p.check_clipboard_source(Some(src)),
            Exclusion::OwnWindow,
            "{src:?}"
        );
    }
    assert_eq!(
        p.check_clipboard_source(Some("my-sister-desktop.exe")),
        Exclusion::Allowed
    );
    assert_eq!(
        p.check_clipboard_source(None),
        Exclusion::Blocked("clipboard source app unknown".to_string())
    );
    let d = Config::default().privacy;
    assert!(
        matches!(d.check_clipboard_source(Some("keepassxc.exe")), Exclusion::Blocked(ref r) if r.contains("keepassxc")),
        "舊規則：密碼管理員來源照舊是 Blocked"
    );
}

#[test]
fn the_old_app_rule_still_says_its_own_reason() {
    let d = Config::default().privacy;
    assert_eq!(
        d.check(&ctx(
            Some("keepassxc.exe"),
            None,
            SensitiveFieldState::Clear
        )),
        Exclusion::Blocked("excluded app: keepassxc.exe".to_string())
    );
    assert_eq!(
        d.check(&ctx(Some("code.exe"), None, SensitiveFieldState::Clear)),
        Exclusion::Allowed
    );
}
