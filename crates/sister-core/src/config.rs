//! 設定與排除規則。
//!
//! 排除規則是隱私架構的第一道實體防線（SPEC §11.2）：它在 capture **當下**
//! 生效，被排除的東西從來沒有存在過，而不是「先存再刪」。
//!
//! 設定放在使用者看得到、改得動的 TOML；預設值就是安全的預設值。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::FocusSnapshot;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub capture: CaptureConfig,
    pub privacy: PrivacyConfig,
    pub retention: RetentionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureConfig {
    /// 總開關。false = 她閉著眼睛（tray 的「看別的地方」）。
    pub enabled: bool,
    /// idle 時的補拍上限（秒）。事件驅動為主，這個只是保底心跳。
    pub heartbeat_secs: u64,
    /// 事件觸發後的最小擷取間隔（毫秒），避免快速切窗時暴衝。
    pub min_interval_ms: u64,
    /// dHash 判定「同一畫面」的 Hamming 門檻。
    pub dedup_threshold: u32,
    /// 降採樣後的長邊上限（像素）。
    pub max_long_edge: u32,
    /// 是否保留畫面檔案。false = text-only 模式（第三張同意書關閉）。
    pub store_images: bool,
    /// 是否對保留幀跑 OCR。
    pub ocr: bool,
    /// 輸入動態的聚合視窗（秒）。
    pub input_window_secs: u64,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            heartbeat_secs: 8,
            min_interval_ms: 400,
            dedup_threshold: crate::dedup::DEFAULT_THRESHOLD,
            max_long_edge: 1568,
            store_images: true,
            ocr: true,
            input_window_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivacyConfig {
    /// app 識別字（小寫比對）。命中則整段不擷取。
    ///
    /// 裸字串 = 子字串比對，要精準比對再自己寫 `*`。理由見
    /// [`app_pattern_matches`]。
    pub excluded_apps: Vec<String>,
    /// URL glob。命中則不擷取。
    pub excluded_urls: Vec<String>,
    /// 視窗標題 glob（小寫比對）。命中則不擷取。
    pub excluded_titles: Vec<String>,
    /// 前景為螢幕分享/會議 app 時自動暫停（旁人畫面防線）。
    pub pause_on_screenshare: bool,
    /// 剪貼簿疑似秘密時不落地內容。
    pub redact_clipboard_secrets: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            // 預設就把最敏感的擋掉；使用者可再加。
            // 寫成裸名字即可：比對是子字串，`keepassxc` 同時涵蓋
            // Windows 的 keepassxc.exe 與 macOS 的 org.keepassxc.keepassxc。
            excluded_apps: [
                "keepassxc",
                "keepass",
                "1password",
                "bitwarden",
                "dashlane",
                "lastpass",
                "enpass",
                "gnome-keyring",
                "seahorse",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            // 一律用「出現在網址任何位置」的子字串規則，不要寫成
            // `*://*.bank.com/*` 這種按結構對齊的樣式：只要少一個 www、
            // 或關鍵字落在網域而不是路徑，那種寫法就會整條失效——
            // 而使用者永遠不會知道自己的網銀畫面其實一直被錄著。
            excluded_urls: [
                "*onlinebanking*",
                "*netbank*",
                "*ebank*",
                "*/ib/*",
                "*cathaybk.com*",
                "*esunbank.com*",
                "*ctbcbank.com*",
                "*bot.com.tw*",
                "*taishinbank.com*",
                "*firstbank.com.tw*",
                "*megabank.com.tw*",
                "*accounts.google.com*",
                "*login.microsoftonline.com*",
                "*password*",
                "*/signin*",
                "*/login*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            excluded_titles: ["*password*", "*密碼*", "*private browsing*", "*無痕*"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            pause_on_screenshare: true,
            redact_clipboard_secrets: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionConfig {
    /// 全解析度畫面保留天數。
    pub frames_days: u32,
    /// 縮圖保留天數。
    pub thumbs_days: u32,
    /// OCR 文字與 L1 事實保留天數。
    pub text_days: u32,
    /// 單日磁碟上限（GB），超過觸發自動降級。
    pub max_disk_gb_per_day: f64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            frames_days: 30,
            thumbs_days: 90,
            text_days: 365,
            max_disk_gb_per_day: 2.0,
        }
    }
}

/// 螢幕分享/會議 app 的識別字（旁人畫面防線）。
///
/// 只收「開著就等於畫面在被分享或正在看別人畫面」的 app。
/// Slack 與 Discord 刻意不在此列：它們也能分享畫面，但那是偶爾的模式，
/// 而它們平時是主要的工作與對話場所。光憑 app 名稱分不出模式，
/// 把它們列進來等於讓整個工作日最重要的對話永遠不被記得——
/// 那個代價比防到的風險大得多。
const SCREENSHARE_APPS: &[&str] = &[
    "zoom",
    "teams",
    "ms-teams",
    "webex",
    "gotomeeting",
    "bluejeans",
    "obs",
    "obs64",
    "streamlabs",
    "google meet",
    "skype",
    "anydesk",
    "teamviewer",
];

/// 排除判定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusion {
    /// 可以擷取。
    Allowed,
    /// 不可擷取，附上人類看得懂的理由（會寫進 system_events 供稽核）。
    Blocked(String),
}

impl Exclusion {
    pub fn is_blocked(&self) -> bool {
        matches!(self, Exclusion::Blocked(_))
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Exclusion::Blocked(r) => Some(r),
            Exclusion::Allowed => None,
        }
    }
}

