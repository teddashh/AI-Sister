use ureq::ResponseExt;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::model::{
    ACCEPT, ACCEPT_ENCODING, CONNECT_TIMEOUT_SECS, GLOBAL_TIMEOUT_SECS, MAX_BODY_BYTES,
    RECEIVE_TIMEOUT_SECS, RESOLVE_TIMEOUT_SECS, SEND_TIMEOUT_SECS, USER_AGENT,
};
use crate::policy::Transport;
use crate::{
    Error, PublicEndpoint, Result, TransportFailure, TransportResponse, read_bounded,
    validate_response,
};

pub struct PublicStatusClient {
    agent: ureq::Agent,
}

impl PublicStatusClient {
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
            .timeout_resolve(Some(std::time::Duration::from_secs(RESOLVE_TIMEOUT_SECS)))
            .timeout_connect(Some(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS)))
            .timeout_send_request(Some(std::time::Duration::from_secs(SEND_TIMEOUT_SECS)))
            .timeout_send_body(Some(std::time::Duration::from_secs(SEND_TIMEOUT_SECS)))
            .timeout_recv_response(Some(std::time::Duration::from_secs(RECEIVE_TIMEOUT_SECS)))
            .timeout_recv_body(Some(std::time::Duration::from_secs(RECEIVE_TIMEOUT_SECS)))
            .timeout_global(Some(std::time::Duration::from_secs(GLOBAL_TIMEOUT_SECS)))
            .user_agent("")
            .accept("")
            .accept_encoding("")
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }

    pub fn fetch(&self, endpoint: PublicEndpoint) -> Result<crate::PublicBoard> {
        let response = self.get(endpoint)?;
        validate_response(&response, endpoint)?;
        match endpoint {
            PublicEndpoint::Status => crate::parse_status_board(&response.body),
            PublicEndpoint::Latest(product) => {
                let row = crate::parse_latest(&response.body, product)?;
                Ok(crate::PublicBoard {
                    updated_at: row
                        .forecast
                        .as_ref()
                        .and_then(|forecast| forecast.computed_at.clone())
                        .unwrap_or_default(),
                    products: vec![row],
                    attribution: crate::SOURCE_ATTRIBUTION,
                })
            }
        }
    }
}

impl Default for PublicStatusClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for PublicStatusClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicStatusClient")
            .field("redirects", &0)
            .field("proxy", &"disabled")
            .field("global_timeout_secs", &GLOBAL_TIMEOUT_SECS)
            .finish_non_exhaustive()
    }
}

pub fn fixed_get_request(endpoint: PublicEndpoint) -> ureq::http::Request<()> {
    ureq::http::Request::get(endpoint.url())
        .header("User-Agent", USER_AGENT)
        .header("Accept", ACCEPT)
        .header("Accept-Encoding", ACCEPT_ENCODING)
        .body(())
        .expect("embedded LimitReset URL is a valid HTTPS GET")
}

impl Transport for PublicStatusClient {
    fn get(&self, endpoint: PublicEndpoint) -> Result<TransportResponse> {
        let response = self
            .agent
            .run(fixed_get_request(endpoint))
            .map_err(|error| Error::Transport(classify_transport_error(&error)))?;
        let status = response.status().as_u16();
        let final_url = response.get_uri().to_string();
        let content_type = unique_response_header(response.headers(), "content-type")?;
        let content_encoding = unique_response_header(response.headers(), "content-encoding")?;
        let content_length = unique_response_header(response.headers(), "content-length")?
            .map(|value| value.parse::<u64>().map_err(|_| TransportFailure::Protocol))
            .transpose()?;
        if content_length.is_some_and(|bytes| bytes > MAX_BODY_BYTES as u64) {
            return Err(Error::BodyTooLarge);
        }
        let mut reader = response.into_body().into_reader();
        let body = read_bounded(&mut reader, MAX_BODY_BYTES, content_length)?;
        Ok(TransportResponse {
            status,
            final_url,
            content_type,
            content_encoding,
            content_length,
            body,
        })
    }
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
    use crate::model::ProductId;
    use ureq::config::AutoHeaderValue;

    #[test]
    fn native_agent_has_no_redirect_proxy_or_unbounded_timeout() {
        let client = PublicStatusClient::new();
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
    }

    #[test]
    fn native_http_request_is_the_exact_status_get_contract() {
        let http = fixed_get_request(PublicEndpoint::Status);
        assert_eq!(http.method(), ureq::http::Method::GET);
        assert_eq!(http.uri().to_string(), PublicEndpoint::Status.url());
        assert_eq!(http.uri().scheme_str(), Some("https"));
        assert_eq!(
            http.uri().authority().map(|authority| authority.as_str()),
            Some("limitreset.net")
        );
        assert!(http.uri().query().is_none());
        assert_eq!(http.headers().len(), 3);
        assert_eq!(http.headers()["user-agent"], USER_AGENT);
        assert_eq!(http.headers()["accept"], ACCEPT);
        assert_eq!(http.headers()["accept-encoding"], ACCEPT_ENCODING);
        assert!(http.headers().get("authorization").is_none());
        assert!(http.headers().get("cookie").is_none());
        assert!(http.headers().get("referer").is_none());
        assert!(http.headers().get("content-type").is_none());
        assert!(matches!(http.body(), &()));
    }

    #[test]
    fn latest_get_is_exact_allowlisted_path() {
        let http = fixed_get_request(PublicEndpoint::Latest(ProductId::Grok));
        assert_eq!(http.method(), ureq::http::Method::GET);
        assert_eq!(http.uri().to_string(), ProductId::Grok.latest_url());
        assert!(http.uri().query().is_none());
        assert!(http.headers().get("cookie").is_none());
    }

    #[test]
    fn ureq_failures_are_classified_without_credentials() {
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
    }
}
