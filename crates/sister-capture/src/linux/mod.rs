//! Linux X11 Developer Preview 原生後端。
//!
//! `DISPLAY` 能連上只證明有一個 X server 願意跟這個 process 說話；它不證明
//! 那是目前登入者的本機 system session，也不證明 session 沒鎖住。這裡因此把
//! transport 與 exact-process session 驗證綁在同一次 preflight：先建立一次
//! [`RustConnection`]，把**同一條連線**交給 verifier 看，再把那條連線本身搬進
//! private、不可 Clone 的 [`X11Ready`]。不會在核准後重連，避免檢查 A、使用 B。
//!
//! Production 以目前 process ID 問 logind，確認 active、local、X11 與 exact
//! display，再沿用 verifier 看過的同一條 X11 connection。AT-SPI 只讀前景程式的
//! focused role 與瀏覽器位址列；密碼欄或任何問不清楚的狀態一律停在 privacy gate。
//! 畫面經 X11 GetImage 進 RAM，OCR 只交給本機 Tesseract process。

use anyhow::{Context, Result, anyhow};
use atspi_common::{ObjectRefOwned, Role, State, StateSet};
use sister_core::capabilities::CapabilityState;
use sister_core::config::Config;
use sister_core::db::Db;
use sister_core::model::{
    BrowserUrlState, FocusSnapshot, Millis, OcrBlock, PrivacyContext, SensitiveFieldState,
};
use std::cell::RefCell;
#[cfg(test)]
use std::ffi::OsString;
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use x11rb::connection::Connection;
use x11rb::image::{Image, PixelLayout};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, parse_display};
use x11rb::rust_connection::RustConnection;
use zbus::blocking::{Connection as BusConnection, Proxy};
use zbus::names::BusName;
use zbus::zvariant::OwnedObjectPath;

use crate::traits::{
    Backend, CapturePermit, ClipboardCapture, ClipboardWatermark, DhashRecheck, OcrAttempt,
    PrivacyObservation, RawFrame, ScreenCapture, SystemContentState, SystemObservation,
    SystemTransition, SystemTransitionKind,
};
use crate::{MasterStopSource, Recorder};

const TESSERACT: &str = "/usr/bin/tesseract";

/// 明確量到「這個環境不是受支援的 X11 desktop」的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedReason {
    /// 這是 Wayland session（即使同時有 XWayland `DISPLAY` 也一樣）。
    Wayland,
    /// 沒有 X11 或 Wayland display endpoint 的 headless／TTY 環境。
    Headless,
}

/// 尚不足以安全開始讀桌面內容的原因。
///
/// 這些不是 [`UnsupportedReason`]：修正環境、權限或補上 session verifier 後可能
/// 立刻可用，所以不能把「沒驗到」寫成一個已驗證的 ✗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    /// session type 與 display endpoint 互相衝突，不能判定 process 所在桌面。
    ContradictorySession,
    /// `DISPLAY` 不是可解析、可連線並完成 X11 handshake 的 endpoint。
    DisplayUncheckable,
    /// 目前 process ID 無法由 logind 對到可信的本機 system session。
    SessionIdentityUncheckable,
    /// verifier 明確指出這條 X11 connection 不屬於目前 process 的 system session。
    SessionMismatch,
}

/// Preflight 對外只交出三態與原因；live X11 connection 不離開本模組。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightState {
    Available,
    Unsupported(UnsupportedReason),
    Unknown(UnknownReason),
}

impl PreflightState {
    /// 供之後的 capability report 使用。Unsupported 是量過的不可用；Unknown
    /// 仍然是沒辦法安全判定，兩者不可壓成同一個 `false`。
    pub const fn capability(self) -> CapabilityState {
        match self {
            Self::Available => CapabilityState::Available,
            Self::Unsupported(_) => CapabilityState::Unavailable,
            Self::Unknown(_) => CapabilityState::Unknown,
        }
    }
}

/// 一次 preflight 的結果。
///
/// 型別刻意不實作 `Clone`。Available 時它私下持有唯一那條已驗證連線；公開 API
/// 只能看狀態，不能取出、替換或自己拼一個 ready token。
#[must_use = "preflight state must be inspected before any Linux content source is opened"]
pub struct Preflight {
    outcome: Outcome,
}

impl Preflight {
    pub fn state(&self) -> PreflightState {
        match &self.outcome {
            Outcome::Available(ready) => {
                // `X11Ready` 的三個成員必須一起活到 consumer 接手。這裡不做
                // roundtrip，只確認保留下來的 screen 仍屬於同一份 setup。
                debug_assert!(ready.screen_index < ready.connection.setup().roots.len());
                debug_assert_eq!(ready.session.process_id, std::process::id());
                PreflightState::Available
            }
            Outcome::Unsupported(reason) => PreflightState::Unsupported(*reason),
            Outcome::Unknown(reason) => PreflightState::Unknown(*reason),
        }
    }
}

enum Outcome {
    Available(X11Ready),
    Unsupported(UnsupportedReason),
    Unknown(UnknownReason),
}

/// 唯一可交給後續 X11 sources 的 ready value。
///
/// Private + non-Clone 是安全邊界：session verifier 看過的 connection 不會被一條
/// 事後新建的 connection 代換。後續 backend composition 也必須留在本模組內消耗它。
struct X11Ready {
    // Box first, verify second, retain that same allocation. The address carried by the
    // verifier ticket therefore continues to identify this exact RustConnection after ready
    // itself moves into the outcome.
    connection: Box<RustConnection>,
    screen_index: usize,
    session: VerifiedExactSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConnectionIdentity {
    instance: usize,
    resource_id_base: u32,
    resource_id_mask: u32,
    root: u32,
}

impl ConnectionIdentity {
    fn read(connection: &RustConnection, screen_index: usize) -> Option<Self> {
        let setup = connection.setup();
        let screen = setup.roots.get(screen_index)?;
        Some(Self {
            instance: std::ptr::from_ref(connection).addr(),
            resource_id_base: setup.resource_id_base,
            resource_id_mask: setup.resource_id_mask,
            root: screen.root,
        })
    }
}

/// 由 verifier 對一個特定 PID + connection claim 核准的票。
/// 票帶著該 X11 client 的 resource-id namespace；不能拿另一條重連結果套用。
struct VerifiedExactSession {
    process_id: u32,
    display: String,
    screen_index: usize,
    connection: ConnectionIdentity,
    system_session_id: String,
}

struct SessionClaim<'a> {
    process_id: u32,
    display: &'a str,
    screen_index: usize,
    // 同時由 production logind verifier 與 injected verifier tests 綁住 exact connection。
    #[allow(dead_code)]
    connection_identity: ConnectionIdentity,
}

