//! AI-Sister 的文字轉語音邊界。
//!
//! 預設 feature 只包含 typed authority、SSML 編碼與 response 驗證，不含 HTTP
//! client。只有明確啟用 `azure` feature 才會編入 `ureq`。這個 crate 不保存
//! credential、不接受任意 endpoint，也不在錯誤或 `Debug` 輸出裡帶出 key／文字。
//! Consent authority 留在 desktop／sister-core 邊界；desktop 的唯一呼叫 helper 必須
//! by-value 消耗持有 shared consent lock 的 `AzureTtsAdmissionGuard`，本 crate 不自行
//! 建立或假裝驗證同意狀態。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;
use std::time::Duration;

#[cfg(feature = "azure")]
mod native;

#[cfg(feature = "azure")]
pub use native::AzureClient;

pub const SSML_CONTENT_TYPE: &str = "application/ssml+xml";
pub const AUDIO_CONTENT_TYPE: &str = "audio/mpeg";
pub const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";
pub const USER_AGENT: &str = "AI-Sister/1 sister-tts";

/// The request body is bounded before transport. Azure documents a ten-minute response cap;
/// keeping our SSML well below that also bounds accidental egress from one admitted new-answer
/// or trusted replay request.
pub const MAX_SSML_BYTES: usize = 64 * 1024;

/// 24 kHz / 48 kbit MP3 needs about 3.6 MB for Azure's documented ten-minute maximum. Eight MiB
/// leaves protocol overhead room while still making a corrupt or hostile response finite.
pub const MAX_AUDIO_BYTES: usize = 8 * 1024 * 1024;

pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
pub const RECEIVE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
pub const RECEIVE_BODY_TIMEOUT: Duration = Duration::from_secs(30);
pub const GLOBAL_TIMEOUT: Duration = Duration::from_secs(45);

const READ_CHUNK_BYTES: usize = 16 * 1024;
pub const MAX_SUBSCRIPTION_KEY_BYTES: usize = 256;

/// The only Azure public-cloud regions alpha.109 may contact.
///
/// Serde values are a persisted wire contract shared by config and the settings UI. Do not derive
/// them from Rust variant spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AzureRegion {
    #[serde(rename = "eastasia")]
    EastAsia,
    #[serde(rename = "southeastasia")]
    SoutheastAsia,
    #[serde(rename = "japaneast")]
    JapanEast,
}

impl AzureRegion {
    pub const ALL: [Self; 3] = [Self::EastAsia, Self::SoutheastAsia, Self::JapanEast];

    pub const fn id(self) -> &'static str {
        match self {
            Self::EastAsia => "eastasia",
            Self::SoutheastAsia => "southeastasia",
            Self::JapanEast => "japaneast",
        }
    }

    /// Exact host shown in the egress disclosure and used by the native transport.
    pub const fn host(self) -> &'static str {
        match self {
            Self::EastAsia => "eastasia.tts.speech.microsoft.com",
            Self::SoutheastAsia => "southeastasia.tts.speech.microsoft.com",
            Self::JapanEast => "japaneast.tts.speech.microsoft.com",
        }
    }

    /// Exact Azure Speech synthesis endpoint. Callers cannot supply or extend this URL.
    pub const fn endpoint(self) -> &'static str {
        match self {
            Self::EastAsia => "https://eastasia.tts.speech.microsoft.com/cognitiveservices/v1",
            Self::SoutheastAsia => {
                "https://southeastasia.tts.speech.microsoft.com/cognitiveservices/v1"
            }
            Self::JapanEast => "https://japaneast.tts.speech.microsoft.com/cognitiveservices/v1",
        }
    }
}

impl fmt::Display for AzureRegion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// The three generally available Taiwanese Mandarin voices exposed by alpha.109.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Voice {
    #[serde(rename = "zh-TW-HsiaoChenNeural")]
    HsiaoChen,
    #[serde(rename = "zh-TW-YunJheNeural")]
    YunJhe,
    #[serde(rename = "zh-TW-HsiaoYuNeural")]
    HsiaoYu,
}

impl Voice {
    pub const ALL: [Self; 3] = [Self::HsiaoChen, Self::YunJhe, Self::HsiaoYu];

    pub const fn short_name(self) -> &'static str {
        match self {
            Self::HsiaoChen => "zh-TW-HsiaoChenNeural",
            Self::YunJhe => "zh-TW-YunJheNeural",
            Self::HsiaoYu => "zh-TW-HsiaoYuNeural",
        }
    }

    pub const fn locale(self) -> &'static str {
        "zh-TW"
    }
}

