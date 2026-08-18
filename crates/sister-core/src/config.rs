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
    /// 每隔多久探一次螢幕（毫秒）。
    pub min_interval_ms: u64,
    /// dHash 判定「同一畫面」的 Hamming 門檻。
    pub dedup_threshold: u32,
    /// **存檔用**畫面的長邊上限（像素）。
    ///
    /// 只影響寫到磁碟上的 PNG。OCR 看的是原生解析度的另一份像素——
    /// 這兩件事曾經共用同一個數字，結果是「為了省磁碟而把字縮到讀不出來」，
    /// 兩邊同時輸。
    pub max_long_edge: u32,
    /// 兩張**畫面檔**之間的最小間隔（毫秒）。文字完全不受影響。
    ///
    /// 磁碟預算幾乎全部花在 PNG 上，而 PNG 是這裡面唯一可以少存、卻不會
    /// 少記住東西的：文字仍然逐幀進索引，搜尋結果一筆都不會少，只是其中
    /// 一些點回不出一張圖。SPEC §2.3 給截圖層的設計點是 0.1–0.2 有效 fps，
    /// 5000ms 正是那個區間寬鬆的那一端。
    ///
    /// 實測沒有這道閘門時是 0.35 fps、11.4 GB/天，而預算是 300MB/天。
    pub image_min_interval_ms: u64,
    /// 每天畫面檔的**硬上限**（MB）。用完就只留文字，隔天自動歸零。
    ///
    /// 為什麼光有間隔節流不夠：間隔只管得住**速率**，管不住總量。5 秒一張
    /// 的最壞情況仍然是一天 17,280 張，乘上一張 500KB 就是 8.8 GB——比
    /// 沒節流的 11.4 GB 好不了多少。而「最壞情況」不是假想，是一台開著
    /// 影片、儀表板或編譯輸出的機器的日常。
    ///
    /// 所以要有一個直接盯著**那個預算數字本身**的閘門。它不管螢幕多忙，
    /// 都保證磁碟不會失控，而且用完的時候會在摘要上講出來——
    /// SPEC §2.3 講的「自動降級上限」就是這個。
    ///
    /// 額度在啟動時**從資料庫接回來**，不是從 0 開始：不然關掉再開就重新
    /// 拿一份，一天重開十次就是十倍額度，而那個「上限」等於不存在。
    ///
    /// 設 0 = 不設上限（那就要自己盯著磁碟）。
    pub max_image_mb_per_day: u64,
    /// 是否保留畫面檔案。false = text-only 模式（第三張同意書關閉）。
    pub store_images: bool,
    /// 是否對保留幀跑 OCR。
    pub ocr: bool,
    /// OCR 語言的偏好順序（BCP-47），第一個裝得起來的就用。
    ///
    /// 這個順序直接決定她讀不讀得懂你的螢幕。Windows 的 OCR 是**逐語言**
    /// 安裝的；沒裝中文的話引擎會安靜地退回英文，然後把滿螢幕的中文讀成
    /// 空白。`sister doctor` 會把實際選中的語言講出來，就是為了這件事。
    pub ocr_languages: Vec<String>,
    /// 輸入動態的聚合視窗（秒）。
    pub input_window_secs: u64,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_interval_ms: 400,
            dedup_threshold: crate::dedup::DEFAULT_THRESHOLD,
            max_long_edge: 1568,
            image_min_interval_ms: 5_000,
            // Phase 0 的驗收是「磁碟 < 300MB/天」，那是**全部**加起來。
            // 文字與索引一天約幾 MB，剩下的留給畫面。
            max_image_mb_per_day: 250,
            store_images: true,
            ocr: true,
            // 繁體中文優先，英文墊底。中文的 OCR 引擎本來就讀得懂拉丁字母，
            // 反過來則完全不行——所以順序不能顛倒。
            ocr_languages: ["zh-Hant-TW", "zh-Hant", "en-US"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
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
/// 東西活多久。由 [`crate::retention`] 執行。
///
/// 這裡曾經還有 `thumbs_days` 和 `max_disk_gb_per_day` 兩個欄位。它們被
/// 拿掉了，因為**沒有縮圖、也沒有任何東西會在超過磁碟上限時降級**——
/// 一個什麼都不做的設定項比沒有這個設定項更糟：使用者會調它、會相信它，
/// 然後在需要它的那天發現它從來沒有存在過。要就實作，不然就刪掉。
///
/// `0` 代表**立刻刪除**，不是「永久保留」。理由見
/// [`crate::retention::cutoff`]。
pub struct RetentionConfig {
    /// 全解析度畫面保留天數。到期後刪掉 PNG，但**保留文字**。
    pub frames_days: u32,
    /// OCR 文字與 L1 事實保留天數。到期後整列消失。
    pub text_days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            frames_days: 30,
            text_days: 365,
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

        // 排在所有規則之前。規則是在猜「這個 app 大概會有秘密」，
        // 這一條是「她**現在正在**輸入秘密」——後者確定得多，而且不需要
        // 任何設定就成立。使用者沒設定過的東西也該保護得到。
        if focus.password_field {
            return Exclusion::Blocked("password field focused".to_string());
        }

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

/// 讀起來正確、但**在真實輸入上不會命中**的網址規則。
///
/// 這條 lint 的存在是因為我們拿得到的網址不是使用者以為的那一個。
/// Windows 上唯一讀得到網址的地方是瀏覽器的網址列，而 Chromium 交出來的是
/// **給人看的縮寫版**：`kFormatUrlOmitHTTPS | kFormatUrlOmitTrivialSubdomains`
/// 會把 scheme 和 `www.` 拿掉。使用者看到網址列寫著 `mybank.com/login`，
/// 她照抄進設定檔的 `https://mybank.com/login` 就永遠不會命中。
///
/// 這正是本專案已經踩過三次的那個形狀：規則讀起來完全正確，但什麼都沒比對到，
/// 而且**少擋了不會有任何症狀**。所以寧可在 `doctor` 上吵一句。
///
/// 回傳 `(規則, 為什麼不會命中)`。
pub fn suspicious_url_rules(rules: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for rule in rules {
        let lc = rule.to_ascii_lowercase();
        // 去掉開頭的 `*` 再看，`*https://...*` 一樣有問題
        let bare = lc.trim_start_matches('*');
        if bare.starts_with("http://") || bare.starts_with("https://") {
            out.push((
                rule.clone(),
                "含 scheme：瀏覽器網址列交出來的字串已經把 `https://` 拿掉了，\
                 這條規則不會命中。把 scheme 刪掉即可"
                    .to_string(),
            ));
        } else if bare.starts_with("www.") {
            out.push((
                rule.clone(),
                "以 `www.` 開頭：網址列會省略 `www.`，這條規則不會命中。\
                 改成 `*` 開頭的子字串即可"
                    .to_string(),
            ));
        } else if !rule.contains('*') && !rule.is_empty() {
            out.push((
                rule.clone(),
                "沒有 `*`：網址規則是全字比對，整條網址要一字不差才會命中。\
                 多半應該寫成 `*關鍵字*`"
                    .to_string(),
            ));
        }
    }
    out
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
mod url_rule_lint_tests {
    use super::*;

    /// 使用者從網址列抄下來的東西會長這樣，而她抄的時候會補上 scheme。
    #[test]
    fn a_rule_with_a_scheme_is_flagged() {
        let bad = vec!["https://mybank.com/login".to_string()];
        let found = suspicious_url_rules(&bad);
        assert_eq!(found.len(), 1, "含 scheme 的規則沒被抓出來");
        assert!(found[0].1.contains("scheme"));
    }

    #[test]
    fn a_wildcarded_scheme_is_still_flagged() {
        let found = suspicious_url_rules(&["*https://mybank.com*".to_string()]);
        assert_eq!(found.len(), 1, "前面加了 * 就漏掉了");
    }

    #[test]
    fn a_www_prefix_is_flagged() {
        let found = suspicious_url_rules(&["www.mybank.com".to_string()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].1.contains("www."));
    }

    /// 沒有 `*` 的網址規則是全字比對，幾乎不可能命中一整條真實網址。
    #[test]
    fn a_bare_substring_without_wildcards_is_flagged() {
        let found = suspicious_url_rules(&["mybank.com".to_string()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].1.contains('*'));
    }

    /// 反面對照：這條 lint 不可以對正確的規則亂叫，否則使用者會學會無視它。
    #[test]
    fn well_formed_rules_are_left_alone() {
        let good = vec![
            "*cathaybk.com*".to_string(),
            "*/login*".to_string(),
            "*accounts.google.com*".to_string(),
        ];
        assert!(
            suspicious_url_rules(&good).is_empty(),
            "對正確的規則誤報：{:?}",
            suspicious_url_rules(&good)
        );
    }

    /// **我們自己的預設值必須通過這條 lint。**
    ///
    /// 這一條比上面所有測試都重要：預設清單是絕大多數使用者唯一會用到的
    /// 保護，而它出錯的方式是安靜的。如果哪天有人往預設值裡加了一條
    /// `https://...`，這裡會立刻紅。
    #[test]
    fn our_own_defaults_would_actually_match_something() {
        let defaults = PrivacyConfig::default().excluded_urls;
        assert!(!defaults.is_empty(), "預設清單空了，這個測試就沒有意義");
        let bad = suspicious_url_rules(&defaults);
        assert!(
            bad.is_empty(),
            "預設的網址規則裡有寫了也不會生效的：{bad:#?}"
        );
    }

    /// THREAT_MODEL 說「每一條規則都必須有測試證明它在真實的識別碼形狀下
    /// 會命中」。在這條測試之前，那句話沒有任何東西在執行。
    ///
    /// 既有的測試各自舉了幾個網址當例子，加起來大概碰到 16 條裡的一半；
    /// `our_own_defaults_would_actually_match_something` 只擋得住**語法上**
    /// 就註定不會命中的規則（帶 scheme、帶 www.）。兩個都攔不住這一種：
    ///
    ///     "*cathybk.com*"      ← 少一個 a
    ///
    /// 它語法完全正確、`suspicious_url_rules` 一聲不吭、沒有任何既有測試
    /// 舉的例子會碰到它。使用者看到的是「✓ 17 條規則」。
    /// **規則的數量從來不是問題，規則會不會命中才是**——而數量是唯一被
    /// 印出來的那個數字。
    ///
    /// 所以這裡強制一條規則配一個證人，而且兩邊都不准有多的：加規則不補
    /// 證人會紅，刪規則忘了刪證人也會紅。
    #[test]
    fn every_default_url_rule_is_demonstrated_by_a_real_url() {
        // 證人一律寫成**縮寫版**——那是 Chromium 位址列真正交出來的形狀
        // （見 `uia.rs` 模組說明）。拿完整網址當證人會讓這張表在一個
        // 我們實際上拿不到的字串上通過。
        const WITNESSES: &[(&str, &str)] = &[
            ("*onlinebanking*", "hsbc.com.tw/onlinebanking/logon"),
            ("*netbank*", "netbank.example.com/transfer"),
            ("*ebank*", "ebank.megabank.com.tw/"),
            ("*/ib/*", "cathaybk.com.tw/ib/login"),
            ("*cathaybk.com*", "cathaybk.com.tw/mybank"),
            ("*esunbank.com*", "esunbank.com.tw/ib/home"),
            ("*ctbcbank.com*", "ctbcbank.com/twrbo/zh_tw"),
            ("*bot.com.tw*", "bot.com.tw/tw/personal-banking"),
            ("*taishinbank.com*", "taishinbank.com.tw/TSB/personal"),
            ("*firstbank.com.tw*", "firstbank.com.tw/sites/fcb/index"),
            ("*megabank.com.tw*", "megabank.com.tw/personal/deposit"),
            ("*accounts.google.com*", "accounts.google.com/signin/v2"),
            (
                "*login.microsoftonline.com*",
                "login.microsoftonline.com/common/oauth2",
            ),
            ("*password*", "github.com/settings/password"),
            ("*/signin*", "example.com/signin?next=/"),
            ("*/login*", "app.example.com/login?next=/"),
        ];

        let rules = PrivacyConfig::default().excluded_urls;

        // 兩邊必須完全對得起來。少了證人代表有規則沒有被證明過；
        // 多了證人代表這張表在替一條已經不存在的規則作證。
        let named: std::collections::BTreeSet<&str> =
            WITNESSES.iter().map(|(rule, _)| *rule).collect();
        let actual: std::collections::BTreeSet<&str> = rules.iter().map(String::as_str).collect();
        assert_eq!(
            actual, named,
            "預設規則和證人對不起來。新增一條規則就要補一個\
             「真的長這樣的網址」證明它會命中，不然它可能是個錯字"
        );

        for (rule, witness) in WITNESSES {
            // 關鍵是**指名道姓**：不能只問「這個網址有沒有被擋下來」。
            // `*password*` 幾乎什麼登入頁都吃得到，所以一個給 `*/login*`
            // 寫的證人很可能是被別條順手擋掉的——那樣的話這條規則就算
            // 拼錯了，測試照樣是綠的。
            assert!(
                glob_match(&rule.to_ascii_lowercase(), witness),
                "規則 {rule} 擋不下 {witness}——它自己舉的例子它都碰不到"
            );
        }
    }

    /// 另外三份預設清單，同一個要求。
    ///
    /// 網址那條寫完之後回頭看，其他三份清單的破口一模一樣，而且更大：
    /// 既有測試碰過的是 app 9 條裡的 7 條、標題 4 條裡的 3 條、會議 app
    /// **13 條裡的 5 條**。沒被碰到的那些之中包括 `gnome-keyring`、
    /// `seahorse`、`*private browsing*`、`anydesk`、`skype`——每一條都
    /// 可能已經是錯的，而 `doctor` 照樣把它們算進「N 條規則」裡。
    #[test]
    fn every_other_default_rule_is_demonstrated_too() {
        // app 是子字串比對（見 `app_pattern_matches`），所以證人寫成
        // 真實的執行檔名／bundle id，不是規則自己。
        const APPS: &[(&str, &str)] = &[
            ("keepassxc", "keepassxc.exe"),
            ("keepass", "KeePass.exe"),
            ("1password", "1Password.exe"),
            ("bitwarden", "Bitwarden.exe"),
            ("dashlane", "dashlane.exe"),
            ("lastpass", "LastPass.exe"),
            ("enpass", "Enpass.exe"),
            ("gnome-keyring", "gnome-keyring-daemon"),
            ("seahorse", "org.gnome.seahorse.Application"),
        ];
        const TITLES: &[(&str, &str)] = &[
            ("*password*", "change password — github"),
            ("*密碼*", "變更密碼"),
            ("*private browsing*", "private browsing — mozilla firefox"),
            ("*無痕*", "無痕視窗"),
        ];
        const MEETINGS: &[(&str, &str)] = &[
            ("zoom", "Zoom.exe"),
            ("teams", "Teams.exe"),
            ("ms-teams", "ms-teams.exe"),
            ("webex", "webexmta.exe"),
            ("gotomeeting", "gotomeeting.exe"),
            ("bluejeans", "bluejeans.exe"),
            ("obs", "obs.exe"),
            ("obs64", "obs64.exe"),
            ("streamlabs", "streamlabs obs.exe"),
            ("google meet", "google meet.exe"),
            ("skype", "Skype.exe"),
            ("anydesk", "AnyDesk.exe"),
            ("teamviewer", "TeamViewer.exe"),
        ];

        let cfg = PrivacyConfig::default();
        let same = |label: &str, rules: Vec<String>, table: &[(&str, &str)]| {
            let named: std::collections::BTreeSet<String> =
                table.iter().map(|(r, _)| (*r).to_string()).collect();
            let actual: std::collections::BTreeSet<String> = rules.into_iter().collect();
            assert_eq!(
                actual, named,
                "{label} 的預設清單和證人對不起來——加了規則就要補一個證人"
            );
        };

        same("excluded_apps", cfg.excluded_apps.clone(), APPS);
        same("excluded_titles", cfg.excluded_titles.clone(), TITLES);
        same(
            "SCREENSHARE_APPS",
            SCREENSHARE_APPS.iter().map(|s| s.to_string()).collect(),
            MEETINGS,
        );

        // app 與會議 app 都走小寫子字串，證人要先降成小寫再比——
        // `check()` 就是這樣做的（`focus.app_key()`）。
        for (rule, witness) in APPS {
            assert!(
                app_pattern_matches(rule, &witness.to_ascii_lowercase()),
                "app 規則 {rule} 碰不到它自己舉的例子 {witness}"
            );
        }
        for (rule, witness) in MEETINGS {
            assert!(
                witness.to_ascii_lowercase().contains(rule),
                "會議 app 規則 {rule} 碰不到它自己舉的例子 {witness}"
            );
        }
        for (rule, witness) in TITLES {
            assert!(
                glob_match(&rule.to_ascii_lowercase(), &witness.to_ascii_lowercase()),
                "標題規則 {rule} 碰不到它自己舉的例子 {witness}"
            );
        }
    }

    /// Chromium 交出來的是縮寫版網址。預設規則必須在**那個**形狀上命中，
    /// 不是在我們想像中的完整網址上命中。
    #[test]
    fn defaults_match_the_elided_url_the_browser_actually_gives_us() {
        let privacy = PrivacyConfig::default();
        // 網址列實際顯示的樣子：沒有 scheme、沒有 www.
        for elided in [
            "cathaybk.com.tw/ib/login",
            "accounts.google.com/signin/v2",
            "esunbank.com.tw/ib/home",
        ] {
            let hit = privacy
                .excluded_urls
                .iter()
                .any(|p| glob_match(&p.to_ascii_lowercase(), elided));
            assert!(hit, "縮寫版網址「{elided}」沒有被任何一條預設規則擋下來");
        }
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
            "keepassxc",               // Linux
            "keepassxc.exe",           // Windows
            "KeePassXC.exe",           // Windows，大小寫不一
            "org.keepassxc.keepassxc", // macOS bundle id
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
        for id in [
            "chrome.exe",
            "code.exe",
            "explorer.exe",
            "firefox",
            "Terminal",
        ] {
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
            password_field: false,
        }
    }

    /// 焦點在密碼欄上時，**什麼規則都不必命中**這一幀就該被丟掉。
    ///
    /// 這條測試的價值在於它用的是一個**完全無害**的脈絡：記事本、無趣的
    /// 標題、沒有網址。所有排除規則都不會命中它。如果密碼欄那一條被刪掉
    /// 或搬到規則後面，這裡就會紅——而在真實世界裡，同一個錯誤的症狀是
    /// 「她把使用者打密碼的那個畫面錄下來了」，永遠不會有人回報。
    #[test]
    fn a_focused_password_field_blocks_even_a_completely_innocent_window() {
        let privacy = PrivacyConfig::default();
        let mut focus = focus("notepad.exe", "未命名 - 記事本", None);
        assert!(
            !privacy.check(&focus).is_blocked(),
            "這個脈絡本身應該是可以錄的，否則這條測試證明不了任何事"
        );

        focus.password_field = true;
        let reason = privacy.check(&focus);
        assert!(reason.is_blocked(), "焦點在密碼欄上還照錄");
        assert!(
            reason.reason().unwrap().contains("password"),
            "理由要看得出是密碼欄，不然稽核紀錄講不清楚：{reason:?}"
        );
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
    fn a_misspelled_key_is_rejected_instead_of_silently_ignored() {
        // 這裡真正保護的不是「已經刪掉的欄位」，而是**打錯字的隱私規則**。
        // 少了 `deny_unknown_fields`，`excluded_app` 會被 serde 安靜地丟掉，
        // 於是使用者手上有一條讀起來完全正確、卻永遠不會命中的排除規則——
        // 他以為銀行網頁沒被錄，其實從第一天就在錄。
        //
        // 寧可開不起來。開不起來會被看見，而一條不命中的規則不會。
        let typos = [
            (
                "[privacy]\nexcluded_app = [\"1password\"]\n",
                "excluded_app",
            ),
            ("[capture]\nstore_image = false\n", "store_image"),
            // 曾經存在、後來刪掉的欄位（理由見 `RetentionConfig` 的說明）。
            // 舊設定檔會被指名擋下，而不是繼續調一個沒人在讀的數字。
            ("[retention]\nthumbs_days = 7\n", "thumbs_days"),
            (
                "[retention]\nmax_disk_gb_per_day = 5\n",
                "max_disk_gb_per_day",
            ),
            // 連區塊名稱打錯都要擋——那是 `Config` 自己那一層。
            ("[privacyy]\nexcluded_apps = []\n", "privacyy"),
        ];
        for (text, offending) in typos {
            let Err(err) = toml::from_str::<Config>(text) else {
                panic!("`{offending}` 應該要讓設定檔開不起來，而不是被安靜忽略");
            };
            let msg = err.to_string();
            assert!(
                msg.contains(offending),
                "錯誤訊息必須指名是哪個 key 出問題，否則使用者第一個念頭\
                 就是把 deny_unknown_fields 拿掉：{msg}"
            );
        }
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
