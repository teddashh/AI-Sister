//! Linux X11 backend 的可信啟動邊界。
//!
//! `DISPLAY` 能連上只證明有一個 X server 願意跟這個 process 說話；它不證明
//! 那是目前登入者的本機 system session，也不證明 session 沒鎖住。這裡因此把
//! transport 與 exact-process session 驗證綁在同一次 preflight：先建立一次
//! [`RustConnection`]，把**同一條連線**交給 verifier 看，再把那條連線本身搬進
//! private、不可 Clone 的 [`X11Ready`]。不會在核准後重連，避免檢查 A、使用 B。
//!
//! 現在 production verifier 還沒有 logind source，所以真 X11 會誠實停在
//! [`PreflightState::Unknown`]。這個 foundation 不是 Linux Developer Preview：
//! 尚未有 capture／focus／clipboard／input／OCR 的完整 composition。

use sister_core::capabilities::CapabilityState;
#[cfg(test)]
use std::ffi::OsString;
use x11rb::connection::Connection;
use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, parse_display};
use x11rb::rust_connection::RustConnection;

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
    /// 尚未能用目前 process ID 對到可信的本機 system session。
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
}

struct SessionClaim<'a> {
    process_id: u32,
    display: &'a str,
    screen_index: usize,
    connection: &'a RustConnection,
    // Used by the logind verifier ticket once that production source lands. Until then the
    // field is exercised by the injected verifier tests below.
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

/// 必須用 `process_id` 查 system authority（未來是 logind），不能相信 renderer、
/// `XDG_SESSION_ID` 或一段外部傳入的 session 名稱。trait 留在模組內，只有 production
/// source 與定點測試能核發 ticket。
trait ExactProcessSessionVerifier {
    fn verify(&mut self, claim: &SessionClaim<'_>) -> SessionVerification;
}

/// logind source 尚未落地前，production 不會把 X socket 可連誤報成 Available。
struct PendingLogindVerifier;

impl ExactProcessSessionVerifier for PendingLogindVerifier {
    fn verify(&mut self, claim: &SessionClaim<'_>) -> SessionVerification {
        let _ = (
            claim.process_id,
            claim.display,
            claim.screen_index,
            claim.connection.setup().protocol_major_version,
        );
        SessionVerification::Uncheckable
    }
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
        connection: &connection,
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

/// Probe this process's Linux desktop without opening any content source or writing any file.
///
/// Until the exact-process logind verifier is implemented, an otherwise valid X11 connection
/// intentionally reports Unknown. Callers must fail closed and must not describe that state as
/// Linux Developer Preview support.
pub fn preflight() -> Preflight {
    preflight_with(
        &SessionEnvironment::current(),
        &mut RustConnector,
        &mut PendingLogindVerifier,
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
            let mut child = match Command::new("Xvfb")
                .args([
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    "64x64x24",
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
            self.saw_connection = ConnectionIdentity::read(claim.connection, claim.screen_index);
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
}
