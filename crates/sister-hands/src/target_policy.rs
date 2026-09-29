//! 一個動作的**目標**可不可以交給作業系統。
//!
//! 放在這裡而不是放在字母人裡面，理由只有一個：**每個平台都要測得到。**
//! desktop 是獨立 workspace，Linux 的 `cargo test --workspace` 連編都編不到它；
//! Windows CI 從 alpha.100 起會另跑 desktop tests，但共用規則若留在那裡，Linux
//! 仍然沒有覆蓋，CLI 呼叫端也得再抄一份。搬到 `sister-hands`，根 workspace 在
//! Linux 和 Windows 兩邊都會跑它，而且兩個產品入口只認同一份判斷。
//!
//! URL 由正式 parser 解析，不碰檔案系統，所以在哪個平台跑結果都一樣。

use std::path::Path;
use url::{Position, Url};

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
    // WHATWG 的 http(s) parser 會把反斜線當成斜線；錄製的原字節沒有
    // 這個歧義才准拿來授權。
    if url.contains('\\') {
        return Err("不會開啟：網址含反斜線，瀏覽器對它的解讀不唯一".into());
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err("不會開啟：網址沒有 scheme".into());
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" if rest.starts_with("//") => match parse_web_url(url) {
            Some(parsed) if has_userinfo(&parsed) || raw_authority_has_userinfo(url) => {
                Err("不會開啟：網址不能含 userinfo".into())
            }
            Some(_) => Ok(()),
            None => Err("不會開啟：網址沒有可驗證的主機名稱".into()),
        },
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

/// 只用正式 URL parser 解析 authority。錄製位址列可省略 scheme；在這一種
/// Chromium 顯示形式下才補 HTTPS。userinfo 不可從錄製縮寫猜出來。
fn explicit_http_scheme_len(value: &str) -> Option<usize> {
    if value
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        Some(8)
    } else if value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
    {
        Some(7)
    } else {
        None
    }
}

fn parse_web_url(value: &str) -> Option<Url> {
    if value.is_empty()
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
        || value.contains('\\')
        || percent_decode(value).is_none()
    {
        return None;
    }
    let (to_parse, abbreviated) = if let Some(prefix_len) = explicit_http_scheme_len(value) {
        let rest = &value[prefix_len..];
        if rest.is_empty() || rest.starts_with('/') {
            return None;
        }
        (value.to_owned(), false)
    } else {
        // `://` in a path or query is data, not a leading scheme. An actual
        // unsupported scheme at the beginning remains invalid.
        if value.split_once("://").is_some_and(|(prefix, _)| {
            prefix
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
                && prefix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        }) {
            return None;
        }
        if value.starts_with('/') {
            return None;
        }
        (format!("https://{value}"), true)
    };
    let parsed = Url::parse(&to_parse).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return None;
    }
    let userinfo = &parsed[Position::BeforeUsername..Position::BeforeHost];
    if abbreviated && (userinfo.contains('@') || raw_authority_has_userinfo(value)) {
        return None;
    }
    Some(parsed)
}

fn has_userinfo(parsed: &Url) -> bool {
    parsed[Position::BeforeUsername..Position::BeforeHost].contains('@')
}

// URL parser 會把 `https://@host/` 的空 userinfo 正規化掉。授權仍必須
// 看見原字節裡的 @；只掃 authority，不把 path 裡的 @ 當成 credentials。
fn raw_authority_has_userinfo(value: &str) -> bool {
    let rest = explicit_http_scheme_len(value).map_or(value, |len| &value[len..]);
    rest.split(['/', '?', '#'])
        .next()
        .is_some_and(|authority| authority.contains('@'))
}

/// parser 正規化的 host。完整 URL 即使含 userinfo 仍會回傳真正的 host；
/// standing grant 在 [`same_destination`] 額外拒絕整類 userinfo。
pub fn host_of(url: &str) -> Option<String> {
    let parsed = parse_web_url(url)?;
    if let Some(domain) = parsed.domain() {
        let (shown, status) = idna::domain_to_unicode(domain);
        return Some(if status.is_ok() {
            shown
        } else {
            domain.to_owned()
        });
    }
    Some(parsed[Position::BeforeHost..Position::AfterHost].to_owned())
}

