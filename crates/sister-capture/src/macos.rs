//! macOS 原生擷取的 CI 診斷切片。
//!
//! 這還不是產品後端：它只在 `macos-ci-spike` feature 下存在，用來量出
//! `AI-Sister.app/Contents/MacOS/sister` 能否在同一個 app process tree 裡連結並
//! 呼叫 ScreenCaptureKit。沒有同意書、持續錄製、Vision OCR、AX 或 TCC 撤回
//! 生命週期，所以 shipping build 不會帶這個入口。

use objc2_core_graphics::{CGMainDisplayID, CGPreflightScreenCaptureAccess};
use screencapturekit::error::{SC_STREAM_ERROR_DOMAIN, SCError};
use screencapturekit::screenshot_manager::SCScreenshotManager;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::{
    configuration::SCStreamConfiguration, content_filter::SCContentFilter,
};
use serde::Serialize;

/// `CGPreflightScreenCaptureAccess` 只回一個 bool。`NotGrantedOrUndetermined`
/// 刻意不叫 Denied：CoreGraphics 沒告訴我們使用者拒絕過，還是從未回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightAccess {
    Granted,
    NotGrantedOrUndetermined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStage {
    ShareableContent,
    ContentFilter,
    Screenshot,
}

/// 這是 `screencapturekit::SCError` 的 wrapper variant，不是對原生根因的推測。
/// 目前使用的 shareable-content／screenshot bridge 通常會把底層 NSError 壓成
/// `NoShareableContent`／`ScreenshotError`，那時候 domain 與 code 都必須留空。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureErrorKind {
    InvalidConfiguration,
    InvalidDimension,
    InvalidPixelFormat,
    NoShareableContent,
    DisplayNotFound,
    WindowNotFound,
    ApplicationNotFound,
    Stream,
    CaptureStart,
    CaptureStop,
    BufferLock,
    BufferUnlock,
    InvalidBuffer,
    Screenshot,
    Permission,
    FeatureUnavailable,
    Ffi,
    NullPointer,
    Timeout,
    Internal,
    Os,
    ScreenCaptureKitStream,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeFailure {
    pub kind: CaptureErrorKind,
    /// 只有 crate 真的保留 `SCStreamError` variant 時才有 domain。Screenshot
    /// bridge 通常只留下 localized description；那時候這格必須是 `None`。
    pub domain: Option<&'static str>,
    pub code: Option<i32>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CaptureOutcome {
    /// preflight 不是 true 時不嘗試，避免 CI 或背景啟動路徑叫出 TCC prompt。
    NotAttempted,
    NoDisplay,
    Captured {
        display_id: u32,
        width: usize,
        height: usize,
    },
    InvalidImageDimensions {
        display_id: u32,
        width: usize,
        height: usize,
    },
    Failed {
        stage: CaptureStage,
        error: NativeFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureProbeReport {
    pub schema: u8,
    pub preflight: PreflightAccess,
    pub capture: CaptureOutcome,
}

/// 做一次不落盤的原生 probe。
///
/// `Captured` 的尺寸取自真正回來的 `CGImage`，不是要求 SCK 產生的設定值。
/// `CGImage` 離開函式就釋放；這裡不把 pixel buffer 讀成 Rust bytes，也沒有
/// PNG／clipboard／database 出口。
pub fn diagnostic_probe() -> CaptureProbeReport {
    // screencapturekit 自己的原生 CI 與 examples 也會先呼叫 CGMainDisplayID；
    // CLI child 沒有跑 AppKit event loop，少這步可能被 CGS_REQUIRE_INIT 終止。
    let _ = CGMainDisplayID();
    let preflight = if CGPreflightScreenCaptureAccess() {
        PreflightAccess::Granted
    } else {
        PreflightAccess::NotGrantedOrUndetermined
    };
    if preflight != PreflightAccess::Granted {
        return CaptureProbeReport {
            schema: 1,
            preflight,
            capture: CaptureOutcome::NotAttempted,
        };
    }

    // 不用「逾時就放掉 thread」的假取消：ScreenCaptureKit 的同步 screenshot API
    // 沒有 cancellation handle。這個 probe 本身跑在獨立的 bundled child；外層
    // app 持有 exact Child handle，超時時會 kill + wait 整個 process boundary。
    let capture = capture_once();
    CaptureProbeReport {
        schema: 1,
        preflight,
        capture,
    }
}

fn capture_once() -> CaptureOutcome {
    let content = match SCShareableContent::get() {
        Ok(content) => content,
        Err(error) => return failed(CaptureStage::ShareableContent, error),
    };
    let Some(display) = content.displays().into_iter().next() else {
        return CaptureOutcome::NoDisplay;
    };
    let display_id = display.display_id();
    let filter = match SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .try_build()
    {
        Ok(filter) => filter,
        Err(error) => return failed(CaptureStage::ContentFilter, error),
    };
    let configuration = SCStreamConfiguration::new()
        .with_width(display.width())
        .with_height(display.height());
    let image = match SCScreenshotManager::capture_image(&filter, &configuration) {
        Ok(image) => image,
        Err(error) => return failed(CaptureStage::Screenshot, error),
    };
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        CaptureOutcome::InvalidImageDimensions {
            display_id,
            width,
            height,
        }
    } else {
        CaptureOutcome::Captured {
            display_id,
            width,
            height,
        }
    }
}

fn failed(stage: CaptureStage, error: SCError) -> CaptureOutcome {
    let (kind, domain, code) = error_metadata(&error);
    CaptureOutcome::Failed {
        stage,
        error: NativeFailure {
            kind,
            domain,
            code,
            message: error.to_string(),
        },
    }
}

fn error_metadata(error: &SCError) -> (CaptureErrorKind, Option<&'static str>, Option<i32>) {
    let kind = match error {
        SCError::InvalidConfiguration(_) => CaptureErrorKind::InvalidConfiguration,
        SCError::InvalidDimension { .. } => CaptureErrorKind::InvalidDimension,
        SCError::InvalidPixelFormat(_) => CaptureErrorKind::InvalidPixelFormat,
        SCError::NoShareableContent(_) => CaptureErrorKind::NoShareableContent,
        SCError::DisplayNotFound(_) => CaptureErrorKind::DisplayNotFound,
        SCError::WindowNotFound(_) => CaptureErrorKind::WindowNotFound,
        SCError::ApplicationNotFound(_) => CaptureErrorKind::ApplicationNotFound,
        SCError::StreamError(_) => CaptureErrorKind::Stream,
        SCError::CaptureStartFailed(_) => CaptureErrorKind::CaptureStart,
        SCError::CaptureStopFailed(_) => CaptureErrorKind::CaptureStop,
        SCError::BufferLockError(_) => CaptureErrorKind::BufferLock,
        SCError::BufferUnlockError(_) => CaptureErrorKind::BufferUnlock,
        SCError::InvalidBuffer(_) => CaptureErrorKind::InvalidBuffer,
        SCError::ScreenshotError(_) => CaptureErrorKind::Screenshot,
        SCError::PermissionDenied(_) => CaptureErrorKind::Permission,
        SCError::FeatureNotAvailable { .. } => CaptureErrorKind::FeatureUnavailable,
        SCError::FFIError(_) => CaptureErrorKind::Ffi,
        SCError::NullPointer(_) => CaptureErrorKind::NullPointer,
        SCError::Timeout(_) => CaptureErrorKind::Timeout,
        SCError::InternalError(_) => CaptureErrorKind::Internal,
        SCError::OSError { .. } => CaptureErrorKind::Os,
        SCError::SCStreamError { .. } => CaptureErrorKind::ScreenCaptureKitStream,
        _ => CaptureErrorKind::Other,
    };
    match error {
        SCError::OSError { code, .. } => (kind, None, Some(*code)),
        SCError::SCStreamError { code, .. } => {
            (kind, Some(SC_STREAM_ERROR_DOMAIN), Some(code.as_raw()))
        }
        _ => (kind, None, None),
    }
}
