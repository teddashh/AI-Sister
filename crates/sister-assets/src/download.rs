use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ureq::ResponseExt;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::cache::{InstallTransaction, begin_install, install_from_bytes_in_transaction};
use crate::{
    DownloadFailure, Error, InstallOutcome, PACK_BYTES, PACK_MEDIA_TYPE, PACK_URL, Result,
};

const USER_AGENT: &str = "AI-Sister-Persona-Assets/1";
const READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
struct ExpectedResponse {
    final_url: &'static str,
    media_type: &'static str,
    bytes: usize,
}

const PACK_RESPONSE: ExpectedResponse = ExpectedResponse {
    final_url: PACK_URL,
    media_type: PACK_MEDIA_TYPE,
    bytes: PACK_BYTES,
};

struct TransportResponse {
    status: u16,
    final_url: String,
    content_type: Option<String>,
    content_encoding: Option<String>,
    content_length: Option<usize>,
    body: Box<dyn Read>,
}

/// This private seam has no URL/header/body parameters: even a replacement transport can only
/// perform the one request represented by `get_fixed_pack`.
trait Transport {
    fn get_fixed_pack(&self) -> Result<TransportResponse>;
}

trait DownloadTransaction {
    fn checkpoint(&self, cancelled: &AtomicBool) -> Result<()>;
    fn installed_before_download(&self, cancelled: &AtomicBool) -> Result<bool>;
    fn install(&self, archive: &[u8], cancelled: &AtomicBool) -> Result<InstallOutcome>;
}

impl DownloadTransaction for InstallTransaction {
    fn checkpoint(&self, cancelled: &AtomicBool) -> Result<()> {
        self.checkpoint(cancelled)
    }

    fn installed_before_download(&self, cancelled: &AtomicBool) -> Result<bool> {
        self.installed_before_download(cancelled)
    }

    fn install(&self, archive: &[u8], cancelled: &AtomicBool) -> Result<InstallOutcome> {
        install_from_bytes_in_transaction(self, archive, cancelled)
    }
}

struct NativeTransport {
    agent: ureq::Agent,
}

impl NativeTransport {
    fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .proxy(None)
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    // NativeTls means the OS owns certificate verification. Explicitly retain
                    // its trust store too: ureq otherwise disables Schannel's built-in roots and
                    // replaces them with WebPKI roots, a configuration that failed before the
                    // first response on the GitHub Windows runner.
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(180)))
            .user_agent(USER_AGENT)
            .accept(PACK_MEDIA_TYPE)
            .accept_encoding("identity")
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

/// Build the only outbound request as inspectable data before handing it to ureq. Keeping the
/// method, URL, headers and empty body in one value lets the privacy test assert the request that
/// `Agent::run` actually consumes instead of testing a second, decorative description.
fn fixed_pack_request() -> ureq::http::Request<()> {
    ureq::http::Request::get(PACK_URL)
        .body(())
        .expect("the embedded fixed Persona URL is a valid HTTPS request")
}