/// 她記下來的那條網址，跟她正要打開的那條，是不是同一個站。
///
/// 兩邊都走 [`host_of`]。任何一邊講不出 host 就是 `false`——**「我看不懂」
/// 不可以讀成「可以」**。
///
/// 這支**只比 host**。做完後的畫面核對（`screen_check`）也走這裡：瀏覽器
/// 落地後路徑可能被站方改掉，那一格是憑據不是攔截。無人值守要開之前的來源
/// 票另走 [`same_destination`]：同站不同去處不能借已記錄的 host 當授權。
pub fn same_site(recorded: &str, target: &str) -> bool {
    match (parse_web_url(recorded), parse_web_url(target)) {
        (Some(a), Some(b)) => same_host(
            &a[Position::BeforeHost..Position::AfterHost],
            &b[Position::BeforeHost..Position::AfterHost],
        ),
        _ => false,
    }
}

fn same_host(a: &str, b: &str) -> bool {
    fn without_one_www(host: &str) -> &str {
        host.strip_prefix("www.").unwrap_or(host)
    }
    // Chromium 只可能省略最外層一個 trivial www.。
    a == b || without_one_www(a) == b || a == without_one_www(b)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 解 `%XX`。解不開就 `None`——授權比對不可以把壞掉的編碼讀成「同一頁」。
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_digit(bytes[i + 1])?;
            let lo = hex_digit(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 權威段之後的 path／query／fragment。`@` 只在權威段當 userinfo；
/// path 裡的 `user@inbox` 不是第二個主機。
fn tail_after_authority(url: &str) -> Option<&str> {
    let _ = host_of(url)?;
    let v = url.trim();
    let rest = explicit_http_scheme_len(v).map_or(v, |len| &v[len..]);
    match rest.find(['/', '?', '#']) {
        Some(i) => Some(&rest[i..]),
        None => Some(""),
    }
}

/// 從一個網址解析出 **path**。講不出 host 的字串這裡也講不出 path。
///
/// 空路徑和 `/` 都回 `"/"`。其餘保留原始 path，包含 percent escape 與結尾的
/// `/`：伺服器可以把 `%2F` 和 `/`、`%62` 和 `b` 當成不同路徑。
/// 仍解碼一份**只供驗證**；壞 escape 或解碼後的 `.` / `..` 段都拒絕。
pub fn path_of(url: &str) -> Option<String> {
    let parsed = parse_web_url(url)?;
    let tail = tail_after_authority(url)?;
    let raw = tail.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(raw)?;
    if decoded
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    let raw = if raw.is_empty() { "/" } else { raw };
    // URL parser 會消去 dot segments。授權不能讓不同的原始 path 在這步
    // 合併；只收 parser 沒改過 path 位元組的輸入。
    (parsed.path() == raw).then(|| raw.to_string())
}

/// 兩邊 path 是否相同。任何一邊講不出 path 就是 `false`。
///
/// path 區分大小寫；host 的大小寫在 [`same_site`]。
pub fn same_path(recorded: &str, target: &str) -> bool {
    match (path_of(recorded), path_of(target)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn query_and_fragment(url: &str) -> Option<(Option<String>, Option<String>)> {
    let parsed = parse_web_url(url)?;
    let tail = tail_after_authority(url)?;
    let (before_fragment, raw_fragment) = match tail.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (tail, None),
    };
    let raw_query = before_fragment.split_once('?').map(|(_, query)| query);
    // parser 會替非 ASCII 等字元做序列化。來源票必須保留 query/fragment
    // 的原始寫法與空分隔符，不能用正規化後的值借票。
    if parsed.query() != raw_query || parsed.fragment() != raw_fragment {
        return None;
    }
    Some((
        parsed.query().map(str::to_owned),
        parsed.fragment().map(str::to_owned),
    ))
}

/// query 與 fragment 的原文字節及分隔符是否相同。重複 key 的先後、空欄與 escape
/// 都可能改變伺服器或頁面收到的去處；壞掉的 percent escape 仍拒絕。
pub fn same_query_and_fragment(recorded: &str, target: &str) -> bool {
    match (query_and_fragment(recorded), query_and_fragment(target)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// 無人值守 standing grant 的去處：同一 scheme、有效 port、站、path、query／fragment。
///
/// 只比 host 會讓 `/collect` 借 `example.com/help` 過關；只比 path 會讓
/// `?id=7` 的紀錄去開 `?id=8` 或 `?next=https://evil.example`。漏掉 scheme／port
/// 會讓 HTTP 或另一個服務借 HTTPS 的來源票。任何一欄讀不出來都拒絕；當場按不走這支。
pub fn same_destination(recorded: &str, target: &str) -> bool {
    let (Some(recorded_url), Some(target_url)) = (parse_web_url(recorded), parse_web_url(target))
    else {
        return false;
    };
    // userinfo 是完整 origin 身分的一部分；錄製縮寫不保證它仍可見，
    // 因此任何一端帶 userinfo 都不准使用 standing grant。
    if has_userinfo(&recorded_url)
        || has_userinfo(&target_url)
        || raw_authority_has_userinfo(recorded)
        || raw_authority_has_userinfo(target)
    {
        return false;
    }
    let recorded_host = &recorded_url[Position::BeforeHost..Position::AfterHost];
    let target_host = &target_url[Position::BeforeHost..Position::AfterHost];
    let host_matches = if explicit_http_scheme_len(recorded).is_some() {
        recorded_host == target_host
    } else {
        same_host(recorded_host, target_host)
    };
    recorded_url.scheme() == target_url.scheme()
        && recorded_url.port_or_known_default() == target_url.port_or_known_default()
        && host_matches
        && same_path(recorded, target)
        && same_query_and_fragment(recorded, target)
}

/// 保存的授權書寫的是完整 URL，不是 Chromium 縮寫；host 不得借 `www.`
/// 相容規則。其餘 component 與 [`same_destination`] 使用同一套嚴格比較。
pub fn same_explicit_destination(granted: &str, target: &str) -> bool {
    if explicit_http_scheme_len(granted).is_none()
        || explicit_http_scheme_len(target).is_none()
        || !same_destination(granted, target)
    {
        return false;
    }
    match (parse_web_url(granted), parse_web_url(target)) {
        (Some(a), Some(b)) => {
            a[Position::BeforeHost..Position::AfterHost]
                == b[Position::BeforeHost..Position::AfterHost]
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

    /// 位址列縮寫掉 scheme／www 之後，同一頁的 path 仍要對得上。
    #[test]
    fn abbreviated_and_full_urls_share_the_same_path() {
        assert_eq!(path_of("example.com/bill?id=7").as_deref(), Some("/bill"));
        assert_eq!(
            path_of("https://www.example.com/bill?id=7").as_deref(),
            Some("/bill")
        );
        assert!(same_path(
            "example.com/bill?id=7",
            "https://www.example.com/bill?id=7"
        ));
        assert!(same_path("example.com", "https://example.com/"));
        assert!(!same_path("example.com/a/", "https://example.com/a"));
        assert_eq!(path_of("http://[::1]:8080/x").as_deref(), Some("/x"));
        assert!(same_path(
            "example.com/bill",
            "https://example.com:443/bill"
        ));
        assert!(!same_path(
            "example.com/bill",
            "https://example.com/%62%69%6C%6C"
        ));
        assert_eq!(path_of("https://example.com/help/../collect"), None);
    }

    /// 同站不同路徑不是同一頁。這是 #42 剩下那半：螢幕上埋的 `/collect`
    /// 不能借位址列裡出現過的 `example.com/bill` 當來源票。
    #[test]
    fn a_different_path_on_the_same_site_is_not_the_same_page() {
        assert!(same_site("example.com/bill", "https://example.com/collect"));
        assert!(!same_path(
            "example.com/bill",
            "https://example.com/collect"
        ));
        assert!(!same_path(
            "example.com/help",
            "https://example.com/help/../collect"
        ));
        assert!(!same_path("/", "https://example.com/collect"));
        assert_eq!(path_of("javascript:alert(1)"), None);
        assert!(!same_path("example.com/a", "javascript:alert(1)"));
    }

    #[test]
    fn encoded_path_bytes_cannot_borrow_a_literal_path_ticket() {
        for (recorded, target) in [
            ("example.com/a%2Fb", "https://example.com/a/b"),
            ("example.com/%62ill", "https://example.com/bill"),
            ("example.com/a%2fb", "https://example.com/a%2Fb"),
            ("example.com/a/", "https://example.com/a"),
        ] {
            assert!(
                !same_destination(recorded, target),
                "{recorded} vs {target}"
            );
        }
        assert!(same_destination(
            "example.com/a%2Fb",
            "https://example.com/a%2Fb"
        ));
        assert_eq!(path_of("https://example.com/%2e%2e/collect"), None);
    }

    /// path 裡的 `@` 不是 userinfo。整串 `rsplit('@')` 會把 `/user@inbox` 切錯。
    #[test]
    fn at_sign_in_the_path_is_not_userinfo() {
        assert_eq!(
            host_of("https://example.com/user@inbox").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            path_of("https://example.com/user@inbox").as_deref(),
            Some("/user@inbox")
        );
        assert!(same_destination(
            "example.com/user@inbox",
            "https://example.com/user@inbox"
        ));
    }

    /// 壞掉的 percent-encoding 不可以 panic，也不能拿來當同一去處。
    #[test]
    fn malformed_percent_encoding_is_not_the_recorded_destination() {
        let weird = "https://example.com/bill?id=%漢";
        assert!(path_of(weird).is_none());
        assert!(!same_query_and_fragment("example.com/bill?id=7", weird));
        assert!(!same_destination("example.com/bill?id=7", weird));
        assert_eq!(percent_decode("%漢"), None);
        assert_eq!(percent_decode("%"), None);
        assert_eq!(percent_decode("%2"), None);
    }

    /// 記過的去處包含 query／fragment，參數的順序也是來源票的一部分。
    #[test]
    fn standing_grant_destination_includes_query_and_fragment() {
        assert!(same_destination(
            "example.com/bill?id=7",
            "https://www.example.com/bill?id=7"
        ));
        assert!(!same_destination(
            "example.com/a?b=c&id=7",
            "https://example.com/a?id=7&b=c"
        ));
        assert!(!same_destination(
            "example.com/bill?id=7",
            "https://example.com/bill"
        ));
        assert!(!same_destination(
            "example.com/bill?id=7",
            "https://example.com/bill?id=8"
        ));
        assert!(!same_destination(
            "example.com/help",
            "https://example.com/help?next=https://evil.example/collect"
        ));
        assert!(!same_destination(
            "example.com/help",
            "https://example.com/help?next=https%3A%2F%2Fevil.example%2Fcollect"
        ));
        assert!(!same_destination(
            "example.com/help",
            "https://example.com/help#section"
        ));
        assert!(same_path(
            "example.com/help",
            "https://example.com/help?next=https://evil.example/collect"
        ));
    }

    #[test]
    fn nested_url_in_query_does_not_supply_a_scheme_for_chromium_address() {
        let recorded = "example.com/help?next=https://dest.example/x";
        let target = "https://www.example.com/help?next=https://dest.example/x";
        assert_eq!(host_of(recorded).as_deref(), Some("example.com"));
        assert!(same_destination(recorded, target));
        assert!(!same_explicit_destination(recorded, target));
        assert!(!same_destination(
            recorded,
            "http://www.example.com/help?next=https://dest.example/x"
        ));
        assert!(!same_destination(
            recorded,
            "https://www.example.com/help?next=https://other.example/x"
        ));
        assert_eq!(host_of("ftp://example.com/help"), None);
    }

    #[test]
    fn http_scheme_is_recognized_before_multibyte_host_character() {
        let unicode = "http://é.example/a";
        let punycode = "http://xn--9ca.example/a";
        assert!(validate_url(unicode).is_ok());
        assert_eq!(host_of(unicode).as_deref(), Some("é.example"));
        assert!(same_destination(unicode, punycode));
        assert!(!same_destination(unicode, "https://xn--9ca.example/a"));
    }

    #[test]
    fn query_order_cannot_reverse_duplicate_authorization_values() {
        for (recorded, target) in [
            (
                "example.com/pay?next=trusted&next=evil",
                "https://example.com/pay?next=evil&next=trusted",
            ),
            (
                "example.com/pay?role=user&mode=read&role=admin",
                "https://example.com/pay?role=admin&mode=read&role=user",
            ),
            ("example.com/pay?a=1&b=2", "https://example.com/pay?b=2&a=1"),
        ] {
            assert!(
                !same_destination(recorded, target),
                "{recorded} vs {target}"
            );
        }
        assert!(same_destination(
            "example.com/pay?next=trusted&next=evil",
            "https://example.com/pay?next=trusted&next=evil"
        ));
    }

    #[test]
    fn empty_query_and_fragment_delimiters_are_distinct_destinations() {
        let forms = [
            "https://example.com/a",
            "https://example.com/a?",
            "https://example.com/a#",
            "https://example.com/a?#",
        ];
        for (recorded_index, recorded) in forms.into_iter().enumerate() {
            for (target_index, target) in forms.into_iter().enumerate() {
                assert_eq!(
                    same_destination(recorded, target),
                    recorded_index == target_index,
                    "recorded={recorded:?}, target={target:?}"
                );
            }
        }
        assert!(same_destination(
            "example.com/a?#",
            "https://www.example.com/a?#"
        ));
    }

    #[test]
    fn parsed_origin_never_grants_a_url_with_userinfo() {
        for url in [
            "https://user@example.com/a",
            "https://user:secret@example.com/a",
            "https://@example.com/a",
            "https://example.com@evil.example/a",
        ] {
            assert!(!same_destination(url, url), "{url}");
            assert!(!same_destination("https://example.com/a", url), "{url}");
            assert!(validate_url(url).is_err(), "{url}");
        }
        assert!(same_destination(
            "https://bücher.example/a",
            "https://xn--bcher-kva.example/a"
        ));
        assert!(!same_destination(
            "https://example.com/a?q=é",
            "https://example.com/a?q=%C3%A9"
        ));
        assert!(!same_destination(
            "https://example.com/a#é",
            "https://example.com/a#%C3%A9"
        ));
    }

    #[test]
    fn explicit_grant_does_not_lend_its_host_to_www() {
        assert!(!same_destination(
            "https://example.com/a",
            "https://www.example.com/a"
        ));
        assert!(!same_explicit_destination(
            "https://example.com/a",
            "https://www.example.com/a"
        ));
        assert!(same_explicit_destination(
            "https://EXAMPLE.com:443/a",
            "https://example.com/a"
        ));
    }

    /// 縮寫位址列只代表 HTTPS；scheme 與有效 port 都是授權去處的一部分。
    #[test]
    fn standing_grant_destination_includes_scheme_and_effective_port() {
        for (recorded, target, expected) in [
            ("example.com/pay", "https://www.example.com/pay", true),
            ("example.com/pay", "https://example.com:443/pay", true),
            ("example.com:8443/pay", "https://example.com:8443/pay", true),
            (
                "https://example.com:443/pay",
                "https://example.com/pay",
                true,
            ),
            ("http://example.com:80/pay", "http://example.com/pay", true),
            ("example.com/pay", "http://example.com/pay", false),
            ("https://example.com/pay", "http://example.com/pay", false),
            ("http://example.com/pay", "https://example.com/pay", false),
            (
                "https://example.com/pay",
                "https://example.com:8443/pay",
                false,
            ),
            ("example.com:8443/pay", "https://example.com/pay", false),
            (
                "https://example.com:80/pay",
                "http://example.com:80/pay",
                false,
            ),
            ("[::1]/pay", "https://[::1]:443/pay", true),
            ("https://[::1]:8443/pay", "https://[::1]/pay", false),
        ] {
            assert_eq!(
                same_destination(recorded, target),
                expected,
                "recorded={recorded:?}, target={target:?}"
            );
        }
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