impl SessionClaim<'_> {
    /// 只能由正在被查的 claim 產生 ticket。verifier 應在用 PID 查完 logind、
    /// 並確認它就是這條本機 X11 session 後才呼叫。
    #[allow(dead_code)]
    fn verified(&self) -> VerifiedExactSession {
        VerifiedExactSession {
            process_id: self.process_id,
            display: self.display.to_owned(),
            screen_index: self.screen_index,
            connection: self.connection_identity,
            system_session_id: "test-session".to_owned(),
        }
    }

    fn verified_system_session(&self, system_session_id: String) -> VerifiedExactSession {
        VerifiedExactSession {
            process_id: self.process_id,
            display: self.display.to_owned(),
            screen_index: self.screen_index,
            connection: self.connection_identity,
            system_session_id,
        }
    }
}

enum SessionVerification {
    #[allow(dead_code)]
    Verified(VerifiedExactSession),
    Uncheckable,
    #[allow(dead_code)]
    Mismatch,
}

/// 必須用 `process_id` 查 logind system authority，不能相信 renderer、
/// `XDG_SESSION_ID` 或一段外部傳入的 session 名稱。trait 留在模組內，只有 production
/// source 與定點測試能核發 ticket。
trait ExactProcessSessionVerifier {
    fn verify(&mut self, claim: &SessionClaim<'_>) -> SessionVerification;
}

struct LogindVerifier;

fn same_local_x11_display(left: &str, right: &str) -> bool {
    let Ok(left) = parse_display(Some(left)) else {
        return false;
    };
    let Ok(right) = parse_display(Some(right)) else {
        return false;
    };
    let local = |display: &x11rb::reexports::x11rb_protocol::parse_display::ParsedDisplay| {
        display.host.is_empty()
            && (display.protocol.is_none() || display.protocol.as_deref() == Some("unix"))
    };
    local(&left) && local(&right) && left.display == right.display
}

impl ExactProcessSessionVerifier for LogindVerifier {
    fn verify(&mut self, claim: &SessionClaim<'_>) -> SessionVerification {
        let facts = match logind_session(claim.process_id) {
            Ok(facts) => facts,
            Err(_) => return SessionVerification::Uncheckable,
        };
        if !facts.active
            || facts.remote
            || facts.session_type != "x11"
            || !same_local_x11_display(&facts.display, claim.display)
        {
            return SessionVerification::Mismatch;
        }
        SessionVerification::Verified(claim.verified_system_session(facts.id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogindSession {
    id: String,
    display: String,
    session_type: String,
    active: bool,
    locked: bool,
    remote: bool,
}

fn logind_session(process_id: u32) -> Result<LogindSession> {
    let bus = logind_connection()?;
    logind_session_with(&bus, process_id)
}

fn logind_connection() -> Result<BusConnection> {
    zbus::blocking::connection::Builder::system()?
        .method_timeout(std::time::Duration::from_secs(1))
        .build()
        .context("connect system bus")
}

fn logind_session_with(bus: &BusConnection, process_id: u32) -> Result<LogindSession> {
    let manager = Proxy::new(
        bus,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )?;
    let path: OwnedObjectPath = manager.call("GetSessionByPID", &(process_id,))?;
    let session = Proxy::new(
        bus,
        "org.freedesktop.login1",
        path.as_str(),
        "org.freedesktop.login1.Session",
    )?;
    Ok(LogindSession {
        id: session.get_property("Id")?,
        display: session.get_property("Display")?,
        session_type: session.get_property("Type")?,
        active: session.get_property("Active")?,
        locked: session.get_property("LockedHint")?,
        remote: session.get_property("Remote")?,
    })
}

trait Connector {
    fn connect(&mut self, display: &str) -> Result<(RustConnection, usize), ConnectFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectFailure {
    Display,
}

struct RustConnector;

impl Connector for RustConnector {
    fn connect(&mut self, display: &str) -> Result<(RustConnection, usize), ConnectFailure> {
        let parsed = parse_display(Some(display)).map_err(|_error| ConnectFailure::Display)?;

        // A normal local DISPLAY such as `:0` makes x11rb try the Unix socket first and then
        // localhost TCP. Canonicalize that case to an explicit Unix transport so this capture
        // crate never gains a TCP fallback. Reject remote/SSH DISPLAY values before connect.
        let endpoint = if parsed.protocol.is_none() && parsed.host.is_empty() {
            format!("unix/:{}.{}", parsed.display, parsed.screen)
        } else if parsed
            .connect_instruction()
            .all(|address| matches!(address, ConnectAddress::Socket(_)))
        {
            display.to_owned()
        } else {
            return Err(ConnectFailure::Display);
        };

        // Bad screen, auth rejection, a stale socket and an I/O failure all say only this
        // much: the local endpoint in DISPLAY could not be established safely.
        RustConnection::connect(Some(&endpoint)).map_err(|_error| ConnectFailure::Display)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EnvironmentValue {
    Absent,
    Utf8(String),
    NonUtf8,
}

impl EnvironmentValue {
    fn current(name: &str) -> Self {
        match std::env::var_os(name) {
            None => Self::Absent,
            Some(value) if value.is_empty() => Self::Absent,
            Some(value) => match value.into_string() {
                Ok(value) => Self::Utf8(value),
                Err(_) => Self::NonUtf8,
            },
        }
    }

    #[cfg(test)]
    fn from_os(value: Option<OsString>) -> Self {
        match value {
            None => Self::Absent,
            Some(value) if value.is_empty() => Self::Absent,
            Some(value) => match value.into_string() {
                Ok(value) => Self::Utf8(value),
                Err(_) => Self::NonUtf8,
            },
        }
    }

    fn utf8(&self) -> Option<&str> {
        match self {
            Self::Utf8(value) => Some(value),
            Self::Absent | Self::NonUtf8 => None,
        }
    }
}

struct SessionEnvironment {
    process_id: u32,
    session_type: EnvironmentValue,
    display: EnvironmentValue,
    wayland_display: EnvironmentValue,
}

impl SessionEnvironment {
    fn current() -> Self {
        Self {
            process_id: std::process::id(),
            session_type: EnvironmentValue::current("XDG_SESSION_TYPE"),
            display: EnvironmentValue::current("DISPLAY"),
            wayland_display: EnvironmentValue::current("WAYLAND_DISPLAY"),
        }
    }
}

struct X11Candidate<'a> {
    process_id: u32,
    display: &'a str,
}

fn classify_environment(environment: &SessionEnvironment) -> Result<X11Candidate<'_>, Outcome> {
    let session_type = match &environment.session_type {
        EnvironmentValue::Utf8(value) => Some(value.trim().to_ascii_lowercase()),
        EnvironmentValue::Absent => None,
        EnvironmentValue::NonUtf8 => {
            return Err(Outcome::Unknown(UnknownReason::SessionIdentityUncheckable));
        }
    };
    let display = environment.display.utf8();
    let wayland_display = environment.wayland_display.utf8();

    // An explicitly Wayland session stays unsupported even when XWayland publishes DISPLAY.
    // That endpoint cannot provide the always-on privacy/system-session contract.
    if session_type.as_deref() == Some("wayland") {
        return Err(Outcome::Unsupported(UnsupportedReason::Wayland));
    }

    match session_type.as_deref() {
        Some("x11") => {
            if matches!(environment.display, EnvironmentValue::NonUtf8) {
                return Err(Outcome::Unknown(UnknownReason::DisplayUncheckable));
            }
            if matches!(environment.wayland_display, EnvironmentValue::NonUtf8)
                || display.is_none()
                || wayland_display.is_some()
            {
                return Err(Outcome::Unknown(UnknownReason::ContradictorySession));
            }
            Ok(X11Candidate {
                process_id: environment.process_id,
                display: display.expect("checked above"),
            })
        }
        None => {
            if matches!(environment.wayland_display, EnvironmentValue::NonUtf8) {
                return Err(Outcome::Unknown(UnknownReason::SessionIdentityUncheckable));
            }
            if wayland_display.is_some() {
                return Err(Outcome::Unsupported(UnsupportedReason::Wayland));
            }
            if matches!(environment.display, EnvironmentValue::NonUtf8) || display.is_some() {
                return Err(Outcome::Unknown(UnknownReason::SessionIdentityUncheckable));
            }
            Err(Outcome::Unsupported(UnsupportedReason::Headless))
        }
        Some(_) => {
            if display.is_none() && wayland_display.is_none() {
                Err(Outcome::Unsupported(UnsupportedReason::Headless))
            } else {
                Err(Outcome::Unknown(UnknownReason::ContradictorySession))
            }
        }
    }
}

fn preflight_with(
    environment: &SessionEnvironment,
    connector: &mut impl Connector,
    verifier: &mut impl ExactProcessSessionVerifier,
) -> Preflight {
    let candidate = match classify_environment(environment) {
        Ok(candidate) => candidate,
        Err(outcome) => return Preflight { outcome },
    };

    let (connection, screen_index) = match connector.connect(candidate.display) {
        Ok(connected) => connected,
        Err(ConnectFailure::Display) => {
            return Preflight {
                outcome: Outcome::Unknown(UnknownReason::DisplayUncheckable),
            };
        }
    };
    // Allocate once before verification. Moving the Box later never relocates the exact
    // RustConnection instance which the verifier sees here.
    let connection = Box::new(connection);
    let Some(connection_identity) = ConnectionIdentity::read(&connection, screen_index) else {
        return Preflight {
            outcome: Outcome::Unknown(UnknownReason::DisplayUncheckable),
        };
    };

    let claim = SessionClaim {
        process_id: candidate.process_id,
        display: candidate.display,
        screen_index,
        connection_identity,
    };
    let verified = match verifier.verify(&claim) {
        SessionVerification::Verified(verified) => verified,
        SessionVerification::Uncheckable => {
            return Preflight {
                outcome: Outcome::Unknown(UnknownReason::SessionIdentityUncheckable),
            };
        }
        SessionVerification::Mismatch => {
            return Preflight {
                outcome: Outcome::Unknown(UnknownReason::SessionMismatch),
            };
        }
    };

    // Even a verifier implementation cannot accidentally hand back a ticket minted for a
    // previous connection or another process. No reconnection occurs after this comparison.
    if verified.process_id != candidate.process_id
        || verified.display != candidate.display
        || verified.screen_index != screen_index
        || verified.connection != connection_identity
    {
        return Preflight {
            outcome: Outcome::Unknown(UnknownReason::SessionMismatch),
        };
    }

    Preflight {
        outcome: Outcome::Available(X11Ready {
            connection,
            screen_index,
            session: verified,
        }),
    }
}

/// Probe this process's Linux desktop without capturing a frame or writing any file.
/// Available means logind and the retained local X11 connection identify the same active session.
pub fn preflight() -> Preflight {
    preflight_with(
        &SessionEnvironment::current(),
        &mut RustConnector,
        &mut LogindVerifier,
    )
}

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub screen: CapabilityState,
    pub accessibility: CapabilityState,
    pub ocr: CapabilityState,
    pub ocr_languages: Option<Vec<String>>,
}

impl Capabilities {
    pub fn current(config: &Config) -> Self {
        let screen = preflight().state().capability();
        let accessibility = CapabilityState::from_measured(A11y::connect().is_ok());
        let ocr_languages = TesseractOcr::installed_languages().ok();
        let ocr = if config.capture.ocr {
            ocr_languages
                .as_ref()
                .map_or(CapabilityState::Unknown, |installed| {
                    CapabilityState::from_measured(
                        TesseractOcr::select_languages(&config.capture.ocr_languages, installed)
                            .is_some(),
                    )
                })
        } else {
            CapabilityState::Unavailable
        };
        Self {
            screen,
            accessibility,
            ocr,
            ocr_languages,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowIdentity {
    window: Window,
    pid: u32,
    title: String,
    app_id: Option<String>,
    app_name: Option<String>,
}

struct XAtoms {
    active_window: u32,
    pid: u32,
    net_wm_name: u32,
    utf8_string: u32,
}

impl XAtoms {
    fn new(connection: &RustConnection) -> Result<Self> {
        fn atom(connection: &RustConnection, name: &[u8]) -> Result<u32> {
            Ok(connection.intern_atom(false, name)?.reply()?.atom)
        }
        Ok(Self {
            active_window: atom(connection, b"_NET_ACTIVE_WINDOW")?,
            pid: atom(connection, b"_NET_WM_PID")?,
            net_wm_name: atom(connection, b"_NET_WM_NAME")?,
            utf8_string: atom(connection, b"UTF8_STRING")?,
        })
    }
}

fn property_u32(
    connection: &RustConnection,
    window: Window,
    property: u32,
    property_type: u32,
) -> Result<Option<u32>> {
    let reply = connection
        .get_property(false, window, property, property_type, 0, 1)?
        .reply()?;
    Ok(reply.value32().and_then(|mut values| values.next()))
}

fn property_text(
    connection: &RustConnection,
    window: Window,
    property: u32,
    property_type: u32,
) -> Result<Option<String>> {
    let reply = connection
        .get_property(false, window, property, property_type, 0, 4096)?
        .reply()?;
    if reply.format != 8 || reply.value.is_empty() {
        return Ok(None);
    }
    Ok(String::from_utf8(reply.value)
        .ok()
        .map(|value| value.trim_matches('\0').trim().to_owned())
        .filter(|value| !value.is_empty()))
}

fn foreground(
    connection: &RustConnection,
    screen_index: usize,
    atoms: &XAtoms,
) -> Result<WindowIdentity> {
    let root = connection
        .setup()
        .roots
        .get(screen_index)
        .ok_or_else(|| anyhow!("X11 screen 不存在"))?
        .root;
    let window = property_u32(
        connection,
        root,
        atoms.active_window,
        AtomEnum::WINDOW.into(),
    )?
    .filter(|window| *window != 0)
    .ok_or_else(|| anyhow!("X11 沒有前景視窗"))?;
    let pid = property_u32(connection, window, atoms.pid, AtomEnum::CARDINAL.into())?
        .filter(|pid| *pid != 0)
        .ok_or_else(|| anyhow!("X11 前景視窗沒有 PID"))?;
    let title = property_text(connection, window, atoms.net_wm_name, atoms.utf8_string)?
        .or_else(|| {
            property_text(
                connection,
                window,
                AtomEnum::WM_NAME.into(),
                AtomEnum::STRING.into(),
            )
            .ok()
            .flatten()
        })
        .unwrap_or_default();
    let wm_class = property_text(
        connection,
        window,
        AtomEnum::WM_CLASS.into(),
        AtomEnum::STRING.into(),
    )?;
    let app_name = wm_class.as_deref().and_then(|value| {
        value
            .split('\0')
            .rfind(|part| !part.trim().is_empty())
            .map(str::to_owned)
    });
    let app_id = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .or_else(|| app_name.clone());
    Ok(WindowIdentity {
        window,
        pid,
        title,
        app_id,
        app_name,
    })
}

struct A11y {
    connection: BusConnection,
}

#[derive(Debug)]
struct A11yObservation {
    sensitive: SensitiveFieldState,
    browser_url: BrowserUrlState,
}

impl A11y {
    fn connect() -> Result<Self> {
        let session = zbus::blocking::connection::Builder::session()?
            .method_timeout(std::time::Duration::from_millis(500))
            .build()
            .context("connect session bus")?;
        let bus = Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")?;
        let address: String = bus.call("GetAddress", &())?;
        anyhow::ensure!(
            address.starts_with("unix:") && !address.contains(';'),
            "AT-SPI bus 不是本機 Unix transport"
        );
        let address = address.parse::<zbus::Address>()?;
        let connection = zbus::blocking::connection::Builder::address(address)?
            .method_timeout(std::time::Duration::from_millis(250))
            .build()?;
        Ok(Self { connection })
    }

    fn accessible(&self, object: &ObjectRefOwned) -> Result<Proxy<'static>> {
        let destination = object
            .name_as_str()
            .ok_or_else(|| anyhow!("AT-SPI object 沒有 bus name"))?;
        Ok(Proxy::new_owned(
            self.connection.clone(),
            destination.to_owned(),
            object.path_as_str().to_owned(),
            "org.a11y.atspi.Accessible",
        )?)
    }

    fn application_for_pid(&self, pid: u32) -> Result<ObjectRefOwned> {
        let root = Proxy::new(
            &self.connection,
            "org.a11y.atspi.Registry",
            "/org/a11y/atspi/accessible/root",
            "org.a11y.atspi.Accessible",
        )?;
        let applications: Vec<ObjectRefOwned> = root.call("GetChildren", &())?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&self.connection)?;
        for application in applications {
            let Some(name) = application.name().cloned() else {
                continue;
            };
            let process_id = dbus
                .get_connection_unix_process_id(BusName::Unique(name))
                .ok();
            if process_id == Some(pid) {
                return Ok(application);
            }
        }
        Err(anyhow!("前景程式沒有 AT-SPI tree"))
    }

    fn observe(&self, pid: u32, browser: bool) -> Result<A11yObservation> {
        const MAX_OBJECTS: usize = 2_000;
        const MAX_DEPTH: usize = 32;
        let application = self.application_for_pid(pid)?;
        let mut stack = vec![(application, 0usize)];
        let mut visited = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut focused = None;
        let mut browser_url = None;
        while let Some((object, depth)) = stack.pop() {
            if visited >= MAX_OBJECTS || std::time::Instant::now() >= deadline {
                break;
            }
            visited += 1;
            let accessible = match self.accessible(&object) {
                Ok(accessible) => accessible,
                Err(_) => continue,
            };
            let role: Role = match accessible.call("GetRole", &()) {
                Ok(role) => role,
                Err(_) => continue,
            };
            let state: StateSet = match accessible.call("GetState", &()) {
                Ok(state) => state,
                Err(_) => continue,
            };
            let name: String = accessible.get_property("Name").unwrap_or_default();
            let description: String = accessible.get_property("Description").unwrap_or_default();
            let attributes: std::collections::HashMap<String, String> =
                accessible.call("GetAttributes", &()).unwrap_or_default();
            let marker = format!("{name} {description} {attributes:?}").to_ascii_lowercase();

            if state.contains(State::Focused) {
                let protected = role == Role::PasswordText
                    || ["password", "passcode", "protected"]
                        .iter()
                        .any(|needle| marker.contains(needle));
                if protected {
                    focused = Some(SensitiveFieldState::Focused);
                } else if focused.is_none() {
                    focused = Some(SensitiveFieldState::Clear);
                }
            }

            if browser
                && browser_url.is_none()
                && matches!(role, Role::Entry | Role::Editbar)
                && [
                    "address",
                    "location",
                    "omnibox",
                    "urlbar",
                    "search or enter",
                    "search or type",
                ]
                .iter()
                .any(|needle| marker.contains(needle))
            {
                let text = Proxy::new(
                    &self.connection,
                    object.name_as_str().unwrap_or_default(),
                    object.path_as_str(),
                    "org.a11y.atspi.Text",
                )
                .ok()
                .and_then(|proxy| proxy.call::<_, _, String>("GetText", &(0i32, -1i32)).ok())
                .filter(|value| plausible_browser_url(value));
                browser_url = text;
            }

            if depth < MAX_DEPTH {
                let children: Vec<ObjectRefOwned> =
                    accessible.call("GetChildren", &()).unwrap_or_default();
                stack.extend(
                    children
                        .into_iter()
                        .filter(|child| !child.is_null())
                        .map(|child| (child, depth + 1)),
                );
            }
        }
        let sensitive = focused.unwrap_or(SensitiveFieldState::Unknown);
        let browser_url = if browser {
            browser_url.map_or(BrowserUrlState::Unknown, BrowserUrlState::Known)
        } else {
            BrowserUrlState::NotApplicable
        };
        Ok(A11yObservation {
            sensitive,
            browser_url,
        })
    }
}

fn plausible_browser_url(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    [
        "http://",
        "https://",
        "file://",
        "about:",
        "chrome://",
        "edge://",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
        || (value.contains('.') && !value.starts_with('.'))
}

enum TesseractOcr {
    Disabled,
    Enabled { languages: String },
}

impl TesseractOcr {
    fn installed_languages() -> Result<Vec<String>> {
        let output = Command::new(TESSERACT)
            .arg("--list-langs")
            .stdin(Stdio::null())
            .output()
            .context("找不到本機 Tesseract OCR")?;
        anyhow::ensure!(output.status.success(), "讀不到 Tesseract OCR 語言");
        let stdout = String::from_utf8(output.stdout).context("Tesseract 語言輸出不是 UTF-8")?;
        Ok(stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("List of available"))
            .map(str::to_owned)
            .collect())
    }

    fn language_code(language: &str) -> Option<&'static str> {
        let language = language.trim().to_ascii_lowercase();
        if language.starts_with("zh-hant") || language == "zh-tw" {
            Some("chi_tra")
        } else if language.starts_with("zh-hans") || language == "zh-cn" {
            Some("chi_sim")
        } else if language.starts_with("en") {
            Some("eng")
        } else if language.starts_with("ja") {
            Some("jpn")
        } else if language.starts_with("ko") {
            Some("kor")
        } else {
            None
        }
    }

    fn select_languages(preferred: &[String], installed: &[String]) -> Option<String> {
        let installed: std::collections::BTreeSet<&str> =
            installed.iter().map(String::as_str).collect();
        let selected: Vec<_> = preferred
            .iter()
            .filter_map(|language| Self::language_code(language))
            .filter(|code| installed.contains(code))
            .collect();
        (!selected.is_empty()).then(|| selected.join("+"))
    }

    fn new(config: &Config) -> Result<Self> {
        if !config.capture.ocr {
            return Ok(Self::Disabled);
        }
        let installed = Self::installed_languages()?;
        let languages = Self::select_languages(&config.capture.ocr_languages, &installed)
            .ok_or_else(|| anyhow!("Tesseract 缺少設定中的 OCR 語言"))?;
        Ok(Self::Enabled { languages })
    }

    fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>> {
        let Self::Enabled { languages } = self else {
            return Ok(Vec::new());
        };
        let rgba = frame
            .rgba
            .as_deref()
            .ok_or_else(|| anyhow!("OCR 畫面沒有像素"))?;
        let image = image::RgbaImage::from_raw(frame.width, frame.height, rgba.to_vec())
            .ok_or_else(|| anyhow!("OCR RGBA 尺寸不符"))?;
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut png, image::ImageFormat::Png)?;
        let mut child = Command::new(TESSERACT)
            .args(["stdin", "stdout", "-l", languages, "--psm", "11", "tsv"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("開不起本機 Tesseract OCR")?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Tesseract stdout 沒有開啟"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Tesseract stderr 沒有開啟"))?;
        let stdout_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Tesseract stdin 沒有開啟"))?;
        let png = png.into_inner();
        let input_writer = std::thread::spawn(move || input.write_all(&png));
        use wait_timeout::ChildExt as _;
        let status = match child.wait_timeout(std::time::Duration::from_secs(30)) {
            Ok(Some(status)) => status,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = input_writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(anyhow!("Tesseract OCR 超過 30 秒"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = input_writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error).context("讀不到 Tesseract OCR 行程狀態");
            }
        };
        input_writer
            .join()
            .map_err(|_| anyhow!("Tesseract stdin writer 中止"))??;
        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow!("Tesseract stdout reader 中止"))??;
        let _stderr = stderr_reader
            .join()
            .map_err(|_| anyhow!("Tesseract stderr reader 中止"))??;
        anyhow::ensure!(status.success(), "Tesseract OCR 沒有完成");
        let tsv = String::from_utf8(stdout).context("Tesseract OCR 輸出不是 UTF-8")?;
        Ok(parse_tesseract_tsv(&tsv))
    }
}

fn parse_tesseract_tsv(tsv: &str) -> Vec<OcrBlock> {
    tsv.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.splitn(12, '\t').collect();
            if fields.len() != 12 {
                return None;
            }
            let text = fields[11].trim();
            let confidence: f32 = fields[10].parse().ok()?;
            if text.is_empty() || confidence < 0.0 {
                return None;
            }
            Some(OcrBlock {
                text: text.to_owned(),
                x: fields[6].parse().ok()?,
                y: fields[7].parse().ok()?,
                w: fields[8].parse().ok()?,
                h: fields[9].parse().ok()?,
                confidence: confidence / 100.0,
            })
        })
        .collect()
}

struct LinuxBackend {
    connection: Box<RustConnection>,
    screen_index: usize,
    session: VerifiedExactSession,
    logind: BusConnection,
    atoms: XAtoms,
    a11y: A11y,
    ocr: RefCell<TesseractOcr>,
    approved: Option<(CapturePermit, WindowIdentity, PrivacyContext)>,
    generation: u64,
    last_locked: Option<bool>,
    transition_sequence: u64,
}

impl LinuxBackend {
    fn from_ready(ready: X11Ready, config: &Config) -> Result<Self> {
        let atoms = XAtoms::new(&ready.connection)?;
        Ok(Self {
            connection: ready.connection,
            screen_index: ready.screen_index,
            session: ready.session,
            logind: logind_connection()?,
            atoms,
            a11y: A11y::connect()?,
            ocr: RefCell::new(TesseractOcr::new(config)?),
            approved: None,
            generation: 0,
            last_locked: None,
            transition_sequence: 0,
        })
    }