impl fmt::Display for Voice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.short_name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    HostResolution,
    Tls,
    Timeout,
    Connection,
    Protocol,
    ResponseBody,
    Other,
}

impl fmt::Display for TransportFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HostResolution => "主機名稱解析失敗",
            Self::Tls => "TLS 憑證驗證或握手失敗",
            Self::Timeout => "連線逾時",
            Self::Connection => "網路連線失敗",
            Self::Protocol => "HTTP 傳輸格式失敗",
            Self::ResponseBody => "回應內容讀取失敗",
            Self::Other => "傳輸失敗",
        })
    }
}

impl std::error::Error for TransportFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyText,
    TextTooLong,
    InvalidXmlCharacter,
    InvalidSubscriptionKey,
    Transport(TransportFailure),
    UnexpectedStatus(u16),
    UnexpectedFinalUrl,
    UnexpectedContentType,
    UnexpectedContentEncoding,
    UnexpectedContentLength,
    EmptyAudio,
    AudioTooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText => formatter.write_str("沒有可朗讀的文字"),
            Self::TextTooLong => formatter.write_str("這次朗讀的 SSML 超過單次上限"),
            Self::InvalidXmlCharacter => formatter.write_str("朗讀文字含有 SSML 不允許的字元"),
            Self::InvalidSubscriptionKey => formatter.write_str("Azure Speech key 格式無效"),
            Self::Transport(failure) => write!(formatter, "Azure Speech 連線失敗（{failure}）"),
            Self::UnexpectedStatus(status) => {
                write!(formatter, "Azure Speech 回了未允許的 HTTP 狀態：{status}")
            }
            Self::UnexpectedFinalUrl => {
                formatter.write_str("Azure Speech 回應的最終網址不是所選區域的固定 endpoint")
            }
            Self::UnexpectedContentType => {
                formatter.write_str("Azure Speech 回了未允許的 Content-Type")
            }
            Self::UnexpectedContentEncoding => {
                formatter.write_str("Azure Speech 回了未允許的 Content-Encoding")
            }
            Self::UnexpectedContentLength => {
                formatter.write_str("Azure Speech 回應長度和宣告不一致")
            }
            Self::EmptyAudio => formatter.write_str("Azure Speech 成功回應裡沒有音訊"),
            Self::AudioTooLarge => formatter.write_str("Azure Speech 回應超過單次音訊上限"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(failure) => Some(failure),
            _ => None,
        }
    }
}

impl From<TransportFailure> for Error {
    fn from(value: TransportFailure) -> Self {
        Self::Transport(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A complete, immutable POST request. Its endpoint, headers, voice and SSML are constructed by
/// this crate; consumers can only choose typed authority and provide the ephemeral credential and
/// text. `Debug` intentionally exposes neither secret nor text.
pub struct PostRequest<'a> {
    region: AzureRegion,
    voice: Voice,
    subscription_key: &'a str,
    ssml: String,
}

impl PostRequest<'_> {
    pub const fn region(&self) -> AzureRegion {
        self.region
    }

    pub const fn voice(&self) -> Voice {
        self.voice
    }

    pub const fn host(&self) -> &'static str {
        self.region.host()
    }

    pub const fn endpoint(&self) -> &'static str {
        self.region.endpoint()
    }

    pub const fn method(&self) -> &'static str {
        "POST"
    }

    pub const fn content_type(&self) -> &'static str {
        SSML_CONTENT_TYPE
    }

    pub const fn accept(&self) -> &'static str {
        AUDIO_CONTENT_TYPE
    }

    pub const fn output_format(&self) -> &'static str {
        OUTPUT_FORMAT
    }

    pub const fn user_agent(&self) -> &'static str {
        USER_AGENT
    }

    pub const fn subscription_key(&self) -> &str {
        self.subscription_key
    }

    pub fn ssml(&self) -> &str {
        &self.ssml
    }
}

impl fmt::Debug for PostRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostRequest")
            .field("region", &self.region)
            .field("voice", &self.voice)
            .field("method", &"POST")
            .field("endpoint", &self.endpoint())
            .field("subscription_key", &"[redacted]")
            .field("ssml_bytes", &self.ssml.len())
            .finish()
    }
}

