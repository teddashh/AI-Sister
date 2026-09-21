//! Optional BreezyVoice loopback TTS. Destination is crate-pinned
//! `http://127.0.0.1:8231/{health,tts}`; no URL field, proxy, redirect, or
//! Azure/localService fallback.

use crate::{
    AZURE_TTS_MAX_GENERATION, AZURE_TTS_NO_ACTIVE_GENERATION, Shell, admit_desktop_brain,
    config_path, hold_presentation,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sister_core::config::{LocalTtsEnabled, PersonaId};
use sister_tts::{
    HEALTH_ENDPOINT, LOCAL_TTS_AUDIO_CONTENT_TYPE, LOCAL_TTS_MAX_AUDIO_BYTES, LocalPersona,
    LocalServiceStatus, LoopbackClient, TTS_ENDPOINT,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

const NO_ACTIVE: u64 = AZURE_TTS_NO_ACTIVE_GENERATION;
const MAX_GENERATION: u64 = AZURE_TTS_MAX_GENERATION;

pub struct Runtime {
    pub in_flight: Arc<AtomicBool>,
    pub generation: Arc<AtomicU64>,
    pub active_generation: Arc<AtomicU64>,
    pub admission: Arc<Mutex<()>>,
    pub transition: Arc<Mutex<()>>,
    pub session: Arc<Mutex<Option<Arc<LoopbackClient>>>>,
    pub health_session: Arc<Mutex<Option<Arc<LoopbackClient>>>>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            active_generation: Arc::new(AtomicU64::new(NO_ACTIVE)),
            admission: Arc::new(Mutex::new(())),
            transition: Arc::new(Mutex::new(())),
            session: Arc::new(Mutex::new(None)),
            health_session: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct LocalTtsView {
    generation: u64,
    config_readable: bool,
    enabled: Option<bool>,
    endpoint: Option<&'static str>,
    health_endpoint: Option<&'static str>,
    service: &'static str,
    persona: Option<String>,
    ready: bool,
    config_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalTtsExpected {
    generation: u64,
    enabled: bool,
    persona: String,
}

#[derive(Serialize)]
pub struct LocalTtsAudioView {
    generation: u64,
    content_type: &'static str,
    audio_bytes: usize,
    data_url: String,
    presentation_id: String,
}

pub fn map_persona(id: PersonaId) -> LocalPersona {
    match id {
        PersonaId::Chatgpt => LocalPersona::Chatgpt,
        PersonaId::Claude => LocalPersona::Claude,
        PersonaId::Gemini => LocalPersona::Gemini,
        PersonaId::Grok => LocalPersona::Grok,
        PersonaId::Deepseek => LocalPersona::Deepseek,
        PersonaId::Qwen => LocalPersona::Qwen,
        PersonaId::Mistral => LocalPersona::Mistral,
        PersonaId::Venice => LocalPersona::Venice,
        PersonaId::Sakana => LocalPersona::Sakana,
        PersonaId::Perplexity => LocalPersona::Perplexity,
        PersonaId::Glm => LocalPersona::Glm,
        PersonaId::Kimi => LocalPersona::Kimi,
        PersonaId::Hunyuan => LocalPersona::Hunyuan,
        PersonaId::Minimax => LocalPersona::Minimax,
        PersonaId::Nemotron => LocalPersona::Nemotron,
        PersonaId::Cohere => LocalPersona::Cohere,
        PersonaId::Mimo => LocalPersona::Mimo,
    }
}

fn next_generation(current: u64) -> u64 {
    if current >= MAX_GENERATION {
        0
    } else {
        current + 1
    }
}

fn admission(shell: &Shell) -> std::sync::MutexGuard<'_, ()> {
    shell
        .local_tts
        .admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn transition(shell: &Shell) -> std::sync::MutexGuard<'_, ()> {
    shell
        .local_tts
        .transition
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn view_with_service(shell: &Shell, service: LocalServiceStatus) -> LocalTtsView {
    let generation = shell.local_tts.generation.load(Ordering::Acquire);
    let loaded = config_path().and_then(|path| {
        sister_core::config::Config::load(&path).map_err(|error| format!("{error:#}"))
    });
    match loaded {
        Ok(config) => {
            let enabled = config.shell.local_tts.enabled;
            let persona = config.shell.persona.id;
            LocalTtsView {
                generation,
                config_readable: true,
                enabled: Some(enabled),
                endpoint: Some(TTS_ENDPOINT),
                health_endpoint: Some(HEALTH_ENDPOINT),
                service: service.as_str(),
                persona: Some(persona.as_str().to_owned()),
                ready: sister_tts::speak_allowed(enabled, service).is_ok(),
                config_error: None,
            }
        }
        Err(error) => LocalTtsView {
            generation,
            config_readable: false,
            enabled: None,
            endpoint: None,
            health_endpoint: None,
            service: LocalServiceStatus::Missing.as_str(),
            persona: None,
            ready: false,
            config_error: Some(error),
        },
    }
}

fn cancel_clients(shell: &Shell) {
    if let Ok(mut slot) = shell.local_tts.session.lock()
        && let Some(client) = slot.take()
    {
        client.cancel();
    }
    if let Ok(mut slot) = shell.local_tts.health_session.lock()
        && let Some(client) = slot.take()
    {
        client.cancel();
    }
}

fn stop_intent(app: &tauri::AppHandle, shell: &Shell) {
    {
        let _transition = transition(shell);
        cancel_clients(shell);
        let _ = shell.local_tts.generation.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| Some(next_generation(current)),
        );
    }
    let _ = app.emit("local-tts-stop", ());
}

/// Persona／設定寫入也必須作廢正在飛的本機台灣語音，不能只停 Azure。
pub(crate) fn invalidate(app: &tauri::AppHandle, shell: &Shell) {
    stop_intent(app, shell);
}

async fn probe_health_off_thread(shell: &Shell) -> LocalServiceStatus {
    let client = Arc::new(LoopbackClient::new());
    if let Ok(mut slot) = shell.local_tts.health_session.lock() {
        *slot = Some(Arc::clone(&client));
    }
    let probed = tauri::async_runtime::spawn_blocking(move || client.probe_health())
        .await
        .unwrap_or(Ok(LocalServiceStatus::Protocol))
        .unwrap_or(LocalServiceStatus::Protocol);
    if let Ok(mut slot) = shell.local_tts.health_session.lock() {
        *slot = None;
    }
    probed
}

#[tauri::command]
pub async fn local_tts_read(shell: tauri::State<'_, Shell>) -> Result<LocalTtsView, String> {
    let service = probe_health_off_thread(&shell).await;
    Ok(view_with_service(&shell, service))
}

#[tauri::command]
pub async fn local_tts_config_set(
    enabled: LocalTtsEnabled,
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<LocalTtsView, String> {
    {
        let _admission = admission(&shell);
        stop_intent(&app, &shell);
        let path = config_path()?;
        sister_core::config::Config::update(&path, |config| {
            config.set_local_tts_from_page(enabled);
            Ok(())
        })
        .map_err(|error| format!("{error:#}"))?;
    }
    let service = probe_health_off_thread(&shell).await;
    let status = view_with_service(&shell, service);
    let _ = app.emit("local-tts-changed", status.clone());
    Ok(status)
}

#[tauri::command]
pub fn local_tts_cancel(expected_generation: u64, shell: tauri::State<'_, Shell>) -> bool {
    let _transition = transition(&shell);
    let generation = &shell.local_tts.generation;
    let active = &shell.local_tts.active_generation;
    let next = next_generation(expected_generation);
    if generation
        .compare_exchange(
            expected_generation,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        cancel_clients(&shell);
        return true;
    }
    if active.load(Ordering::Acquire) != expected_generation {
        return false;
    }
    let after = next_generation(next);
    let ok = generation
        .compare_exchange(next, after, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();
    if ok {
        cancel_clients(&shell);
    }
    ok
}

struct InFlightGuard {
    in_flight: Arc<AtomicBool>,
    active_generation: Arc<AtomicU64>,
    session: Arc<Mutex<Option<Arc<LoopbackClient>>>>,
    transition: Arc<Mutex<()>>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(mut slot) = self.session.lock() {
            *slot = None;
        }
        self.active_generation.store(NO_ACTIVE, Ordering::Release);
        self.in_flight.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn local_tts_speak(
    text: String,
    expected: LocalTtsExpected,
    shell: tauri::State<'_, Shell>,
) -> Result<LocalTtsAudioView, String> {
    let data_dir = shell
        .data_dir
        .as_deref()
        .ok_or_else(|| "找不到資料目錄，本機台灣語音沒有開始。".to_string())?;
    let master_stop_admission = admit_desktop_brain(Some(data_dir), "這次本機台灣語音")?;
    if master_stop_admission.stop_requested() {
        return Err("全停已生效；本機台灣語音沒有開始。".to_string());
    }
    if shell.local_tts.generation.load(Ordering::Acquire) != expected.generation {
        return Err("本機台灣語音狀態已變更；沒有送出。".to_string());
    }
    let path = config_path()?;
    let config = sister_core::config::Config::load(&path).map_err(|error| format!("{error:#}"))?;
    if expected.enabled != config.shell.local_tts.enabled {
        return Err("本機台灣語音開關已變更；沒有送出。".to_string());
    }
    if !config.shell.local_tts.enabled {
        return Err("本機台灣語音目前關閉。".to_string());
    }
    let persona = map_persona(config.shell.persona.id);
    if expected.persona != persona.as_str() {
        return Err("角色已變更；沒有送出。".to_string());
    }
    sister_tts::tts_request(persona, &text).map_err(|error| error.to_string())?;

    let client = Arc::new(LoopbackClient::new());
    let request_generation = next_generation(expected.generation);
    {
        let _admission = admission(&shell);
        if shell.local_tts.generation.load(Ordering::Acquire) != expected.generation {
            return Err("本機台灣語音狀態已變更；沒有送出。".to_string());
        }
        let _transition = transition(&shell);
        if master_stop_admission.stop_requested() {
            return Err("全停已生效；本機台灣語音沒有開始。".to_string());
        }
        shell
            .local_tts
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "上一句本機台灣語音還沒結束。".to_string())?;
        shell
            .local_tts
            .active_generation
            .store(expected.generation, Ordering::Release);
        if let Ok(mut slot) = shell.local_tts.session.lock() {
            *slot = Some(Arc::clone(&client));
        }
        if shell
            .local_tts
            .generation
            .compare_exchange(
                expected.generation,
                request_generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            client.cancel();
            if let Ok(mut slot) = shell.local_tts.session.lock() {
                *slot = None;
            }
            shell
                .local_tts
                .active_generation
                .store(NO_ACTIVE, Ordering::Release);
            shell.local_tts.in_flight.store(false, Ordering::Release);
            return Err("本機台灣語音已停止。".to_string());
        }
    }
    let in_flight_guard = InFlightGuard {
        in_flight: Arc::clone(&shell.local_tts.in_flight),
        active_generation: Arc::clone(&shell.local_tts.active_generation),
        session: Arc::clone(&shell.local_tts.session),
        transition: Arc::clone(&shell.local_tts.transition),
    };
    let generation_state = Arc::clone(&shell.local_tts.generation);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _in_flight_guard = in_flight_guard;
        if generation_state.load(Ordering::Acquire) != request_generation
            || master_stop_admission.stop_requested()
        {
            client.cancel();
            return Err("本機台灣語音已停止。".to_string());
        }
        let _boundary = master_stop_admission
            .boundary()
            .ok_or_else(|| "全停已生效；本機台灣語音沒有送出。".to_string())?;
        let service = client.probe_health().map_err(|error| error.to_string())?;
        sister_tts::speak_allowed(true, service).map_err(|error| error.to_string())?;
        if generation_state.load(Ordering::Acquire) != request_generation {
            client.cancel();
            return Err("本機台灣語音已停止。".to_string());
        }
        let audio = client
            .synthesize(persona, &text)
            .map_err(|error| error.to_string())?;
        if generation_state.load(Ordering::Acquire) != request_generation
            || master_stop_admission.stop_requested()
        {
            return Err("本機台灣語音已停止。".to_string());
        }
        Ok((audio, master_stop_admission))
    });
    let (audio, master_stop_admission) = task
        .await
        .map_err(|_| "本機台灣語音沒有完成。".to_string())??;
    let bytes = audio.into_bytes();
    let audio_bytes = bytes.len();
    if audio_bytes > LOCAL_TTS_MAX_AUDIO_BYTES {
        return Err("本機台灣語音回應超過單次音訊上限。".to_string());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(LocalTtsAudioView {
        generation: request_generation,
        content_type: LOCAL_TTS_AUDIO_CONTENT_TYPE,
        audio_bytes,
        data_url: format!("data:{LOCAL_TTS_AUDIO_CONTENT_TYPE};base64,{encoded}"),
        presentation_id: hold_presentation(master_stop_admission),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_persona_ids_map_onto_daemon_ids() {
        assert_eq!(PersonaId::ALL.len(), LocalPersona::ALL.len());
        for id in PersonaId::ALL {
            assert_eq!(map_persona(id).as_str(), id.as_str());
        }
    }
}
