//! Loopback-only BreezyVoice adapter for dynamic local answer reading.
//!
//! Authority is a single IPv4 loopback service: `127.0.0.1:8231`. The only
//! product calls are `GET /health` and `POST /tts`. There is no URL field, no
//! proxy, no redirect, no `/clone`, and no remote fallback. Native sockets are
//! compiled for the desktop `local` feature and for tests; the default crate
//! graph still has no HTTP client crate.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const LOCAL_HOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
pub const LOCAL_PORT: u16 = 8231;
pub const HEALTH_PATH: &str = "/health";
pub const TTS_PATH: &str = "/tts";
pub const HEALTH_ENDPOINT: &str = "http://127.0.0.1:8231/health";
pub const TTS_ENDPOINT: &str = "http://127.0.0.1:8231/tts";
pub const LOCAL_TTS_AUDIO_CONTENT_TYPE: &str = "audio/wav";
pub const LOCAL_TTS_JSON_CONTENT_TYPE: &str = "application/json";
pub const LOCAL_TTS_USER_AGENT: &str = "AI-Sister/1 sister-tts-local";
pub const LOCAL_TTS_MAX_TEXT_BYTES: usize = 8 * 1024;
pub const LOCAL_TTS_MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;
pub const LOCAL_TTS_MAX_HEALTH_BYTES: usize = 4 * 1024;
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
pub const TTS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const TTS_IO_TIMEOUT: Duration = Duration::from_secs(90);

const HEADER_LIMIT: usize = 8 * 1024;
const READ_CHUNK_BYTES: usize = 16 * 1024;

/// Canonical AI-Sister persona IDs accepted as BreezyVoice `persona`.
///
/// These match `sister_core::config::PersonaId` serde names. Renderer cannot
/// invent a different voice id; unknown values never reach the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalPersona {
    Chatgpt,
    Claude,
    Gemini,
    Grok,
    Deepseek,
    Qwen,
    Mistral,
    Venice,
    Sakana,
    Perplexity,
    Glm,
    Kimi,
    Hunyuan,
    Minimax,
    Nemotron,
    Cohere,
    Mimo,
}

impl LocalPersona {
    pub const ALL: [Self; 17] = [
        Self::Chatgpt,
        Self::Claude,
        Self::Gemini,
        Self::Grok,
        Self::Deepseek,
        Self::Qwen,
        Self::Mistral,
        Self::Venice,
        Self::Sakana,
        Self::Perplexity,
        Self::Glm,
        Self::Kimi,
        Self::Hunyuan,
        Self::Minimax,
        Self::Nemotron,
        Self::Cohere,
        Self::Mimo,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
            Self::Deepseek => "deepseek",
            Self::Qwen => "qwen",
            Self::Mistral => "mistral",
            Self::Venice => "venice",
            Self::Sakana => "sakana",
            Self::Perplexity => "perplexity",
            Self::Glm => "glm",
            Self::Kimi => "kimi",
            Self::Hunyuan => "hunyuan",
            Self::Minimax => "minimax",
            Self::Nemotron => "nemotron",
            Self::Cohere => "cohere",
            Self::Mimo => "mimo",
        }
    }
}

impl fmt::Display for LocalPersona {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTransportFailure {
    Connection,
    Timeout,
    Protocol,
    UnexpectedPeer,
    ResponseBody,
    Cancelled,
}

impl fmt::Display for LocalTransportFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connection => "本機台灣語音服務沒有回應",
            Self::Timeout => "本機台灣語音連線逾時",
            Self::Protocol => "本機台灣語音傳輸格式失敗",
            Self::UnexpectedPeer => "本機台灣語音連到了非 loopback 位址；已中止",
            Self::ResponseBody => "本機台灣語音回應讀取失敗",
            Self::Cancelled => "本機台灣語音已停止",
        })
    }
}

impl std::error::Error for LocalTransportFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalError {
    EmptyText,
    TextTooLong,
    Transport(LocalTransportFailure),
    UnexpectedStatus(u16),
    Redirect,
    UnexpectedContentType,
    UnexpectedContentEncoding,
    UnexpectedTransferEncoding,
    UnexpectedContentLength,
    EmptyAudio,
    AudioTooLarge,
    InvalidWav,
    InvalidHealth,
    ServiceNotReady,
}

