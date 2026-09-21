//! Observed usage, remaining quota, and unknown stay three different answers.
//!
//! Default features contain no HTTP client. `public-status` is the only native
//! GET, and it can only hit the exact LimitReset status/latest URLs. Public
//! board events are CC BY 4.0 global product announcements; they are not a
//! personal remaining-quota proof.

mod local;
mod model;
mod parse;
mod policy;
mod sessions;

#[cfg(feature = "public-status")]
mod native;

pub use local::{
    ConfiguredSessionAdapter, LocalUsageAdapter, SyntheticLocalUsage, UnavailableLocalUsage,
};
pub use model::{
    ACCEPT, ACCEPT_ENCODING, AUTO_POLL_INTERVAL_MS, ConfirmedReset, Forecast, HOST,
    LocalAdapterKind, LocalProductUsage, LocalUsage, LocalUsageReport, MAX_BODY_BYTES,
    MIN_GET_INTERVAL_MS, Measured, ProductId, PublicBoard, PublicEndpoint, PublicProductStatus,
    PublicReset, QuotaSnapshot, RefreshReason, ResetReaction, SOURCE_ATTRIBUTION, SOURCE_LICENSE,
    SOURCE_NAME, SOURCE_URL, STATUS_URL, ServedFrom, USER_AGENT, UsageAmount, UsageView,
};
pub use parse::{parse_latest, parse_status_board};
pub use policy::{
    DedupStore, RefreshOutcome, RefreshRequest, STORE_FILE_NAME, STORE_SCHEMA, Transport,
    refresh_board,
};
pub use sessions::{LocalReadRequest, read_sessions};

#[cfg(feature = "public-status")]
pub use native::PublicStatusClient;

use std::fmt;
use std::io::Read;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyBody,
    Malformed,
    UnexpectedProduct,
    Transport(TransportFailure),
    UnexpectedStatus(u16),
    UnexpectedFinalUrl,
    UnexpectedContentType,
    UnexpectedContentEncoding,
    UnexpectedContentLength,
    BodyTooLarge,
    StoreUnreadable,
    StoreUnwritable,
    LocalPathNotAbsolute,
    LocalPathMissing,
    LocalIo,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBody => formatter.write_str("公開看板回應是空的"),
            Self::Malformed => formatter.write_str("公開看板 JSON 無法辨識"),
            Self::UnexpectedProduct => formatter.write_str("公開看板產品不在允許名單"),
            Self::Transport(failure) => write!(formatter, "公開看板連線失敗（{failure}）"),
            Self::UnexpectedStatus(status) => {
                write!(formatter, "公開看板回了未允許的 HTTP 狀態：{status}")
            }
            Self::UnexpectedFinalUrl => {
                formatter.write_str("公開看板回應的最終網址不是固定 endpoint")
            }
            Self::UnexpectedContentType => formatter.write_str("公開看板回了未允許的 Content-Type"),
            Self::UnexpectedContentEncoding => {
                formatter.write_str("公開看板回了未允許的 Content-Encoding")
            }
            Self::UnexpectedContentLength => formatter.write_str("公開看板回應長度和宣告不一致"),
            Self::BodyTooLarge => formatter.write_str("公開看板回應超過單次上限"),
            Self::StoreUnreadable => formatter.write_str("本機公開看板狀態讀不出來"),
            Self::StoreUnwritable => formatter.write_str("本機公開看板狀態寫不進去"),
            Self::LocalPathNotAbsolute => formatter.write_str("本機 session 目錄必須是絕對路徑"),
            Self::LocalPathMissing => formatter.write_str("指定的本機 session 目錄不存在"),
            Self::LocalIo => formatter.write_str("讀本機 session JSONL 失敗"),
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