    fn current_identity(&self) -> Result<WindowIdentity> {
        foreground(&self.connection, self.screen_index, &self.atoms)
    }

    fn observe_current(&self) -> Option<(WindowIdentity, PrivacyContext)> {
        let before = self.current_identity().ok()?;
        let app_key = before
            .app_id
            .as_deref()
            .or(before.app_name.as_deref())
            .unwrap_or("")
            .to_ascii_lowercase();
        let observation = self
            .a11y
            .observe(before.pid, crate::browsers::is_browser(&app_key))
            .ok()?;
        if observation.sensitive == SensitiveFieldState::Unknown {
            return None;
        }
        let after = self.current_identity().ok()?;
        if after != before {
            return None;
        }
        let context = PrivacyContext::known(
            FocusSnapshot {
                app_id: after.app_id.clone(),
                app_name: after.app_name.clone(),
                window_title: (!after.title.is_empty()).then(|| after.title.clone()),
                url: None,
                pid: Some(i64::from(after.pid)),
            },
            observation.sensitive,
            observation.browser_url,
        );
        Some((after, context))
    }

    fn permit_is_current(&self, permit: CapturePermit) -> bool {
        let Some((approved, identity, context)) = self.approved.as_ref() else {
            return false;
        };
        *approved == permit
            && self
                .observe_current()
                .is_some_and(|(now_identity, now_context)| {
                    now_identity == *identity && now_context == *context
                })
    }

