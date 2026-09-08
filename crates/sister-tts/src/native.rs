use std::io::Cursor;
use ureq::ResponseExt;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::{
    AUDIO_CONTENT_TYPE, AzureRegion, CONNECT_TIMEOUT, GLOBAL_TIMEOUT, PostRequest,
    RECEIVE_BODY_TIMEOUT, RECEIVE_RESPONSE_TIMEOUT, RESOLVE_TIMEOUT, Result, SEND_TIMEOUT,
    SSML_CONTENT_TYPE, Transport, TransportFailure, TransportResponse, USER_AGENT, Voice,
    synthesize_with_transport,
};

/// Reusable native Azure Speech client. It stores only connection configuration and pooled
/// sockets; subscription keys and text are borrowed for a single synchronous call and are never
/// retained in this value.
pub struct AzureClient {
    agent: ureq::Agent,
}

impl AzureClient {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .proxy(None)
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .timeout_resolve(Some(RESOLVE_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_send_request(Some(SEND_TIMEOUT))
            .timeout_send_body(Some(SEND_TIMEOUT))
            .timeout_recv_response(Some(RECEIVE_RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(RECEIVE_BODY_TIMEOUT))
            .timeout_global(Some(GLOBAL_TIMEOUT))
            // Every allowed header is placed on the inspectable request below. Disable ureq's
            // automatic values so the effective request does not grow a second policy surface.
            .user_agent("")
            .accept("")
            .accept_encoding("")
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }

    pub fn synthesize(
        &self,
        region: AzureRegion,
        voice: Voice,
        subscription_key: &str,
        text: &str,
    ) -> Result<crate::Audio> {
        synthesize_with_transport(self, region, voice, subscription_key, text)
    }
}

impl Default for AzureClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AzureClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AzureClient")
            .field("redirects", &0)
            .field("proxy", &"disabled")
            .field("global_timeout", &GLOBAL_TIMEOUT)
            .finish_non_exhaustive()
    }
}

impl Transport for AzureClient {
    fn post(
        &self,
        request: &PostRequest<'_>,
    ) -> std::result::Result<TransportResponse, TransportFailure> {
        // `Agent::run` performs one call. There is no retry middleware and the orchestration layer
        // invokes this method once, so 429/5xx/transport errors return directly to the user action.
        let mut ssml = Cursor::new(request.ssml().as_bytes());
        let body = ureq::SendBody::from_reader(&mut ssml);
        let response = self
            .agent
            .run(http_request(request, body)?)
            .map_err(|error| classify_transport_error(&error))?;
        let status = response.status().as_u16();
        let final_url = response.get_uri().to_string();
        let content_type = response_header(&response, "content-type")?;
        let content_encoding = response_header(&response, "content-encoding")?;
        let content_length = response_header(&response, "content-length")?
            .map(|value| value.parse::<u64>().map_err(|_| TransportFailure::Protocol))
            .transpose()?;
        Ok(TransportResponse::new(
            status,
            final_url,
            content_type,
            content_encoding,
            content_length,
            response.into_body().into_reader(),
        ))
    }
}

fn http_request<'request>(
    request: &PostRequest<'_>,
    body: ureq::SendBody<'request>,
) -> std::result::Result<ureq::http::Request<ureq::SendBody<'request>>, TransportFailure> {
    let mut key = request
        .subscription_key()
        .parse::<ureq::http::HeaderValue>()
        .map_err(|_| TransportFailure::Protocol)?;
    key.set_sensitive(true);
    let mut request = ureq::http::Request::post(request.endpoint())
        .header("Content-Type", SSML_CONTENT_TYPE)
        .header("X-Microsoft-OutputFormat", request.output_format())
        .header("User-Agent", USER_AGENT)
        .header("Accept", AUDIO_CONTENT_TYPE)
        .header("Accept-Encoding", "identity")
        .body(body)
        .map_err(|_| TransportFailure::Protocol)?;
    request.headers_mut().insert(
        ureq::http::header::HeaderName::from_static("ocp-apim-subscription-key"),
        key,
    );
    Ok(request)
}

fn response_header(
    response: &ureq::http::Response<ureq::Body>,
    name: &str,
) -> std::result::Result<Option<String>, TransportFailure> {
    unique_response_header(response.headers(), name)
}

fn unique_response_header(
    headers: &ureq::http::HeaderMap,
    name: &str,
) -> std::result::Result<Option<String>, TransportFailure> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(TransportFailure::Protocol);
    }
    value
        .to_str()
        .map(str::to_owned)
        .map(Some)
        .map_err(|_| TransportFailure::Protocol)
}

