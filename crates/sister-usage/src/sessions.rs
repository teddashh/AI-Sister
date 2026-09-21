//! Bounded local session JSONL readers for Codex and Claude.
//!
//! The caller supplies an explicit absolute directory. This module never expands
//! `~`, never searches `$HOME`, and never opens auth/credential files. Remaining
//! token quota is not inferred from spent tokens.

use crate::model::{
    LocalAdapterKind, LocalProductUsage, LocalUsageReport, Measured, ProductId, QuotaSnapshot,
    UsageAmount,
};
use crate::{Error, Result};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const MAX_JSONL_FILES: usize = 64;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 256 * 1024;
pub const MAX_WALK_DEPTH: u32 = 4;
pub const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;

const AUTH_NAMES: &[&str] = &[
    "auth.json",
    "auth.json.lock",
    "credentials.json",
    "credentials",
    "id_token",
    "access_token",
    "key.json",
    ".credentials.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalReadRequest<'a> {
    pub enabled: bool,
    pub root: Option<&'a Path>,
}

pub fn read_sessions(request: LocalReadRequest<'_>) -> LocalUsageReport {
    if !request.enabled {
        return LocalUsageReport::disabled();
    }
    let Some(root) = request.root.filter(|path| !path.as_os_str().is_empty()) else {
        return LocalUsageReport::error(true, "已開啟本機用量，但還沒指定 session JSONL 目錄。");
    };
    match read_configured_root(root) {
        Ok(report) => report,
        Err(error) => LocalUsageReport::error(true, &error.to_string()),
    }
}

fn read_configured_root(root: &Path) -> Result<LocalUsageReport> {
    if !root.is_absolute() {
        return Err(Error::LocalPathNotAbsolute);
    }
    let canonical = fs::canonicalize(root).map_err(|_| Error::LocalPathMissing)?;
    if !canonical.is_dir() {
        return Err(Error::LocalPathMissing);
    }
    let mut skipped_auth = 0_u32;
    let mut files = Vec::new();
    collect_jsonl(&canonical, &canonical, 0, &mut files, &mut skipped_auth)?;
    files.sort();
    files.truncate(MAX_JSONL_FILES);

    let mut total_bytes = 0_u64;
    let mut files_read = 0_u32;
    let mut codex = ProductAcc::new(ProductId::Codex, "codex-session-jsonl");
    let mut claude = ProductAcc::new(ProductId::Claude, "claude-session-jsonl");

    for path in &files {
        let meta = fs::metadata(path).map_err(|_| Error::LocalIo)?;
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        total_bytes = total_bytes.saturating_add(meta.len());
        if total_bytes > MAX_TOTAL_BYTES {
            break;
        }
        parse_jsonl_file(path, &mut codex, &mut claude)?;
        files_read += 1;
        codex.flush_file();
        claude.flush_file();
    }

    let mut products = Vec::new();
    if let Some(row) = codex.finish() {
        products.push(row);
    }
    if let Some(row) = claude.finish() {
        products.push(row);
    }

    Ok(LocalUsageReport {
        enabled: true,
        configured: true,
        products,
        skipped_auth_files: skipped_auth,
        files_read,
        error: None,
        adapter: LocalAdapterKind::ConfiguredSessions,
    })
}

fn collect_jsonl(
    _root: &Path,
    dir: &Path,
    depth: u32,
    files: &mut Vec<PathBuf>,
    skipped_auth: &mut u32,
) -> Result<()> {
    if depth > MAX_WALK_DEPTH || files.len() >= MAX_JSONL_FILES {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|_| Error::LocalIo)?;
    for entry in entries {
        let entry = entry.map_err(|_| Error::LocalIo)?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if is_auth_name(&name) {
            *skipped_auth += 1;
            continue;
        }
        let file_type = entry.file_type().map_err(|_| Error::LocalIo)?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_jsonl(_root, &path, depth + 1, files, skipped_auth)?;
            continue;
        }
        if !name.ends_with(".jsonl") {
            continue;
        }
        if files.len() >= MAX_JSONL_FILES {
            break;
        }
        files.push(path);
    }
    Ok(())
}

fn is_auth_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    AUTH_NAMES.iter().any(|denied| *denied == lower)
        || lower.contains("auth")
        || lower.contains("credential")
        || lower.ends_with(".token")
}

fn parse_jsonl_file(path: &Path, codex: &mut ProductAcc, claude: &mut ProductAcc) -> Result<()> {
    let file = File::open(path).map_err(|_| Error::LocalIo)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|_| Error::LocalIo)?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        ingest_line(&value, codex, claude);
    }
    Ok(())
}

