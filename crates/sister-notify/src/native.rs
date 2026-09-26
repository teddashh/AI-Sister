use crate::WatchReport;
use std::io::{Cursor, Read};
use std::time::Duration;
use ureq::ResponseExt;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

const DISCORD_ORIGIN: &str = "https://discord.com";
const USER_AGENT: &str = "AI-Sister-Watch/1";
const MAX_RESPONSE_BYTES: usize = 8 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

/// Webhook URL 是 credential：只從 caller 指定的環境變數讀，沒有 `Display`，
/// `Debug` 也只承認 host，不把 token 帶進 log 或錯誤。
#[derive(Clone)]
pub struct DiscordWebhook(String);

impl std::fmt::Debug for DiscordWebhook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiscordWebhook")
            .field("origin", &DISCORD_ORIGIN)
            .field("credential", &"redacted")
            .finish()
    }
}

impl DiscordWebhook {
    pub fn from_env(name: &str) -> Result<Self, DeliveryError> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            return Err(DeliveryError::InvalidEnvironmentName);
        }
        let value = std::env::var(name).map_err(|_| DeliveryError::MissingEnvironmentValue)?;
        Self::parse(value)
    }

    pub fn parse(value: String) -> Result<Self, DeliveryError> {
        let uri = value
            .parse::<ureq::http::Uri>()
            .map_err(|_| DeliveryError::InvalidWebhook)?;
        if uri.scheme_str() != Some("https")
            || uri.authority().map(|a| a.as_str()) != Some("discord.com")
            || uri.query().is_some()
        {
            return Err(DeliveryError::InvalidWebhook);
        }
        let segments = uri.path().split('/').collect::<Vec<_>>();
        let valid = matches!(segments.as_slice(), ["", "api", "webhooks", id, token]
            if !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && token.len() >= 32
                && token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')));
        if !valid {
            return Err(DeliveryError::InvalidWebhook);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryError {
    InvalidEnvironmentName,
    MissingEnvironmentValue,
    InvalidWebhook,
    Serialize,
    Transport,
    UnexpectedResponse,
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEnvironmentName => "Discord webhook 的環境變數名稱不合法",
            Self::MissingEnvironmentValue => "指定的 Discord webhook 環境變數不存在",
            Self::InvalidWebhook => "Discord webhook 只接受 https://discord.com/api/webhooks/<id>/<token>，不接受 query、其他 host 或其他 path",
            Self::Serialize => "遠端通報 JSON 無法序列化",
            Self::Transport => "Discord 通報沒有送達；沒有重試",
            Self::UnexpectedResponse => "Discord 沒有接受通報；沒有重試",
        })
    }
}

impl std::error::Error for DeliveryError {}

pub struct DiscordClient {
    agent: ureq::Agent,
}

impl DiscordClient {
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
            .timeout_resolve(Some(TIMEOUT))
            .timeout_connect(Some(TIMEOUT))
            .timeout_send_request(Some(TIMEOUT))
            .timeout_send_body(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            .timeout_recv_body(Some(TIMEOUT))
            .timeout_global(Some(TIMEOUT))
            .user_agent("")
            .accept("")
            .accept_encoding("")
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }

    pub fn send(
        &self,
        webhook: &DiscordWebhook,
        report: &WatchReport,
    ) -> Result<(), DeliveryError> {
        let json =
            serde_json::to_vec(&report.discord_body()).map_err(|_| DeliveryError::Serialize)?;
        let mut body = Cursor::new(json.as_slice());
        let response = self
            .agent
            .run(request(webhook, &mut body)?)
            .map_err(|_| DeliveryError::Transport)?;
        if response.status().as_u16() != 204 || response.get_uri().to_string() != webhook.0 {
            return Err(DeliveryError::UnexpectedResponse);
        }
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        if content_length.is_some_and(|value| value > MAX_RESPONSE_BYTES) {
            return Err(DeliveryError::UnexpectedResponse);
        }
        let mut body = response
            .into_body()
            .into_reader()
            .take((MAX_RESPONSE_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes)
            .map_err(|_| DeliveryError::UnexpectedResponse)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(DeliveryError::UnexpectedResponse);
        }
        Ok(())
    }
}

impl Default for DiscordClient {
    fn default() -> Self {
        Self::new()
    }
}

fn request<'a>(
    webhook: &DiscordWebhook,
    json: &'a mut dyn Read,
) -> Result<ureq::http::Request<ureq::SendBody<'a>>, DeliveryError> {
    ureq::http::Request::post(&webhook.0)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "identity")
        .body(ureq::SendBody::from_reader(json))
        .map_err(|_| DeliveryError::InvalidWebhook)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WatchCounts, WatchOutcome};
    use ureq::config::AutoHeaderValue;

    const SECRET: &str = "abcdefghijklmnopqrstuvwxyz0123456789.ABC_def-ghi";

    fn webhook() -> DiscordWebhook {
        DiscordWebhook::parse(format!(
            "https://discord.com/api/webhooks/123456789/{SECRET}"
        ))
        .unwrap()
    }

    fn report() -> WatchReport {
        WatchReport::new(WatchOutcome::Deadline, 100, 500, 0, WatchCounts::default())
    }

    #[test]
    fn only_the_exact_discord_webhook_shape_is_accepted() {
        for bad in [
            format!("http://discord.com/api/webhooks/123/{SECRET}"),
            format!("https://evil.example/api/webhooks/123/{SECRET}"),
            format!("https://discord.com.evil.example/api/webhooks/123/{SECRET}"),
            format!("https://discord.com/api/webhooks/not-digits/{SECRET}"),
            format!("https://discord.com/api/webhooks/123/{SECRET}?wait=true"),
            format!("https://discord.com/api/webhooks/123/{SECRET}/extra"),
        ] {
            assert_eq!(
                DiscordWebhook::parse(bad).unwrap_err(),
                DeliveryError::InvalidWebhook
            );
        }
        assert_eq!(
            format!("{:?}", webhook()),
            "DiscordWebhook { origin: \"https://discord.com\", credential: \"redacted\" }"
        );
        assert!(!format!("{:?}", webhook()).contains(SECRET));
    }

    #[test]
    fn native_agent_has_no_redirect_proxy_retry_or_unbounded_timeout() {
        let client = DiscordClient::new();
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
        assert_eq!(config.timeouts().global, Some(TIMEOUT));
    }

    #[test]
    fn request_has_only_the_allowlisted_headers_and_text_free_json() {
        let body = serde_json::to_vec(&report().discord_body()).unwrap();
        let mut reader = Cursor::new(body.as_slice());
        let request = request(&webhook(), &mut reader).unwrap();
        assert_eq!(request.method(), ureq::http::Method::POST);
        assert_eq!(request.uri().scheme_str(), Some("https"));
        assert_eq!(request.uri().host(), Some("discord.com"));
        assert!(request.uri().query().is_none());
        assert_eq!(request.headers().len(), 4);
        assert_eq!(request.headers()["content-type"], "application/json");
        assert_eq!(request.headers()["user-agent"], USER_AGENT);
        assert_eq!(request.headers()["accept"], "application/json");
        assert_eq!(request.headers()["accept-encoding"], "identity");
        for absent in ["authorization", "cookie", "referer"] {
            assert!(request.headers().get(absent).is_none());
        }
        let raw = String::from_utf8(body).unwrap();
        for absent in [
            "private-question",
            "private-screen-text",
            "private-app",
            "https://private.example",
            "C:\\\\private",
            SECRET,
        ] {
            assert!(!raw.contains(absent), "{raw}");
        }
    }
}