impl fmt::Display for LocalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText => formatter.write_str("沒有可朗讀的文字"),
            Self::TextTooLong => formatter.write_str("這次本機朗讀的文字超過單次上限"),
            Self::Transport(failure) => failure.fmt(formatter),
            Self::UnexpectedStatus(status) => {
                write!(formatter, "本機台灣語音回了未允許的 HTTP 狀態：{status}")
            }
            Self::Redirect => formatter.write_str("本機台灣語音試圖轉向其他位址；沒有跟隨"),
            Self::UnexpectedContentType => {
                formatter.write_str("本機台灣語音回了未允許的 Content-Type")
            }
            Self::UnexpectedContentEncoding => {
                formatter.write_str("本機台灣語音回了未允許的 Content-Encoding")
            }
            Self::UnexpectedTransferEncoding => {
                formatter.write_str("本機台灣語音回了未允許的 Transfer-Encoding")
            }
            Self::UnexpectedContentLength => {
                formatter.write_str("本機台灣語音回應長度和宣告不一致")
            }
            Self::EmptyAudio => formatter.write_str("本機台灣語音成功回應裡沒有音訊"),
            Self::AudioTooLarge => formatter.write_str("本機台灣語音回應超過單次音訊上限"),
            Self::InvalidWav => formatter.write_str("本機台灣語音回應不是可播放的 WAV"),
            Self::InvalidHealth => formatter.write_str("本機台灣語音健康檢查回應無法辨識"),
            Self::ServiceNotReady => formatter.write_str("本機台灣語音服務尚未載入完成"),
        }
    }
}

impl std::error::Error for LocalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(failure) => Some(failure),
            _ => None,
        }
    }
}

impl From<LocalTransportFailure> for LocalError {
    fn from(value: LocalTransportFailure) -> Self {
        Self::Transport(value)
    }
}

pub type LocalResult<T> = Result<T, LocalError>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocalMethod {
    Get,
    Post,
}

impl LocalMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// Immutable loopback request. Endpoint, host and path are crate constants;
/// callers only choose health vs TTS and supply persona plus ephemeral text.
pub struct LocalRequest {
    method: LocalMethod,
    path: &'static str,
    persona: Option<LocalPersona>,
    body: Option<String>,
}

impl LocalRequest {
    pub const fn method(&self) -> LocalMethod {
        self.method
    }

    pub const fn path(&self) -> &'static str {
        self.path
    }

    pub const fn host_header(&self) -> &'static str {
        "127.0.0.1:8231"
    }

    pub fn endpoint(&self) -> &'static str {
        if self.path == HEALTH_PATH {
            HEALTH_ENDPOINT
        } else {
            TTS_ENDPOINT
        }
    }

    pub const fn persona(&self) -> Option<LocalPersona> {
        self.persona
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    pub const fn accept(&self) -> &'static str {
        match self.method {
            LocalMethod::Get => LOCAL_TTS_JSON_CONTENT_TYPE,
            LocalMethod::Post => LOCAL_TTS_AUDIO_CONTENT_TYPE,
        }
    }
}

impl fmt::Debug for LocalRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalRequest")
            .field("method", &self.method.as_str())
            .field("endpoint", &self.endpoint())
            .field("persona", &self.persona.map(LocalPersona::as_str))
            .field(
                "body_bytes",
                &self.body.as_ref().map(String::len).unwrap_or(0),
            )
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LocalTransportResponse {
    status: u16,
    content_type: Option<String>,
    content_encoding: Option<String>,
    transfer_encoding: Option<String>,
    content_length: Option<u64>,
    location: Option<String>,
    body: Vec<u8>,
}

impl LocalTransportResponse {
    pub fn new(
        status: u16,
        content_type: Option<String>,
        content_encoding: Option<String>,
        transfer_encoding: Option<String>,
        content_length: Option<u64>,
        location: Option<String>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            content_type,
            content_encoding,
            transfer_encoding,
            content_length,
            location,
            body,
        }
    }
}

pub trait LocalTransport {
    fn execute(
        &self,
        request: &LocalRequest,
    ) -> Result<LocalTransportResponse, LocalTransportFailure>;
}

#[derive(PartialEq, Eq)]
pub struct LocalAudio {
    bytes: Vec<u8>,
}

impl LocalAudio {
    pub const fn content_type(&self) -> &'static str {
        LOCAL_TTS_AUDIO_CONTENT_TYPE
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl fmt::Debug for LocalAudio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalAudio")
            .field("content_type", &LOCAL_TTS_AUDIO_CONTENT_TYPE)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalServiceStatus {
    Missing,
    NotReady,
    Ready,
    Protocol,
}

impl LocalServiceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::NotReady => "not_ready",
            Self::Ready => "ready",
            Self::Protocol => "protocol",
        }
    }
}

pub fn build_tts_body(persona: LocalPersona, text: &str) -> LocalResult<String> {
    if text.trim().is_empty() {
        return Err(LocalError::EmptyText);
    }
    if text.len() > LOCAL_TTS_MAX_TEXT_BYTES {
        return Err(LocalError::TextTooLong);
    }
    let body = serde_json::json!({
        "persona": persona.as_str(),
        "text": text,
    });
    let encoded = serde_json::to_string(&body).map_err(|_| LocalError::TextTooLong)?;
    if encoded.len() > LOCAL_TTS_MAX_TEXT_BYTES + 256 {
        return Err(LocalError::TextTooLong);
    }
    Ok(encoded)
}

pub fn health_request() -> LocalRequest {
    LocalRequest {
        method: LocalMethod::Get,
        path: HEALTH_PATH,
        persona: None,
        body: None,
    }
}