fn ingest_line(value: &Value, codex: &mut ProductAcc, claude: &mut ProductAcc) {
    if ingest_codex(value, codex) {
        return;
    }
    let _ = ingest_claude(value, claude);
}

fn ingest_codex(value: &Value, acc: &mut ProductAcc) -> bool {
    let type_name = value.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = if type_name == "event_msg" {
        value.get("payload")
    } else {
        Some(value)
    };
    let Some(payload) = payload else {
        return false;
    };
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return false;
    }
    let observed_at = parse_time(value.get("timestamp"));
    // Official TokenCountEvent: info is optional. Rate-only events re-emit
    // last_token_usage if info is present from a previous merge; we only trust
    // total_token_usage as the cumulative session total, and never add last.
    if let Some(info) = payload.get("info").filter(|info| !info.is_null()) {
        if let Some(total) = token_total(info.get("total_token_usage")) {
            acc.set_file_total(total, observed_at);
        } else if let Some(last) = token_total(info.get("last_token_usage")) {
            acc.set_file_last_fallback(last, observed_at);
        }
        // model_context_window is context size, not account remaining quota.
    }
    let limits = payload.get("rate_limits");
    if let Some(limits) = limits.filter(|value| !value.is_null())
        && let Some(primary) = limits.get("primary")
    {
        acc.set_quota(quota_from_window(
            primary,
            limits.get("plan_type").and_then(Value::as_str),
            observed_at,
            "codex-token_count.rate_limits.primary",
        ));
    }
    true
}

fn ingest_claude(value: &Value, acc: &mut ProductAcc) -> bool {
    let type_name = value.get("type").and_then(Value::as_str).unwrap_or("");
    let observed_at = parse_time(value.get("timestamp")).or_else(|| parse_time(value.get("ts")));
    if type_name == "assistant"
        && let Some(usage) = value
            .pointer("/message/usage")
            .or_else(|| value.get("usage"))
    {
        acc.add_tokens(claude_tokens(usage), observed_at);
        return true;
    }
    if type_name == "rate_limit_event"
        || value.get("subtype").and_then(Value::as_str) == Some("rate_limit_event")
    {
        let window = value
            .get("rate_limit")
            .or_else(|| value.get("payload"))
            .unwrap_or(value);
        acc.set_quota(quota_from_window(
            window,
            None,
            observed_at,
            "claude-rate_limit_event",
        ));
        return true;
    }
    false
}

fn token_total(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    // Official TokenUsage.total_tokens already includes the parts. Cache is a
    // subset of input and reasoning is a subset of output — never add them.
    as_nonneg_int(value.get("total_tokens"))
}

fn claude_tokens(usage: &Value) -> Option<u64> {
    // Claude reports uncached input separately from cache_*; those are extra.
    let input = as_nonneg_int(usage.get("input_tokens")).unwrap_or(0);
    let output = as_nonneg_int(usage.get("output_tokens")).unwrap_or(0);
    let cache_read = as_nonneg_int(usage.get("cache_read_input_tokens")).unwrap_or(0);
    let cache_create = as_nonneg_int(usage.get("cache_creation_input_tokens")).unwrap_or(0);
    let sum = input
        .saturating_add(output)
        .saturating_add(cache_read)
        .saturating_add(cache_create);
    (sum > 0).then_some(sum)
}

fn as_nonneg_int(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    let n = value.as_i64()?;
    (n >= 0).then_some(n as u64)
}

fn quota_from_window(
    window: &Value,
    plan_type: Option<&str>,
    observed_at_unix_ms: Option<i64>,
    source: &'static str,
) -> Option<QuotaSnapshot> {
    let used = window.get("used_percent").and_then(Value::as_f64)?;
    // Official RateLimitWindow.used_percent is 0..=100. Do not scale 0..=1.
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    let window_minutes = match window.get("window_minutes") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let minutes = value.as_i64()?;
            if minutes <= 0 {
                return None;
            }
            Some(minutes)
        }
    };
    Some(QuotaSnapshot {
        used_percent: used,
        window_minutes,
        resets_at_unix: window
            .get("resets_at")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0),
        plan_type: plan_type.map(str::to_owned).or_else(|| {
            window
                .get("plan_type")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        observed_at_unix_ms,
        source,
    })
}

fn parse_time(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(ms) = value.as_i64() {
        return Some(ms);
    }
    let text = value.as_str()?;
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|when| when.timestamp_millis())
}

struct ProductAcc {
    product: ProductId,
    provenance: &'static str,
    /// Sum of per-file cumulative totals. Never a sum of in-file snapshots.
    across_files: u64,
    file_total: Option<u64>,
    file_last_fallback: Option<u64>,
    observed_at: Option<i64>,
    quota: Option<QuotaSnapshot>,
    saw_event: bool,
}

