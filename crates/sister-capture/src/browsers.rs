//! 哪些程式值得問 UIA。
//!
//! 這道閘門看起來像效能最佳化，其實是隱私機制的一部分，而且兩個方向的
//! 代價都很硬：
//!
//! - **漏掉一個瀏覽器** → 那個瀏覽器讀不到網址 → `excluded_urls` 那整組
//!   規則對它完全不生效，網銀與登入頁被錄進去（THREAT_MODEL 安靜失效 #2）。
//! - **多抓到一個非瀏覽器** → 對它發 UIA 呼叫。UIA 沒有辦法取消，卡住就
//!   燒掉一次 `MAX_ABANDONS`，而連續三次就**永久**放棄讀網址——於是
//!   十六條規則一起失效。跟漏掉瀏覽器殊途同歸，只是更難查。
//!
//! 所以這裡不是「寬鬆一點比較安全」，兩邊都會通到同一個地方。
//!
//! 這個模組刻意**不放在 `windows/` 底下**：它全部是字串判斷，放在這裡
//! 開發機（Linux）就跑得到測試。同樣的理由見 `uia::plausible_url`。

/// 會叫 UIA 的瀏覽器（比對小寫執行檔名，例如 `chrome.exe`）。
///
/// 只寫可辨識的字根，讓 `chrome` 一條同時涵蓋 `chrome.exe`、
/// `chrome_proxy.exe` 與各家 Chromium 分支的命名。
const BROWSERS: &[&str] = &[
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "vivaldi",
    "opera",
    "chromium",
    "arc",
    "zen",
    "librewolf",
    "waterfox",
    "floorp",
    "thorium",
    "iexplore",
];

/// 這個 app 該不該被問 UIA。
///
/// 比對的是**詞首**，不是任意子字串。原本是 `app_key.contains(b)`，
/// 而清單裡有 `arc` 這種三個字母的字根——於是
/// `searchapp.exe`、`searchhost.exe`（Windows 11 自己的開始/搜尋介面）
/// 全都被當成瀏覽器。那不是白花幾微秒而已：開始功能表是個 UWP host，
/// UIA 對它卡住三次就把網址擷取整個關掉，然後十六條網銀規則一起消失。
///
/// 「詞首」放得比 `starts_with` 寬：`app_key` 在拿不到執行檔名的時候
/// 會退回顯示名稱（`google chrome`），那時候字根不在開頭。所以只要求
/// 前一個字元不是英數即可——空白、`-`、`_`、`.`、`\` 都算界線。
pub fn is_browser(app_key: &str) -> bool {
    BROWSERS.iter().any(|b| starts_a_word(app_key, b))
}

fn starts_a_word(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(at, _)| {
        haystack[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 自己的搜尋介面不是瀏覽器。
    ///
    /// 這是一條迴歸測試：`arc`（Arc 瀏覽器）以前是用子字串比對的，而
    /// `se-arc-happ.exe` 裡面就有那三個字母。開始功能表是使用者一天按
    /// 幾十次的東西，等於把「對 UWP host 發 UIA 呼叫」變成日常——
    /// 而那條路的盡頭是十六條網址規則一起失效。
    #[test]
    fn windows_own_search_ui_is_not_a_browser() {
        for not_a_browser in [
            "searchapp.exe",  // Win10/11 的搜尋
            "searchhost.exe", // Win11 的開始功能表
            "monarch.exe",    // 也含 arc
            "explorer.exe",
            "code.exe",
            "slack.exe",
            "1password.exe",
            "notepad.exe",
            "",
        ] {
            assert!(
                !is_browser(not_a_browser),
                "{not_a_browser} 不是瀏覽器，不該對它發 UIA 呼叫"
            );
        }
    }

    /// 每一條字根都要有一個真的執行檔名證明它會命中。
    ///
    /// 跟 `config` 那幾份預設清單同一個規矩：一條沒有人證明過的規則，
    /// 跟不存在是同一件事——只是 `doctor` 還是會把它算進總數。
    #[test]
    fn every_browser_in_the_list_is_demonstrated_by_a_real_exe_name() {
        // 字根 → 真實世界裡會出現的 app_key
        let witnesses = [
            ("chrome", "chrome.exe"),
            ("msedge", "msedge.exe"),
            ("firefox", "firefox.exe"),
            ("brave", "brave.exe"),
            ("vivaldi", "vivaldi.exe"),
            ("opera", "opera.exe"),
            ("chromium", "chromium.exe"),
            ("arc", "arc.exe"),
            ("zen", "zen.exe"),
            ("librewolf", "librewolf.exe"),
            ("waterfox", "waterfox.exe"),
            ("floorp", "floorp.exe"),
            ("thorium", "thorium.exe"),
            ("iexplore", "iexplore.exe"),
        ];

        // 集合相等：加了字根卻沒給證人會紅，刪了字根卻留著證人也會紅。
        let named: std::collections::BTreeSet<&str> =
            witnesses.iter().map(|(root, _)| *root).collect();
        let actual: std::collections::BTreeSet<&str> = BROWSERS.iter().copied().collect();
        assert_eq!(actual, named, "BROWSERS 有字根沒有被證明過");

        for (root, witness) in witnesses {
            assert!(
                is_browser(witness),
                "字根 `{root}` 連 `{witness}` 都認不出來"
            );
        }
    }

    /// 詞首的定義要放得夠寬，容得下 `app_key` 退回顯示名稱的那條路。
    ///
    /// 抓不到執行檔名時 `app_key()` 會用顯示名稱，字根就不在字串開頭了。
    /// 這也是為什麼這裡不能直接寫 `starts_with`。
    #[test]
    fn a_word_boundary_is_not_only_the_start_of_the_string() {
        for browser in [
            "google chrome",          // 退回顯示名稱
            "brave-browser.exe",      // 連字號
            "chrome_proxy.exe",       // 底線
            "ungoogled-chromium.exe", // 分支命名
            "msedgewebview2.exe",     // 字根在開頭、後面接東西
        ] {
            assert!(is_browser(browser), "{browser} 應該要被當成瀏覽器");
        }
    }
}
