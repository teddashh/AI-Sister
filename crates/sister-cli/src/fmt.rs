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
