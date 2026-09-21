//! Optional LimitReset public status GET and local session usage projection.
//!
//! Renderer only receives IPC snapshots. HTTP is sister-usage's exact GET.

use serde::Serialize;
use sister_core::config::{
    UsageLocalSessionsEnabled, UsagePublicStatusEnabled, UsageResetReactionEnabled,
};
use sister_usage::{
    AUTO_POLL_INTERVAL_MS, ConfiguredSessionAdapter, DedupStore, HOST, LocalUsageAdapter,
    Measured, PublicReset, SOURCE_ATTRIBUTION, SOURCE_LICENSE, SOURCE_NAME, SOURCE_URL,
    STATUS_URL, RefreshReason, ResetReaction, ServedFrom,
};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

pub const NO_ACTIVE_GENERATION: u64 = u64::MAX;

pub struct Runtime {
    pub in_flight: Arc<AtomicBool>,
    pub generation: Arc<AtomicU64>,
    pub active_generation: Arc<AtomicU64>,
    pub admission: Arc<Mutex<()>>,
    pub transition: Arc<Mutex<()>>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            active_generation: Arc::new(AtomicU64::new(NO_ACTIVE_GENERATION)),
            admission: Arc::new(Mutex::new(())),
            transition: Arc::new(Mutex::new(())),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct UsageProductView {
    pub id: &'static str,
    pub name: &'static str,
    pub reset: &'static str,
    pub event_id: Option<String>,
    pub announced_at: Option<String>,
    pub public_event_count: Option<u64>,
    pub forecast_p24: Option<f64>,
    pub forecast_p48: Option<f64>,
    pub forecast_basis: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct LocalProductView {
    pub id: &'static str,
    pub name: &'static str,
    pub observed_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub quota_used_percent: Option<f64>,
    pub quota_window_minutes: Option<i64>,
    pub quota_resets_at_unix: Option<i64>,
    pub provenance: &'static str,
}

#[derive(Clone, Serialize)]
pub struct UsageStatusView {
    pub generation: u64,
    pub config_readable: bool,
    pub enabled: Option<bool>,
    pub reaction_enabled: Option<bool>,
    pub local_sessions_enabled: Option<bool>,
    pub local_sessions_dir: Option<String>,
    pub stopped: bool,
    pub served_from: &'static str,
    pub fetch_error: Option<String>,
    pub local_error: Option<String>,
    pub local_files_read: u32,
    pub local_skipped_auth: u32,
    pub local_products: Vec<LocalProductView>,
    pub local_unknown_reason: &'static str,
    pub board_live: bool,
    pub board_updated_at: Option<String>,
    pub products: Vec<UsageProductView>,
    pub attribution: &'static str,
    pub source_name: &'static str,
    pub source_url: &'static str,
    pub license: &'static str,
    pub endpoint: &'static str,
    pub host: &'static str,
    pub last_success_unix_ms: Option<i64>,
    pub config_error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct UsageResetReactionView {
    pub product: &'static str,
    pub product_name: &'static str,
    pub event_id: String,
    pub announced_at: String,
    pub attribution: &'static str,
    pub line: String,
}

pub fn next_generation(current: u64) -> u64 {
    current.saturating_add(1)
}

pub fn stop_intent(runtime: &Runtime) {
    let _transition = runtime
        .transition
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = runtime.generation.fetch_update(
        Ordering::AcqRel,
        Ordering::Acquire,
        |current| Some(next_generation(current)),
    );
}

pub fn read_view(runtime: &Runtime, data_dir: Option<&Path>, stopped: bool) -> UsageStatusView {
    let generation = runtime.generation.load(Ordering::Acquire);
    let path = match sister_core::config::Config::default_path() {
        Some(path) => path,
        None => return unreadable(generation, stopped, "找不到設定檔路徑。"),
    };
    let config = match sister_core::config::Config::load(&path) {
        Ok(config) => config,
        Err(error) => return unreadable(generation, stopped, &format!("{error:#}")),
    };
    let usage = config.shell.usage;
    let store = data_dir
        .map(DedupStore::load)
        .and_then(Result::ok)
        .unwrap_or_else(DedupStore::empty);
    let local = ConfiguredSessionAdapter::from_config(
        usage.local_sessions_enabled,
        &usage.local_sessions_dir,
    )
    .report();
    project(
        generation,
        true,
        None,
        stopped,
        &usage,
        &store,
        local,
        if stopped {
            ServedFrom::Stopped
        } else if !usage.public_status_enabled {
            ServedFrom::Disabled
        } else {
            ServedFrom::CooldownCache
        },
        None,
        usage.public_status_enabled.then(|| store.cached_board.clone()).flatten(),
        false,
    )
}

pub fn refresh_blocking(
    runtime: &Runtime,
    data_dir: Option<&Path>,
    stopped: bool,
    reason: RefreshReason,
    expected_generation: u64,
) -> Result<(UsageStatusView, Option<UsageResetReactionView>), String> {
    if runtime.generation.load(Ordering::Acquire) != expected_generation {
        return Ok((read_view(runtime, data_dir, stopped), None));
    }
    let path = sister_core::config::Config::default_path()
        .ok_or_else(|| "找不到設定檔路徑。".to_string())?;
    let config = sister_core::config::Config::load(&path).map_err(|error| format!("{error:#}"))?;
    let usage = config.shell.usage;
    let Some(dir) = data_dir else {
        return Ok((
            unreadable(
                runtime.generation.load(Ordering::Acquire),
                stopped,
                "找不到資料目錄，公開看板狀態沒有地方可放。",
            ),
            None,
        ));
    };
    let mut store = match DedupStore::load(dir) {
        Ok(store) => store,
        Err(_) => {
            return Ok((
                unreadable(
                    runtime.generation.load(Ordering::Acquire),
                    stopped,
                    "本機公開看板狀態讀不出來；沒有連線，也沒有用空檔假裝沒看過舊事件。",
                ),
                None,
            ));
        }
    };
    let _admission = runtime
        .admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if runtime.generation.load(Ordering::Acquire) != expected_generation {
        return Ok((read_view(runtime, data_dir, stopped), None));
    }
    if runtime
        .in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok((read_view(runtime, data_dir, stopped), None));
    }
    runtime
        .active_generation
        .store(expected_generation, Ordering::Release);

    let adapter = ConfiguredSessionAdapter::from_config(
        usage.local_sessions_enabled,
        &usage.local_sessions_dir,
    );
    let request = sister_usage::RefreshRequest {
        enabled: usage.public_status_enabled,
        reaction_enabled: usage.reset_reaction,
        stopped,
        now_unix_ms: sister_core::now_ms(),
        reason,
    };
    let outcome = {
        let _transition = runtime
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime.generation.load(Ordering::Acquire) != expected_generation {
            runtime.in_flight.store(false, Ordering::Release);
            runtime
                .active_generation
                .store(NO_ACTIVE_GENERATION, Ordering::Release);
            return Ok((read_view(runtime, data_dir, stopped), None));
        }
        let client = sister_usage::PublicStatusClient::new();
        let outcome = sister_usage::refresh_board(&client, &mut store, &adapter, request);
        runtime.in_flight.store(false, Ordering::Release);
        runtime
            .active_generation
            .store(NO_ACTIVE_GENERATION, Ordering::Release);
        outcome
    };
    if runtime.generation.load(Ordering::Acquire) != expected_generation {
        return Ok((read_view(runtime, data_dir, stopped), None));
    }
    if let Err(error) = store.save(dir) {
        return Ok((
            unreadable(
                runtime.generation.load(Ordering::Acquire),
                stopped,
                &error.to_string(),
            ),
            None,
        ));
    }
    let reaction = match outcome.reaction {
        ResetReaction::NewlyConfirmed { product, event } => Some(UsageResetReactionView {
            product: product.as_str(),
            product_name: product.display_name(),
            event_id: event.event_id.clone(),
            announced_at: event.announced_at.clone(),
            attribution: SOURCE_ATTRIBUTION,
            line: format!(
                "公開看板剛確認一次 {} 重置。這是全球產品公告，不是你的帳號額度。",
                product.display_name()
            ),
        }),
        ResetReaction::None => None,
    };
    Ok((
        project(
            runtime.generation.load(Ordering::Acquire),
            true,
            None,
            stopped,
            &usage,
            &store,
            outcome.view.local_report,
            outcome.view.served_from,
            outcome.view.fetch_error,
            outcome.view.board,
            outcome.view.board_is_live,
        ),
        reaction,
    ))
}

pub fn due_for_poll(data_dir: Option<&Path>, now_unix_ms: i64) -> bool {
    let Some(dir) = data_dir else {
        return false;
    };
    let Ok(store) = DedupStore::load(dir) else {
        return false;
    };
    match store.last_attempt_unix_ms {
        None => true,
        Some(last) => now_unix_ms.saturating_sub(last) >= AUTO_POLL_INTERVAL_MS,
    }
}

pub fn poll_interval() -> Duration {
    Duration::from_secs(30)
}

pub fn set_from_page(
    public_status: UsagePublicStatusEnabled,
    reset_reaction: UsageResetReactionEnabled,
    local_sessions: UsageLocalSessionsEnabled,
    local_sessions_dir: String,
) -> Result<(), String> {
    let path =
        sister_core::config::Config::default_path().ok_or_else(|| "找不到設定檔路徑".to_string())?;
    sister_core::config::Config::update(&path, |config| {
        config.set_usage_from_page(
            public_status,
            reset_reaction,
            local_sessions,
            local_sessions_dir,
        );
        Ok(())
    })
    .map(|_| ())
    .map_err(|error| format!("{error:#}"))
}

fn unreadable(generation: u64, stopped: bool, error: &str) -> UsageStatusView {
    UsageStatusView {
        generation,
        config_readable: false,
        enabled: None,
        reaction_enabled: None,
        local_sessions_enabled: None,
        local_sessions_dir: None,
        stopped,
        served_from: "unreadable",
        fetch_error: None,
        local_error: None,
        local_files_read: 0,
        local_skipped_auth: 0,
        local_products: Vec::new(),
        local_unknown_reason: "本機用量目錄未指定或讀不到；未知不是量到 0。",
        board_live: false,
        board_updated_at: None,
        products: Vec::new(),
        attribution: SOURCE_ATTRIBUTION,
        source_name: SOURCE_NAME,
        source_url: SOURCE_URL,
        license: SOURCE_LICENSE,
        endpoint: STATUS_URL,
        host: HOST,
        last_success_unix_ms: None,
        config_error: Some(error.to_owned()),
    }
}

fn project(
    generation: u64,
    config_readable: bool,
    config_error: Option<String>,
    stopped: bool,
    usage: &sister_core::config::UsageConfig,
    store: &DedupStore,
    local: sister_usage::LocalUsageReport,
    served_from: ServedFrom,
    fetch_error: Option<String>,
    board: Option<sister_usage::PublicBoard>,
    board_live: bool,
) -> UsageStatusView {
    let local_products = local
        .products
        .iter()
        .map(|row| LocalProductView {
            id: row.product.as_str(),
            name: row.product.display_name(),
            observed_tokens: match row.observed_tokens {
                Measured::Unknown => None,
                Measured::Observed(amount) => Some(amount.get()),
            },
            remaining_tokens: match row.remaining_tokens {
                Measured::Unknown => None,
                Measured::Observed(amount) => Some(amount.get()),
            },
            quota_used_percent: match &row.quota {
                Measured::Observed(snapshot) => Some(snapshot.used_percent),
                Measured::Unknown => None,
            },
            quota_window_minutes: match &row.quota {
                Measured::Observed(snapshot) => snapshot.window_minutes,
                Measured::Unknown => None,
            },
            quota_resets_at_unix: match &row.quota {
                Measured::Observed(snapshot) => snapshot.resets_at_unix,
                Measured::Unknown => None,
            },
            provenance: row.provenance,
        })
        .collect();
    let products = board
        .as_ref()
        .map(|board| {
            board
                .products
                .iter()
                .map(|row| {
                    let (reset, event_id, announced_at) = match &row.reset {
                        PublicReset::NoneRecorded => ("none", None, None),
                        PublicReset::Unverified {
                            event_id,
                            announced_at,
                        } => (
                            "unverified",
                            Some(event_id.clone()),
                            Some(announced_at.clone()),
                        ),
                        PublicReset::OtherKind {
                            event_id,
                            announced_at,
                            ..
                        } => ("other", Some(event_id.clone()), Some(announced_at.clone())),
                        PublicReset::Confirmed(event) => (
                            "confirmed",
                            Some(event.event_id.clone()),
                            Some(event.announced_at.clone()),
                        ),
                    };
                    UsageProductView {
                        id: row.product.as_str(),
                        name: row.product.display_name(),
                        reset,
                        event_id,
                        announced_at,
                        public_event_count: row.public_event_count,
                        forecast_p24: row.forecast.as_ref().map(|forecast| forecast.p24),
                        forecast_p48: row.forecast.as_ref().map(|forecast| forecast.p48),
                        forecast_basis: row.forecast.as_ref().map(|forecast| forecast.basis.clone()),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    UsageStatusView {
        generation,
        config_readable,
        enabled: Some(usage.public_status_enabled),
        reaction_enabled: Some(usage.reset_reaction),
        local_sessions_enabled: Some(usage.local_sessions_enabled),
        local_sessions_dir: Some(usage.local_sessions_dir.clone()),
        stopped,
        served_from: served_word(served_from),
        fetch_error,
        local_error: local.error,
        local_files_read: local.files_read,
        local_skipped_auth: local.skipped_auth_files,
        local_products,
        local_unknown_reason: "剩餘 token 未知。公開看板與已用百分比都不能換成帳號剩餘額度。本機數字是 session 記錄，不是帳單。",
        board_live,
        board_updated_at: board.as_ref().map(|board| board.updated_at.clone()),
        products,
        attribution: SOURCE_ATTRIBUTION,
        source_name: SOURCE_NAME,
        source_url: SOURCE_URL,
        license: SOURCE_LICENSE,
        endpoint: STATUS_URL,
        host: HOST,
        last_success_unix_ms: store.last_success_unix_ms,
        config_error,
    }
}

fn served_word(served: ServedFrom) -> &'static str {
    match served {
        ServedFrom::Disabled => "disabled",
        ServedFrom::Stopped => "stopped",
        ServedFrom::CooldownCache => "cache",
        ServedFrom::Network => "network",
        ServedFrom::NetworkErrorKeptPrevious => "network-error",
        ServedFrom::UnreadableStore => "unreadable",
    }
}
