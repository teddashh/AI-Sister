//! 一個動作的**目標**可不可以交給作業系統。
//!
//! 放在這裡而不是放在字母人裡面，理由只有一個：**每個平台都要測得到。**
//! desktop 是獨立 workspace，Linux 的 `cargo test --workspace` 連編都編不到它；
//! Windows CI 從 alpha.100 起會另跑 desktop tests，但共用規則若留在那裡，Linux
//! 仍然沒有覆蓋，CLI 呼叫端也得再抄一份。搬到 `sister-hands`，根 workspace 在
//! Linux 和 Windows 兩邊都會跑它，而且兩個產品入口只認同一份判斷。
//!
//! 判斷全部是字串比對，不碰檔案系統，所以在哪個平台跑結果都一樣。

use std::path::Path;

use crate::Suggestion;

/// 三種公開 suggestion 共用的一道目標白名單。執行隘口在呼叫 `Executor` 前走
/// 一次；平台實作貼著 OS 呼叫再走一次，擋住日後繞過隘口或 TOCTOU 的退步。
pub(crate) fn validate_suggestion(suggestion: &Suggestion) -> Result<(), String> {
    match suggestion {
        Suggestion::OpenUrl { url, .. } => validate_url(url),
        Suggestion::OpenFile { path, .. } => validate_file(path),
        Suggestion::FocusWindow { title, .. } => validate_window_title(title),
    }
}