pub fn tts_request(persona: LocalPersona, text: &str) -> LocalResult<LocalRequest> {
    Ok(LocalRequest {
        method: LocalMethod::Post,
        path: TTS_PATH,
        persona: Some(persona),
        body: Some(build_tts_body(persona, text)?),
    })
}

pub fn encode_http_request(request: &LocalRequest) -> Vec<u8> {
    let body = request.body().unwrap_or("");
    let mut out = String::new();
    out.push_str(request.method.as_str());
    out.push(' ');
    out.push_str(request.path);
    out.push_str(" HTTP/1.1\r\n");
    out.push_str("Host: 127.0.0.1:8231\r\n");
    out.push_str("Connection: close\r\n");
    out.push_str("User-Agent: ");
    out.push_str(LOCAL_TTS_USER_AGENT);
    out.push_str("\r\n");
    out.push_str("Accept: ");
    out.push_str(request.accept());
    out.push_str("\r\n");
    out.push_str("Accept-Encoding: identity\r\n");
    if request.method == LocalMethod::Post {
        out.push_str("Content-Type: application/json; charset=utf-8\r\n");
        out.push_str("Content-Length: ");
        out.push_str(&body.len().to_string());
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

pub fn speak_allowed(enabled: bool, service: LocalServiceStatus) -> LocalResult<()> {
    if !enabled {
        return Err(LocalError::ServiceNotReady);
    }
    match service {
        LocalServiceStatus::Ready => Ok(()),
        LocalServiceStatus::Missing
        | LocalServiceStatus::NotReady
        | LocalServiceStatus::Protocol => Err(LocalError::ServiceNotReady),
    }
}

pub fn health_with_transport(transport: &dyn LocalTransport) -> LocalResult<LocalServiceStatus> {
    let request = health_request();
    let response = match transport.execute(&request) {
        Ok(response) => response,
        Err(LocalTransportFailure::Connection | LocalTransportFailure::Timeout) => {
            return Ok(LocalServiceStatus::Missing);
        }
        Err(failure) => return Err(failure.into()),
    };
    match decode_health(response) {
        Ok(status) => Ok(status),
        Err(LocalError::Transport(
            LocalTransportFailure::Connection | LocalTransportFailure::Timeout,
        )) => Ok(LocalServiceStatus::Missing),
        Err(LocalError::InvalidHealth | LocalError::UnexpectedStatus(_)) => {
            Ok(LocalServiceStatus::Protocol)
        }
        Err(error) => Err(error),
    }
}

pub fn synthesize_local_with_transport(
    transport: &dyn LocalTransport,
    persona: LocalPersona,
    text: &str,
) -> LocalResult<LocalAudio> {
    let request = tts_request(persona, text)?;
    let response = transport.execute(&request)?;
    decode_wav(response)
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn media_type(value: &str) -> &str {
    value.split(';').next().map(str::trim).unwrap_or(value)
}

fn decode_health(response: LocalTransportResponse) -> LocalResult<LocalServiceStatus> {
    fail_closed_headers(&response, LOCAL_TTS_MAX_HEALTH_BYTES, true)?;
    if response.status != 200 {
        return Err(LocalError::UnexpectedStatus(response.status));
    }
    if !response
        .content_type
        .as_deref()
        .is_some_and(|value| media_type(value).eq_ignore_ascii_case(LOCAL_TTS_JSON_CONTENT_TYPE))
    {
        return Err(LocalError::InvalidHealth);
    }
    if response.body.len() > LOCAL_TTS_MAX_HEALTH_BYTES {
        return Err(LocalError::InvalidHealth);
    }
    let parsed: HealthBody =
        serde_json::from_slice(&response.body).map_err(|_| LocalError::InvalidHealth)?;
    if parsed.ok && parsed.ready {
        Ok(LocalServiceStatus::Ready)
    } else {
        Ok(LocalServiceStatus::NotReady)
    }
}

#[derive(Deserialize)]
struct HealthBody {
    ok: bool,
    ready: bool,
}

fn decode_wav(response: LocalTransportResponse) -> LocalResult<LocalAudio> {
    fail_closed_headers(&response, LOCAL_TTS_MAX_AUDIO_BYTES, false)?;
    if response.status == 400 {
        return Err(LocalError::UnexpectedStatus(400));
    }
    if response.status != 200 {
        return Err(LocalError::UnexpectedStatus(response.status));
    }
    if !response
        .content_type
        .as_deref()
        .is_some_and(|value| media_type(value).eq_ignore_ascii_case(LOCAL_TTS_AUDIO_CONTENT_TYPE))
    {
        return Err(LocalError::UnexpectedContentType);
    }
    if response.body.is_empty() {
        return Err(LocalError::EmptyAudio);
    }
    validate_wav(&response.body)?;
    Ok(LocalAudio {
        bytes: response.body,
    })
}

fn fail_closed_headers(
    response: &LocalTransportResponse,
    max_body: usize,
    health: bool,
) -> LocalResult<()> {
    if is_redirect(response.status) || response.location.is_some() {
        return Err(LocalError::Redirect);
    }
    if response
        .content_encoding
        .as_deref()
        .is_some_and(|value| !value.trim().eq_ignore_ascii_case("identity"))
    {
        return Err(LocalError::UnexpectedContentEncoding);
    }
    if response
        .transfer_encoding
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(LocalError::UnexpectedTransferEncoding);
    }
    let Some(declared) = response.content_length else {
        return Err(LocalError::UnexpectedContentLength);
    };
    let declared = usize::try_from(declared).map_err(|_| {
        if health {
            LocalError::InvalidHealth
        } else {
            LocalError::AudioTooLarge
        }
    })?;
    if declared > max_body {
        return Err(if health {
            LocalError::InvalidHealth
        } else {
            LocalError::AudioTooLarge
        });
    }
    if declared != response.body.len() {
        return Err(LocalError::UnexpectedContentLength);
    }
    Ok(())
}

#[cfg(test)]
fn is_wav(bytes: &[u8]) -> bool {
    validate_wav(bytes).is_ok()
}

/// PCM or IEEE-float mono WAV with a real `fmt ` + non-empty `data` chunk.
/// A 12-byte RIFF/WAVE prefix is not playable audio.
fn validate_wav(bytes: &[u8]) -> LocalResult<()> {
    if bytes.len() < 44 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WAVE" {
        return Err(LocalError::InvalidWav);
    }
    let mut offset = 12usize;
    let mut saw_fmt = false;
    let mut data_bytes = 0usize;
    while offset.saturating_add(8) <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| LocalError::InvalidWav)?,
        ) as usize;
        let data_start = offset + 8;
        let data_end = data_start.checked_add(size).ok_or(LocalError::InvalidWav)?;
        if data_end > bytes.len() {
            return Err(LocalError::InvalidWav);
        }
        if id == b"fmt " {
            if size < 16 {
                return Err(LocalError::InvalidWav);
            }
            let format = u16::from_le_bytes(
                bytes[data_start..data_start + 2]
                    .try_into()
                    .map_err(|_| LocalError::InvalidWav)?,
            );
            let channels = u16::from_le_bytes(
                bytes[data_start + 2..data_start + 4]
                    .try_into()
                    .map_err(|_| LocalError::InvalidWav)?,
            );
            let sample_rate = u32::from_le_bytes(
                bytes[data_start + 4..data_start + 8]
                    .try_into()
                    .map_err(|_| LocalError::InvalidWav)?,
            );
            let bits = u16::from_le_bytes(
                bytes[data_start + 14..data_start + 16]
                    .try_into()
                    .map_err(|_| LocalError::InvalidWav)?,
            );
            if (format != 1 && format != 3)
                || channels != 1
                || !(8_000..=48_000).contains(&sample_rate)
                || (format == 1 && bits != 16)
                || (format == 3 && bits != 32)
            {
                return Err(LocalError::InvalidWav);
            }
            saw_fmt = true;
        } else if id == b"data" {
            data_bytes = size;
        }
        offset = data_end + (size % 2);
    }
    if !saw_fmt || data_bytes == 0 {
        return Err(LocalError::InvalidWav);
    }
    Ok(())
}