impl PrivacyConfig {
    /// 依前景脈絡判斷這一刻能不能擷取。
    ///
    /// 這個函式是 capture 迴圈裡最先被呼叫的東西——它回 `Blocked` 時，
    /// 連截圖都不會發生。
    pub fn check(&self, focus: &FocusSnapshot) -> Exclusion {
        let app = focus.app_key();

        if self.pause_on_screenshare && !app.is_empty() {
            for s in SCREENSHARE_APPS {
                if app.contains(s) {
                    return Exclusion::Blocked(format!("screenshare app: {app}"));
                }
            }
        }

        if !app.is_empty() {
            for pat in &self.excluded_apps {
                if app_pattern_matches(&pat.to_ascii_lowercase(), &app) {
                    return Exclusion::Blocked(format!("excluded app: {app}"));
                }
            }
        }

        if let Some(url) = focus.url.as_deref() {
            let url_lc = url.to_ascii_lowercase();
            for pat in &self.excluded_urls {
                if glob_match(&pat.to_ascii_lowercase(), &url_lc) {
                    return Exclusion::Blocked("excluded url".to_string());
                }
            }
        }

        if let Some(title) = focus.window_title.as_deref() {
            let title_lc = title.to_ascii_lowercase();
            for pat in &self.excluded_titles {
                if glob_match(&pat.to_ascii_lowercase(), &title_lc) {
                    return Exclusion::Blocked("excluded window title".to_string());
                }
            }
        }

        Exclusion::Allowed
    }
}

/// app 排除規則的比對。**不含 `*` 的樣式一律視為子字串。**
///
/// 因為使用者寫下 `keepassxc` 時，她的意思是「這個程式不要錄」，
/// 而不是「app 識別碼要恰好等於這九個字」。平台給的識別碼長什麼樣
/// 她不知道也不該知道：Windows 是 `keepassxc.exe`、macOS 是
/// `org.keepassxc.keepassxc`、Linux 是 `keepassxc`。用全字比對的話，
/// 同一條規則只在三個平台的其中一個生效——而且是靜默地不生效。
///
/// 副作用是可能多擋（`code` 會連 `vscode.exe` 一起擋掉）。這個方向的錯
/// 是「你以為會記的沒記到」，使用者看得見也改得掉；反過來的錯是
/// 「你以為擋住的其實一直在錄」，她永遠不會發現。兩者不對等。
pub fn app_pattern_matches(pattern: &str, app: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    if pattern.contains('*') {
        glob_match(pattern, app)
    } else {
        app.contains(pattern)
    }
}

/// 極簡 glob：只支援 `*`（比對任意長度，含空字串）。
///
/// 刻意不引入 glob crate——規則是使用者手寫的，語法越小越不會寫錯。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();

    // 標準的雙指標回溯法，O(n·m) 最壞但輸入都很短
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut backtrack) = (usize::MAX, 0usize);

    while t < txt.len() {
        if p < pat.len() && (pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = p;
            backtrack = t;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            backtrack += 1;
            t = backtrack;
        } else {
            return false;
        }
    }

    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

