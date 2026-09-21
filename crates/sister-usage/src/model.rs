//! Typed quantities that must not collapse into a fake zero.
//!
//! Public LimitReset events are global product announcements. They never fill
//! [`Measured::Observed`] remaining quota for an account.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const SOURCE_NAME: &str = "LimitReset";
pub const SOURCE_URL: &str = "https://limitreset.net/";
pub const SOURCE_LICENSE: &str = "CC BY 4.0";
pub const SOURCE_ATTRIBUTION: &str = "LimitReset（limitreset.net），CC BY 4.0";

pub const HOST: &str = "limitreset.net";
pub const STATUS_PATH: &str = "/api/v1/status";
pub const STATUS_URL: &str = "https://limitreset.net/api/v1/status";
pub const USER_AGENT: &str = "AI-Sister/1 sister-usage";
pub const ACCEPT: &str = "application/json";
pub const ACCEPT_ENCODING: &str = "identity";

pub const MAX_BODY_BYTES: usize = 256 * 1024;
pub const MIN_GET_INTERVAL_MS: i64 = 60_000;
pub const AUTO_POLL_INTERVAL_MS: i64 = 300_000;

pub const RESOLVE_TIMEOUT_SECS: u64 = 5;
pub const CONNECT_TIMEOUT_SECS: u64 = 10;
pub const SEND_TIMEOUT_SECS: u64 = 10;
pub const RECEIVE_TIMEOUT_SECS: u64 = 15;
pub const GLOBAL_TIMEOUT_SECS: u64 = 20;

/// Allowlisted LimitReset product identifiers. Paths are built only from these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProductId {
    Codex,
    Claude,
    Chatgpt,
    Cursor,
    Gemini,
    Copilot,
    Grok,
}

impl ProductId {
    pub const ALL: [Self; 7] = [
        Self::Codex,
        Self::Claude,
        Self::Chatgpt,
        Self::Cursor,
        Self::Gemini,
        Self::Copilot,
        Self::Grok,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Chatgpt => "chatgpt",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Grok => "grok",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Chatgpt => "ChatGPT",
            Self::Cursor => "Cursor",
            Self::Gemini => "Gemini",
            Self::Copilot => "Copilot",
            Self::Grok => "Grok",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|product| product.as_str() == value)
    }

    pub const fn latest_path(self) -> &'static str {
        match self {
            Self::Codex => "/api/v1/codex/latest",
            Self::Claude => "/api/v1/claude/latest",
            Self::Chatgpt => "/api/v1/chatgpt/latest",
            Self::Cursor => "/api/v1/cursor/latest",
            Self::Gemini => "/api/v1/gemini/latest",
            Self::Copilot => "/api/v1/copilot/latest",
            Self::Grok => "/api/v1/grok/latest",
        }
    }

    pub const fn latest_url(self) -> &'static str {
        match self {
            Self::Codex => "https://limitreset.net/api/v1/codex/latest",
            Self::Claude => "https://limitreset.net/api/v1/claude/latest",
            Self::Chatgpt => "https://limitreset.net/api/v1/chatgpt/latest",
            Self::Cursor => "https://limitreset.net/api/v1/cursor/latest",
            Self::Gemini => "https://limitreset.net/api/v1/gemini/latest",
            Self::Copilot => "https://limitreset.net/api/v1/copilot/latest",
            Self::Grok => "https://limitreset.net/api/v1/grok/latest",
        }
    }
}

impl fmt::Display for ProductId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Exact GET this crate may perform. Callers cannot supply a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicEndpoint {
    Status,
    Latest(ProductId),
}

impl PublicEndpoint {
    pub const fn method(self) -> &'static str {
        "GET"
    }

    pub const fn host(self) -> &'static str {
        HOST
    }

    pub const fn path(self) -> &'static str {
        match self {
            Self::Status => STATUS_PATH,
            Self::Latest(product) => product.latest_path(),
        }
    }

    pub const fn url(self) -> &'static str {
        match self {
            Self::Status => STATUS_URL,
            Self::Latest(product) => product.latest_url(),
        }
    }
}

/// A measured quantity. Missing measurement is [`Self::Unknown`], never a silent 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measured<T> {
    Unknown,
    Observed(T),
}