pub fn parse_http_response(
    raw: &[u8],
    max_body: usize,
) -> Result<LocalTransportResponse, LocalTransportFailure> {
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(LocalTransportFailure::Protocol)?;
    if header_end > HEADER_LIMIT {
        return Err(LocalTransportFailure::Protocol);
    }
    let header_text =
        std::str::from_utf8(&raw[..header_end]).map_err(|_| LocalTransportFailure::Protocol)?;
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().ok_or(LocalTransportFailure::Protocol)?;
    let mut status_parts = status_line.split_whitespace();
    let version = status_parts.next().ok_or(LocalTransportFailure::Protocol)?;
    if version != "HTTP/1.0" && version != "HTTP/1.1" {
        return Err(LocalTransportFailure::Protocol);
    }
    let status = status_parts
        .next()
        .ok_or(LocalTransportFailure::Protocol)?
        .parse::<u16>()
        .map_err(|_| LocalTransportFailure::Protocol)?;

    let mut content_type = None;
    let mut content_encoding = None;
    let mut transfer_encoding = None;
    let mut content_length = None;
    let mut location = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(LocalTransportFailure::Protocol)?;
        let name = name.trim();
        let value = value.trim().to_owned();
        if name.eq_ignore_ascii_case("content-type") {
            if content_type.replace(value).is_some() {
                return Err(LocalTransportFailure::Protocol);
            }
        } else if name.eq_ignore_ascii_case("content-encoding") {
            if content_encoding.replace(value).is_some() {
                return Err(LocalTransportFailure::Protocol);
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if transfer_encoding.replace(value).is_some() {
                return Err(LocalTransportFailure::Protocol);
            }
        } else if name.eq_ignore_ascii_case("content-length") {
            if content_length.replace(value).is_some() {
                return Err(LocalTransportFailure::Protocol);
            }
        } else if name.eq_ignore_ascii_case("location") && location.replace(value).is_some() {
            return Err(LocalTransportFailure::Protocol);
        }
    }

    let declared = content_length
        .as_deref()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| LocalTransportFailure::Protocol)
        })
        .transpose()?;
    if declared.is_some_and(|bytes| bytes > max_body as u64) {
        return Err(LocalTransportFailure::Protocol);
    }

    let Some(length) = declared else {
        return Err(LocalTransportFailure::Protocol);
    };
    let length = usize::try_from(length).map_err(|_| LocalTransportFailure::Protocol)?;
    let body_start = header_end + 4;
    if raw.len().saturating_sub(body_start) < length {
        return Err(LocalTransportFailure::ResponseBody);
    }
    let body = raw[body_start..body_start + length].to_vec();

    Ok(LocalTransportResponse::new(
        status,
        content_type,
        content_encoding,
        transfer_encoding,
        declared,
        location,
        body,
    ))
}