    fn capture(&self, ts: Millis) -> Result<RawFrame> {
        capture_x11(&self.connection, self.screen_index, ts)
    }
}

fn capture_x11(connection: &RustConnection, screen_index: usize, ts: Millis) -> Result<RawFrame> {
    let screen = connection
        .setup()
        .roots
        .get(screen_index)
        .ok_or_else(|| anyhow!("X11 screen 不存在"))?;
    let (image, visual_id) = Image::get(
        connection,
        screen.root,
        0,
        0,
        screen.width_in_pixels,
        screen.height_in_pixels,
    )?;
    let visual = connection
        .setup()
        .roots
        .get(screen_index)
        .and_then(|root| {
            root.allowed_depths
                .iter()
                .flat_map(|depth| depth.visuals.iter())
                .find(|visual| visual.visual_id == visual_id)
        })
        .copied()
        .ok_or_else(|| anyhow!("X11 畫面 visual 不存在"))?;
    let layout = PixelLayout::from_visual_type(visual)?;
    let mut rgba = Vec::with_capacity(usize::from(image.width()) * usize::from(image.height()) * 4);
    for y in 0..image.height() {
        for x in 0..image.width() {
            let (red, green, blue) = layout.decode(image.get_pixel(x, y));
            rgba.extend_from_slice(&[(red >> 8) as u8, (green >> 8) as u8, (blue >> 8) as u8, 255]);
        }
    }
    Ok(RawFrame::from_rgba(
        ts,
        i32::try_from(screen_index).unwrap_or(i32::MAX),
        u32::from(image.width()),
        u32::from(image.height()),
        rgba,
    ))
}

impl Backend for LinuxBackend {
    fn name(&self) -> &str {
        "linux-x11-logind-atspi-tesseract-v1"
    }