pub struct TransportResponse {
    pub status: u16,
    pub final_url: String,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

pub fn validate_response(response: &TransportResponse, endpoint: PublicEndpoint) -> Result<()> {
    if response.status != 200 {
        return Err(Error::UnexpectedStatus(response.status));
    }
    if response.final_url != endpoint.url() {
        return Err(Error::UnexpectedFinalUrl);
    }
    if !content_type_allowed(response.content_type.as_deref()) {
        return Err(Error::UnexpectedContentType);
    }
    if !content_encoding_allowed(response.content_encoding.as_deref()) {
        return Err(Error::UnexpectedContentEncoding);
    }
    if response
        .content_length
        .is_some_and(|bytes| bytes > MAX_BODY_BYTES as u64)
    {
        return Err(Error::BodyTooLarge);
    }
    if response.body.len() > MAX_BODY_BYTES {
        return Err(Error::BodyTooLarge);
    }
    if response.body.is_empty() {
        return Err(Error::EmptyBody);
    }
    if response
        .content_length
        .is_some_and(|bytes| bytes != response.body.len() as u64)
    {
        return Err(Error::UnexpectedContentLength);
    }
    Ok(())
}

fn content_type_allowed(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let media = value.split(';').next().unwrap_or(value).trim();
    media.eq_ignore_ascii_case("application/json")
}

fn content_encoding_allowed(value: Option<&str>) -> bool {
    match value {
        None => true,
        Some(value) => value.trim().eq_ignore_ascii_case("identity"),
    }
}

pub fn read_bounded(reader: &mut impl Read, max: usize, declared: Option<u64>) -> Result<Vec<u8>> {
    if declared.is_some_and(|bytes| bytes > max as u64) {
        return Err(Error::BodyTooLarge);
    }
    let mut body = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| Error::Transport(TransportFailure::ResponseBody))?;
        if read == 0 {
            break;
        }
        if body.len().saturating_add(read) > max {
            return Err(Error::BodyTooLarge);
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_metadata_is_checked_before_trusting_the_body() {
        let endpoint = PublicEndpoint::Status;
        let ok = TransportResponse {
            status: 200,
            final_url: endpoint.url().to_owned(),
            content_type: Some("application/json; charset=utf-8".to_owned()),
            content_encoding: Some("identity".to_owned()),
            content_length: Some(2),
            body: b"{}".to_vec(),
        };
        assert!(validate_response(&ok, endpoint).is_ok());

        let redirected = TransportResponse {
            final_url: "https://limitreset.net/elsewhere".to_owned(),
            ..ok_response()
        };
        assert_eq!(
            validate_response(&redirected, endpoint).unwrap_err(),
            Error::UnexpectedFinalUrl
        );
        assert_eq!(
            validate_response(
                &TransportResponse {
                    status: 302,
                    ..ok_response()
                },
                endpoint
            )
            .unwrap_err(),
            Error::UnexpectedStatus(302)
        );
        assert_eq!(
            validate_response(
                &TransportResponse {
                    content_type: Some("text/html".to_owned()),
                    ..ok_response()
                },
                endpoint
            )
            .unwrap_err(),
            Error::UnexpectedContentType
        );
        assert_eq!(
            validate_response(
                &TransportResponse {
                    content_encoding: Some("gzip".to_owned()),
                    ..ok_response()
                },
                endpoint
            )
            .unwrap_err(),
            Error::UnexpectedContentEncoding
        );
        assert_eq!(
            validate_response(
                &TransportResponse {
                    content_length: Some((MAX_BODY_BYTES as u64) + 1),
                    ..ok_response()
                },
                endpoint
            )
            .unwrap_err(),
            Error::BodyTooLarge
        );
    }

    fn ok_response() -> TransportResponse {
        TransportResponse {
            status: 200,
            final_url: PublicEndpoint::Status.url().to_owned(),
            content_type: Some("application/json".to_owned()),
            content_encoding: None,
            content_length: Some(2),
            body: b"{}".to_vec(),
        }
    }

    #[test]
    fn bounded_reader_stops_before_unbounded_allocation() {
        let mut huge = std::io::repeat(b'x');
        assert_eq!(
            read_bounded(&mut huge, 8, Some(9)).unwrap_err(),
            Error::BodyTooLarge
        );
        let mut over = std::io::Cursor::new(vec![1; 9]);
        assert_eq!(
            read_bounded(&mut over, 8, None).unwrap_err(),
            Error::BodyTooLarge
        );
    }
}