pub fn validate_url(url: &str) -> Result<(), String> {
    if url.chars().any(char::is_whitespace) || url.chars().any(char::is_control) {
        return Err("不會開啟：網址含空白或控制字元".into());
    }
    // WHATWG 的 http(s) parser 會把反斜線當成斜線，手寫的 authority
    // parser 卻不會。例如 `https://evil.example\@example.com/` 在這裡若照
    // `@` 切會讀成 `example.com`，瀏覽器實際開的卻是 `evil.example`。
    // 這裡不嘗試重寫整套瀏覽器 parser；有歧義就不交給它。
    if url.contains('\\') {
        return Err("不會開啟：網址含反斜線，瀏覽器對它的解讀不唯一".into());
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err("不會開啟：網址沒有 scheme".into());
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" if rest.starts_with("//") && host_of(url).is_some() => Ok(()),
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
    // `PCWSTR` 以 NUL 結尾。`evil.exe\0.pdf` 若按 Rust 字串檢查會像文件，
    // ShellExecuteW 實際只看見前面的 executable。其他控制字元也沒有一個
    // 跨平台、可稽核的檔名語意，一起 fail-closed。
    if shown.chars().any(char::is_control) {
        return Err("不會開啟：檔案路徑含控制字元".into());
    }
    // `\\?\` 和 `\\.\` 也從這裡走。兩個 separator 的任何組合都算：Windows
    // API 也接受 `//server/share`，而這支函式底下本來就刻意把 `/` 與 `\` 視為
    // 同一種分隔符。只擋 `\\` 會讓同一條 UNC 換個拼法就通過。
    let begins_with_two_separators = shown
        .as_bytes()
        .get(..2)
        .is_some_and(|pair| pair.iter().all(|byte| matches!(byte, b'\\' | b'/')));
    if begins_with_two_separators {
        return Err("不會開啟：網路路徑與 Windows device path 不在允許範圍".into());
    }
    // `ShellExecuteW` 的 lpFile 不只收檔案，也收 URI。若只看最後一段的副檔名，
    // `https://evil.example/report.pdf` 會被當成可讀文件放行，最後卻由瀏覽器開啟，
    // 整道 URL 來源政策因此被 action kind 繞掉。冒號只准出現在絕對 Windows
    // 磁碟機前綴（`C:\\` / `C:/`）；`C:report.pdf` 是 drive-relative 路徑，語意
    // 也取決於行程狀態，所以不收。
    let bytes = shown.as_bytes();
    let windows_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let has_other_colon = if windows_drive {
        shown[2..].contains(':')
    } else {
        shown.contains(':')
    };
    if has_other_colon {
        return Err("不會開啟：檔案路徑不能是 URI，也不能含磁碟機前綴以外的「:」".into());
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
        // 沒有副檔名可能是資料夾，也可能是 extensionless executable；光看字串
        // 分不出來，而 ShellExecuteW 可以直接執行後者。這個 action 叫 open-file，
        // 先整類拒絕；將來要開資料夾得在 OS 邊界用 metadata 證明它真是資料夾。
        None => Err("不會開啟：路徑沒有可驗證的文件副檔名".into()),
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

/// 方括號裡的 IPv6 literal。刻意不碰 `std::net`：產品的結構性承諾是出貨樹與
/// 原始碼都沒有 socket API，`check-no-network.sh` 會把那個 namespace 也視為
/// 越界。這裡只需要解析，不需要網路；IPv4-mapped 與 zone id 先 fail-closed。
fn valid_ipv6_literal(value: &str) -> bool {
    if value.is_empty()
        || value.contains(":::")
        || value.chars().any(|c| c != ':' && !c.is_ascii_hexdigit())
    {
        return false;
    }
    let compressed = value.match_indices("::").count();
    if compressed > 1 {
        return false;
    }
    let groups: Vec<&str> = value.split(':').filter(|group| !group.is_empty()).collect();
    if groups.iter().any(|group| group.len() > 4) {
        return false;
    }
    match compressed {
        0 => groups.len() == 8 && !value.starts_with(':') && !value.ends_with(':'),
        1 => groups.len() < 8,
        _ => false,
    }
}

/// 從一個網址解析出 **host**，並統一 ASCII 大小寫。它不在這裡剝 `www.`；
/// Chromium 可能省略一層的等價規則只寫在 [`same_site`]，才不會不小心剝兩層。
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
    if v.is_empty()
        || v.chars().any(char::is_whitespace)
        || v.chars().any(char::is_control)
        || v.contains('\\')
    {
        return None;
    }
    // scheme 是選配的：她記下來的那一份被砍掉了 scheme，模型讀來的那一份通常有。
    let (rest, had_scheme) = match v.split_once("://") {
        Some((scheme, rest)) => {
            // **只有 http/https 講得出「站」。** `chrome://settings` 有 `://`
            // 卻不是一個網站，早一版這裡把 `settings` 當成 host 回出去了。
            // 這一行和 `validate_url` 的白名單是同一個決定，兩邊要一起讀。
            if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
                return None;
            }
            (rest, true)
        }
        None => {
            // 沒有 `://` 卻有 `:` 在第一個 `/` 之前，可能是 `about:blank`
            // 這種；也可能是 `example.com:8080/x`。用「冒號後面是不是全數字」
            // 分辨——是就當 port，不是就當 scheme 而且沒有 authority。
            let head = v.split(['/', '?', '#']).next().unwrap_or(v);
            if let Some((before, after)) = head.split_once(':') {
                if !before.is_empty()
                    && !after.is_empty()
                    && !after.chars().all(|c| c.is_ascii_digit())
                    && !before.starts_with('[')
                {
                    return None;
                }
            }
            (v, false)
        }
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Chromium 會省略 scheme，但 scheme-less 的 `name@company.com` 更常是位址列
    // 裡拿來搜尋的 email，不是一個帶 userinfo 的已完成 URL。完整 http(s) URL
    // 仍按最後一個 `@` 解析；沒有 scheme 的歧義字串不拿來替網址背書。
    if !had_scheme && authority.contains('@') {
        return None;
    }
    // userinfo：取最後一個 `@` 之後。
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let host = if authority.starts_with('[') {
        let end = authority.find(']')?;
        if !valid_ipv6_literal(&authority[1..end]) {
            return None;
        }
        let after = &authority[end + 1..];
        if !after.is_empty() {
            match after.strip_prefix(':') {
                Some(port) if port.parse::<u16>().is_ok() => {}
                _ => return None,
            }
        }
        // IPv6 字面值：連方括號一起留著，`[::1]` 和 `::1` 不要變成兩個答案。
        &authority[..=end]
    } else {
        if authority.contains(['[', ']']) {
            return None;
        }
        match authority.split_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => host,
            Some(_) => return None,
            None => authority,
        }
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(host)
}

/// 她記下來的那條網址，跟她正要打開的那條，是不是同一個站。
///
/// 兩邊都走 [`host_of`]。任何一邊講不出 host 就是 `false`——**「我看不懂」
/// 不可以讀成「可以」**。
pub fn same_site(recorded: &str, target: &str) -> bool {
    fn without_one_www(host: &str) -> &str {
        host.strip_prefix("www.").unwrap_or(host)
    }
    match (host_of(recorded), host_of(target)) {
        (Some(a), Some(b)) => {
            // Chromium 可能把位址列最外面一層 trivial `www.` 省掉。只允許**一邊
            // 少一層**，不反覆剝：`www.www.evil` 和 `evil` 仍是兩個站。
            a == b || without_one_www(&a) == b || a == without_one_www(&b)
        }
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
        // 沒有 scheme 的這個形狀也可能只是位址列裡拿來搜尋的 email；不能把它
        // 當成「company.com 出現過」的憑據。
        assert_eq!(host_of("john.smith@company.com"), None);
        assert!(!same_site("john.smith@company.com", "https://company.com/"));
    }

    /// http(s) 裡的反斜線會被瀏覽器當成斜線。如果這裡按普通字元
    /// 讀，`@` 後面那一段會借到一個已記錄網站的票，但瀏覽器會開另一站。
    #[test]
    fn a_backslash_cannot_borrow_the_site_after_an_at_sign() {
        let disguised = r"https://evil.example\@example.com/collect";
        assert_eq!(host_of(disguised), None);
        assert!(!same_site("example.com", disguised));
        let error = validate_url(disguised).unwrap_err();
        assert!(error.contains("反斜線"), "{error}");
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
        for malformed in [
            "https://[::1",
            "https://[::1]evil.example/",
            "https://[::1]:evil/",
            "https://[evil.example]/",
            "https://[]/",
            "https://[::::]/",
            "https://[::1]:99999/",
            "https://example.com:evil/",
            "https://example.com:65536/",
            "https:///example.com/",
            "https:////example.com/",
        ] {
            assert_eq!(host_of(malformed), None, "{malformed}");
            assert!(validate_url(malformed).is_err(), "{malformed}");
        }
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
            "//server/share/note.txt",
            r"/\server/share/note.txt",
            r"\/server/share/note.txt",
            "//?/C:/note.txt",
            "C:\\work\\evil.exe\0.pdf",
            r"C:\work\payload",
            r"C:\work\inbox",
            "https://evil.example/report.pdf",
            "file:///C:/work/report.pdf",
            r"C:report.pdf",
            r"C:\work\https://evil.example/report.pdf",
        ] {
            let error = validate_file(Path::new(bad)).unwrap_err();
            assert!(error.contains("不會開啟"), "{bad}: {error}");
            assert!(!error.contains("已開啟"), "{bad}: {error}");
        }
        for ok in [
            r"C:\work\report.pdf",
            r"C:\work\notes.md",
            r"C:\work\data.CSV",
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