    fn poll_system(&mut self, ts: Millis) -> Result<SystemObservation> {
        let facts = match logind_session_with(&self.logind, self.session.process_id) {
            Ok(facts) => facts,
            Err(_) => {
                self.approved = None;
                return Ok(SystemObservation::Unknown);
            }
        };
        if !facts.active
            || facts.remote
            || facts.session_type != "x11"
            || !same_local_x11_display(&facts.display, &self.session.display)
            || facts.id != self.session.system_session_id
        {
            self.approved = None;
            return Ok(SystemObservation::Unknown);
        }
        let mut transitions = Vec::new();
        if let Some(previous) = self.last_locked {
            if previous != facts.locked {
                self.transition_sequence = self.transition_sequence.saturating_add(1);
                transitions.push(SystemTransition {
                    sequence: self.transition_sequence,
                    ts,
                    kind: if facts.locked {
                        SystemTransitionKind::Lock
                    } else {
                        SystemTransitionKind::Unlock
                    },
                });
            }
        }
        self.last_locked = Some(facts.locked);
        Ok(SystemObservation::Known {
            state: if facts.locked {
                SystemContentState::locked_awake()
            } else {
                SystemContentState::active()
            },
            transitions,
        })
    }

    fn grab_screen(&mut self, ts: Millis, permit: CapturePermit) -> Result<ScreenCapture> {
        if !self.permit_is_current(permit) {
            return Ok(ScreenCapture::PrivacyChanged);
        }
        let frame = self.capture(ts)?;
        if !self.permit_is_current(permit) {
            return Ok(ScreenCapture::PrivacyChanged);
        }
        Ok(ScreenCapture::Frame(frame))
    }

