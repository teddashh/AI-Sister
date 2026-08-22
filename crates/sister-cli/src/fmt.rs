//! 終端輸出的格式化。
//!
//! 刻意樸素：這些輸出是給人核對用的證據，不是儀表板。

use chrono::{Local, TimeZone};
use sister_core::model::Millis;

/// 絕對時間，本地時區。
pub fn timestamp(ts: Millis) -> String {
    match Local.timestamp_millis_opt(ts).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => format!("ts:{ts}"),
    }
}

/// 相對於現在的口語說法（「3 天前」）。
pub fn relative(ts: Millis) -> String {
    let delta = sister_core::now_ms() - ts;
    if delta < 0 {
        return "未來".to_string();
    }
    let secs = delta / 1000;
    match secs {
        0..=59 => "剛剛".to_string(),
        60..=3599 => format!("{} 分鐘前", secs / 60),
        3600..=86_399 => format!("{} 小時前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

/// 一段時間有多長。
///
/// 進位到分鐘就不再顯示秒：「暫停了 3 小時 12 分 07 秒」裡那個秒數沒有人
/// 需要，只會讓真正重要的「3 小時」變得比較難讀。
pub fn duration_ms(ms: Millis) -> String {
    let secs = (ms / 1000).max(0);
    match secs {
        0..=59 => format!("{secs} 秒"),
        60..=3599 => format!("{} 分鐘", secs / 60),
        _ => {
            let (h, m) = (secs / 3600, (secs % 3600) / 60);
            if m == 0 {
                format!("{h} 小時")
            } else {
                format!("{h} 小時 {m} 分")
            }
        }
    }
}

/// 人類看得懂的位元組數。
pub fn bytes(n: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// 把 snippet 壓成單行，免得 OCR 的換行把版面撐爛。
pub fn one_line(s: &str, max_chars: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut out = String::new();
    let mut prev_space = false;
    for c in flat.chars() {
        let is_space = c == ' ';
        if !(is_space && prev_space) {
            out.push(c);
        }
        prev_space = is_space;
    }
    let out = out.trim().to_string();
    if out.chars().count() <= max_chars {
        return out;
    }
    let cut: String = out.chars().take(max_chars).collect();
    format!("{cut}…")
}

/// 補空白到 `width` 個**終端機欄位**，中文算兩格。
///
/// `{:<24}` 補的是位元組，而一個中文字是三個位元組、佔兩欄——所以任何一張
/// 有中文的表用 `{:<}` 對齊，出來的會是鋸齒狀。
///
/// 判斷用一條粗線（CJK 區段 + 全形標點）而不是引 `unicode-width`：這個 crate
/// 的相依樹是 `scripts/check-no-network.sh` 盯著的資產之一，而這裡對齊錯一欄
/// 的代價是「有點醜」。
pub fn pad(s: &str, width: usize) -> String {
    let used = display_width(s);
    let mut out = s.to_string();
    for _ in used..width {
        out.push(' ');
    }
    out
}

/// 一段字在終端機上佔的欄數；和 [`pad`] 使用完全相同的寬字規則。
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F      // 韓文字母
        | 0x2E80..=0x303E    // CJK 部首、注音、全形標點
        | 0x3041..=0x33FF    // 假名、韓文、CJK 相容
        | 0x3400..=0x4DBF    // CJK 擴充 A
        | 0x4E00..=0x9FFF    // CJK 基本區
        | 0xA000..=0xA4CF    // 彝文
        | 0xAC00..=0xD7A3    // 韓文音節
        | 0xF900..=0xFAFF    // CJK 相容表意
        | 0xFE30..=0xFE6F    // CJK 相容形式
        | 0xFF00..=0xFF60    // 全形 ASCII
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD  // CJK 擴充 B 以後
    )
}

/// 出處那一行：app · 視窗標題。
pub fn context_line(app: Option<&str>, title: Option<&str>) -> String {
    match (app, title) {
        (Some(a), Some(t)) => format!("{a} · {}", one_line(t, 60)),
        (Some(a), None) => a.to_string(),
        (None, Some(t)) => one_line(t, 60),
        (None, None) => "(未知來源)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_down_to_the_unit_that_matters() {
        assert_eq!(duration_ms(0), "0 秒");
        assert_eq!(duration_ms(59_999), "59 秒");
        assert_eq!(duration_ms(60_000), "1 分鐘");
        assert_eq!(duration_ms(3_599_000), "59 分鐘");
        assert_eq!(duration_ms(3_600_000), "1 小時");
        assert_eq!(duration_ms(3_600_000 + 12 * 60_000 + 7_000), "1 小時 12 分");
        // 負數不該印出「-1 秒」這種東西。時戳倒退是資料壞了，不是一段負的時間。
        assert_eq!(duration_ms(-5_000), "0 秒");
    }

    #[test]
    fn bytes_scales() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1024 * 1024 * 3), "3.0 MB");
        assert_eq!(bytes(1024_i64.pow(4)), "1.0 TB");
    }

    #[test]
    fn one_line_collapses_and_truncates() {
        assert_eq!(one_line("a\nb\t c", 100), "a b c");
        assert_eq!(one_line("  padded  ", 100), "padded");
        assert_eq!(one_line("abcdefghij", 5), "abcde…");
        // 依字元而非位元組截斷，不能砍壞 UTF-8
        assert_eq!(one_line("本期應繳金額", 3), "本期應…");
    }

    #[test]
    fn relative_reads_naturally() {
        let now = sister_core::now_ms();
        assert_eq!(relative(now), "剛剛");
        assert_eq!(relative(now - 120_000), "2 分鐘前");
        assert_eq!(relative(now - 3 * 3_600_000), "3 小時前");
        assert_eq!(relative(now - 3 * 86_400_000), "3 天前");
    }

    #[test]
    fn context_line_degrades_gracefully() {
        assert_eq!(
            context_line(Some("chrome.exe"), Some("帳單")),
            "chrome.exe · 帳單"
        );
        assert_eq!(context_line(Some("chrome.exe"), None), "chrome.exe");
        assert_eq!(context_line(None, Some("帳單")), "帳單");
        assert_eq!(context_line(None, None), "(未知來源)");
    }

    #[test]
    fn timestamp_survives_nonsense_input() {
        assert!(timestamp(0).starts_with("19") || timestamp(0).starts_with("20"));
        assert_eq!(timestamp(i64::MAX), format!("ts:{}", i64::MAX));
    }
}