impl<T> Measured<T> {
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// Counted usage that was actually read from a local adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageAmount(u64);

impl UsageAmount {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAdapterKind {
    /// No local session/auth reader is wired. This is not a measured zero.
    Unavailable,
    /// Explicit configured session directory. Never the implicit home path.
    ConfiguredSessions,
    /// Test-only fixture. Production desktop uses configured sessions or unavailable.
    SyntheticFixture,
}

/// Rate-limit snapshot copied from a session event. This is not remaining tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSnapshot {
    /// Official Codex `RateLimitWindow.used_percent` is 0..=100, not remaining tokens.
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at_unix: Option<i64>,
    pub plan_type: Option<String>,
    pub observed_at_unix_ms: Option<i64>,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalProductUsage {
    pub product: ProductId,
    pub observed_tokens: Measured<UsageAmount>,
    pub remaining_tokens: Measured<UsageAmount>,
    pub quota: Measured<QuotaSnapshot>,
    pub observed_at_unix_ms: Option<i64>,
    pub adapter: LocalAdapterKind,
    pub provenance: &'static str,
}

impl LocalProductUsage {
    pub fn as_local_usage(&self) -> LocalUsage {
        LocalUsage {
            product: Some(self.product),
            used: self.observed_tokens,
            remaining: self.remaining_tokens,
            adapter: self.adapter,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalUsageReport {
    pub enabled: bool,
    pub configured: bool,
    pub products: Vec<LocalProductUsage>,
    pub skipped_auth_files: u32,
    pub files_read: u32,
    pub error: Option<String>,
    pub adapter: LocalAdapterKind,
}

impl LocalUsageReport {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            configured: false,
            products: Vec::new(),
            skipped_auth_files: 0,
            files_read: 0,
            error: None,
            adapter: LocalAdapterKind::Unavailable,
        }
    }

    pub fn error(enabled: bool, message: &str) -> Self {
        Self {
            enabled,
            configured: false,
            products: Vec::new(),
            skipped_auth_files: 0,
            files_read: 0,
            error: Some(message.to_owned()),
            adapter: LocalAdapterKind::Unavailable,
        }
    }

    pub fn primary(&self) -> LocalUsage {
        self.products
            .first()
            .map(LocalProductUsage::as_local_usage)
            .unwrap_or_else(LocalUsage::unavailable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUsage {
    pub product: Option<ProductId>,
    pub used: Measured<UsageAmount>,
    pub remaining: Measured<UsageAmount>,
    pub adapter: LocalAdapterKind,
}

impl LocalUsage {
    pub const fn unavailable() -> Self {
        Self {
            product: None,
            used: Measured::Unknown,
            remaining: Measured::Unknown,
            adapter: LocalAdapterKind::Unavailable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedReset {
    pub event_id: String,
    pub announced_at: String,
    pub announced_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PublicReset {
    NoneRecorded,
    Unverified {
        event_id: String,
        announced_at: String,
    },
    OtherKind {
        event_id: String,
        kind: String,
        announced_at: String,
    },
    Confirmed(ConfirmedReset),
}

impl PublicReset {
    pub fn confirmed(&self) -> Option<&ConfirmedReset> {
        match self {
            Self::Confirmed(event) => Some(event),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    pub p24: f64,
    pub p48: f64,
    pub basis: String,
    pub days_since_last: Option<f64>,
    pub computed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublicProductStatus {
    pub product: ProductId,
    pub reset: PublicReset,
    /// Count of public events on the board. `None` = the field was absent, not 0.
    pub public_event_count: Option<u64>,
    pub forecast: Option<Forecast>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublicBoard {
    pub updated_at: String,
    pub products: Vec<PublicProductStatus>,
    pub attribution: &'static str,
}

impl PublicBoard {
    pub fn product(&self, id: ProductId) -> Option<&PublicProductStatus> {
        self.products.iter().find(|row| row.product == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedFrom {
    Disabled,
    Stopped,
    CooldownCache,
    Network,
    NetworkErrorKeptPrevious,
    UnreadableStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshReason {
    Enable,
    Disable,
    Startup,
    Manual,
    Poll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetReaction {
    None,
    NewlyConfirmed {
        product: ProductId,
        event: ConfirmedReset,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub enabled: bool,
    pub reaction_enabled: bool,
    pub stopped: bool,
    pub served_from: ServedFrom,
    pub fetch_error: Option<String>,
    pub local: LocalUsage,
    pub local_report: LocalUsageReport,
    pub board: Option<PublicBoard>,
    pub board_is_live: bool,
    pub attribution: &'static str,
    pub endpoint: &'static str,
    pub last_success_unix_ms: Option<i64>,
    pub last_attempt_unix_ms: Option<i64>,
}

impl UsageView {
    pub fn disabled(local: LocalUsage) -> Self {
        Self {
            enabled: false,
            reaction_enabled: false,
            stopped: false,
            served_from: ServedFrom::Disabled,
            fetch_error: None,
            local: local.clone(),
            local_report: LocalUsageReport::disabled(),
            board: None,
            board_is_live: false,
            attribution: SOURCE_ATTRIBUTION,
            endpoint: STATUS_URL,
            last_success_unix_ms: None,
            last_attempt_unix_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_paths_are_exact_and_have_no_query() {
        assert_eq!(PublicEndpoint::Status.url(), STATUS_URL);
        assert_eq!(PublicEndpoint::Status.path(), STATUS_PATH);
        assert_eq!(ProductId::ALL.len(), 7);
        for product in ProductId::ALL {
            let url = product.latest_url();
            assert!(url.starts_with("https://limitreset.net/api/v1/"));
            assert!(url.ends_with("/latest"));
            assert!(!url.contains('?'));
            assert_eq!(ProductId::parse(product.as_str()), Some(product));
            assert_eq!(PublicEndpoint::Latest(product).url(), url);
        }
        assert_eq!(ProductId::parse("westus"), None);
        assert_eq!(ProductId::parse("Codex"), None);
    }

    #[test]
    fn unknown_is_not_a_zero_amount() {
        let unknown = LocalUsage::unavailable();
        assert!(unknown.used.is_unknown());
        assert!(unknown.remaining.is_unknown());
        assert_ne!(
            unknown.used,
            Measured::Observed(UsageAmount::new(0)),
            "unknown remaining/used must not be printed as a counted 0"
        );
    }
}