impl Config {
    /// 預設設定檔路徑。
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("com", "ted-h", "AI-Sister")
            .map(|d| d.config_dir().join("config.toml"))
    }

    /// 預設資料目錄（DB 與畫面檔）。
    pub fn default_data_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("com", "ted-h", "AI-Sister")
            .map(|d| d.data_dir().to_path_buf())
    }

    /// 讀取設定；檔案不存在則回傳預設值（不自動寫檔）。
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> FocusSnapshot {
        FocusSnapshot {
            app_id: Some(id.into()),
            ..Default::default()
        }
    }

    /// 這條規則的預設值是裸名字，但三個平台給的識別碼長得都不一樣。
    /// 曾經只有剛好寫成 `1password.exe` 的那一條在 Windows 上生效，
    /// 其餘密碼管理員全部靜默漏接。
    #[test]
    fn password_managers_are_blocked_in_every_platform_naming_style() {
        let c = Config::default();
        for id in [
            "keepassxc",                    // Linux
            "keepassxc.exe",                // Windows
            "KeePassXC.exe",                // Windows，大小寫不一
            "org.keepassxc.keepassxc",      // macOS bundle id
            "1Password.exe",
            "com.1password.1password",
            "bitwarden.exe",
            "Bitwarden.exe",
            "dashlane.exe",
            "lastpass.exe",
            "enpass.exe",
        ] {
            assert!(
                c.privacy.check(&app(id)).is_blocked(),
                "{id} 沒有被擋下來——使用者會以為它被擋住了"
            );
        }
    }

    #[test]
    fn ordinary_apps_still_get_recorded() {
        let c = Config::default();
        for id in ["chrome.exe", "code.exe", "explorer.exe", "firefox", "Terminal"] {
            assert!(!c.privacy.check(&app(id)).is_blocked(), "{id} 不該被擋");
        }
    }

    #[test]
    fn star_patterns_still_mean_glob_not_substring() {
        // 有寫 `*` 的人是刻意的，要照 glob 語意走
        assert!(app_pattern_matches("*pass*", "keepassxc.exe"));
        assert!(!app_pattern_matches("keepass", "kee-pass.exe"));
        assert!(!app_pattern_matches("*.exe", "keepassxc"));
        assert!(app_pattern_matches("*.exe", "keepassxc.exe"));
        // 空樣式不該變成「擋掉全部」
        assert!(!app_pattern_matches("", "chrome.exe"));
    }

    fn focus(app: &str, title: &str, url: Option<&str>) -> FocusSnapshot {
        FocusSnapshot {
            app_id: Some(app.into()),
            app_name: Some(app.into()),
            window_title: Some(title.into()),
            url: url.map(|u| u.into()),
            pid: Some(1),
        }
    }

    #[test]
    fn glob_basics() {
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("abc", "abc"));
        assert!(!glob_match("abc", "abd"));
        assert!(glob_match("*password*", "my password manager"));
        assert!(glob_match("keepass*", "keepassxc"));
        assert!(glob_match("*.exe", "chrome.exe"));
        assert!(!glob_match("*.exe", "chrome.dll"));
        assert!(glob_match(
            "*://*.cathaybk.com.tw/*",
            "https://www.cathaybk.com.tw/net/login"
        ));
        assert!(!glob_match(
            "*://*.cathaybk.com.tw/*",
            "https://example.com/"
        ));
        // 多個星號不能爆炸
        assert!(glob_match("*a*b*c*", "xxaxxbxxcxx"));
        assert!(!glob_match("*a*b*c*", "xxaxxcxxbxx"));
    }

    #[test]
    fn password_manager_is_blocked_by_default() {
        let p = PrivacyConfig::default();
        let v = p.check(&focus("KeePassXC", "Database", None));
        assert!(
            v.is_blocked(),
            "password manager must be excluded at capture time"
        );
        assert!(v.reason().unwrap().contains("excluded app"));
    }

    #[test]
    fn screenshare_app_pauses_capture() {
        let p = PrivacyConfig::default();
        let v = p.check(&focus("Zoom", "Meeting", None));
        assert!(v.is_blocked(), "bystanders' screens must not be recorded");
        assert!(v.reason().unwrap().contains("screenshare"));
    }

    #[test]
    fn banking_url_is_blocked() {
        let p = PrivacyConfig::default();
        let v = p.check(&focus(
            "chrome.exe",
            "Bank",
            Some("https://www.cathaybk.com.tw/net/transfer"),
        ));
        assert!(v.is_blocked());
        assert_eq!(v.reason(), Some("excluded url"));
    }

    #[test]
    fn sensitive_title_is_blocked_in_both_languages() {
        let p = PrivacyConfig::default();
        assert!(
            p.check(&focus("chrome.exe", "Change Password", None))
                .is_blocked()
        );
        assert!(p.check(&focus("chrome.exe", "變更密碼", None)).is_blocked());
        assert!(p.check(&focus("chrome.exe", "無痕視窗", None)).is_blocked());
    }

    #[test]
    fn ordinary_work_is_allowed() {
        let p = PrivacyConfig::default();
        assert_eq!(
            p.check(&focus("code.exe", "SPEC.md - AI-Sister", None)),
            Exclusion::Allowed
        );
        assert_eq!(
            p.check(&focus(
                "chrome.exe",
                "Cloudflare Dashboard",
                Some("https://dash.cloudflare.com/dns")
            )),
            Exclusion::Allowed
        );
        // 空的 focus 不該被誤擋
        assert_eq!(p.check(&FocusSnapshot::default()), Exclusion::Allowed);
    }

    #[test]
    fn screenshare_pause_can_be_disabled() {
        let p = PrivacyConfig {
            pause_on_screenshare: false,
            ..Default::default()
        };
        assert_eq!(p.check(&focus("Zoom", "Meeting", None)), Exclusion::Allowed);
    }

    #[test]
    fn config_roundtrips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.capture.dedup_threshold, cfg.capture.dedup_threshold);
        assert_eq!(
            back.privacy.excluded_apps.len(),
            cfg.privacy.excluded_apps.len()
        );
        assert_eq!(back.retention.text_days, 365);
    }

    #[test]
    fn partial_config_fills_defaults() {
        // 使用者只寫一兩行也要能讀，其餘用安全預設
        let cfg: Config = toml::from_str("[capture]\nstore_images = false\n").expect("parse");
        assert!(!cfg.capture.store_images);
        assert!(
            cfg.capture.enabled,
            "unspecified fields must fall back to defaults"
        );
        assert!(cfg.privacy.redact_clipboard_secrets);
        assert_eq!(cfg.retention.frames_days, 30);
    }

    #[test]
    fn banking_urls_are_blocked_whatever_shape_the_host_takes() {
        // 迴歸測試：原本的規則寫成 `*://*/*netbank*`，把關鍵字綁在路徑上，
        // 於是 https://netbank.example.com/transfer 一路被錄了下來。
        // 規則必須是「出現在網址任何位置」，不能依賴網址的結構。
        let p = PrivacyConfig::default();
        for url in [
            "https://netbank.example.com/transfer",
            "https://www.netbank.example.com/transfer",
            "https://example.com/netbank/transfer",
            "https://cathaybk.com.tw/transfer",
            "https://www.cathaybk.com.tw/net/transfer",
            "https://ebank.megabank.com.tw/",
            "https://accounts.google.com/signin/v2",
            "https://app.example.com/login?next=/",
        ] {
            assert!(
                p.check(&focus("chrome.exe", "Bank", Some(url)))
                    .is_blocked(),
                "must block {url}"
            );
        }
    }

    #[test]
    fn ordinary_urls_still_get_through() {
        // 排除規則太寬會讓她變成瞎子，一樣是 bug
        let p = PrivacyConfig::default();
        for url in [
            "https://github.com/teddashh/AI-Sister",
            "https://bill.cht.com.tw/query",
            "https://docs.rs/rusqlite/latest/rusqlite/",
            "https://news.ycombinator.com/item?id=1",
        ] {
            assert!(
                !p.check(&focus("chrome.exe", "ok", Some(url))).is_blocked(),
                "must allow {url}"
            );
        }
    }

    #[test]
    fn chat_apps_are_not_treated_as_screen_sharing() {
        // 迴歸測試：slack 曾在螢幕分享清單裡，於是整個工作日最重要的
        // 對話永遠不會被記得。分不出「正在分享」就不該整個 app 封殺。
        let p = PrivacyConfig::default();
        for app in ["slack.exe", "discord.exe", "Slack"] {
            assert!(
                !p.check(&focus(app, "#general", None)).is_blocked(),
                "{app} must be recorded"
            );
        }
    }

    #[test]
    fn real_meeting_apps_still_pause_capture() {
        let p = PrivacyConfig::default();
        for app in [
            "Zoom.exe",
            "ms-teams.exe",
            "webex",
            "obs64.exe",
            "TeamViewer",
        ] {
            let v = p.check(&focus(app, "Meeting", None));
            assert!(v.is_blocked(), "{app} must pause capture");
            assert!(v.reason().unwrap_or_default().contains("screenshare"));
        }
    }
}