fn classify_transport_error(error: &ureq::Error) -> TransportFailure {
    match error {
        ureq::Error::HostNotFound => TransportFailure::HostResolution,
        ureq::Error::Tls(_)
        | ureq::Error::NativeTls(_)
        | ureq::Error::Pem(_)
        | ureq::Error::Der(_) => TransportFailure::Tls,
        ureq::Error::Timeout(_) => TransportFailure::Timeout,
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed => TransportFailure::Connection,
        ureq::Error::Protocol(_)
        | ureq::Error::Http(_)
        | ureq::Error::BadUri(_)
        | ureq::Error::StatusCode(_)
        | ureq::Error::RedirectFailed
        | ureq::Error::TooManyRedirects => TransportFailure::Protocol,
        _ => TransportFailure::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use ureq::config::AutoHeaderValue;

    #[test]
    fn native_agent_has_no_redirect_proxy_or_unbounded_timeout() {
        let client = AzureClient::new();
        let config = client.agent.config();
        assert!(config.https_only());
        assert_eq!(config.max_redirects(), 0);
        assert!(config.proxy().is_none());
        assert!(!config.http_status_as_error());
        assert_eq!(config.tls_config().provider(), TlsProvider::NativeTls);
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
        assert!(matches!(config.user_agent(), AutoHeaderValue::None));
        assert!(matches!(config.accept(), AutoHeaderValue::None));
        assert!(matches!(config.accept_encoding(), AutoHeaderValue::None));

        let timeouts = config.timeouts();
        assert_eq!(timeouts.resolve, Some(RESOLVE_TIMEOUT));
        assert_eq!(timeouts.connect, Some(CONNECT_TIMEOUT));
        assert_eq!(timeouts.send_request, Some(SEND_TIMEOUT));
        assert_eq!(timeouts.send_body, Some(SEND_TIMEOUT));
        assert_eq!(timeouts.recv_response, Some(RECEIVE_RESPONSE_TIMEOUT));
        assert_eq!(timeouts.recv_body, Some(RECEIVE_BODY_TIMEOUT));
        assert_eq!(timeouts.global, Some(GLOBAL_TIMEOUT));
    }

    #[test]
    fn native_http_request_is_the_exact_post_contract() {
        let request = PostRequest {
            region: AzureRegion::JapanEast,
            voice: Voice::HsiaoYu,
            subscription_key: "secret-key",
            ssml: crate::build_ssml(Voice::HsiaoYu, "A&B").unwrap(),
        };
        let mut ssml = Cursor::new(request.ssml().as_bytes());
        let body = ureq::SendBody::from_reader(&mut ssml);
        let http = http_request(&request, body).unwrap();
        assert_eq!(http.method(), ureq::http::Method::POST);
        assert_eq!(http.uri().to_string(), AzureRegion::JapanEast.endpoint());
        assert_eq!(http.uri().scheme_str(), Some("https"));
        assert_eq!(
            http.uri().authority().map(|authority| authority.as_str()),
            Some(AzureRegion::JapanEast.host())
        );
        assert!(http.uri().query().is_none());
        assert_eq!(http.headers().len(), 6);
        assert_eq!(http.headers()["ocp-apim-subscription-key"], "secret-key");
        assert!(http.headers()["ocp-apim-subscription-key"].is_sensitive());
        assert!(!format!("{:?}", http.headers()).contains("secret-key"));
        assert_eq!(http.headers()["content-type"], SSML_CONTENT_TYPE);
        assert_eq!(
            http.headers()["x-microsoft-outputformat"],
            crate::OUTPUT_FORMAT
        );
        assert_eq!(http.headers()["user-agent"], USER_AGENT);
        assert_eq!(http.headers()["accept"], AUDIO_CONTENT_TYPE);
        assert_eq!(http.headers()["accept-encoding"], "identity");
        assert!(http.headers().get("authorization").is_none());
        assert!(http.headers().get("cookie").is_none());
        assert!(http.headers().get("referer").is_none());
        let mut sent = Vec::new();
        http.into_body()
            .into_reader()
            .read_to_end(&mut sent)
            .unwrap();
        assert_eq!(sent, request.ssml().as_bytes());
    }

    #[test]
    fn duplicate_or_non_text_response_headers_fail_closed() {
        for name in ["content-type", "content-encoding", "content-length"] {
            let mut headers = ureq::http::HeaderMap::new();
            headers.append(
                ureq::http::HeaderName::from_static(name),
                ureq::http::HeaderValue::from_static("first"),
            );
            headers.append(
                ureq::http::HeaderName::from_static(name),
                ureq::http::HeaderValue::from_static("second"),
            );
            assert_eq!(
                unique_response_header(&headers, name),
                Err(TransportFailure::Protocol)
            );
        }

        let mut headers = ureq::http::HeaderMap::new();
        headers.insert(
            "content-type",
            ureq::http::HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        assert_eq!(
            unique_response_header(&headers, "content-type"),
            Err(TransportFailure::Protocol)
        );
    }

    #[test]
    fn ureq_failures_are_classified_without_credentials_or_text() {
        assert_eq!(
            classify_transport_error(&ureq::Error::HostNotFound),
            TransportFailure::HostResolution
        );
        assert_eq!(
            classify_transport_error(&ureq::Error::Timeout(ureq::Timeout::Connect)),
            TransportFailure::Timeout
        );
        assert_eq!(
            classify_transport_error(&ureq::Error::ConnectionFailed),
            TransportFailure::Connection
        );
        assert_eq!(
            classify_transport_error(&ureq::Error::BadUri("fixed request".to_owned())),
            TransportFailure::Protocol
        );
    }
}
