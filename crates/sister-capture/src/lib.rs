//! `sister-capture` — 感官層。
//!
//! 這個 crate 是平台差異的唯一收容所。它往上只交出
//! [`sister_core::model`] 裡的訊號型別，因此核心與所有上層邏輯
//! 完全不知道自己跑在哪個作業系統上。
//!
//! [`replay`] 讓整條錄製迴圈能在沒有螢幕的機器上被完整測試，
//! 也是 SPEC §12 replay 評測的地基。

pub mod frames;
pub mod recorder;
pub mod replay;
pub mod traits;

pub use recorder::{Recorder, RecorderStats, Tick};
pub use replay::{ReplayBackend, Scenario, Step};
pub use traits::{
    Backend, ClipboardSource, CompositeBackend, FocusSource, InputSource, NullClipboard, NullFocus,
    NullInput, NullOcr, NullScreen, Ocr, RawFrame, ScreenSource,
};
