//! Optional LimitReset public status GET and local session usage projection.
//!
//! Renderer only receives IPC snapshots. HTTP is sister-usage's exact GET.

use serde::Serialize;
use sister_core::config::{
    UsageLocalSessionsEnabled, UsagePublicStatusEnabled, UsageResetReactionEnabled,
};
use sister_usage::view::{UsageSettings, board_file_unreadable, project, unreadable};
use sister_usage::{
    AUTO_POLL_INTERVAL_MS, ConfiguredSessionAdapter, DedupStore, LocalUsageAdapter, RefreshReason,
    ResetReaction, SOURCE_ATTRIBUTION,
};

pub use sister_usage::view::UsageStatusView;
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
    crate::advance_generation(&runtime.generation, next_generation);
}

pub fn read_view(runtime: &Runtime, data_dir: Option<&Path>, stopped: bool) -> UsageStatusView {
    let generation = runtime.generation.load(Ordering::Acquire);
    let path = match sister_core::config::Config::default_path() {
        Some(path) => path,
        None => {
            return unreadable(
                generation,
                stopped,
                sister_usage::usage_config_missing_message(),
            );
        }
    };
    let config = match sister_core::config::Config::load(&path) {
        Ok(config) => config,
        Err(error) => {
            return unreadable(
                generation,
                stopped,
                &sister_usage::usage_config_parse_message(&format!("{error:#}")),
            );
        }
    };
    let usage = config.shell.usage;
    let settings = usage_settings(&usage);
    let local = ConfiguredSessionAdapter::from_config(
        usage.local_sessions_enabled,
        &usage.local_sessions_dir,
    )
    .report();
    let Some(dir) = data_dir else {
        return board_file_unreadable(
            generation,
            stopped,
            &settings,
            local,
            sister_usage::usage_data_dir_missing_message(),
        );
    };
    let store = match DedupStore::load(dir) {
        Ok(store) => store,
        Err(_) => {
            return board_file_unreadable(
                generation,
                stopped,
                &settings,
                local,
                sister_usage::usage_store_unreadable_message(),
            );
        }
    };
    let recalled = sister_usage::recall_stored_board(&store, usage.public_status_enabled, stopped);
    project(
        generation,
        true,
        None,
        stopped,
        &settings,
        &store,
        local,
        recalled.served_from,
        recalled.fetch_error,
        recalled.board,
        recalled.board_is_live,
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
    let path = match sister_core::config::Config::default_path() {
        Some(path) => path,
        None => {
            return Ok((
                unreadable(
                    runtime.generation.load(Ordering::Acquire),
                    stopped,
                    sister_usage::usage_config_missing_message(),
                ),
                None,
            ));
        }
    };
    let config = match sister_core::config::Config::load(&path) {
        Ok(config) => config,
        Err(error) => {
            return Ok((
                unreadable(
                    runtime.generation.load(Ordering::Acquire),
                    stopped,
                    &sister_usage::usage_config_parse_message(&format!("{error:#}")),
                ),
                None,
            ));
        }
    };
    let usage = config.shell.usage;
    let settings = usage_settings(&usage);
    let Some(dir) = data_dir else {
        return Ok((
            board_file_unreadable(
                runtime.generation.load(Ordering::Acquire),
                stopped,
                &settings,
                scan_configured(&usage),
                sister_usage::usage_data_dir_missing_message(),
            ),
            None,
        ));
    };
    let mut store = match DedupStore::load(dir) {
        Ok(store) => store,
        Err(_) => {
            return Ok((
                board_file_unreadable(
                    runtime.generation.load(Ordering::Acquire),
                    stopped,
                    &settings,
                    scan_configured(&usage),
                    sister_usage::usage_store_unreadable_message(),
                ),
                None,
            ));
        }
    };
    let Some(_admit) = sister_hands::master_stop::admit(dir) else {
        return Ok((read_view(runtime, data_dir, true), None));
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
        if sister_hands::master_stop::is_stopped(dir)
            || runtime.generation.load(Ordering::Acquire) != expected_generation
        {
            runtime.in_flight.store(false, Ordering::Release);
            runtime
                .active_generation
                .store(NO_ACTIVE_GENERATION, Ordering::Release);
            return Ok((read_view(runtime, data_dir, true), None));
        }
        let client = sister_usage::PublicStatusClient::new();
        let outcome = sister_usage::refresh_board(&client, &mut store, &adapter, request, || {
            sister_hands::master_stop::is_stopped(dir)
                || runtime.generation.load(Ordering::Acquire) != expected_generation
        });
        runtime.in_flight.store(false, Ordering::Release);
        runtime
            .active_generation
            .store(NO_ACTIVE_GENERATION, Ordering::Release);
        outcome
    };
    if runtime.generation.load(Ordering::Acquire) != expected_generation {
        return Ok((read_view(runtime, data_dir, stopped), None));
    }
    if store.save(dir).is_err() {
        return Ok((
            project(
                runtime.generation.load(Ordering::Acquire),
                true,
                Some(sister_usage::usage_store_unwritable_message().to_owned()),
                stopped,
                &settings,
                &store,
                outcome.view.local_report.clone(),
                outcome.view.served_from,
                outcome.view.fetch_error.clone(),
                outcome.view.board.clone(),
                outcome.view.board_is_live,
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
            &settings,
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
    let path = sister_core::config::Config::default_path()
        .ok_or_else(|| "找不到設定檔路徑".to_string())?;
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

fn usage_settings(usage: &sister_core::config::UsageConfig) -> UsageSettings {
    UsageSettings {
        public_status_enabled: usage.public_status_enabled,
        reset_reaction: usage.reset_reaction,
        local_sessions_enabled: usage.local_sessions_enabled,
        local_sessions_dir: usage.local_sessions_dir.clone(),
    }
}

fn scan_configured(usage: &sister_core::config::UsageConfig) -> sister_usage::LocalUsageReport {
    ConfiguredSessionAdapter::from_config(usage.local_sessions_enabled, &usage.local_sessions_dir)
        .report()
}