/// Response metadata plus a streaming body. A fake transport can construct this without enabling
/// the `azure` feature, so all validation paths stay under ordinary unit-test coverage.
pub struct TransportResponse {
    status: u16,
    final_url: String,
    content_type: Option<String>,
    content_encoding: Option<String>,
    content_length: Option<u64>,
    body: Box<dyn Read>,
}

impl TransportResponse {
    pub fn new(
        status: u16,
        final_url: impl Into<String>,
        content_type: Option<String>,
        content_encoding: Option<String>,
        content_length: Option<u64>,
        body: impl Read + 'static,
    ) -> Self {
        Self {
            status,
            final_url: final_url.into(),
            content_type,
            content_encoding,
            content_length,
            body: Box::new(body),
        }
    }
}

/// The single injectable transport seam. One call to `synthesize_with_transport` invokes `post`
/// at most once. Implementations must not retry; the native implementation follows that contract.
pub trait Transport {
    fn post(
        &self,
        request: &PostRequest<'_>,
    ) -> std::result::Result<TransportResponse, TransportFailure>;
}

#[derive(PartialEq, Eq)]
pub struct Audio {
    bytes: Vec<u8>,
}

impl Audio {
    pub const fn content_type(&self) -> &'static str {
        AUDIO_CONTENT_TYPE
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl fmt::Debug for Audio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Audio")
            .field("content_type", &AUDIO_CONTENT_TYPE)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Escape untrusted plain text and wrap it in the only SSML shape this crate sends.
pub fn build_ssml(voice: Voice, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err(Error::EmptyText);
    }
    if text.len() > MAX_SSML_BYTES {
        return Err(Error::TextTooLong);
    }
    if text
        .chars()
        .any(|character| !is_xml_1_0_character(character))
    {
        return Err(Error::InvalidXmlCharacter);
    }

    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }

    let ssml = format!(
        "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"zh-TW\"><voice name=\"{}\">{escaped}</voice></speak>",
        voice.short_name()
    );
    if ssml.len() > MAX_SSML_BYTES {
        return Err(Error::TextTooLong);
    }
    Ok(ssml)
}

/// Execute exactly one typed synthesis request through an injected transport.
///
/// The borrowed subscription key is validated, handed to `post`, and never retained. This
/// low-level function neither mints nor verifies consent and does not retry. Desktop must keep its
/// only call site behind the helper that consumes `AzureTtsAdmissionGuard`; another request
/// requires that boundary to authorize another call.
pub fn synthesize_with_transport(
    transport: &dyn Transport,
    region: AzureRegion,
    voice: Voice,
    subscription_key: &str,
    text: &str,
) -> Result<Audio> {
    validate_subscription_key(subscription_key)?;
    let request = PostRequest {
        region,
        voice,
        subscription_key,
        ssml: build_ssml(voice, text)?,
    };
    let response = transport.post(&request)?;
    validate_response(response, request.endpoint(), MAX_AUDIO_BYTES)
}

fn validate_subscription_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > MAX_SUBSCRIPTION_KEY_BYTES
        || !key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(Error::InvalidSubscriptionKey);
    }
    Ok(())
}

fn is_xml_1_0_character(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{a}' | '\u{d}')
        || ('\u{20}'..='\u{d7ff}').contains(&character)
        || ('\u{e000}'..='\u{fffd}').contains(&character)
        || ('\u{10000}'..='\u{10ffff}').contains(&character)
}

fn is_audio_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(AUDIO_CONTENT_TYPE))
}

