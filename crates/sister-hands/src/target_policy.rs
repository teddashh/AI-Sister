//! 一個動作的**目標**可不可以交給作業系統。
//!
//! 放在這裡而不是放在字母人裡面，理由只有一個：**測試要跑得到。** CI 對
//! `apps/desktop` 只做 `clippy` 和 `cargo build --release`，沒有 `cargo test`；
//! 那個 crate 在 Linux 上連編都編不起來（缺 dbus）。一份寫在那裡的
//! `#[cfg(test)] mod tests` 是四道閘門全綠也證明不了任何事的東西——這個 repo
//! 已經有一整層那樣的程式碼了。搬到 `sister-hands`，`cargo test --workspace`
//! 在 Linux 和 Windows 兩邊都會跑它。
//!
//! 判斷全部是字串比對，不碰檔案系統，所以在哪個平台跑結果都一樣。

use std::path::Path;

pub fn validate_url(url: &str) -> Result<(), String> {
    if url.chars().any(char::is_whitespace) || url.chars().any(char::is_control) {
        return Err("不會開啟：網址含空白或控制字元".into());
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err("不會開啟：網址沒有 scheme".into());
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" if rest.starts_with("//") && rest.len() > 2 => Ok(()),
        "http" | "https" => Err("不會開啟：網址沒有主機名稱".into()),
        // 白名單。`file:` `javascript:` `vbscript:` `data:` `shell:` `ms-…:`
        // 全部走這一條，不另外列一份看起來很兇、其實和這一行做同一件事的黑名單。
        _ => Err(format!("不會開啟：{scheme}: scheme 不在允許清單")),
    }
}

/// 打開來是「看的」那些副檔名。
///
/// **這是白名單，不是黑名單。** 黑名單那一版擋掉 `.exe` `.bat` `.ps1`，卻放行
/// `.hta`（mshta 會執行它）、`.vbs` `.js` `.wsf`（wscript 會執行它）、`.scf`
/// `.reg` `.msc` `.cpl` `.pif`——而這裡的路徑是模型從螢幕上讀來的字
/// （SPEC §9.4：螢幕上的字是資料不是指令）。列不完的那一邊，不能是會執行的那一邊。
const OPENABLE: &[&str] = &[
    "txt", "md", "log", "csv", "tsv", "json", "yaml", "yml", "toml", "xml", "ini", "pdf", "rtf",
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "png", "jpg", "jpeg", "gif",
    "webp", "bmp", "svg", "heic", "mp3", "wav", "mp4", "mov", "mkv", "zip",
];

pub fn validate_file(path: &Path) -> Result<(), String> {
    let shown = path.to_string_lossy();
    if shown.trim().is_empty() {
        return Err("不會開啟：路徑是空的".into());
    }
    // `\\?\` 和 `\\.\` 也從這裡走：兩者都以 `\\` 開頭，多寫兩個 `starts_with`
    // 只是看起來比較周到，實際上一列都到不了。
    if shown.starts_with(r"\\") {
        return Err("不會開啟：網路路徑與 Windows device path 不在允許範圍".into());
    }
    // **不用 `Path::file_name` / `Path::extension`。** 那兩支在 Linux 上不認得
    // `\`，所以 `C:\work\report.pdf` 整串都會被當成檔名——連 `C:` 的冒號都算進去。
    // 這裡的判斷要在兩個平台上得到同一個答案，否則本機跑綠的測試證明的是另一支
    // 函式。自己切分隔符，兩邊都切。
    let name = shown.rsplit(['\\', '/']).next().unwrap_or("");
    // `a.exe:b.txt` 的副檔名是 `txt`，可是 ShellExecuteW 開的是那條 alternate
    // data stream。檔名裡的 `:` 在 Windows 上只有這一個用途。
    if name.contains(':') {
        return Err("不會開啟：檔名裡有「:」（alternate data stream）".into());
    }
    match name.rsplit_once('.') {
        // 沒有副檔名：資料夾，或一個 Windows 不知道要拿什麼開的檔案。兩者都不會被
        // 執行——Windows 是看副檔名決定要不要執行的。
        None => Ok(()),
        Some((_, ext)) if OPENABLE.iter().any(|ok| ext.eq_ignore_ascii_case(ok)) => Ok(()),
        Some((_, ext)) => Err(format!("不會開啟：「.{ext}」不在可開啟的檔案類型清單裡")),
    }
}

/// 要被叫到前面來的那個視窗，標題認不認得出來。
///
/// 平台那一側是 `EnumWindows` + 標題**子字串**比對，比到第一個就停。所以
/// 空字串會比中畫面上第一個有標題的視窗——她會把一個誰也沒指定的視窗拉到
/// 前面來，而 action log 上那一列會寫「聚焦視窗：」，後面什麼都沒有。
///
/// 這裡只擋得掉「空的」。**兩個視窗標題長得像的時候她仍然可能挑錯一個**，
/// 那要 `EnumWindows` 那一側改成收集全部再讓人選，不是這一支能解決的。
pub fn validate_window_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("不會聚焦：沒有指定視窗標題".into());
    }
    Ok(())
}