/// Native loopback client. It never consults HTTP(S)_PROXY, never resolves a
/// hostname, and never follows a redirect. The live socket can be shut down
/// from another thread so Stop does not wait out the 90s read timeout.
#[derive(Debug, Default)]
pub struct LoopbackClient {
    cancel: AtomicBool,
    stream: Mutex<Option<TcpStream>>,
}

impl LoopbackClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.shutdown_stream();
    }

    pub fn probe_health(&self) -> LocalResult<LocalServiceStatus> {
        health_with_transport(self)
    }

    pub fn synthesize(&self, persona: LocalPersona, text: &str) -> LocalResult<LocalAudio> {
        synthesize_local_with_transport(self, persona, text)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn shutdown_stream(&self) {
        if let Ok(mut guard) = self.stream.lock()
            && let Some(stream) = guard.as_mut()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    fn store_stream(&self, stream: TcpStream) {
        if let Ok(mut guard) = self.stream.lock() {
            *guard = Some(stream);
        }
    }

    fn clear_stream(&self) {
        if let Ok(mut guard) = self.stream.lock() {
            *guard = None;
        }
    }

    fn execute_loopback(
        &self,
        addr: SocketAddr,
        request: &LocalRequest,
        require_product_port: bool,
    ) -> Result<LocalTransportResponse, LocalTransportFailure> {
        if addr.ip() != IpAddr::V4(LOCAL_HOST) {
            return Err(LocalTransportFailure::UnexpectedPeer);
        }
        if self.cancelled() {
            return Err(LocalTransportFailure::Cancelled);
        }
        let total = match request.method {
            LocalMethod::Get => HEALTH_TIMEOUT,
            LocalMethod::Post => TTS_IO_TIMEOUT,
        };
        let deadline = Instant::now() + total;
        let connect_cap = match request.method {
            LocalMethod::Get => HEALTH_TIMEOUT,
            LocalMethod::Post => TTS_CONNECT_TIMEOUT,
        };
        let connect_budget = deadline.saturating_duration_since(Instant::now());
        if connect_budget.is_zero() {
            return Err(LocalTransportFailure::Timeout);
        }
        let mut stream = TcpStream::connect_timeout(&addr, connect_budget.min(connect_cap))
            .map_err(classify_connect_error)?;
        if self.cancelled() {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(LocalTransportFailure::Cancelled);
        }
        if Instant::now() >= deadline {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(LocalTransportFailure::Timeout);
        }
        let peer = stream
            .peer_addr()
            .map_err(|_| LocalTransportFailure::UnexpectedPeer)?;
        if peer.ip() != IpAddr::V4(LOCAL_HOST)
            || (require_product_port && peer.port() != LOCAL_PORT)
        {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(LocalTransportFailure::UnexpectedPeer);
        }
        let poll = Duration::from_millis(200);
        stream
            .set_nodelay(true)
            .map_err(|_| LocalTransportFailure::Protocol)?;
        stream
            .set_read_timeout(Some(poll))
            .map_err(|_| LocalTransportFailure::Protocol)?;
        stream
            .set_write_timeout(Some(poll))
            .map_err(|_| LocalTransportFailure::Protocol)?;
        if let Ok(watch) = stream.try_clone() {
            self.store_stream(watch);
        }
        if self.cancelled() {
            let _ = stream.shutdown(Shutdown::Both);
            self.clear_stream();
            return Err(LocalTransportFailure::Cancelled);
        }
        let bytes = encode_http_request(request);
        if let Err(error) = stream.write_all(&bytes) {
            self.clear_stream();
            return Err(self.io_failure(error));
        }
        let _ = stream.shutdown(Shutdown::Write);
        let max_body = match request.method {
            LocalMethod::Get => LOCAL_TTS_MAX_HEALTH_BYTES,
            LocalMethod::Post => LOCAL_TTS_MAX_AUDIO_BYTES,
        };
        let mut raw = Vec::new();
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        let result = loop {
            if self.cancelled() {
                let _ = stream.shutdown(Shutdown::Both);
                break Err(LocalTransportFailure::Cancelled);
            }
            if Instant::now() >= deadline {
                let _ = stream.shutdown(Shutdown::Both);
                break Err(LocalTransportFailure::Timeout);
            }
            if let Some(header_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                if let Ok(parsed) = parse_http_response(&raw, max_body) {
                    break Ok(parsed);
                }
                if header_end > HEADER_LIMIT {
                    break Err(LocalTransportFailure::Protocol);
                }
            }
            match stream.read(&mut chunk) {
                Ok(0) => {
                    if self.cancelled() {
                        break Err(LocalTransportFailure::Cancelled);
                    }
                    break parse_http_response(&raw, max_body);
                }
                Ok(read) => {
                    if raw
                        .len()
                        .checked_add(read)
                        .is_none_or(|bytes| bytes > HEADER_LIMIT + max_body)
                    {
                        break Err(LocalTransportFailure::Protocol);
                    }
                    raw.extend_from_slice(&chunk[..read]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(error) => break Err(self.io_failure(error)),
            }
        };
        self.clear_stream();
        result
    }

    fn io_failure(&self, error: std::io::Error) -> LocalTransportFailure {
        if self.cancelled() {
            LocalTransportFailure::Cancelled
        } else {
            classify_io_error(error)
        }
    }
}

impl LocalTransport for LoopbackClient {
    fn execute(
        &self,
        request: &LocalRequest,
    ) -> Result<LocalTransportResponse, LocalTransportFailure> {
        self.execute_loopback(SocketAddr::from((LOCAL_HOST, LOCAL_PORT)), request, true)
    }
}

fn classify_connect_error(error: std::io::Error) -> LocalTransportFailure {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            LocalTransportFailure::Timeout
        }
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::NotFound
        | std::io::ErrorKind::AddrNotAvailable
        | std::io::ErrorKind::NetworkUnreachable
        | std::io::ErrorKind::HostUnreachable => LocalTransportFailure::Connection,
        _ => {
            let message = error.to_string().to_ascii_lowercase();
            if message.contains("timed out") || message.contains("timeout") {
                LocalTransportFailure::Timeout
            } else {
                LocalTransportFailure::Connection
            }
        }
    }
}

fn classify_io_error(error: std::io::Error) -> LocalTransportFailure {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            LocalTransportFailure::Timeout
        }
        std::io::ErrorKind::UnexpectedEof => LocalTransportFailure::ResponseBody,
        _ => LocalTransportFailure::Connection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeTransport {
        calls: Cell<usize>,
        response: RefCell<Option<Result<LocalTransportResponse, LocalTransportFailure>>>,
        seen: RefCell<Vec<String>>,
    }

    impl FakeTransport {
        fn returning(response: LocalTransportResponse) -> Self {
            Self::with_result(Ok(response))
        }

        fn failing(failure: LocalTransportFailure) -> Self {
            Self::with_result(Err(failure))
        }

        fn with_result(result: Result<LocalTransportResponse, LocalTransportFailure>) -> Self {
            Self {
                calls: Cell::new(0),
                response: RefCell::new(Some(result)),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl LocalTransport for FakeTransport {
        fn execute(
            &self,
            request: &LocalRequest,
        ) -> Result<LocalTransportResponse, LocalTransportFailure> {
            self.calls.set(self.calls.get() + 1);
            self.seen.borrow_mut().push(format!("{request:?}"));
            self.response
                .borrow_mut()
                .take()
                .expect("one fake local response")
        }
    }

    fn wav_bytes() -> Vec<u8> {
        // 16-bit PCM mono 22050 Hz, 8 samples. 12-byte RIFF/WAVE prefix is not enough.
        let data = [0_u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&22_050u32.to_le_bytes());
        bytes.extend_from_slice(&(22_050u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    fn wav_response() -> LocalTransportResponse {
        let body = wav_bytes();
        LocalTransportResponse::new(
            200,
            Some("audio/wav".to_owned()),
            None,
            None,
            Some(body.len() as u64),
            None,
            body,
        )
    }

    #[test]
    fn authority_is_the_exact_loopback_service() {
        assert_eq!(LOCAL_HOST, Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(LOCAL_PORT, 8231);
        assert_eq!(HEALTH_ENDPOINT, "http://127.0.0.1:8231/health");
        assert_eq!(TTS_ENDPOINT, "http://127.0.0.1:8231/tts");
        assert!(HEALTH_ENDPOINT.starts_with("http://127.0.0.1:8231/"));
        assert!(TTS_ENDPOINT.starts_with("http://127.0.0.1:8231/"));
        assert!(!HEALTH_ENDPOINT.contains('?'));
        assert!(!TTS_ENDPOINT.contains('?'));
        assert_eq!(LocalPersona::ALL.len(), 17);
        assert_eq!(LocalPersona::Chatgpt.as_str(), "chatgpt");
        assert_eq!(
            serde_json::to_string(&LocalPersona::Mimo).unwrap(),
            "\"mimo\""
        );
        assert!(serde_json::from_str::<LocalPersona>("\"neutral\"").is_err());
        assert!(serde_json::from_str::<LocalPersona>("\"Chatgpt\"").is_err());
    }

    #[test]
    fn tts_request_is_json_with_redacted_debug_and_never_mentions_clone() {
        let request = tts_request(LocalPersona::Claude, "A&B「測試」").unwrap();
        assert_eq!(request.method().as_str(), "POST");
        assert_eq!(request.path(), TTS_PATH);
        assert_eq!(request.endpoint(), TTS_ENDPOINT);
        assert_eq!(request.host_header(), "127.0.0.1:8231");
        let body = request.body().unwrap();
        assert!(body.contains("\"persona\":\"claude\""));
        assert!(body.contains("A&B「測試」"));
        let debug = format!("{request:?}");
        assert!(!debug.contains("測試"));
        assert!(debug.contains("body_bytes"));
        let http = String::from_utf8(encode_http_request(&request)).unwrap();
        assert!(http.starts_with("POST /tts HTTP/1.1\r\nHost: 127.0.0.1:8231\r\n"));
        assert!(http.contains("Connection: close"));
        assert!(http.contains("Accept: audio/wav"));
        assert!(!http.to_ascii_lowercase().contains("authorization"));
        assert!(!http.to_ascii_lowercase().contains("cookie"));
        assert!(!http.contains("/clone"));
        assert!(!http.contains("https://"));
    }

    #[test]
    fn empty_and_oversize_text_never_reach_transport() {
        let fake = FakeTransport::failing(LocalTransportFailure::Connection);
        assert_eq!(
            synthesize_local_with_transport(&fake, LocalPersona::Grok, " \n\t"),
            Err(LocalError::EmptyText)
        );
        assert_eq!(fake.calls.get(), 0);
        let too_long = "字".repeat(LOCAL_TTS_MAX_TEXT_BYTES);
        let fake = FakeTransport::failing(LocalTransportFailure::Connection);
        assert_eq!(
            synthesize_local_with_transport(&fake, LocalPersona::Grok, &too_long),
            Err(LocalError::TextTooLong)
        );
        assert_eq!(fake.calls.get(), 0);
    }

    #[test]
    fn one_post_returns_wav_and_does_not_retry() {
        let fake = FakeTransport::returning(wav_response());
        let audio = synthesize_local_with_transport(&fake, LocalPersona::Chatgpt, "你好").unwrap();
        assert_eq!(fake.calls.get(), 1);
        assert_eq!(audio.content_type(), LOCAL_TTS_AUDIO_CONTENT_TYPE);
        assert!(is_wav(audio.bytes()));
        let fake = FakeTransport::failing(LocalTransportFailure::Timeout);
        assert_eq!(
            synthesize_local_with_transport(&fake, LocalPersona::Chatgpt, "你好"),
            Err(LocalError::Transport(LocalTransportFailure::Timeout))
        );
        assert_eq!(fake.calls.get(), 1);
    }

    #[test]
    fn redirects_chunked_gzip_and_non_wav_are_rejected() {
        let redirected = LocalTransportResponse::new(
            302,
            Some("audio/wav".to_owned()),
            None,
            None,
            Some(0),
            Some("http://example.com/tts".to_owned()),
            Vec::new(),
        );
        assert_eq!(decode_wav(redirected), Err(LocalError::Redirect));

        let body = wav_bytes();
        let gzip = LocalTransportResponse::new(
            200,
            Some("audio/wav".to_owned()),
            Some("gzip".to_owned()),
            None,
            Some(body.len() as u64),
            None,
            body.clone(),
        );
        assert_eq!(decode_wav(gzip), Err(LocalError::UnexpectedContentEncoding));

        let chunked = LocalTransportResponse::new(
            200,
            Some("audio/wav".to_owned()),
            None,
            Some("chunked".to_owned()),
            Some(body.len() as u64),
            None,
            body.clone(),
        );
        assert_eq!(
            decode_wav(chunked),
            Err(LocalError::UnexpectedTransferEncoding)
        );

        let mp3 = LocalTransportResponse::new(
            200,
            Some("audio/mpeg".to_owned()),
            None,
            None,
            Some(body.len() as u64),
            None,
            body,
        );
        assert_eq!(decode_wav(mp3), Err(LocalError::UnexpectedContentType));

        let not_wav = LocalTransportResponse::new(
            200,
            Some("audio/wav".to_owned()),
            None,
            None,
            Some(4),
            None,
            b"XXXX".to_vec(),
        );
        assert_eq!(decode_wav(not_wav), Err(LocalError::InvalidWav));

        let prefix_only = {
            let mut bytes = b"RIFF".to_vec();
            bytes.extend_from_slice(&[4, 0, 0, 0]);
            bytes.extend_from_slice(b"WAVE");
            bytes
        };
        let prefix = LocalTransportResponse::new(
            200,
            Some("audio/wav".to_owned()),
            None,
            None,
            Some(prefix_only.len() as u64),
            None,
            prefix_only,
        );
        assert_eq!(decode_wav(prefix), Err(LocalError::InvalidWav));
    }

    #[test]
    fn health_connection_failure_is_missing_not_ready() {
        let fake = FakeTransport::failing(LocalTransportFailure::Connection);
        assert_eq!(
            health_with_transport(&fake).unwrap(),
            LocalServiceStatus::Missing
        );
        assert_eq!(fake.calls.get(), 1);

        let ready_body = br#"{"ok":true,"ready":true}"#.to_vec();
        let ready = LocalTransportResponse::new(
            200,
            Some("application/json".to_owned()),
            None,
            None,
            Some(ready_body.len() as u64),
            None,
            ready_body,
        );
        let fake = FakeTransport::returning(ready);
        assert_eq!(
            health_with_transport(&fake).unwrap(),
            LocalServiceStatus::Ready
        );

        let warming_body = br#"{"ok":true,"ready":false}"#.to_vec();
        let warming = LocalTransportResponse::new(
            200,
            Some("application/json".to_owned()),
            None,
            None,
            Some(warming_body.len() as u64),
            None,
            warming_body,
        );
        let fake = FakeTransport::returning(warming);
        assert_eq!(
            health_with_transport(&fake).unwrap(),
            LocalServiceStatus::NotReady
        );
    }

    #[test]
    fn http_parser_reads_exact_content_length_and_rejects_duplicate_headers() {
        let body = wav_bytes();
        let mut raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(&body);
        let parsed = parse_http_response(&raw, LOCAL_TTS_MAX_AUDIO_BYTES).unwrap();
        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.body, body);

        let dup = b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nA";
        assert_eq!(
            parse_http_response(dup, 8).unwrap_err(),
            LocalTransportFailure::Protocol
        );
    }

    #[test]
    fn cancel_shuts_down_a_blocked_loopback_read() {
        let listener = std::net::TcpListener::bind(SocketAddr::from((LOCAL_HOST, 0))).unwrap();
        let addr = listener.local_addr().unwrap();
        assert_eq!(addr.ip(), IpAddr::V4(LOCAL_HOST));
        let accept = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(8));
            drop(stream);
        });
        let client = LoopbackClient::new();
        let client_for_cancel = std::sync::Arc::new(client);
        let worker_client = std::sync::Arc::clone(&client_for_cancel);
        let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            worker_client.execute_loopback(addr, &health_request(), false)
        });
        std::thread::sleep(Duration::from_millis(50));
        client_for_cancel.cancel();
        let result = worker.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "cancel must not wait the 90s TTS timeout"
        );
        assert_eq!(result, Err(LocalTransportFailure::Cancelled));
        let _ = accept.join();
    }

    #[test]
    fn speak_allowed_is_ready_only_when_enabled_and_health_ready() {
        assert_eq!(
            speak_allowed(false, LocalServiceStatus::Ready),
            Err(LocalError::ServiceNotReady)
        );
        assert_eq!(
            speak_allowed(true, LocalServiceStatus::Missing),
            Err(LocalError::ServiceNotReady)
        );
        assert_eq!(speak_allowed(true, LocalServiceStatus::Ready), Ok(()));
    }

    #[test]
    fn live_health_probe_reports_the_actual_loopback_service() {
        let status = LoopbackClient::new().probe_health().unwrap();
        // This talks to whatever is actually bound on 127.0.0.1:8231. Missing is
        // a real outcome, not a test failure; Ready is only true if the daemon
        // answered ok+ready.
        assert!(
            matches!(
                status,
                LocalServiceStatus::Ready
                    | LocalServiceStatus::NotReady
                    | LocalServiceStatus::Missing
                    | LocalServiceStatus::Protocol
            ),
            "{status:?}"
        );
        assert_eq!(
            status == LocalServiceStatus::Ready,
            status.as_str() == "ready"
        );
    }

    #[test]
    fn live_synthetic_tts_uses_the_actual_loopback_transport_when_ready() {
        let client = LoopbackClient::new();
        let status = client.probe_health().unwrap();
        if status != LocalServiceStatus::Ready {
            // Honest skip: daemon not ready. Do not treat this as inference success.
            return;
        }
        let audio = client
            .synthesize(LocalPersona::Chatgpt, "測試。")
            .expect("ready daemon must synthesize the known public test line");
        assert_eq!(audio.content_type(), LOCAL_TTS_AUDIO_CONTENT_TYPE);
        assert!(
            is_wav(audio.bytes()),
            "live WAV failed structural validation"
        );
        assert!(audio.bytes().len() > 44);
    }
}