    fn privacy_context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
        let Some((identity, context)) = self.observe_current() else {
            self.approved = None;
            return Ok(PrivacyObservation::Unknown);
        };
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("Linux capture permit generation exhausted"))?;
        let permit = CapturePermit::backend_local(self.generation);
        self.approved = Some((permit, identity, context.clone()));
        Ok(PrivacyObservation::known(context, permit))
    }

    fn capture_permit_is_current(&mut self, permit: CapturePermit) -> Result<bool> {
        Ok(self.permit_is_current(permit))
    }

    fn poll_clipboard(&mut self, _ts: Millis, permit: CapturePermit) -> Result<ClipboardCapture> {
        Ok(if self.permit_is_current(permit) {
            ClipboardCapture::Event(None)
        } else {
            ClipboardCapture::ContextChanged
        })
    }

    fn skip_clipboard(&mut self, _ts: Millis) -> Result<ClipboardWatermark> {
        Ok(ClipboardWatermark::Unknown)
    }

    fn drain_input(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
        Ok(None)
    }

    fn suspend_input(&mut self, _ts: Millis) -> Result<()> {
        Ok(())
    }

    fn resume_input(&mut self, _ts: Millis) -> Result<()> {
        Ok(())
    }

    fn recognize(&mut self, frame: &RawFrame) -> OcrAttempt {
        OcrAttempt::full(frame, || self.ocr.borrow_mut().recognize(frame))
    }

    fn recheck_ocr_dhash_duplicate(&mut self, _frame: &RawFrame) -> DhashRecheck {
        DhashRecheck::Duplicate {
            gate_elapsed: None,
            work: None,
            rejected_regions: None,
            error: None,
        }
    }
}