fn validate_response(
    mut response: TransportResponse,
    expected_url: &str,
    max_audio_bytes: usize,
) -> Result<Audio> {
    if response.status != 200 {
        return Err(Error::UnexpectedStatus(response.status));
    }
    if response.final_url != expected_url {
        return Err(Error::UnexpectedFinalUrl);
    }
    if !response
        .content_type
        .as_deref()
        .is_some_and(is_audio_content_type)
    {
        return Err(Error::UnexpectedContentType);
    }
    if response
        .content_encoding
        .as_deref()
        .is_some_and(|value| !value.trim().eq_ignore_ascii_case("identity"))
    {
        return Err(Error::UnexpectedContentEncoding);
    }

    let declared_length = response
        .content_length
        .map(usize::try_from)
        .transpose()
        .map_err(|_| Error::AudioTooLarge)?;
    if declared_length.is_some_and(|bytes| bytes > max_audio_bytes) {
        return Err(Error::AudioTooLarge);
    }

    let mut audio = Vec::with_capacity(declared_length.unwrap_or(READ_CHUNK_BYTES));
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    loop {
        let read = response
            .body
            .read(&mut chunk)
            .map_err(|_| Error::Transport(TransportFailure::ResponseBody))?;
        if read == 0 {
            break;
        }
        if audio
            .len()
            .checked_add(read)
            .is_none_or(|bytes| bytes > max_audio_bytes)
        {
            return Err(Error::AudioTooLarge);
        }
        audio.extend_from_slice(&chunk[..read]);
    }

    if audio.is_empty() {
        return Err(Error::EmptyAudio);
    }
    if declared_length.is_some_and(|bytes| bytes != audio.len()) {
        return Err(Error::UnexpectedContentLength);
    }
    Ok(Audio { bytes: audio })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::io::{self, Cursor};

    struct FakeTransport {
        calls: Cell<usize>,
        response: RefCell<Option<std::result::Result<TransportResponse, TransportFailure>>>,
        seen: RefCell<Vec<SeenRequest>>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SeenRequest {
        method: &'static str,
        region: AzureRegion,
        voice: Voice,
        endpoint: &'static str,
        host: &'static str,
        content_type: &'static str,
        accept: &'static str,
        output_format: &'static str,
        user_agent: &'static str,
        key: String,
        ssml: String,
    }

    impl FakeTransport {
        fn returning(response: TransportResponse) -> Self {
            Self::with_result(Ok(response))
        }

        fn failing(failure: TransportFailure) -> Self {
            Self::with_result(Err(failure))
        }

        fn with_result(result: std::result::Result<TransportResponse, TransportFailure>) -> Self {
            Self {
                calls: Cell::new(0),
                response: RefCell::new(Some(result)),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for FakeTransport {
        fn post(
            &self,
            request: &PostRequest<'_>,
        ) -> std::result::Result<TransportResponse, TransportFailure> {
            self.calls.set(self.calls.get() + 1);
            self.seen.borrow_mut().push(SeenRequest {
                method: request.method(),
                region: request.region(),
                voice: request.voice(),
                endpoint: request.endpoint(),
                host: request.host(),
                content_type: request.content_type(),
                accept: request.accept(),
                output_format: request.output_format(),
                user_agent: request.user_agent(),
                key: request.subscription_key().to_owned(),
                ssml: request.ssml().to_owned(),
            });
            self.response
                .borrow_mut()
                .take()
                .expect("one fake response")
        }
    }

    fn response(
        region: AzureRegion,
        status: u16,
        content_type: Option<&str>,
        content_length: Option<u64>,
        body: impl Read + 'static,
    ) -> TransportResponse {
        TransportResponse::new(
            status,
            region.endpoint(),
            content_type.map(str::to_owned),
            None,
            content_length,
            body,
        )
    }

    struct PanicReader;

    impl Read for PanicReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            panic!("invalid metadata must be rejected before reading the body")
        }
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("fake response failure"))
        }
    }

    #[test]
    fn region_serde_hosts_and_endpoints_are_the_exact_three_item_authority() {
        let expected = [
            (
                AzureRegion::EastAsia,
                "eastasia",
                "eastasia.tts.speech.microsoft.com",
                "https://eastasia.tts.speech.microsoft.com/cognitiveservices/v1",
            ),
            (
                AzureRegion::SoutheastAsia,
                "southeastasia",
                "southeastasia.tts.speech.microsoft.com",
                "https://southeastasia.tts.speech.microsoft.com/cognitiveservices/v1",
            ),
            (
                AzureRegion::JapanEast,
                "japaneast",
                "japaneast.tts.speech.microsoft.com",
                "https://japaneast.tts.speech.microsoft.com/cognitiveservices/v1",
            ),
        ];
        assert_eq!(AzureRegion::ALL.len(), expected.len());
        for (region, id, host, endpoint) in expected {
            assert!(AzureRegion::ALL.contains(&region));
            assert_eq!(region.id(), id);
            assert_eq!(region.to_string(), id);
            assert_eq!(region.host(), host);
            assert_eq!(region.endpoint(), endpoint);
            assert_eq!(serde_json::to_string(&region).unwrap(), format!("\"{id}\""));
            assert_eq!(
                serde_json::from_str::<AzureRegion>(&format!("\"{id}\"")).unwrap(),
                region
            );
            assert!(endpoint.starts_with("https://"));
            assert!(endpoint.ends_with("/cognitiveservices/v1"));
            assert!(!endpoint.contains('?'));
        }
        assert!(serde_json::from_str::<AzureRegion>("\"westus\"").is_err());
        assert!(serde_json::from_str::<AzureRegion>("\"EastAsia\"").is_err());
    }

    #[test]
    fn voice_serde_is_exact_and_rejects_free_form_names() {
        let expected = [
            (Voice::HsiaoChen, "zh-TW-HsiaoChenNeural"),
            (Voice::YunJhe, "zh-TW-YunJheNeural"),
            (Voice::HsiaoYu, "zh-TW-HsiaoYuNeural"),
        ];
        assert_eq!(Voice::ALL.len(), expected.len());
        for (voice, short_name) in expected {
            assert!(Voice::ALL.contains(&voice));
            assert_eq!(voice.short_name(), short_name);
            assert_eq!(voice.locale(), "zh-TW");
            assert_eq!(voice.to_string(), short_name);
            assert_eq!(
                serde_json::to_string(&voice).unwrap(),
                format!("\"{short_name}\"")
            );
            assert_eq!(
                serde_json::from_str::<Voice>(&format!("\"{short_name}\"")).unwrap(),
                voice
            );
        }
        assert!(serde_json::from_str::<Voice>("\"en-US-AvaNeural\"").is_err());
    }

    #[test]
    fn ssml_escapes_every_xml_metacharacter_and_keeps_unicode() {
        assert_eq!(
            build_ssml(Voice::HsiaoChen, "妳說：<&>\"' 好").unwrap(),
            "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"zh-TW\"><voice name=\"zh-TW-HsiaoChenNeural\">妳說：&lt;&amp;&gt;&quot;&apos; 好</voice></speak>"
        );
    }

    #[test]
    fn empty_invalid_xml_and_oversize_text_never_reach_transport() {
        let cases = [
            (" \n\t", Error::EmptyText),
            ("bad\u{0}text", Error::InvalidXmlCharacter),
        ];
        for (text, expected) in cases {
            let fake = FakeTransport::failing(TransportFailure::Other);
            assert_eq!(
                synthesize_with_transport(
                    &fake,
                    AzureRegion::EastAsia,
                    Voice::HsiaoChen,
                    "valid-key",
                    text,
                ),
                Err(expected)
            );
            assert_eq!(fake.calls.get(), 0);
        }

        let too_long = "&".repeat(MAX_SSML_BYTES);
        let fake = FakeTransport::failing(TransportFailure::Other);
        assert_eq!(
            synthesize_with_transport(
                &fake,
                AzureRegion::EastAsia,
                Voice::HsiaoChen,
                "valid-key",
                &too_long,
            ),
            Err(Error::TextTooLong)
        );
        assert_eq!(fake.calls.get(), 0);
    }

    #[test]
    fn invalid_keys_are_rejected_without_transport_and_debug_is_redacted() {
        for key in ["", " leading", "trailing ", "line\nbreak"] {
            let fake = FakeTransport::failing(TransportFailure::Other);
            assert_eq!(
                synthesize_with_transport(
                    &fake,
                    AzureRegion::EastAsia,
                    Voice::HsiaoChen,
                    key,
                    "你好",
                ),
                Err(Error::InvalidSubscriptionKey)
            );
            assert_eq!(fake.calls.get(), 0);
        }

        let key = "super-secret-key";
        let text = "private spoken text";
        let request = PostRequest {
            region: AzureRegion::EastAsia,
            voice: Voice::HsiaoChen,
            subscription_key: key,
            ssml: build_ssml(Voice::HsiaoChen, text).unwrap(),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains(key));
        assert!(!debug.contains(text));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn one_call_sends_the_exact_typed_post_and_returns_mp3_bytes() {
        let region = AzureRegion::SoutheastAsia;
        let body = b"fake-mp3".to_vec();
        let fake = FakeTransport::returning(response(
            region,
            200,
            Some("audio/mpeg; codec=mp3"),
            Some(body.len() as u64),
            Cursor::new(body.clone()),
        ));
        let audio =
            synthesize_with_transport(&fake, region, Voice::HsiaoYu, "ephemeral-key", "A&B")
                .unwrap();
        assert_eq!(fake.calls.get(), 1);
        assert_eq!(audio.content_type(), AUDIO_CONTENT_TYPE);
        assert_eq!(audio.bytes(), body);
        assert_eq!(
            *fake.seen.borrow(),
            [SeenRequest {
                method: "POST",
                region,
                voice: Voice::HsiaoYu,
                endpoint: region.endpoint(),
                host: region.host(),
                content_type: SSML_CONTENT_TYPE,
                accept: AUDIO_CONTENT_TYPE,
                output_format: OUTPUT_FORMAT,
                user_agent: USER_AGENT,
                key: "ephemeral-key".to_owned(),
                ssml: build_ssml(Voice::HsiaoYu, "A&B").unwrap(),
            }]
        );
    }

    #[test]
    fn transport_failure_is_returned_after_exactly_one_attempt() {
        let fake = FakeTransport::failing(TransportFailure::Timeout);
        assert_eq!(
            synthesize_with_transport(&fake, AzureRegion::JapanEast, Voice::YunJhe, "key", "你好",),
            Err(Error::Transport(TransportFailure::Timeout))
        );
        assert_eq!(fake.calls.get(), 1, "orchestration must not retry");
    }

    #[test]
    fn bad_status_url_type_and_encoding_are_rejected_before_body_read() {
        let region = AzureRegion::EastAsia;
        let bad_status = response(region, 429, Some(AUDIO_CONTENT_TYPE), None, PanicReader);
        assert_eq!(
            validate_response(bad_status, region.endpoint(), 8),
            Err(Error::UnexpectedStatus(429))
        );

        let bad_url = TransportResponse::new(
            200,
            AzureRegion::JapanEast.endpoint(),
            Some(AUDIO_CONTENT_TYPE.to_owned()),
            None,
            None,
            PanicReader,
        );
        assert_eq!(
            validate_response(bad_url, region.endpoint(), 8),
            Err(Error::UnexpectedFinalUrl)
        );

        for content_type in [None, Some("text/plain"), Some("audio/wav")] {
            let bad_type = response(region, 200, content_type, None, PanicReader);
            assert_eq!(
                validate_response(bad_type, region.endpoint(), 8),
                Err(Error::UnexpectedContentType)
            );
        }

        let encoded = TransportResponse::new(
            200,
            region.endpoint(),
            Some(AUDIO_CONTENT_TYPE.to_owned()),
            Some("gzip".to_owned()),
            None,
            PanicReader,
        );
        assert_eq!(
            validate_response(encoded, region.endpoint(), 8),
            Err(Error::UnexpectedContentEncoding)
        );
    }

    #[test]
    fn response_size_empty_length_and_reader_failures_are_distinct() {
        let region = AzureRegion::EastAsia;
        let declared_large = response(region, 200, Some(AUDIO_CONTENT_TYPE), Some(9), PanicReader);
        assert_eq!(
            validate_response(declared_large, region.endpoint(), 8),
            Err(Error::AudioTooLarge)
        );

        let actual_large = response(
            region,
            200,
            Some(AUDIO_CONTENT_TYPE),
            None,
            Cursor::new(vec![1; 9]),
        );
        assert_eq!(
            validate_response(actual_large, region.endpoint(), 8),
            Err(Error::AudioTooLarge)
        );

        let empty = response(
            region,
            200,
            Some(AUDIO_CONTENT_TYPE),
            Some(0),
            Cursor::new(Vec::new()),
        );
        assert_eq!(
            validate_response(empty, region.endpoint(), 8),
            Err(Error::EmptyAudio)
        );

        let truncated = response(
            region,
            200,
            Some(AUDIO_CONTENT_TYPE),
            Some(4),
            Cursor::new(vec![1, 2, 3]),
        );
        assert_eq!(
            validate_response(truncated, region.endpoint(), 8),
            Err(Error::UnexpectedContentLength)
        );

        let failed = response(region, 200, Some(AUDIO_CONTENT_TYPE), None, FailingReader);
        assert_eq!(
            validate_response(failed, region.endpoint(), 8),
            Err(Error::Transport(TransportFailure::ResponseBody))
        );
    }

    #[test]
    fn identity_content_encoding_and_case_insensitive_media_type_are_allowed() {
        let region = AzureRegion::JapanEast;
        let accepted = TransportResponse::new(
            200,
            region.endpoint(),
            Some("Audio/MPEG ; charset=binary".to_owned()),
            Some("IDENTITY".to_owned()),
            Some(3),
            Cursor::new(vec![1, 2, 3]),
        );
        assert_eq!(
            validate_response(accepted, region.endpoint(), 8)
                .unwrap()
                .bytes(),
            [1, 2, 3]
        );
    }
}
