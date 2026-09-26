//! 遠端收尾通報的資料邊界。
//!
//! 這個 crate 刻意不知道 `watch` 的問題、畫面文字、app、網址、檔案路徑或記憶 ID。
//! 呼叫端只能交進 typed outcome、計數、時間與退出碼，因此 transport 沒有一條 API
//! 能把未授權文字順手塞進 payload。預設 feature 沒有 HTTP client；只有出貨 CLI
//! 明確啟用 `discord` 才能建立 Discord POST。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[cfg(feature = "discord")]
mod native;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchOutcome {
    ConditionObserved,
    Deadline,
    ScreenQuiet,
    BudgetExhausted,
    ConsentRevoked,
    MasterStopped,
}

impl WatchOutcome {
    pub const fn status_summary(self) -> &'static str {
        match self {
            Self::ConditionObserved => "監控條件已出現。",
            Self::Deadline => "監控時間已到。",
            Self::ScreenQuiet => "畫面長時間沒有新的文字。",
            Self::BudgetExhausted => "監控因外送預算用完而停止。",
            Self::ConsentRevoked => "監控因同意撤回而停止。",
            Self::MasterStopped => "監控因全停而停止；遠端通報不會送出。",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchCounts {
    pub answered: usize,
    pub unanswered: usize,
    pub not_sent: usize,
    pub no_new_screen_text: usize,
}

/// 唯一能離機的 `watch` 報告。
///
/// 欄位刻意封閉；沒有 `metadata`、`message`、`details` 或任意 JSON escape hatch。
/// `status_summary` 由 outcome 在這個 crate 內算，不收 caller 的字串。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchReport {
    pub schema_version: u32,
    pub source: ReportSource,
    pub outcome: WatchOutcome,
    pub status_summary: &'static str,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    /// `None` 代表結束時鐘早於開始時鐘；不能拿 `0` 冒充量到零毫秒。
    pub duration_ms: Option<u64>,
    pub exit_code: i32,
    pub counts: WatchCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportSource {
    Watch,
}

impl WatchReport {
    pub fn new(
        outcome: WatchOutcome,
        started_at_ms: i64,
        ended_at_ms: i64,
        exit_code: i32,
        counts: WatchCounts,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            source: ReportSource::Watch,
            outcome,
            status_summary: outcome.status_summary(),
            started_at_ms,
            ended_at_ms,
            duration_ms: ended_at_ms
                .checked_sub(started_at_ms)
                .and_then(|span| u64::try_from(span).ok()),
            exit_code,
            counts,
        }
    }

    pub fn discord_body(&self) -> DiscordBody<'_> {
        DiscordBody {
            content: DiscordContent(self),
            allowed_mentions: AllowedMentions { parse: [] },
        }
    }
}

/// Discord 的 `content` 只從 typed report 組出來，沒有 caller 字串。
#[derive(Debug, Clone, Copy)]
pub struct DiscordContent<'a>(&'a WatchReport);

impl Serialize for DiscordContent<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let report = self.0;
        serializer.serialize_str(&format!(
            "AI-Sister watch：{} outcome={:?} duration_ms={} exit_code={} answered={} unanswered={} not_sent={} no_new_screen_text={}",
            report.status_summary,
            report.outcome,
            report
                .duration_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            report.exit_code,
            report.counts.answered,
            report.counts.unanswered,
            report.counts.not_sent,
            report.counts.no_new_screen_text,
        ))
    }
}

#[derive(Debug, Serialize)]
pub struct DiscordBody<'a> {
    content: DiscordContent<'a>,
    allowed_mentions: AllowedMentions,
}

#[derive(Debug, Serialize)]
struct AllowedMentions {
    parse: [String; 0],
}

/// 以 JSONL 追加。每一場一列，不覆寫前一場，也不把同一個 `0` 同時拿來表示
/// 「沒有事件」和「檔案沒打開」。append、serialize、flush、sync 各自都能失敗。
pub fn append_json_report(path: &Path, report: &WatchReport) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("建立 JSON 回報目錄失敗：{}", parent.display()))?;
    }
    let mut row = serde_json::to_vec(report).context("序列化 JSON 回報失敗")?;
    row.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("開啟 JSON 回報失敗：{}", path.display()))?;
    fs4::FileExt::lock(&file)
        .with_context(|| format!("取得 JSON 回報寫鎖失敗：{}", path.display()))?;
    file.write_all(&row).context("寫入 JSON 回報失敗")?;
    file.flush().context("flush JSON 回報失敗")?;
    file.sync_data().context("同步 JSON 回報失敗")?;
    fs4::FileExt::unlock(&file).context("釋放 JSON 回報寫鎖失敗")?;
    Ok(())
}

#[cfg(feature = "discord")]
pub use native::{DeliveryError, DiscordClient, DiscordWebhook};

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> WatchReport {
        WatchReport::new(
            WatchOutcome::ScreenQuiet,
            1_000,
            9_500,
            0,
            WatchCounts {
                answered: 2,
                unanswered: 1,
                not_sent: 3,
                no_new_screen_text: 4,
            },
        )
    }

    #[test]
    fn json_schema_has_no_arbitrary_text_escape_hatch() {
        let value = serde_json::to_value(report()).unwrap();
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["outcome"], "screen_quiet");
        assert_eq!(value["duration_ms"], 8_500);
        let object = value.as_object().unwrap();
        let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "counts",
                "duration_ms",
                "ended_at_ms",
                "exit_code",
                "outcome",
                "schema_version",
                "source",
                "started_at_ms",
                "status_summary",
            ]
        );
        let raw = value.to_string();
        for forbidden in [
            "private-question",
            "private-screen-text",
            "private-app",
            "https://private.example",
            "C:\\\\private",
            "memory:123",
        ] {
            assert!(!raw.contains(forbidden), "unexpected field or value: {raw}");
        }
    }

    #[test]
    fn discord_body_disables_mentions_and_contains_only_typed_summary() {
        let raw = serde_json::to_string(&report().discord_body()).unwrap();
        assert!(raw.contains(r#""allowed_mentions":{"parse":[]}"#), "{raw}");
        assert!(raw.contains("duration_ms=8500"), "{raw}");
        assert!(!raw.contains("@everyone"), "{raw}");
        assert!(!raw.contains("private-question"), "{raw}");
    }

    #[test]
    fn a_backwards_clock_is_unknown_duration_not_a_fake_zero() {
        let report = WatchReport::new(
            WatchOutcome::Deadline,
            9_500,
            1_000,
            0,
            WatchCounts::default(),
        );
        assert_eq!(report.duration_ms, None);
        assert!(
            serde_json::to_string(&report.discord_body())
                .unwrap()
                .contains("duration_ms=unknown")
        );
    }

    #[test]
    fn json_report_appends_one_complete_line_per_run() {
        let dir = std::env::temp_dir().join(format!(
            "sister-notify-json-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("reports.jsonl");
        append_json_report(&path, &report()).unwrap();
        append_json_report(&path, &report()).unwrap();
        let raw = std::fs::read_to_string(path).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(parsed, serde_json::to_value(report()).unwrap());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_writers_leave_whole_json_lines() {
        let dir =
            std::env::temp_dir().join(format!("sister-notify-concurrent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("reports.jsonl");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let path = path.clone();
                scope.spawn(move || append_json_report(&path, &report()).unwrap());
            }
        });
        let raw = std::fs::read_to_string(path).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 8);
        assert!(
            lines
                .iter()
                .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