fn backend(config: &Config) -> Result<LinuxBackend> {
    let preflight = preflight();
    let ready = match preflight.outcome {
        Outcome::Available(ready) => ready,
        Outcome::Unsupported(UnsupportedReason::Wayland) => {
            return Err(anyhow!("Linux 版目前支援 X11；這個 session 是 Wayland"));
        }
        Outcome::Unsupported(UnsupportedReason::Headless) => {
            return Err(anyhow!("這個 Linux session 沒有桌面畫面"));
        }
        Outcome::Unknown(_) => return Err(anyhow!("讀不到目前的本機 X11 session")),
    };
    LinuxBackend::from_ready(ready, config)
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
    use std::io::{BufRead, BufReader};
    use std::os::unix::ffi::OsStringExt;
    use std::process::{Child, Command, Stdio};

    fn environment(
        session_type: Option<OsString>,
        display: Option<OsString>,
        wayland_display: Option<OsString>,
    ) -> SessionEnvironment {
        SessionEnvironment {
            process_id: std::process::id(),
            session_type: EnvironmentValue::from_os(session_type),
            display: EnvironmentValue::from_os(display),
            wayland_display: EnvironmentValue::from_os(wayland_display),
        }
    }

    fn text(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    struct MustNotConnect;

    impl Connector for MustNotConnect {
        fn connect(&mut self, _: &str) -> Result<(RustConnection, usize), ConnectFailure> {
            panic!("non-X11 environment reached the X server")
        }
    }

    struct MustNotVerify;

    impl ExactProcessSessionVerifier for MustNotVerify {
        fn verify(&mut self, _: &SessionClaim<'_>) -> SessionVerification {
            panic!("unusable environment reached the session verifier")
        }
    }

    #[test]
    fn explicit_wayland_is_unsupported_even_with_xwayland_display() {
        let result = preflight_with(
            &environment(text("wayland"), text(":0"), text("wayland-0")),
            &mut MustNotConnect,
            &mut MustNotVerify,
        );
        assert_eq!(
            result.state(),
            PreflightState::Unsupported(UnsupportedReason::Wayland)
        );
        assert_eq!(result.state().capability(), CapabilityState::Unavailable);
    }

    #[test]
    fn a_wayland_endpoint_without_session_metadata_is_still_unsupported() {
        let result = preflight_with(
            &environment(None, None, text("wayland-0")),
            &mut MustNotConnect,
            &mut MustNotVerify,
        );
        assert_eq!(
            result.state(),
            PreflightState::Unsupported(UnsupportedReason::Wayland)
        );
    }

    #[test]
    fn no_desktop_endpoints_is_measured_headless_not_unknown() {
        let result = preflight_with(
            &environment(None, None, None),
            &mut MustNotConnect,
            &mut MustNotVerify,
        );
        assert_eq!(
            result.state(),
            PreflightState::Unsupported(UnsupportedReason::Headless)
        );
        assert_eq!(result.state().capability(), CapabilityState::Unavailable);
    }

    #[test]
    fn contradictory_x11_environment_is_unknown_and_never_connects() {
        for case in [
            environment(text("x11"), None, None),
            environment(text("x11"), text(":0"), text("wayland-0")),
            environment(text("tty"), text(":0"), None),
        ] {
            let result = preflight_with(&case, &mut MustNotConnect, &mut MustNotVerify);
            assert_eq!(
                result.state(),
                PreflightState::Unknown(UnknownReason::ContradictorySession)
            );
            assert_eq!(result.state().capability(), CapabilityState::Unknown);
        }
    }

    #[test]
    fn display_without_exact_session_identity_is_unknown_and_never_connects() {
        for display in [text(":0"), Some(OsString::from_vec(vec![b':', 0xff]))] {
            let result = preflight_with(
                &environment(None, display, None),
                &mut MustNotConnect,
                &mut MustNotVerify,
            );
            assert_eq!(
                result.state(),
                PreflightState::Unknown(UnknownReason::SessionIdentityUncheckable)
            );
        }
    }

    struct BrokenDisplay;

    impl Connector for BrokenDisplay {
        fn connect(&mut self, _: &str) -> Result<(RustConnection, usize), ConnectFailure> {
            Err(ConnectFailure::Display)
        }
    }

    #[test]
    fn bad_or_unreachable_display_is_unknown_not_unsupported() {
        let malformed = preflight_with(
            &environment(text("x11"), text("not a display"), None),
            &mut RustConnector,
            &mut MustNotVerify,
        );
        assert_eq!(
            malformed.state(),
            PreflightState::Unknown(UnknownReason::DisplayUncheckable)
        );

        // This must be rejected by parsing/transport policy, without making a TCP request.
        let remote = preflight_with(
            &environment(text("x11"), text("example.com:0"), None),
            &mut RustConnector,
            &mut MustNotVerify,
        );
        assert_eq!(
            remote.state(),
            PreflightState::Unknown(UnknownReason::DisplayUncheckable)
        );

        let unreachable = preflight_with(
            &environment(text("x11"), text(":65534"), None),
            &mut BrokenDisplay,
            &mut MustNotVerify,
        );
        assert_eq!(
            unreachable.state(),
            PreflightState::Unknown(UnknownReason::DisplayUncheckable)
        );

        let non_utf8 = preflight_with(
            &environment(
                text("x11"),
                Some(OsString::from_vec(vec![b':', 0xff])),
                None,
            ),
            &mut MustNotConnect,
            &mut MustNotVerify,
        );
        assert_eq!(
            non_utf8.state(),
            PreflightState::Unknown(UnknownReason::DisplayUncheckable)
        );
    }

    #[test]
    fn logind_display_matches_the_same_local_x11_server_across_screen_spelling() {
        assert!(same_local_x11_display(":0", ":0.0"));
        assert!(same_local_x11_display("unix/:7", "unix/:7.1"));
        assert!(!same_local_x11_display(":0", ":1"));
        assert!(!same_local_x11_display("example.com:0", ":0"));
        assert!(!same_local_x11_display("not a display", ":0"));
    }

    #[test]
    fn non_utf8_session_identity_is_unknown_not_headless() {
        let result = preflight_with(
            &environment(Some(OsString::from_vec(vec![b'x', 0xff])), None, None),
            &mut MustNotConnect,
            &mut MustNotVerify,
        );
        assert_eq!(
            result.state(),
            PreflightState::Unknown(UnknownReason::SessionIdentityUncheckable)
        );
    }

    struct Xvfb {
        child: Child,
        display: String,
    }

    impl Xvfb {
        fn start_if_available() -> Option<Self> {
            Self::start_with_geometry("64x64x24")
        }

        fn start_with_geometry(geometry: &str) -> Option<Self> {
            let mut child = match Command::new("Xvfb")
                .args([
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    geometry,
                    "-nolisten",
                    "tcp",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
                Err(error) => panic!("start Xvfb: {error}"),
            };
            let stdout = child.stdout.take().expect("piped Xvfb stdout");
            let mut line = String::new();
            BufReader::new(stdout)
                .read_line(&mut line)
                .expect("read Xvfb display number");
            let number: u32 = line.trim().parse().expect("Xvfb display number");
            Some(Self {
                child,
                display: format!(":{number}"),
            })
        }
    }

    impl Drop for Xvfb {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct CountingConnector {
        calls: usize,
    }

    impl Connector for CountingConnector {
        fn connect(&mut self, display: &str) -> Result<(RustConnection, usize), ConnectFailure> {
            self.calls += 1;
            RustConnector.connect(display)
        }
    }

    #[derive(Clone, Copy)]
    enum VerifierAnswer {
        Exact,
        Uncheckable,
        Mismatch,
        StaleTicket,
    }

    struct InspectingVerifier {
        answer: VerifierAnswer,
        calls: usize,
        saw_process: Option<u32>,
        saw_connection: Option<ConnectionIdentity>,
    }

    impl InspectingVerifier {
        fn new(answer: VerifierAnswer) -> Self {
            Self {
                answer,
                calls: 0,
                saw_process: None,
                saw_connection: None,
            }
        }
    }

    impl ExactProcessSessionVerifier for InspectingVerifier {
        fn verify(&mut self, claim: &SessionClaim<'_>) -> SessionVerification {
            self.calls += 1;
            self.saw_process = Some(claim.process_id);
            self.saw_connection = Some(claim.connection_identity);
            match self.answer {
                VerifierAnswer::Exact => SessionVerification::Verified(claim.verified()),
                VerifierAnswer::Uncheckable => SessionVerification::Uncheckable,
                VerifierAnswer::Mismatch => SessionVerification::Mismatch,
                VerifierAnswer::StaleTicket => {
                    let mut ticket = claim.verified();
                    ticket.connection.resource_id_base ^= 1;
                    SessionVerification::Verified(ticket)
                }
            }
        }
    }

    fn with_xvfb(answer: VerifierAnswer) -> Option<(Xvfb, Preflight, usize, InspectingVerifier)> {
        let xvfb = Xvfb::start_if_available()?;
        let environment = environment(text("x11"), text(&xvfb.display), None);
        let mut connector = CountingConnector { calls: 0 };
        let mut verifier = InspectingVerifier::new(answer);
        let result = preflight_with(&environment, &mut connector, &mut verifier);
        let calls = connector.calls;
        // Return the server guard with the preflight so the retained RustConnection is still
        // live—not merely carrying a cached Setup—while each assertion runs.
        Some((xvfb, result, calls, verifier))
    }

    #[test]
    fn session_verifier_non_available_answers_remain_unknown() {
        let Some((_xvfb, uncheckable, calls, verifier)) = with_xvfb(VerifierAnswer::Uncheckable)
        else {
            return;
        };
        assert_eq!(calls, 1);
        assert_eq!(verifier.calls, 1);
        assert_eq!(verifier.saw_process, Some(std::process::id()));
        assert_eq!(
            uncheckable.state(),
            PreflightState::Unknown(UnknownReason::SessionIdentityUncheckable)
        );

        let Some((_xvfb, mismatch, calls, verifier)) = with_xvfb(VerifierAnswer::Mismatch) else {
            return;
        };
        assert_eq!(calls, 1);
        assert_eq!(verifier.calls, 1);
        assert_eq!(
            mismatch.state(),
            PreflightState::Unknown(UnknownReason::SessionMismatch)
        );
    }

    #[test]
    fn a_stale_verified_ticket_cannot_authorize_a_different_connection() {
        let Some((_xvfb, result, calls, verifier)) = with_xvfb(VerifierAnswer::StaleTicket) else {
            return;
        };
        assert_eq!(calls, 1);
        assert_eq!(verifier.calls, 1);
        assert_eq!(
            result.state(),
            PreflightState::Unknown(UnknownReason::SessionMismatch)
        );
    }

    #[test]
    fn xvfb_only_proves_one_transport_is_retained_after_exact_verification() {
        let Some((_xvfb, result, calls, verifier)) = with_xvfb(VerifierAnswer::Exact) else {
            return;
        };
        assert_eq!(calls, 1, "preflight must not reconnect after verification");
        assert_eq!(verifier.calls, 1);

        let Outcome::Available(ready) = &result.outcome else {
            panic!("exact verifier did not produce an available transport")
        };
        let retained = ConnectionIdentity::read(&ready.connection, ready.screen_index)
            .expect("retained connection identity");
        assert_eq!(Some(retained), verifier.saw_connection);
        assert_eq!(ready.session.connection, retained);
        assert_eq!(result.state(), PreflightState::Available);
        assert_eq!(result.state().capability(), CapabilityState::Available);
    }

    #[test]
    fn retained_x11_connection_returns_exact_rgba_frame() {
        let Some((_xvfb, result, calls, _verifier)) = with_xvfb(VerifierAnswer::Exact) else {
            return;
        };
        assert_eq!(calls, 1);
        let Outcome::Available(ready) = &result.outcome else {
            panic!("exact verifier did not produce an available transport")
        };
        let frame = capture_x11(&ready.connection, ready.screen_index, 123).expect("X11 frame");
        assert_eq!((frame.width, frame.height), (64, 64));
        assert_eq!(frame.ts, 123);
        assert_eq!(frame.rgba.as_ref().map(Vec::len), Some(64 * 64 * 4));
        assert!(
            frame
                .rgba
                .as_ref()
                .is_some_and(|pixels| { pixels.chunks_exact(4).all(|pixel| pixel[3] == 255) })
        );
    }

    #[test]
    fn x11_frame_runs_through_local_tesseract() {
        let required = matches!(
            std::env::var("AI_SISTER_REQUIRE_LINUX_OCR_SMOKE"),
            Ok(value) if value == "1"
        );
        let installed = match TesseractOcr::installed_languages() {
            Ok(installed) if installed.iter().any(|language| language == "eng") => installed,
            Ok(_) if required => panic!("Linux OCR smoke requires the eng Tesseract language"),
            Err(error) if required => panic!("Linux OCR smoke requires Tesseract: {error:#}"),
            _ => return,
        };
        let Some(xvfb) = Xvfb::start_with_geometry("800x300x24") else {
            assert!(!required, "Linux OCR smoke requires Xvfb");
            return;
        };
        let environment = environment(text("x11"), text(&xvfb.display), None);
        let mut connector = RustConnector;
        let mut verifier = InspectingVerifier::new(VerifierAnswer::Exact);
        let result = preflight_with(&environment, &mut connector, &mut verifier);
        let mut message = match Command::new("xmessage")
            .env("DISPLAY", &xvfb.display)
            .args([
                "-center",
                "-geometry",
                "700x220",
                "-fn",
                "12x24",
                "-fg",
                "black",
                "-bg",
                "white",
                "-buttons",
                "",
                "AI SISTER 7429",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && required => {
                panic!("Linux OCR smoke requires xmessage")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("start xmessage: {error}"),
        };
        let attempt = (|| -> Result<String> {
            anyhow::ensure!(
                message.try_wait()?.is_none(),
                "xmessage exited before capture"
            );
            let Outcome::Available(ready) = &result.outcome else {
                return Err(anyhow!(
                    "exact verifier did not produce an available transport"
                ));
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let frame = loop {
                let frame = capture_x11(&ready.connection, ready.screen_index, 123)?;
                let rgba = frame
                    .rgba
                    .as_deref()
                    .ok_or_else(|| anyhow!("captured X11 frame has no pixels"))?;
                let first = rgba.get(..4).unwrap_or(&[]);
                let changed_pixels = rgba.chunks_exact(4).filter(|pixel| *pixel != first).count();
                if changed_pixels > 1_000 {
                    break frame;
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "captured X11 frame has only {changed_pixels} pixels distinct from its first pixel"
                );
                anyhow::ensure!(
                    message.try_wait()?.is_none(),
                    "xmessage exited before its window was painted"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            let languages = TesseractOcr::select_languages(&["en-US".to_owned()], &installed)
                .ok_or_else(|| anyhow!("installed eng language disappeared"))?;
            let blocks = TesseractOcr::Enabled { languages }.recognize(&frame)?;
            Ok(blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_uppercase())
        })();
        let _ = message.kill();
        let _ = message.wait();
        let text = attempt.expect("X11 capture through Tesseract");
        assert!(text.contains("7429"), "OCR text was {text:?}");
    }

    #[test]
    fn tesseract_tsv_keeps_measured_coordinates_and_confidence() {
        let blocks = parse_tesseract_tsv(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n5\t1\t1\t1\t1\t1\t12\t34\t56\t18\t96.5\t姊妹\n",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "姊妹");
        assert_eq!(
            (blocks[0].x, blocks[0].y, blocks[0].w, blocks[0].h),
            (12, 34, 56, 18)
        );
        assert!((blocks[0].confidence - 0.965).abs() < f32::EPSILON * 4.0);
    }
}
