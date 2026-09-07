//! AI-Sister Persona 固定素材包。
//!
//! 這個 crate 刻意分成兩種 dependency graph：預設只有純本機 authority、ZIP
//! 驗證與 cache；`download` feature 才帶入唯一的 HTTP client，而且只有 desktop
//! artifact 會打開。API 不接受 URL、header、persona 選擇或任何記憶內容。

mod authority;
mod cache;
#[cfg(feature = "download")]
mod download;
mod zip;

pub use authority::{
    CANONICAL_MANIFEST_SHA256, HOST, ORIGIN, PACK_BYTES, PACK_ENTRY_COUNT, PACK_EXTRACTED_BYTES,
    PACK_MEDIA_TYPE, PACK_PATH, PACK_SHA256, PACK_URL, Persona, RELEASE_ID, REQUEST_BOUNDARY_ZH_TW,
    VoiceLine,
};
pub use cache::{
    CACHE_DIRECTORY, CacheState, INSTALLED_ASSET_BYTES, InstallOutcome, Portrait, RemovalOutcome,
    Voice, VoiceMetadata, cache_state, install_from_bytes, read_portrait, read_voice, remove,
    voice_metadata,
};
#[cfg(feature = "download")]
pub use download::download_and_install;

/// 下載前由 native authority 提供給畫面的資料。每一格都是已知值；這個型別沒有
/// `Option`，所以 UI 不可能拿空字串或 0 冒充「還沒量到」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disclosure {
    pub release_id: &'static str,
    pub host: &'static str,
    pub bytes: usize,
    pub boundary_zh_tw: &'static str,
}

pub const fn disclosure() -> Disclosure {
    Disclosure {
        release_id: RELEASE_ID,
        host: HOST,
        bytes: PACK_BYTES,
        boundary_zh_tw: REQUEST_BOUNDARY_ZH_TW,
    }
}

#[cfg(feature = "download")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFailure {
    HostResolution,
    Tls,
    Timeout,
    Connection,
    Protocol,
    ResponseBody,
    Transport,
}

#[cfg(feature = "download")]
impl std::fmt::Display for DownloadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::HostResolution => "主機名稱解析失敗",
            Self::Tls => "TLS 憑證驗證或握手失敗",
            Self::Timeout => "連線逾時",
            Self::Connection => "網路連線失敗",
            Self::Protocol => "HTTP 傳輸格式失敗",
            Self::ResponseBody => "回應內容讀取失敗",
            Self::Transport => "傳輸失敗",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("素材包不是內嵌 authority 指定的 exact archive")]
    ArchiveRejected,
    #[error("本機素材 cache 不是完整、已驗證的 release")]
    CacheRejected,
    #[error("本機素材 cache 裡有不屬於這個 release 的項目，未自動刪除")]
    CacheHasUnknownEntries,
    #[error("另一個 AI-Sister 行程正在使用同一份 Persona 素材 cache")]
    CacheBusy,
    #[error("Persona 素材已被撤回；未完成的下載不會重新啟用它")]
    CacheRevoked,
    #[error("OS 無法提供 Persona 素材 cache 需要的安全隨機數")]
    SecureRandomUnavailable,
    #[error("素材檔案 I/O 失敗：{0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "download")]
    #[error("連不到固定素材下載主機（{0}）")]
    DownloadUnavailable(DownloadFailure),
    #[cfg(feature = "download")]
    #[error("素材主機回了未允許的 HTTP 狀態：{0}")]
    UnexpectedStatus(u16),
    #[cfg(feature = "download")]
    #[error("素材請求的最終網址不是內嵌 authority 指定的 exact URL")]
    UnexpectedFinalUrl,
    #[cfg(feature = "download")]
    #[error("素材主機回了未允許的 Content-Type")]
    UnexpectedContentType,
    #[cfg(feature = "download")]
    #[error("素材主機回了未允許的 Content-Encoding")]
    UnexpectedContentEncoding,
    #[cfg(feature = "download")]
    #[error("素材主機宣告的 Content-Length 不等於內嵌 authority")]
    UnexpectedContentLength,
    #[error("素材下載已由使用者停止")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
