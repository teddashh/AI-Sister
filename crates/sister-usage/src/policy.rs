//! Enable / disable / stop / cache / restart-dedup / reaction policy.
//!
//! A public reset announcement never writes remaining quota. Forecasts never
//! produce a persona reaction.

use crate::local::LocalUsageAdapter;
use crate::model::{
    ConfirmedReset, LocalUsage, LocalUsageReport, MIN_GET_INTERVAL_MS, ProductId, PublicBoard,
    PublicEndpoint, RefreshReason, ResetReaction, SOURCE_ATTRIBUTION, STATUS_URL, ServedFrom,
    UsageView,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const STORE_SCHEMA: &str = "ai-sister/usage-public-status/v1";
pub const STORE_FILE_NAME: &str = "usage-public-status-v1.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenEvent {
    pub id: String,
    pub announced_at: String,
    pub announced_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DedupStore {
    pub schema: String,
    #[serde(default)]
    pub seen: BTreeMap<String, SeenEvent>,
    #[serde(default)]
    pub baseline_complete: bool,
    #[serde(default)]
    pub last_success_unix_ms: Option<i64>,
    #[serde(default)]
    pub last_attempt_unix_ms: Option<i64>,
    #[serde(default)]
    pub cached_updated_at: Option<String>,
    #[serde(default)]
    pub cached_board: Option<PublicBoard>,
}

impl Default for DedupStore {
    fn default() -> Self {
        Self {
            schema: STORE_SCHEMA.to_owned(),
            seen: BTreeMap::new(),
            baseline_complete: false,
            last_success_unix_ms: None,
            last_attempt_unix_ms: None,
            cached_updated_at: None,
            cached_board: None,
        }
    }
}

impl DedupStore {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn path(data_dir: &Path) -> std::path::PathBuf {
        data_dir.join(STORE_FILE_NAME)
    }

    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = Self::path(data_dir);
        match std::fs::read(&path) {
            Ok(bytes) => Self::parse_bytes(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(_) => Err(Error::StoreUnreadable),
        }
    }

    pub fn parse_bytes(bytes: &[u8]) -> Result<Self> {
        let store: Self = serde_json::from_slice(bytes).map_err(|_| Error::StoreUnreadable)?;
        if store.schema != STORE_SCHEMA {
            return Err(Error::StoreUnreadable);
        }
        Ok(store)
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir).map_err(|_| Error::StoreUnwritable)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| Error::StoreUnwritable)?;
        let final_path = Self::path(data_dir);
        let tmp = data_dir.join(format!("{STORE_FILE_NAME}.tmp"));
        std::fs::write(&tmp, &bytes).map_err(|_| Error::StoreUnwritable)?;
        std::fs::rename(&tmp, final_path).map_err(|_| Error::StoreUnwritable)
    }

    fn remember_board(&mut self, board: &PublicBoard) {
        for row in &board.products {
            if let Some(event) = row.reset.confirmed() {
                let key = row.product.as_str().to_owned();
                let raise = match self.seen.get(&key) {
                    Some(seen) if event.announced_unix_ms < seen.announced_unix_ms => false,
                    Some(seen)
                        if event.announced_unix_ms == seen.announced_unix_ms
                            && seen.id != event.event_id =>
                    {
                        false
                    }
                    _ => true,
                };
                if raise {
                    self.seen.insert(
                        key,
                        SeenEvent {
                            id: event.event_id.clone(),
                            announced_at: event.announced_at.clone(),
                            announced_unix_ms: event.announced_unix_ms,
                        },
                    );
                }
            }
        }
        self.cached_board = Some(board.clone());
        self.cached_updated_at = Some(board.updated_at.clone());
        self.baseline_complete = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshRequest {
    pub enabled: bool,
    pub reaction_enabled: bool,
    pub stopped: bool,
    pub now_unix_ms: i64,
    pub reason: RefreshReason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshOutcome {
    pub view: UsageView,
    pub reaction: ResetReaction,
    pub did_get: bool,
}

pub trait Transport {
    fn get(&self, endpoint: PublicEndpoint) -> Result<crate::TransportResponse>;
}

pub fn refresh_board(
    transport: &impl Transport,
    store: &mut DedupStore,
    local: &impl LocalUsageAdapter,
    request: RefreshRequest,
    is_stopped: impl Fn() -> bool,
) -> RefreshOutcome {
    let local_report = local.report();
    let local_usage = local_report.primary();
    if request.stopped || is_stopped() {
        return RefreshOutcome {
            view: stopped_view(store, local_usage, local_report.clone(), request),
            reaction: ResetReaction::None,
            did_get: false,
        };
    }
    if !request.enabled {
        return RefreshOutcome {
            view: disabled_view(store, local_usage, local_report.clone(), request),
            reaction: ResetReaction::None,
            did_get: false,
        };
    }
    if request.reason == RefreshReason::Disable {
        return RefreshOutcome {
            view: disabled_view(store, local_usage, local_report.clone(), request),
            reaction: ResetReaction::None,
            did_get: false,
        };
    }

    if should_serve_cache(store, request)
        && let Some(board) = store.cached_board.clone()
    {
        return RefreshOutcome {
            view: live_view(
                store,
                local_usage,
                local_report.clone(),
                request,
                ServedFrom::CooldownCache,
                Some(board),
                None,
            ),
            reaction: ResetReaction::None,
            did_get: false,
        };
    }

    if is_stopped() {
        return RefreshOutcome {
            view: stopped_view(store, local_usage, local_report, request),
            reaction: ResetReaction::None,
            did_get: false,
        };
    }
    store.last_attempt_unix_ms = Some(request.now_unix_ms);
    match fetch_status(transport) {
        Ok(board) => {
            if is_stopped() {
                return RefreshOutcome {
                    view: stopped_view(store, local_usage, local_report, request),
                    reaction: ResetReaction::None,
                    did_get: true,
                };
            }
            let reaction = apply_board(store, &board, request);
            store.last_success_unix_ms = Some(request.now_unix_ms);
            store.remember_board(&board);
            RefreshOutcome {
                view: live_view(
                    store,
                    local_usage.clone(),
                    local_report.clone(),
                    request,
                    ServedFrom::Network,
                    Some(board),
                    None,
                ),
                reaction,
                did_get: true,
            }
        }
        Err(error) => {
            let previous = store.cached_board.clone();
            let served = if previous.is_some() {
                ServedFrom::NetworkErrorKeptPrevious
            } else {
                ServedFrom::Network
            };
            RefreshOutcome {
                view: live_view(
                    store,
                    local_usage,
                    local_report,
                    request,
                    served,
                    previous,
                    Some(error.to_string()),
                ),
                reaction: ResetReaction::None,
                did_get: true,
            }
        }
    }
}

fn fetch_status(transport: &impl Transport) -> Result<PublicBoard> {
    let response = transport.get(PublicEndpoint::Status)?;
    crate::validate_response(&response, PublicEndpoint::Status)?;
    crate::parse::parse_status_board(&response.body)
}

fn should_serve_cache(store: &DedupStore, request: RefreshRequest) -> bool {
    if request.reason == RefreshReason::Enable {
        return false;
    }
    let Some(last_success) = store.last_success_unix_ms else {
        return false;
    };
    if store.cached_board.is_none() {
        return false;
    }
    request.now_unix_ms.saturating_sub(last_success) < MIN_GET_INTERVAL_MS
}

fn apply_board(
    store: &mut DedupStore,
    board: &PublicBoard,
    request: RefreshRequest,
) -> ResetReaction {
    if request.reason == RefreshReason::Disable {
        return ResetReaction::None;
    }
    let baseline = !store.baseline_complete
        || store.seen.is_empty()
        || matches!(
            request.reason,
            RefreshReason::Enable | RefreshReason::Startup
        );

    let mut newly = Vec::new();
    for row in &board.products {
        let Some(event) = row.reset.confirmed() else {
            continue;
        };
        if is_duplicate_or_stale(store, row.product, event) {
            continue;
        }
        if request.now_unix_ms >= 1_577_836_800_000
            && event.announced_unix_ms > request.now_unix_ms.saturating_add(60_000)
        {
            continue;
        }
        if !baseline {
            newly.push((row.product, event.clone()));
        }
    }

    if !request.reaction_enabled || baseline || newly.is_empty() {
        return ResetReaction::None;
    }
    let (product, event) = newly.into_iter().next().expect("non-empty");
    ResetReaction::NewlyConfirmed { product, event }
}

fn is_duplicate_or_stale(store: &DedupStore, product: ProductId, event: &ConfirmedReset) -> bool {
    match store.seen.get(product.as_str()) {
        None => false,
        Some(seen) if seen.id == event.event_id => true,
        Some(seen) if event.announced_unix_ms <= seen.announced_unix_ms => true,
        Some(_) => false,
    }
}

fn disabled_view(
    store: &DedupStore,
    local: LocalUsage,
    local_report: LocalUsageReport,
    request: RefreshRequest,
) -> UsageView {
    UsageView {
        enabled: false,
        reaction_enabled: request.reaction_enabled,
        stopped: false,
        served_from: ServedFrom::Disabled,
        fetch_error: None,
        local,
        local_report,
        board: None,
        board_is_live: false,
        attribution: SOURCE_ATTRIBUTION,
        endpoint: STATUS_URL,
        last_success_unix_ms: store.last_success_unix_ms,
        last_attempt_unix_ms: store.last_attempt_unix_ms,
    }
}

fn stopped_view(
    store: &DedupStore,
    local: LocalUsage,
    local_report: LocalUsageReport,
    request: RefreshRequest,
) -> UsageView {
    UsageView {
        enabled: request.enabled,
        reaction_enabled: request.reaction_enabled,
        stopped: true,
        served_from: ServedFrom::Stopped,
        fetch_error: None,
        local,
        local_report,
        board: store.cached_board.clone(),
        board_is_live: false,
        attribution: SOURCE_ATTRIBUTION,
        endpoint: STATUS_URL,
        last_success_unix_ms: store.last_success_unix_ms,
        last_attempt_unix_ms: store.last_attempt_unix_ms,
    }
}

fn live_view(
    store: &DedupStore,
    local: LocalUsage,
    local_report: LocalUsageReport,
    request: RefreshRequest,
    served_from: ServedFrom,
    board: Option<PublicBoard>,
    fetch_error: Option<String>,
) -> UsageView {
    let board_is_live = fetch_error.is_none()
        && matches!(served_from, ServedFrom::Network | ServedFrom::CooldownCache);
    UsageView {
        enabled: true,
        reaction_enabled: request.reaction_enabled,
        stopped: false,
        served_from,
        fetch_error,
        local,
        local_report,
        board,
        board_is_live,
        attribution: SOURCE_ATTRIBUTION,
        endpoint: STATUS_URL,
        last_success_unix_ms: store.last_success_unix_ms,
        last_attempt_unix_ms: store.last_attempt_unix_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::{SyntheticLocalUsage, UnavailableLocalUsage};
    use crate::model::{Measured, PublicProductStatus, PublicReset};
    use crate::parse::parse_status_board;
    use std::cell::{Cell, RefCell};

    struct FakeTransport {
        calls: Cell<usize>,
        body: RefCell<Option<Result<Vec<u8>>>>,
    }

    impl FakeTransport {
        fn body(bytes: &[u8]) -> Self {
            Self {
                calls: Cell::new(0),
                body: RefCell::new(Some(Ok(bytes.to_vec()))),
            }
        }

        fn fail(error: Error) -> Self {
            Self {
                calls: Cell::new(0),
                body: RefCell::new(Some(Err(error))),
            }
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, endpoint: PublicEndpoint) -> Result<crate::TransportResponse> {
            self.calls.set(self.calls.get() + 1);
            assert_eq!(endpoint, PublicEndpoint::Status);
            let body = self.body.borrow_mut().take().expect("one fake body")?;
            Ok(crate::TransportResponse {
                status: 200,
                final_url: PublicEndpoint::Status.url().to_owned(),
                content_type: Some("application/json".to_owned()),
                content_encoding: None,
                content_length: Some(body.len() as u64),
                body,
            })
        }
    }

    fn confirmed_board() -> Vec<u8> {
        br#"{
            "updatedAt":"2026-09-21T01:00:22.000Z",
            "products":{
                "codex":{
                    "latestEvent":{
                        "id":"codex:2026-09-12",
                        "productId":"codex",
                        "kind":"reset",
                        "announcedAt":"2026-09-12T03:20:36.000Z",
                        "verified":true
                    },
                    "forecast":{"p24":0.0,"p48":0.27,"basis":"empirical"},
                    "total":32
                },
                "claude":{"latestEvent":null,"forecast":null,"total":0},
                "chatgpt":{"latestEvent":null,"forecast":null,"total":0},
                "cursor":{"latestEvent":null,"forecast":null,"total":0},
                "gemini":{"latestEvent":null,"forecast":null,"total":0},
                "copilot":{"latestEvent":null,"forecast":null,"total":0},
                "grok":{"latestEvent":null,"forecast":null,"total":0}
            }
        }"#
        .to_vec()
    }

    fn newer_board() -> Vec<u8> {
        String::from_utf8(confirmed_board())
            .unwrap()
            .replace("codex:2026-09-12", "codex:2026-09-21")
            .replace("2026-09-12T03:20:36.000Z", "2026-09-21T04:00:00.000Z")
            .into_bytes()
    }

    fn request(reason: RefreshReason, reaction: bool, now: i64) -> RefreshRequest {
        RefreshRequest {
            enabled: true,
            reaction_enabled: reaction,
            stopped: false,
            now_unix_ms: now,
            reason,
        }
    }

    #[test]
    fn enable_baselines_stale_board_without_reaction() {
        let transport = FakeTransport::body(&confirmed_board());
        let mut store = DedupStore::empty();
        let outcome = refresh_board(
            &transport,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        assert!(outcome.did_get);
        assert_eq!(outcome.reaction, ResetReaction::None);
        assert_eq!(transport.calls.get(), 1);
        assert!(store.seen.contains_key("codex"));
        assert!(outcome.view.local.remaining.is_unknown());
        assert!(outcome.view.attribution.contains("CC BY 4.0"));
    }

    #[test]
    fn duplicate_and_forecast_never_react() {
        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&confirmed_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let second = FakeTransport::body(&confirmed_board());
        let outcome = refresh_board(
            &second,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(outcome.reaction, ResetReaction::None);
        assert_eq!(second.calls.get(), 1);
    }

    #[test]
    fn stale_older_event_does_not_react() {
        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&newer_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let older = FakeTransport::body(&confirmed_board());
        let outcome = refresh_board(
            &older,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(outcome.reaction, ResetReaction::None);
    }

    #[test]
    fn newly_confirmed_reset_reacts_only_after_opt_in() {
        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&confirmed_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let second = FakeTransport::body(&newer_board());
        let muted = refresh_board(
            &second,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, false, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(muted.reaction, ResetReaction::None);

        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&confirmed_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let second = FakeTransport::body(&newer_board());
        let outcome = refresh_board(
            &second,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        match outcome.reaction {
            ResetReaction::NewlyConfirmed { product, event } => {
                assert_eq!(product, ProductId::Codex);
                assert_eq!(event.event_id, "codex:2026-09-21");
            }
            other => panic!("expected reaction, got {other:?}"),
        }
    }

    #[test]
    fn empty_store_startup_is_baseline() {
        let transport = FakeTransport::body(&confirmed_board());
        let mut store = DedupStore::empty();
        let outcome = refresh_board(
            &transport,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Startup, true, 1_000),
            || false,
        );
        assert_eq!(outcome.reaction, ResetReaction::None);
    }

    #[test]
    fn restart_with_seen_ids_does_not_recelebrate_duplicate() {
        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&confirmed_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let persisted = serde_json::to_vec(&store).unwrap();
        let mut restored = DedupStore::parse_bytes(&persisted).unwrap();
        assert!(restored.cached_board.is_some());
        let second = FakeTransport::body(&confirmed_board());
        let outcome = refresh_board(
            &second,
            &mut restored,
            &UnavailableLocalUsage,
            request(RefreshReason::Startup, true, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(outcome.reaction, ResetReaction::None);
    }

    #[test]
    fn disable_and_stop_do_not_get() {
        let transport = FakeTransport::fail(Error::Transport(crate::TransportFailure::Timeout));
        let mut store = DedupStore::empty();
        let disabled = refresh_board(
            &transport,
            &mut store,
            &UnavailableLocalUsage,
            RefreshRequest {
                enabled: false,
                reaction_enabled: true,
                stopped: false,
                now_unix_ms: 1,
                reason: RefreshReason::Disable,
            },
            || false,
        );
        assert!(!disabled.did_get);
        assert_eq!(disabled.view.served_from, ServedFrom::Disabled);
        assert_eq!(transport.calls.get(), 0);

        let stopped = refresh_board(
            &transport,
            &mut store,
            &UnavailableLocalUsage,
            RefreshRequest {
                enabled: true,
                reaction_enabled: true,
                stopped: true,
                now_unix_ms: 1,
                reason: RefreshReason::Poll,
            },
            || false,
        );
        assert!(!stopped.did_get);
        assert_eq!(stopped.view.served_from, ServedFrom::Stopped);
        assert_eq!(transport.calls.get(), 0);
    }

    #[test]
    fn network_error_keeps_previous_and_does_not_invent_zero_remaining() {
        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&confirmed_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let failing = FakeTransport::fail(Error::Transport(crate::TransportFailure::Timeout));
        let outcome = refresh_board(
            &failing,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert!(outcome.did_get);
        assert_eq!(outcome.reaction, ResetReaction::None);
        assert_eq!(
            outcome.view.served_from,
            ServedFrom::NetworkErrorKeptPrevious
        );
        assert!(outcome.view.fetch_error.is_some());
        assert!(outcome.view.board.is_some());
        assert!(!outcome.view.board_is_live);
        assert!(matches!(outcome.view.local.remaining, Measured::Unknown));
    }

    #[test]
    fn cooldown_serves_cache_without_a_second_get() {
        let mut store = DedupStore::empty();
        let first = FakeTransport::body(&confirmed_board());
        refresh_board(
            &first,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let second = FakeTransport::body(&newer_board());
        let outcome = refresh_board(
            &second,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Manual, true, 1_000 + 10),
            || false,
        );
        assert!(!outcome.did_get);
        assert_eq!(outcome.view.served_from, ServedFrom::CooldownCache);
        assert_eq!(second.calls.get(), 0);
        assert_eq!(outcome.reaction, ResetReaction::None);
    }

    #[test]
    fn public_board_cannot_be_turned_into_remaining_quota() {
        let board = parse_status_board(&confirmed_board()).unwrap();
        let local = SyntheticLocalUsage {
            usage: crate::local::synthetic_observed_without_remaining(ProductId::Codex, 9),
        }
        .read();
        assert!(local.remaining.is_unknown());
        assert!(
            board
                .product(ProductId::Codex)
                .unwrap()
                .reset
                .confirmed()
                .is_some()
        );
        let unused: Option<PublicProductStatus> = None;
        assert!(unused.is_none());
        assert!(!matches!(
            board.product(ProductId::Claude).unwrap().reset,
            PublicReset::Confirmed(_)
        ));
    }

    #[test]
    fn first_poll_without_seen_ids_is_baseline() {
        let transport = FakeTransport::body(&confirmed_board());
        let mut store = DedupStore::empty();
        let outcome = refresh_board(
            &transport,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000),
            || false,
        );
        assert_eq!(outcome.reaction, ResetReaction::None);
        assert!(store.baseline_complete);
    }

    #[test]
    fn newer_then_older_then_same_newer_does_not_react_twice() {
        let mut store = DedupStore::empty();
        refresh_board(
            &FakeTransport::body(&confirmed_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let first = refresh_board(
            &FakeTransport::body(&newer_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert!(matches!(
            first.reaction,
            ResetReaction::NewlyConfirmed { .. }
        ));
        let older = refresh_board(
            &FakeTransport::body(&confirmed_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + 2 * MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(older.reaction, ResetReaction::None);
        let again = refresh_board(
            &FakeTransport::body(&newer_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + 3 * MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(again.reaction, ResetReaction::None);
        assert_eq!(store.seen.get("codex").unwrap().id, "codex:2026-09-21");
    }

    #[test]
    fn stop_during_get_discards_reaction() {
        let stopped = std::cell::Cell::new(false);
        let transport = FakeTransport::body(&newer_board());
        let mut store = DedupStore::empty();
        refresh_board(
            &FakeTransport::body(&confirmed_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        stopped.set(true);
        let outcome = refresh_board(
            &transport,
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, 1_000 + MIN_GET_INTERVAL_MS),
            || stopped.get(),
        );
        assert!(!outcome.did_get);
        assert_eq!(outcome.view.served_from, ServedFrom::Stopped);
        assert_eq!(outcome.reaction, ResetReaction::None);
        assert_eq!(transport.calls.get(), 0);
    }

    #[test]
    fn future_announcement_does_not_react() {
        let mut store = DedupStore::empty();
        let now = 1_757_644_836_000;
        refresh_board(
            &FakeTransport::body(&confirmed_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, now),
            || false,
        );
        let future = String::from_utf8(confirmed_board())
            .unwrap()
            .replace("codex:2026-09-12", "codex:2099-01-01")
            .replace("2026-09-12T03:20:36.000Z", "2099-01-01T00:00:00.000Z");
        let outcome = refresh_board(
            &FakeTransport::body(future.as_bytes()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Poll, true, now + MIN_GET_INTERVAL_MS),
            || false,
        );
        assert_eq!(outcome.reaction, ResetReaction::None);
    }

    #[test]
    fn persisted_board_serves_cooldown_after_reload() {
        let mut store = DedupStore::empty();
        refresh_board(
            &FakeTransport::body(&confirmed_board()),
            &mut store,
            &UnavailableLocalUsage,
            request(RefreshReason::Enable, true, 1_000),
            || false,
        );
        let bytes = serde_json::to_vec(&store).unwrap();
        let mut restored = DedupStore::parse_bytes(&bytes).unwrap();
        let second = FakeTransport::body(&newer_board());
        let outcome = refresh_board(
            &second,
            &mut restored,
            &UnavailableLocalUsage,
            request(RefreshReason::Manual, true, 1_000 + 10),
            || false,
        );
        assert!(!outcome.did_get);
        assert_eq!(outcome.view.served_from, ServedFrom::CooldownCache);
        assert_eq!(second.calls.get(), 0);
    }
}