impl ProductAcc {
    fn new(product: ProductId, provenance: &'static str) -> Self {
        Self {
            product,
            provenance,
            across_files: 0,
            file_total: None,
            file_last_fallback: None,
            observed_at: None,
            quota: None,
            saw_event: false,
        }
    }

    fn add_tokens(&mut self, tokens: Option<u64>, at: Option<i64>) {
        let Some(tokens) = tokens else {
            return;
        };
        self.saw_event = true;
        self.across_files = self.across_files.saturating_add(tokens);
        self.observed_at = at.or(self.observed_at);
    }

    fn set_file_total(&mut self, tokens: u64, at: Option<i64>) {
        self.saw_event = true;
        self.file_total = Some(tokens);
        self.observed_at = at.or(self.observed_at);
    }

    fn set_file_last_fallback(&mut self, tokens: u64, at: Option<i64>) {
        if self.file_total.is_some() {
            return;
        }
        self.saw_event = true;
        self.file_last_fallback = Some(tokens);
        self.observed_at = at.or(self.observed_at);
    }

    fn flush_file(&mut self) {
        if let Some(total) = self.file_total.or(self.file_last_fallback) {
            self.across_files = self.across_files.saturating_add(total);
        }
        self.file_total = None;
        self.file_last_fallback = None;
    }

    fn set_quota(&mut self, quota: Option<QuotaSnapshot>) {
        let Some(quota) = quota else {
            return;
        };
        self.saw_event = true;
        self.quota = Some(quota);
    }

