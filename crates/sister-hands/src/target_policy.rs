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

#[cfg(test)]
mod tests {
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
    }
}
