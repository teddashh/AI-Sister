//! macOS 14+ 原生擷取後端。
//!
//! 畫面只由 ScreenCaptureKit 讀進 RAM，OCR 只呼叫 Apple Vision。
//! Accessibility 在每一拍讀畫面前先確定前景 app、視窗、密碼欄與瀏覽器
//! 位址；任一項問不出來就不會進入 screen source。擷取前後再以 PID +
//! window title 重驗同一份 permit，避免在慢 OCR 期間切到另一個視窗後落盤。

use accessibility::{
    AXAttribute, AXUIElement, AXUIElementAttributes, TreeVisitor, TreeWalker, TreeWalkerFlow,
};
use accessibility_sys::kAXFocusedUIElementAttribute;
use anyhow::{Context, Result, anyhow};
use core_foundation::base::CFType;
use core_foundation::string::CFString;
use objc2::AnyThread;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWorkspace;
use objc2_application_services::AXIsProcessTrusted;
use objc2_core_graphics::{CGMainDisplayID, CGPreflightScreenCaptureAccess};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
    VNRequestTextRecognitionLevel,
};
use screencapturekit::error::{SC_STREAM_ERROR_DOMAIN, SCError};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::{
    configuration::SCStreamConfiguration, content_filter::SCContentFilter,
};
use serde::Serialize;
use sister_core::capabilities::CapabilityState;
use sister_core::config::Config;
use sister_core::db::Db;
use sister_core::model::{
    BrowserUrlState, FocusSnapshot, Millis, OcrBlock, PrivacyContext, SensitiveFieldState,
};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::traits::{
    CapturePermit, CompositeBackend, FocusSource, NullClipboard, NullInput, Ocr,
    PrivacyObservation, RawFrame, ScreenSource, SystemContentState, SystemLockState,
    SystemObservation, SystemPowerState, SystemSource, SystemTransition, SystemTransitionKind,
};
use crate::{Backend, MasterStopSource, Recorder};

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
    Pixels,
}

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
    pub domain: Option<&'static str>,
    pub code: Option<i32>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CaptureOutcome {
    NotAttempted,
    NoDisplay,
    Captured {
        display_id: u32,
        width: usize,
        height: usize,
        rgba_bytes: usize,
    },
    Failed {
        stage: CaptureStage,
        error: NativeFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureProbeReport {
    pub schema: u8,
    pub screen_recording: PreflightAccess,
    pub accessibility: PreflightAccess,
    pub capture: CaptureOutcome,
}

/// 設定頁與 CLI 共用的原生權限事實。這支不觸發 TCC prompt。
pub fn access_report() -> CaptureProbeReport {
    let _ = CGMainDisplayID();
    let screen_recording = measured_access(CGPreflightScreenCaptureAccess());
    let accessibility = measured_access(unsafe { AXIsProcessTrusted() });
    let capture = if screen_recording == PreflightAccess::Granted {
        match capture_rgba(None) {
            Ok(Some(frame)) => CaptureOutcome::Captured {
                display_id: frame.display_id,
                width: frame.width,
                height: frame.height,
                rgba_bytes: frame.rgba.len(),
            },
            Ok(None) => CaptureOutcome::NoDisplay,
            Err(failure) => CaptureOutcome::Failed {
                stage: failure.stage,
                error: failure.error,
            },
        }
    } else {
        CaptureOutcome::NotAttempted
    };
    CaptureProbeReport {
        schema: 2,
        screen_recording,
        accessibility,
        capture,
    }
}

/// 相容舊的 diagnostic 呼叫名；現在報告同時驗像素與 AX 權限。
pub fn diagnostic_probe() -> CaptureProbeReport {
    access_report()
}

fn measured_access(granted: bool) -> PreflightAccess {
    if granted {
        PreflightAccess::Granted
    } else {
        PreflightAccess::NotGrantedOrUndetermined
    }
}

struct CapturedRgba {
    display_id: u32,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

struct CaptureFailure {
    stage: CaptureStage,
    error: NativeFailure,
}

fn capture_rgba(
    target: Option<&ForegroundIdentity>,
) -> std::result::Result<Option<CapturedRgba>, CaptureFailure> {
    let content = SCShareableContent::get()
        .map_err(|error| capture_failure(CaptureStage::ShareableContent, error))?;
    let displays = content.displays();
    let display = if let Some(target) = target {
        let windows = content.windows();
        let window = windows
            .iter()
            .filter(|window| {
                window.is_on_screen()
                    && window
                        .owning_application()
                        .is_some_and(|application| application.process_id() == target.pid)
            })
            .max_by_key(|window| {
                (
                    window.is_active(),
                    window.title().as_deref() == Some(target.window_title.as_str()),
                )
            });
        let Some(window) = window else {
            return Err(native_capture_failure(
                CaptureStage::ShareableContent,
                CaptureErrorKind::WindowNotFound,
                "ScreenCaptureKit 找不到目前的前景視窗",
            ));
        };
        let window_frame = window.frame();
        let selected = displays
            .iter()
            .map(|display| (display, overlap_area(window_frame, display.frame())))
            .max_by(|(_, left), (_, right)| left.total_cmp(right));
        match selected {
            Some((display, overlap)) if overlap > 0.0 => Some(display),
            _ => {
                return Err(native_capture_failure(
                    CaptureStage::ShareableContent,
                    CaptureErrorKind::DisplayNotFound,
                    "ScreenCaptureKit 找不到前景視窗所在的顯示器",
                ));
            }
        }
    } else {
        let main = CGMainDisplayID();
        displays
            .iter()
            .find(|display| display.display_id() == main)
            .or_else(|| displays.first())
    };
    let Some(display) = display else {
        return Ok(None);
    };
    let display_id = display.display_id();
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .try_build()
        .map_err(|error| capture_failure(CaptureStage::ContentFilter, error))?;
    let configuration = SCStreamConfiguration::new()
        .with_width(display.width())
        .with_height(display.height());
    let image = SCScreenshotManager::capture_image(&filter, &configuration)
        .map_err(|error| capture_failure(CaptureStage::Screenshot, error))?;
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(CaptureFailure {
            stage: CaptureStage::Screenshot,
            error: NativeFailure {
                kind: CaptureErrorKind::InvalidDimension,
                domain: None,
                code: None,
                message: format!("ScreenCaptureKit returned {width}x{height}"),
            },
        });
    }
    let rgba = image
        .rgba_data()
        .map_err(|error| capture_failure(CaptureStage::Pixels, error))?;
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| CaptureFailure {
            stage: CaptureStage::Pixels,
            error: NativeFailure {
                kind: CaptureErrorKind::InvalidDimension,
                domain: None,
                code: None,
                message: "ScreenCaptureKit image dimensions overflow".to_owned(),
            },
        })?;
    if rgba.len() != expected {
        return Err(CaptureFailure {
            stage: CaptureStage::Pixels,
            error: NativeFailure {
                kind: CaptureErrorKind::InvalidBuffer,
                domain: None,
                code: None,
                message: format!(
                    "ScreenCaptureKit returned {} RGBA bytes; expected {expected}",
                    rgba.len()
                ),
            },
        });
    }
    Ok(Some(CapturedRgba {
        display_id,
        width,
        height,
        rgba,
    }))
}

fn overlap_area(left: screencapturekit::cg::CGRect, right: screencapturekit::cg::CGRect) -> f64 {
    let left_x2 = left.origin.x + left.size.width;
    let left_y2 = left.origin.y + left.size.height;
    let right_x2 = right.origin.x + right.size.width;
    let right_y2 = right.origin.y + right.size.height;
    let width = left_x2.min(right_x2) - left.origin.x.max(right.origin.x);
    let height = left_y2.min(right_y2) - left.origin.y.max(right.origin.y);
    width.max(0.0) * height.max(0.0)
}

fn native_capture_failure(
    stage: CaptureStage,
    kind: CaptureErrorKind,
    message: &str,
) -> CaptureFailure {
    CaptureFailure {
        stage,
        error: NativeFailure {
            kind,
            domain: None,
            code: None,
            message: message.to_owned(),
        },
    }
}

fn capture_failure(stage: CaptureStage, error: SCError) -> CaptureFailure {
    let (kind, domain, code) = error_metadata(&error);
    CaptureFailure {
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

pub struct MacScreen;

impl ScreenSource for MacScreen {
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        let target = frontmost()?.identity;
        let Some(frame) = capture_rgba(Some(&target)).map_err(|failure| {
            anyhow!(
                "ScreenCaptureKit {:?}: {}",
                failure.stage,
                failure.error.message
            )
        })?
        else {
            return Ok(None);
        };
        let width = u32::try_from(frame.width).context("macOS capture width exceeds u32")?;
        let height = u32::try_from(frame.height).context("macOS capture height exceeds u32")?;
        let monitor = i32::try_from(frame.display_id).context("macOS display id exceeds i32")?;
        Ok(Some(RawFrame::from_rgba(
            ts, monitor, width, height, frame.rgba,
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForegroundIdentity {
    pid: i32,
    window_title: String,
}

struct Frontmost {
    identity: ForegroundIdentity,
    app_id: Option<String>,
    app_name: Option<String>,
    window: AXUIElement,
    focused: AXUIElement,
}

fn frontmost() -> Result<Frontmost> {
    let app = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .context("macOS has no frontmost application")?;
    let pid = app.processIdentifier();
    anyhow::ensure!(pid > 0, "macOS frontmost application has no process id");
    let app_id = app.bundleIdentifier().map(|value| value.to_string());
    let app_name = app.localizedName().map(|value| value.to_string());
    let ax_app = AXUIElement::application(pid);
    ax_app
        .set_messaging_timeout(1.0)
        .context("set macOS Accessibility timeout")?;
    let window = ax_app
        .focused_window()
        .context("read macOS focused window")?;
    let focused_attribute =
        AXAttribute::<CFType>::new(&CFString::from_static_string(kAXFocusedUIElementAttribute));
    let focused = ax_app
        .attribute(&focused_attribute)
        .context("read macOS focused element")?
        .downcast_into::<AXUIElement>()
        .context("macOS focused element has unexpected type")?;
    let window_title = window
        .title()
        .map(|title| title.to_string())
        .unwrap_or_default();
    Ok(Frontmost {
        identity: ForegroundIdentity { pid, window_title },
        app_id,
        app_name,
        window,
        focused,
    })
}

fn sensitive_state(element: &AXUIElement) -> SensitiveFieldState {
    let Ok(role) = element.role().map(|value| value.to_string()) else {
        return SensitiveFieldState::Unknown;
    };
    let subrole = element
        .subrole()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let marker = [
        element.description().map(|v| v.to_string()).ok(),
        element.title().map(|v| v.to_string()).ok(),
        element.placeholder_value().map(|v| v.to_string()).ok(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();
    if role == "AXSecureTextField"
        || subrole == "AXSecureTextField"
        || (role == "AXTextField"
            && ["password", "passcode", "密碼", "密码"]
                .iter()
                .any(|needle| marker.contains(needle)))
    {
        SensitiveFieldState::Focused
    } else {
        SensitiveFieldState::Clear
    }
}

struct AddressVisitor {
    found: RefCell<Option<String>>,
    depth: Cell<usize>,
    visited: Cell<usize>,
    deadline: Instant,
}

impl AddressVisitor {
    fn new() -> Self {
        Self {
            found: RefCell::new(None),
            depth: Cell::new(0),
            visited: Cell::new(0),
            deadline: Instant::now() + Duration::from_secs(2),
        }
    }
}

impl TreeVisitor for AddressVisitor {
    fn enter_element(&self, element: &AXUIElement) -> TreeWalkerFlow {
        self.depth.set(self.depth.get() + 1);
        self.visited.set(self.visited.get() + 1);
        if self.found.borrow().is_some() {
            return TreeWalkerFlow::Exit;
        }
        if Instant::now() >= self.deadline {
            return TreeWalkerFlow::Exit;
        }
        if self.depth.get() > 32 || self.visited.get() > 2_000 {
            return TreeWalkerFlow::SkipSubtree;
        }
        let role = element
            .role()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if role != "AXTextField" && role != "AXComboBox" {
            return TreeWalkerFlow::Continue;
        }
        let marker = [
            element.identifier().map(|v| v.to_string()).ok(),
            element.description().map(|v| v.to_string()).ok(),
            element.title().map(|v| v.to_string()).ok(),
            element.placeholder_value().map(|v| v.to_string()).ok(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
        if !["address", "location", "url", "omnibox", "search or enter"]
            .iter()
            .any(|needle| marker.contains(needle))
        {
            return TreeWalkerFlow::Continue;
        }
        let value = element
            .value()
            .ok()
            .and_then(|value: CFType| value.downcast::<CFString>())
            .map(|value| value.to_string());
        if let Some(value) = value.filter(|value| plausible_browser_url(value)) {
            self.found.replace(Some(value));
            TreeWalkerFlow::Exit
        } else {
            TreeWalkerFlow::Continue
        }
    }

    fn exit_element(&self, _element: &AXUIElement) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

fn plausible_browser_url(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return false;
    }
    [
        "http://",
        "https://",
        "file://",
        "about:",
        "chrome://",
        "edge://",
    ]
    .iter()
    .any(|prefix| value.to_ascii_lowercase().starts_with(prefix))
        || (value.contains('.') && !value.starts_with('.'))
}

fn browser_url(window: &AXUIElement) -> Option<String> {
    let visitor = AddressVisitor::new();
    TreeWalker::new().walk(window, &visitor);
    visitor.found.into_inner()
}

pub struct MacFocus {
    approved: Option<(CapturePermit, ForegroundIdentity, PrivacyContext)>,
    generation: u64,
}

impl MacFocus {
    fn new() -> Self {
        Self {
            approved: None,
            generation: 0,
        }
    }

    fn observe_current() -> Option<(ForegroundIdentity, PrivacyContext)> {
        if !unsafe { AXIsProcessTrusted() } {
            return None;
        }
        let front = frontmost().ok()?;
        let identity = front.identity.clone();
        let sensitive = sensitive_state(&front.focused);
        let app_key = front
            .app_id
            .as_deref()
            .or(front.app_name.as_deref())
            .unwrap_or("")
            .to_ascii_lowercase();
        let url = if crate::browsers::is_browser(&app_key) {
            browser_url(&front.window).map_or(BrowserUrlState::Unknown, BrowserUrlState::Known)
        } else {
            BrowserUrlState::NotApplicable
        };
        let context = PrivacyContext::known(
            FocusSnapshot {
                app_id: front.app_id,
                app_name: front.app_name,
                window_title: (!identity.window_title.is_empty())
                    .then(|| identity.window_title.clone()),
                url: None,
                pid: Some(i64::from(identity.pid)),
            },
            sensitive,
            url,
        );
        (frontmost().ok()?.identity == identity).then_some((identity, context))
    }
}

impl FocusSource for MacFocus {
    fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
        let Some((identity, context)) = Self::observe_current() else {
            self.approved = None;
            return Ok(PrivacyObservation::Unknown);
        };
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("macOS capture permit generation exhausted"))?;
        let permit = CapturePermit::backend_local(self.generation);
        self.approved = Some((permit, identity, context.clone()));
        Ok(PrivacyObservation::known(context, permit))
    }

    fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
        let Some((approved, identity, context)) = self.approved.as_ref() else {
            return Ok(false);
        };
        if *approved != permit {
            return Ok(false);
        }
        Ok(
            Self::observe_current().is_some_and(|(now_identity, now_context)| {
                now_identity == *identity && now_context == *context
            }),
        )
    }
}

pub struct MacSystem {
    last: Option<SystemContentState>,
    sequence: u64,
}

impl MacSystem {
    fn new() -> Self {
        Self {
            last: None,
            sequence: 0,
        }
    }

    fn current() -> Option<SystemContentState> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let bundle = app.bundleIdentifier().map(|value| value.to_string());
        Some(if bundle.as_deref() == Some("com.apple.loginwindow") {
            SystemContentState::new(SystemLockState::Locked, SystemPowerState::Awake)
        } else {
            SystemContentState::active()
        })
    }
}

impl SystemSource for MacSystem {
    fn poll(&mut self, ts: Millis) -> Result<SystemObservation> {
        let Some(state) = Self::current() else {
            return Ok(SystemObservation::Unknown);
        };
        let transition = match self.last {
            Some(previous) if previous.lock() != state.lock() => Some(match state.lock() {
                SystemLockState::Locked => SystemTransitionKind::Lock,
                SystemLockState::Unlocked => SystemTransitionKind::Unlock,
            }),
            _ => None,
        };
        self.last = Some(state);
        let transitions = transition
            .map(|kind| {
                self.sequence = self.sequence.saturating_add(1);
                vec![SystemTransition {
                    sequence: self.sequence,
                    ts,
                    kind,
                }]
            })
            .unwrap_or_default();
        Ok(SystemObservation::Known { state, transitions })
    }
}

pub struct VisionOcr {
    languages: Vec<String>,
}

impl VisionOcr {
    fn new(languages: &[String]) -> Self {
        Self {
            languages: languages.to_vec(),
        }
    }
}

impl Ocr for VisionOcr {
    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>> {
        let rgba = frame
            .rgba
            .as_deref()
            .context("Vision OCR needs RGBA pixels")?;
        let png = crate::frames::encode_downscaled(rgba, frame.width, frame.height, 0)
            .context("encode Vision OCR input")?;
        let data = NSData::with_bytes(&png);
        let options = NSDictionary::<VNImageOption, AnyObject>::from_slices::<NSString>(&[], &[]);
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(false);
        let languages: Vec<_> = self
            .languages
            .iter()
            .map(|language| NSString::from_str(language))
            .collect();
        if !languages.is_empty() {
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
        }
        let base: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[base]))
            .map_err(|error| anyhow!("Vision OCR: {error}"))?;
        let Some(results) = request.results() else {
            return Ok(Vec::new());
        };
        let mut blocks = Vec::with_capacity(results.count());
        for index in 0..results.count() {
            let observation = results.objectAtIndex(index);
            let candidates = observation.topCandidates(1);
            let Some(candidate) = candidates.firstObject() else {
                continue;
            };
            let text = candidate.string().to_string();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let rect = unsafe { observation.boundingBox() };
            let width = f64::from(frame.width);
            let height = f64::from(frame.height);
            let x = (rect.origin.x * width).round();
            let y = ((1.0 - rect.origin.y - rect.size.height) * height).round();
            let w = (rect.size.width * width).round();
            let h = (rect.size.height * height).round();
            blocks.push(OcrBlock {
                text: text.to_owned(),
                x: x.clamp(0.0, f64::from(i32::MAX)) as i32,
                y: y.clamp(0.0, f64::from(i32::MAX)) as i32,
                w: w.clamp(0.0, f64::from(i32::MAX)) as i32,
                h: h.clamp(0.0, f64::from(i32::MAX)) as i32,
                confidence: candidate.confidence(),
            });
        }
        Ok(blocks)
    }
}

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub screen: CapabilityState,
    pub accessibility: CapabilityState,
    pub ocr: CapabilityState,
}

impl Capabilities {
    pub fn current() -> Self {
        Self {
            screen: CapabilityState::from_measured(CGPreflightScreenCaptureAccess()),
            accessibility: CapabilityState::from_measured(unsafe { AXIsProcessTrusted() }),
            ocr: CapabilityState::Available,
        }
    }

    pub fn report(&self) -> sister_core::capabilities::Report {
        sister_core::capabilities::Report {
            at: sister_core::now_ms(),
            url: self.accessibility,
            input_hook: CapabilityState::Unavailable,
            ..Default::default()
        }
    }
}

fn backend(config: &Config) -> Result<impl Backend + use<>> {
    anyhow::ensure!(
        CGPreflightScreenCaptureAccess(),
        "macOS 螢幕與系統音訊錄製權限未開啟"
    );
    anyhow::ensure!(unsafe { AXIsProcessTrusted() }, "macOS 輔助使用權限未開啟");
    Ok(CompositeBackend {
        name: "macos-screencapturekit-vision-ax-v1".to_owned(),
        system: MacSystem::new(),
        screen: MacScreen,
        focus: MacFocus::new(),
        clipboard: NullClipboard,
        input: NullInput,
        ocr: VisionOcr::new(&config.capture.ocr_languages),
    })
}

pub fn recorder(
    config: Config,
    db: Db,
    image_dir: Option<PathBuf>,
    data_dir: PathBuf,
) -> Result<Recorder<impl Backend + use<>>> {
    let backend = backend(&config)?;
    Recorder::new(
        backend,
        db,
        config,
        image_dir,
        MasterStopSource::Latch(data_dir),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_url_filter_rejects_search_text_and_accepts_addresses() {
        assert!(plausible_browser_url("https://example.com/a"));
        assert!(plausible_browser_url("example.com"));
        assert!(plausible_browser_url("about:blank"));
        assert!(!plausible_browser_url("search words"));
        assert!(!plausible_browser_url(""));
    }
}