/// 一個網址的 **host**，正規化到可以跟「她記下來的那一份」比對。
///
/// 為什麼是 host 而不是整條網址：`focus_events.url` 那一欄的來源是 Chromium
/// 的位址列，而位址列給的是**給人看的縮寫**——`kFormatUrlOmitHTTPS` 與
/// `kFormatUrlOmitTrivialSubdomains` 會把 `https://www.example.com/a?b=c`
/// 顯示成 `example.com/a?b=c`（見 `sister-capture` 的 `windows/uia.rs` 開頭）。
/// 逐字比對兩邊永遠不會相等，而且**不會有人發現**：它只會安靜地一個都不放行。
///
/// **這裡不可以用子字串比對。** `evil.com/?next=example.com` 含著
/// `example.com`，`example.com.evil.com` 也含著。所以先切出 authority，
/// 再整段相等比對。切的順序有三個容易錯的地方，三個都有測試釘著：
///
/// - **userinfo**：`https://example.com@evil.com/` 的 host 是 `evil.com`，
///   要取**最後**一個 `@` 之後。
/// - **IPv6**：`[::1]:8080` 的 host 是 `[::1]`，不能看到 `:` 就砍。
/// - **path/query/fragment**：三個都可能先出現，取最早的那一個。
///
/// 回 `None` 表示「這個字串講不出一個 host」——呼叫端必須把它當成
/// **不放行**，不是當成「沒有限制」。
pub fn host_of(url: &str) -> Option<String> {
    let v = url.trim();
    if v.is_empty() || v.chars().any(char::is_whitespace) {
        return None;
    }
    // scheme 是選配的：她記下來的那一份被砍掉了 scheme，模型讀來的那一份通常有。
    let rest = match v.split_once("://") {
        Some((scheme, rest)) => {
            // **只有 http/https 講得出「站」。** `chrome://settings` 有 `://`
            // 卻不是一個網站，早一版這裡把 `settings` 當成 host 回出去了。
            // 這一行和 `validate_url` 的白名單是同一個決定，兩邊要一起讀。
            if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                return None;
            }
            rest
        }
        None => {
            // 沒有 `://` 卻有 `:` 在第一個 `/` 之前，可能是 `about:blank`
            // 這種；也可能是 `example.com:8080/x`。用「冒號後面是不是全數字」
            // 分辨——是就當 port，不是就當 scheme 而且沒有 authority。
            let head = v.split(['/', '?', '#']).next().unwrap_or(v);
            if let Some((before, after)) = head.split_once(':')
                && !before.is_empty()
                && !after.is_empty()
                && !after.chars().all(|c| c.is_ascii_digit())
                && !before.starts_with('[')
            {
                return None;
            }
            v
        }
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // userinfo：取最後一個 `@` 之後。
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let host = if let Some(end) = authority.strip_prefix('[').and_then(|r| r.find(']')) {
        // IPv6 字面值：連方括號一起留著，`[::1]` 和 `::1` 不要變成兩個答案。
        &authority[..end + 2]
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    // `www.` 是位址列會省略的那一個，所以兩邊都要省，否則同一個站會變兩個答案。
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if host.is_empty() { None } else { Some(host) }
}

/// 她記下來的那條網址，跟她正要打開的那條，是不是同一個站。
///
/// 兩邊都走 [`host_of`]。任何一邊講不出 host 就是 `false`——**「我看不懂」
/// 不可以讀成「可以」**。
pub fn same_site(recorded: &str, target: &str) -> bool {
    match (host_of(recorded), host_of(target)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {

    /// 她記下來的是縮寫版，模型讀來的是完整版。這兩個必須是同一個站，
    /// 否則整道閘門會安靜地一個都不放行——而「一個都不放行」看起來很安全，
    /// 實際上是這個功能整個沒接上，沒有人會發現。
    #[test]
    fn the_abbreviated_one_she_recorded_is_the_same_site_as_the_full_one() {
        assert!(same_site(
            "example.com/a?b=c",
            "https://www.example.com/a?b=c"
        ));
        assert!(same_site("example.com", "https://example.com/"));
        assert!(same_site("EXAMPLE.com/x", "https://Example.COM/y"));
        assert!(same_site("example.com/a", "https://example.com:8443/a"));
    }

    /// `@` 前面那一段是 userinfo，不是主機。這一條擋的是把記得住的網域
    /// 貼在 `@` 前面來借過的那一招。
    #[test]
    fn the_bit_before_the_at_sign_is_not_the_host() {
        assert_eq!(
            host_of("https://example.com@evil.com/").as_deref(),
            Some("evil.com")
        );
        assert!(!same_site("example.com", "https://example.com@evil.com/"));
        // 兩個 `@` 也一樣，取最後一個。
        assert_eq!(
            host_of("https://a@example.com@evil.com/").as_deref(),
            Some("evil.com")
        );
    }

    /// 子字串比對會全中的那三種，這裡必須全不中。
    #[test]
    fn a_host_that_merely_contains_the_remembered_one_is_a_different_site() {
        for impostor in [
            "https://evil.com/?next=example.com",
            "https://example.com.evil.com/",
            "https://notexample.com/",
            "https://example.community/",
        ] {
            assert!(
                !same_site("example.com", impostor),
                "{impostor} 不是 example.com，可是它被放行了"
            );
        }
    }

    /// 講不出 host 的一律不是同一個站。**「我看不懂」不可以讀成「可以」**。
    #[test]
    fn something_it_cannot_read_is_never_the_same_site() {
        for unreadable in [
            "",
            "   ",
            "about:blank",
            "chrome://settings",
            "javascript:alert(1)",
            "/just/a/path",
        ] {
            assert_eq!(host_of(unreadable), None, "{unreadable} 竟然講得出 host");
            assert!(!same_site("example.com", unreadable));
            assert!(!same_site(unreadable, "https://example.com/"));
        }
    }

    /// `www.` 是位址列會省略的那一個，所以兩邊都要省——否則同一個站在
    /// 她的紀錄裡和在模型嘴裡會是兩個答案。而別的子網域**不可以**省。
    #[test]
    fn only_www_is_dropped_and_other_subdomains_are_not() {
        assert!(same_site("example.com", "https://www.example.com/"));
        assert!(same_site("www.example.com", "https://example.com/"));
        assert!(!same_site("example.com", "https://mail.example.com/"));
        assert_eq!(
            host_of("https://mail.example.com/").as_deref(),
            Some("mail.example.com")
        );
    }

    /// IPv6 字面值不可以被冒號切成兩半。開發時整天看的 `localhost:3000` 也一樣。
    #[test]
    fn a_colon_inside_brackets_is_not_a_port() {
        assert_eq!(host_of("http://[::1]:8080/x").as_deref(), Some("[::1]"));
        assert!(same_site("[::1]/a", "http://[::1]:8080/b"));
        assert_eq!(
            host_of("http://localhost:3000/x").as_deref(),
            Some("localhost")
        );
        assert!(same_site("localhost:3000/a", "http://localhost/b"));
    }
    use super::*;

    #[test]
    fn url_policy_blocks_active_and_local_schemes_without_blocking_https() {
        for bad in [
            "file:///C:/secret.txt",
            "javascript:alert(1)",
            "vbscript:msgbox(1)",
        ] {
            let error = validate_url(bad).unwrap_err();
            assert!(error.contains("不會開啟"), "{bad}: {error}");
            assert!(!error.contains("已開啟"), "{bad}: {error}");
        }
        assert!(validate_url("https://example.com/task/7").is_ok());
        assert!(validate_url("https://example.com/a&calc.exe").is_ok());
        assert!(validate_url("http://localhost/docs").is_ok());
    }

    /// 白名單擋掉的，要包含黑名單那一版整批漏掉的「雙擊就會跑」的類型。
    ///
    /// `.hta` `.vbs` `.js` `.wsf` `.scf` `.reg` `.msc` `.cpl` `.pif` 一個都不在
    /// 舊的 `blocked` 陣列裡。這一條測的是「列不完的那一邊不是會執行的那一邊」，
    /// 不是「這九個字串有被寫進某個陣列」。
    #[test]
    fn file_policy_blocks_everything_that_is_not_a_document() {
        for bad in [
            r"C:\work\run.exe",
            r"C:\work\go.cmd",
            r"C:\work\p.ps1",
            r"C:\work\page.hta",
            r"C:\work\s.vbs",
            r"C:\work\s.js",
            r"C:\work\s.wsf",
            r"C:\work\s.scf",
            r"C:\work\add.reg",
            r"C:\work\x.msc",
            r"C:\work\x.cpl",
            r"C:\work\x.pif",
            r"C:\work\note.txt.exe",
            r"C:\work\a.exe:note.txt",
            r"\\server\share\note.txt",
            r"\\?\C:\note.txt",
        ] {
            let error = validate_file(Path::new(bad)).unwrap_err();
            assert!(error.contains("不會開啟"), "{bad}: {error}");
            assert!(!error.contains("已開啟"), "{bad}: {error}");
        }
        for ok in [
            r"C:\work\report.pdf",
            r"C:\work\notes.md",
            r"C:\work\data.CSV",
            // 資料夾：沒有副檔名的東西 Windows 不會拿去執行。
            r"C:\work\inbox",
            // 同一組字串換成正斜線，答案必須一樣——這一支不能有兩種平台行為。
            "C:/work/report.pdf",
        ] {
            assert!(validate_file(Path::new(ok)).is_ok(), "{ok} 被擋掉了");
        }
        // 空路徑到得了 `ShellExecuteW`，而它一列副檔名都沒有。
        assert!(validate_file(Path::new("")).is_err());
        assert!(validate_file(Path::new("   ")).is_err());
    }

    /// 空標題會比中第一個有標題的視窗，那不是「聚焦」，那是隨便抓一個。
    #[test]
    fn an_empty_window_title_matches_everything_so_it_matches_nothing() {
        for bad in ["", "   ", "\t\n"] {
            let error = validate_window_title(bad).unwrap_err();
            assert!(error.contains("不會聚焦"), "{bad:?}: {error}");
        }
        assert!(validate_window_title("Visual Studio Code").is_ok());
    }
}