    fn finish(mut self) -> Option<LocalProductUsage> {
        self.flush_file();
        if !self.saw_event {
            return None;
        }
        Some(LocalProductUsage {
            product: self.product,
            observed_tokens: if self.across_files > 0 {
                Measured::Observed(UsageAmount::new(self.across_files))
            } else {
                Measured::Unknown
            },
            remaining_tokens: Measured::Unknown,
            quota: match self.quota {
                Some(snapshot) => Measured::Observed(snapshot),
                None => Measured::Unknown,
            },
            observed_at_unix_ms: self.observed_at,
            adapter: LocalAdapterKind::ConfiguredSessions,
            provenance: self.provenance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-usage-sessions-{}-{}-{}",
            std::process::id(),
            nanos,
            n
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    fn codex_event(total: u64, last: u64, percent: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-09-12T03:20:36.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5,"reasoning_output_tokens":0,"total_tokens":{total}}},"last_token_usage":{{"total_tokens":{last}}},"model_context_window":258400}},"rate_limits":{{"plan_type":"plus","primary":{{"used_percent":{percent},"window_minutes":300,"resets_at":1783800000}}}}}}}}"#
        )
    }

    const CLAUDE_ASSISTANT: &str = r#"{"type":"assistant","timestamp":"2026-09-12T04:00:00.000Z","message":{"usage":{"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":2,"cache_creation_input_tokens":0}}}"#;
    const CLAUDE_LIMIT: &str = r#"{"type":"rate_limit_event","timestamp":"2026-09-12T04:00:01.000Z","used_percent":40,"window_minutes":300,"resets_at":1783800000}"#;

    #[test]
    fn disabled_and_missing_dir_are_unknown_not_zero() {
        let off = read_sessions(LocalReadRequest {
            enabled: false,
            root: None,
        });
        assert!(!off.enabled);
        assert!(off.products.is_empty());

        let missing = read_sessions(LocalReadRequest {
            enabled: true,
            root: None,
        });
        assert!(missing.error.is_some());
        assert!(missing.products.is_empty());
    }

    #[test]
    fn relative_path_is_rejected() {
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(Path::new("sessions")),
        });
        assert!(report.error.unwrap().contains("絕對"));
    }

    #[test]
    fn codex_jsonl_records_observed_tokens_and_quota_snapshot_without_inventing_remaining() {
        let dir = scratch();
        write(
            &dir,
            "rollout-2026-09-12T00-00-00-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl",
            &format!("{}\n", codex_event(125, 55, "12")),
        );
        write(&dir, "auth.json", r#"{"token":"secret-must-not-be-read"}"#);
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        assert_eq!(report.skipped_auth_files, 1);
        assert_eq!(report.files_read, 1);
        let row = report
            .products
            .iter()
            .find(|row| row.product == ProductId::Codex)
            .unwrap();
        assert_eq!(
            row.observed_tokens,
            Measured::Observed(UsageAmount::new(125)),
            "use cumulative total_tokens, not last and not a subset sum"
        );
        assert!(row.remaining_tokens.is_unknown());
        let quota = match &row.quota {
            Measured::Observed(snapshot) => snapshot,
            other => panic!("quota should be observed snapshot, got {other:?}"),
        };
        assert!((quota.used_percent - 12.0).abs() < f64::EPSILON);
        assert_eq!(quota.source, "codex-token_count.rate_limits.primary");
        let auth = fs::read_to_string(dir.join("auth.json")).unwrap();
        assert!(auth.contains("secret-must-not-be-read"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn spent_tokens_without_rate_limit_leave_quota_and_remaining_unknown() {
        let dir = scratch();
        write(
            &dir,
            "session.jsonl",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":99}}}}
"#,
        );
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        let row = &report.products[0];
        assert_eq!(
            row.observed_tokens,
            Measured::Observed(UsageAmount::new(99))
        );
        assert!(row.remaining_tokens.is_unknown());
        assert!(row.quota.is_unknown());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claude_jsonl_sums_assistant_usage_and_keeps_quota_snapshot_separate() {
        let dir = scratch();
        write(
            &dir,
            "claude-session.jsonl",
            &format!("{CLAUDE_ASSISTANT}\n{CLAUDE_LIMIT}\n"),
        );
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        let row = report
            .products
            .iter()
            .find(|row| row.product == ProductId::Claude)
            .unwrap();
        assert_eq!(
            row.observed_tokens,
            Measured::Observed(UsageAmount::new(20))
        );
        assert!(row.remaining_tokens.is_unknown());
        let quota = match &row.quota {
            Measured::Observed(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        assert!((quota.used_percent - 40.0).abs() < f64::EPSILON);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cumulative_updates_and_rate_only_events_do_not_double_count() {
        let dir = scratch();
        write(
            &dir,
            "one.jsonl",
            &format!(
                "{}\n{}\n{}\n",
                codex_event(100, 40, "10"),
                codex_event(180, 80, "11"),
                r#"{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":15.5,"window_minutes":300,"resets_at":1783800000}}}}"#
            ),
        );
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        let row = &report.products[0];
        assert_eq!(
            row.observed_tokens,
            Measured::Observed(UsageAmount::new(180))
        );
        let quota = match &row.quota {
            Measured::Observed(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        assert!((quota.used_percent - 15.5).abs() < f64::EPSILON);
        assert!(row.remaining_tokens.is_unknown());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multiple_session_files_sum_latest_totals_and_reread_is_not_a_double_count() {
        let dir = scratch();
        write(&dir, "a.jsonl", &format!("{}\n", codex_event(100, 10, "8")));
        write(&dir, "b.jsonl", &format!("{}\n", codex_event(40, 40, "9")));
        let first = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        let second = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        assert_eq!(
            first.products[0].observed_tokens,
            Measured::Observed(UsageAmount::new(140))
        );
        assert_eq!(
            second.products[0].observed_tokens,
            first.products[0].observed_tokens
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_line_and_out_of_range_percent_are_ignored() {
        let dir = scratch();
        write(
            &dir,
            "partial.jsonl",
            &format!(
                "{}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\"\n{}\n",
                codex_event(70, 10, "101"),
                r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":12,"window_minutes":300}}}}"#
            ),
        );
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        let row = &report.products[0];
        assert_eq!(
            row.observed_tokens,
            Measured::Observed(UsageAmount::new(70))
        );
        let quota = match &row.quota {
            Measured::Observed(snapshot) => snapshot,
            other => panic!("{other:?}"),
        };
        assert!((quota.used_percent - 12.0).abs() < f64::EPSILON);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn context_window_and_elapsed_reset_do_not_fill_remaining() {
        let dir = scratch();
        write(
            &dir,
            "ctx.jsonl",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":10},"last_token_usage":{"total_tokens":10},"model_context_window":258400},"rate_limits":{"primary":{"used_percent":3,"window_minutes":300,"resets_at":1}}}}
"#,
        );
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        let row = &report.products[0];
        assert_eq!(
            row.observed_tokens,
            Measured::Observed(UsageAmount::new(10))
        );
        assert!(row.remaining_tokens.is_unknown());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_lines_are_skipped_and_empty_dir_is_unknown() {
        let dir = scratch();
        write(&dir, "broken.jsonl", "not-json\n{]\n");
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        assert_eq!(report.files_read, 1);
        assert!(report.products.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn auth_named_jsonl_is_not_opened() {
        let dir = scratch();
        write(
            &dir,
            "auth.jsonl",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":1}}}}"#,
        );
        let report = read_sessions(LocalReadRequest {
            enabled: true,
            root: Some(&dir),
        });
        assert_eq!(report.skipped_auth_files, 1);
        assert_eq!(report.files_read, 0);
        assert!(report.products.is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
