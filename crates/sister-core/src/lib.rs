//! `sister-core` — 平台無關的核心。
//!
//! 這個 crate 只認識 [`model::Signal`]，不認識 Windows、macOS 或 X11。
//! 所有平台專屬的東西住在 `sister-capture`；所有 UI 住在 `sister-shell`。
//! 因此核心在無頭 Linux 上可以完整測試——這不是為了方便，
//! 而是 Phase 2 replay 評測（SPEC §12）的前提。
//!
//! # 分層（SPEC §0）
//!
//! - **L0 證據**：[`model`] 的訊號型別 + [`db`] 的 append-only 資料表。不經 LLM。
//! - **L1 事實**：[`facts`] 的 typed 抽取。純規則、零幻覺、可重跑。
//! - L2 假設 / L3 狀態：Phase 4 以後才存在。Phase 0 沒有任何模型呼叫。
//!
//! 「抄寫歸程式，意圖歸模型」——這個 crate 目前為止全是抄寫。

pub mod answer;
pub mod capabilities;
pub mod config;
pub mod consent;
pub mod control;
pub mod db;
pub mod dedup;
pub mod eval;
pub mod facts;
pub mod heartbeat;
pub mod model;
pub mod moments;
pub mod pause;
pub mod question;
pub mod redact;
pub mod replay;
pub mod retention;
pub mod retrieval;

pub use config::{CaptureConfig, Config, Exclusion, PrivacyConfig, RetentionConfig};
pub use db::{Db, DbStats, FactRow, FrameContext, SCHEMA_VERSION};
pub use dedup::{Deduper, FrameVerdict, dhash_gray, dhash_rgb, hamming};
pub use facts::{ExtractedFact, FactKind, extract};
pub use model::{
    ClipboardEvent, ClipboardKind, FocusEvent, FocusKind, FocusSnapshot, FrameCapture,
    InputMetrics, Millis, OcrBlock, SearchHit, Signal, SourceKind, SystemEvent, SystemKind,
    TextChunk, now_ms,
};
pub use redact::{SecretKind, looks_like_secret};

/// Crate 版本，寫進 sessions 表供事後回溯用。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