impl Transport for NativeTransport {
    fn get_fixed_pack(&self) -> Result<TransportResponse> {
        let response = self
            .agent
            .run(fixed_pack_request())
            .map_err(|error| Error::DownloadUnavailable(classify_transport_error(&error)))?;
        let status = response.status().as_u16();
        let final_url = response.get_uri().to_string();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_encoding = response
            .headers()
            .get("content-encoding")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        Ok(TransportResponse {
            status,
            final_url,
            content_type,
            content_encoding,
            content_length,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

fn classify_transport_error(error: &ureq::Error) -> DownloadFailure {
    match error {
        ureq::Error::HostNotFound => DownloadFailure::HostResolution,
        ureq::Error::Tls(_)
        | ureq::Error::NativeTls(_)
        | ureq::Error::Pem(_)
        | ureq::Error::Der(_) => DownloadFailure::Tls,
        ureq::Error::Timeout(_) => DownloadFailure::Timeout,
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed => DownloadFailure::Connection,
        ureq::Error::Protocol(_)
        | ureq::Error::Http(_)
        | ureq::Error::BadUri(_)
        | ureq::Error::StatusCode(_)
        | ureq::Error::RedirectFailed
        | ureq::Error::TooManyRedirects => DownloadFailure::Protocol,
        _ => DownloadFailure::Transport,
    }
}

/// 執行唯一獲准的 native HTTP request，驗完整包後才交給 cache。
///
/// API 刻意不收 URL、header、body 或 Persona。四位角色和所有本機狀態走到這裡
/// 都只能做同一個 GET。`progress` 只收到已讀 byte 數；不會收到 media bytes。
pub fn download_and_install(
    cache_root: &Path,
    cancelled: &AtomicBool,
    progress: impl Fn(usize),
) -> Result<InstallOutcome> {
    let transport = NativeTransport::new();
    download_and_install_with_transport(cache_root, &transport, PACK_RESPONSE, cancelled, progress)
}

fn download_and_install_with_transport(
    cache_root: &Path,
    transport: &impl Transport,
    expected: ExpectedResponse,
    cancelled: &AtomicBool,
    progress: impl Fn(usize),
) -> Result<InstallOutcome> {
    // Acquiring the real cross-process transaction here guarantees every transport, including
    // the native one, remains untouched when the cache is busy.
    let transaction = begin_install(cache_root)?;
    download_transaction(&transaction, transport, expected, cancelled, progress)
}

fn download_transaction(
    transaction: &impl DownloadTransaction,
    transport: &impl Transport,
    expected: ExpectedResponse,
    cancelled: &AtomicBool,
    progress: impl Fn(usize),
) -> Result<InstallOutcome> {
    transaction.checkpoint(cancelled)?;
    if transaction.installed_before_download(cancelled)? {
        return Ok(InstallOutcome::AlreadyInstalled);
    }

    transaction.checkpoint(cancelled)?;
    let response = transport.get_fixed_pack()?;
    transaction.checkpoint(cancelled)?;
    if response.status != 200 {
        return Err(Error::UnexpectedStatus(response.status));
    }
    if response.final_url != expected.final_url {
        return Err(Error::UnexpectedFinalUrl);
    }
    if response.content_type.as_deref() != Some(expected.media_type) {
        return Err(Error::UnexpectedContentType);
    }
    if response
        .content_encoding
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(Error::UnexpectedContentEncoding);
    }
    if response.content_length != Some(expected.bytes) {
        return Err(Error::UnexpectedContentLength);
    }

    let mut reader = response.body;
    let mut archive = Vec::with_capacity(expected.bytes);
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        transaction.checkpoint(cancelled)?;
        let read = reader
            .read(&mut chunk)
            .map_err(|_| Error::DownloadUnavailable(DownloadFailure::ResponseBody))?;
        if read == 0 {
            break;
        }
        if archive
            .len()
            .checked_add(read)
            .is_none_or(|size| size > expected.bytes)
        {
            return Err(Error::UnexpectedContentLength);
        }
        archive.extend_from_slice(&chunk[..read]);
        progress(archive.len());
    }
    if archive.len() != expected.bytes {
        return Err(Error::UnexpectedContentLength);
    }
    transaction.checkpoint(cancelled)?;
    transaction.install(&archive, cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{begin_install, cleanup_test_sidecars};
    use crate::{CacheState, cache_state, remove};
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use ureq::config::AutoHeaderValue;
    use ureq::tls::{RootCerts, TlsProvider};

    const TEST_RESPONSE: ExpectedResponse = ExpectedResponse {
        final_url: PACK_URL,
        media_type: PACK_MEDIA_TYPE,
        bytes: 4,
    };

    struct FakeTransport {
        calls: AtomicUsize,
        response: Mutex<Option<TransportResponse>>,
    }

    impl FakeTransport {
        fn with_response(response: TransportResponse) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                response: Mutex::new(Some(response)),
            }
        }

        fn unavailable() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                response: Mutex::new(None),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Acquire)
        }
    }

    impl Transport for FakeTransport {
        fn get_fixed_pack(&self) -> Result<TransportResponse> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.response
                .lock()
                .expect("fake transport lock")
                .take()
                .ok_or(Error::DownloadUnavailable(DownloadFailure::Transport))
        }
    }

    struct FakeTransaction {
        already_installed: bool,
        installs: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeTransaction {
        fn new(already_installed: bool) -> Self {
            Self {
                already_installed,
                installs: Mutex::new(Vec::new()),
            }
        }
    }

    impl DownloadTransaction for FakeTransaction {
        fn checkpoint(&self, cancelled: &AtomicBool) -> Result<()> {
            if cancelled.load(Ordering::Acquire) {
                Err(Error::Cancelled)
            } else {
                Ok(())
            }
        }

        fn installed_before_download(&self, cancelled: &AtomicBool) -> Result<bool> {
            self.checkpoint(cancelled)?;
            Ok(self.already_installed)
        }

        fn install(&self, archive: &[u8], cancelled: &AtomicBool) -> Result<InstallOutcome> {
            self.checkpoint(cancelled)?;
            self.installs
                .lock()
                .expect("fake install lock")
                .push(archive.to_vec());
            Ok(InstallOutcome::Installed)
        }
    }

    fn response(status: u16, body: &[u8], declared_bytes: usize) -> TransportResponse {
        TransportResponse {
            status,
            final_url: TEST_RESPONSE.final_url.to_owned(),
            content_type: Some(TEST_RESPONSE.media_type.to_owned()),
            content_encoding: None,
            content_length: Some(declared_bytes),
            body: Box::new(Cursor::new(body.to_vec())),
        }
    }

    fn temp_cache(label: &str) -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let parent = std::env::temp_dir().join(format!(
            "ai-sister-assets-download-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&parent).expect("test parent");
        let cache = parent.join(crate::CACHE_DIRECTORY);
        (parent, cache)
    }

    fn cleanup_failed_download(parent: &Path, cache: &Path) {
        assert!(
            !cache.exists(),
            "failed download created a media cache root"
        );
        cleanup_test_sidecars(cache).expect("cleanup cache controls");
        fs::remove_dir(parent).expect("empty test parent");
    }

    fn assert_provided_header(value: &AutoHeaderValue, expected: &str) {
        match value {
            AutoHeaderValue::Provided(value) => assert_eq!(value.as_str(), expected),
            other => panic!("expected one fixed provided header, got {other:?}"),
        }
    }

    #[test]
    fn native_request_and_agent_config_are_the_fixed_privacy_contract() {
        let request = fixed_pack_request();
        assert_eq!(request.method(), ureq::http::Method::GET);
        assert_eq!(request.uri().to_string(), PACK_URL);
        assert_eq!(request.uri().scheme_str(), Some("https"));
        assert_eq!(
            request.uri().authority().map(|value| value.as_str()),
            Some(crate::HOST)
        );
        assert!(request.uri().query().is_none());
        assert!(request.headers().is_empty());
        assert_eq!(*request.body(), ());

        let transport = NativeTransport::new();
        let config = transport.agent.config();
        assert!(config.https_only());
        assert_eq!(config.max_redirects(), 0);
        assert!(config.proxy().is_none());
        assert!(!config.http_status_as_error());
        assert_eq!(config.tls_config().provider(), TlsProvider::NativeTls);
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
        assert_eq!(config.timeouts().connect, Some(Duration::from_secs(15)));
        assert_eq!(config.timeouts().global, Some(Duration::from_secs(180)));
        assert_provided_header(config.user_agent(), USER_AGENT);
        assert_provided_header(config.accept(), PACK_MEDIA_TYPE);
        assert_provided_header(config.accept_encoding(), "identity");
    }

    #[test]
    fn transport_failures_keep_the_stage_without_exposing_request_data() {
        assert_eq!(
            classify_transport_error(&ureq::Error::HostNotFound),
            DownloadFailure::HostResolution
        );
        assert_eq!(
            classify_transport_error(&ureq::Error::Timeout(ureq::Timeout::Connect)),
            DownloadFailure::Timeout
        );
        assert_eq!(
            classify_transport_error(&ureq::Error::ConnectionFailed),
            DownloadFailure::Connection
        );
        assert_eq!(
            classify_transport_error(&ureq::Error::BadUri("fixed request".to_owned())),
            DownloadFailure::Protocol
        );
        assert_eq!(
            Error::DownloadUnavailable(DownloadFailure::Tls).to_string(),
            "連不到固定素材下載主機（TLS 憑證驗證或握手失敗）"
        );
    }

    #[test]
    fn installed_preflight_and_busy_real_lock_both_make_zero_gets() {
        let cancelled = AtomicBool::new(false);
        let installed = FakeTransaction::new(true);
        let skipped = FakeTransport::unavailable();
        assert_eq!(
            download_transaction(&installed, &skipped, TEST_RESPONSE, &cancelled, |_| {})
                .expect("already installed"),
            InstallOutcome::AlreadyInstalled
        );
        assert_eq!(skipped.calls(), 0);

        let (parent, cache) = temp_cache("busy-zero-get");
        let held = begin_install(&cache).expect("hold real cross-process lock");
        let busy_transport = FakeTransport::unavailable();
        assert!(matches!(
            download_and_install_with_transport(
                &cache,
                &busy_transport,
                TEST_RESPONSE,
                &cancelled,
                |_| {}
            ),
            Err(Error::CacheBusy)
        ));
        assert_eq!(busy_transport.calls(), 0);
        drop(held);
        cleanup_failed_download(&parent, &cache);
    }

    #[test]
    fn one_install_attempt_performs_one_get_and_passes_exact_body_once() {
        let transaction = FakeTransaction::new(false);
        let transport = FakeTransport::with_response(response(200, b"pack", 4));
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(Vec::new());
        assert_eq!(
            download_transaction(
                &transaction,
                &transport,
                TEST_RESPONSE,
                &cancelled,
                |read| progress.lock().expect("progress lock").push(read)
            )
            .expect("fake install"),
            InstallOutcome::Installed
        );
        assert_eq!(transport.calls(), 1);
        assert_eq!(*progress.lock().expect("progress values"), [4]);
        assert_eq!(
            *transaction.installs.lock().expect("fake installs"),
            [b"pack".to_vec()]
        );
    }

    #[test]
    fn bad_or_truncated_fresh_response_stays_available_and_releases_lock() {
        let (parent, cache) = temp_cache("bad-fresh");
        let cancelled = AtomicBool::new(false);
        let bad_status = FakeTransport::with_response(response(503, b"", 4));
        assert!(matches!(
            download_and_install_with_transport(
                &cache,
                &bad_status,
                TEST_RESPONSE,
                &cancelled,
                |_| {}
            ),
            Err(Error::UnexpectedStatus(503))
        ));
        assert_eq!(bad_status.calls(), 1);
        assert_eq!(
            cache_state(&cache).expect("state after bad status"),
            CacheState::Available
        );

        let truncated = FakeTransport::with_response(response(200, b"bad", 4));
        assert!(matches!(
            download_and_install_with_transport(
                &cache,
                &truncated,
                TEST_RESPONSE,
                &cancelled,
                |_| {}
            ),
            Err(Error::UnexpectedContentLength)
        ));
        assert_eq!(truncated.calls(), 1);
        assert_eq!(
            cache_state(&cache).expect("state after truncated body"),
            CacheState::Available
        );
        drop(begin_install(&cache).expect("error path released lock"));
        cleanup_failed_download(&parent, &cache);
    }

    #[test]
    fn cancellation_happens_before_get_and_releases_the_real_lock() {
        let (parent, cache) = temp_cache("cancel-zero-get");
        let cancelled = AtomicBool::new(true);
        let transport = FakeTransport::unavailable();
        assert!(matches!(
            download_and_install_with_transport(
                &cache,
                &transport,
                TEST_RESPONSE,
                &cancelled,
                |_| {}
            ),
            Err(Error::Cancelled)
        ));
        assert_eq!(transport.calls(), 0);
        drop(begin_install(&cache).expect("cancel path released lock"));
        cleanup_failed_download(&parent, &cache);
    }

    #[test]
    #[ignore = "release/manual：需要 AI_SISTER_ALLOW_ASSET_NETWORK=1 才會連固定 CDN"]
    fn fixed_network_path_downloads_validates_installs_and_removes() {
        assert_eq!(
            std::env::var("AI_SISTER_ALLOW_ASSET_NETWORK").as_deref(),
            Ok("1"),
            "set AI_SISTER_ALLOW_ASSET_NETWORK=1 to acknowledge the one fixed GET"
        );
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let parent = std::env::temp_dir().join(format!(
            "ai-sister-assets-network-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&parent).expect("test parent");
        let cache = parent.join(crate::CACHE_DIRECTORY);
        let cancelled = AtomicBool::new(false);
        let last = AtomicUsize::new(0);

        let outcome = download_and_install(&cache, &cancelled, |read| {
            last.store(read, Ordering::Release);
        })
        .expect("fixed download and install");
        assert_eq!(outcome, InstallOutcome::Installed);
        assert_eq!(last.load(Ordering::Acquire), PACK_BYTES);
        assert_eq!(
            cache_state(&cache).expect("cache state"),
            CacheState::Installed
        );
        remove(&cache).expect("remove exact cache");
        cleanup_test_sidecars(&cache).expect("cleanup cache controls");
        fs::remove_dir(parent).expect("empty test parent");
    }
}
