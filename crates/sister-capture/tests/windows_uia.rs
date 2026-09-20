//! Native UIA against owned WPF and Edge providers. Run alone: this
//! fixture deliberately owns the foreground. It does not inspect a user's apps.
#![cfg(windows)]

use sister_capture::{
    MasterStopSource, Recorder, Tick,
    ocr_regions::ChangedRegionOcr,
    traits::*,
    windows::{focus::WindowsFocus, ocr::WindowsOcr, screen::WindowsScreen},
};
use sister_core::model::{AssistiveBlock, PrivacyContext, SensitiveFieldState};
use sister_core::{config::Config, db::Db, grounded_answer, retrieval::RetrievalProfile};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
static FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    child: Child,
    dir: PathBuf,
}
impl Fixture {
    fn start(script: &str) -> Self {
        Self::start_with_args(script, &[])
    }
    fn start_with_args(script: &str, args: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "sister-native-uia-{}-{}-{script}",
            std::process::id(),
            FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).expect("fresh fixture directory");
        let child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                if script == "uia-visible-text.ps1" {
                    "-STA"
                } else {
                    "-MTA"
                },
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(script),
            )
            .arg("-StateDir")
            .arg(&dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start owned UIA provider");
        Self { child, dir }
    }
    fn show(&mut self, mode: &str) {
        std::fs::write(self.dir.join("request"), mode).unwrap();
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Ok(error) = std::fs::read_to_string(self.dir.join("error")) {
                panic!("UIA fixture: {error}");
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "UIA fixture exited before {mode}"
            );
            if std::fs::read_to_string(self.dir.join("ready"))
                .ok()
                .as_deref()
                == Some(mode)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "UIA fixture did not focus its {mode} control: {} / {}",
                std::fs::read_to_string(self.dir.join("stage")).unwrap_or_default(),
                std::fs::read_to_string(self.dir.join("metadata")).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn observe(&self, focus: &mut WindowsFocus, expected: SensitiveFieldState) -> CapturePermit {
        let deadline = Instant::now() + Duration::from_secs(5);
        let owned_pid = std::fs::read_to_string(self.dir.join("provider-pid"))
            .ok()
            .map(|pid| pid.parse::<i64>().expect("owned provider PID"))
            .unwrap_or(i64::from(self.child.id()));
        loop {
            assert!(
                Instant::now() < deadline,
                "owned UIA provider did not observe {expected:?}"
            );
            if let PrivacyObservation::Known {
                context:
                    PrivacyContext::Known {
                        focus: snapshot,
                        sensitive_field,
                        browser_url,
                        ..
                    },
                permit,
            } = focus.context(0).unwrap()
            {
                if snapshot.pid != Some(owned_pid) {
                    // Startup/teardown can briefly leave another window in front.
                    // Never issue a content read until our own PID is observed.
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }
                if sensitive_field == expected {
                    if self.dir.join("provider-pid").exists() {
                        let name = std::fs::read_to_string(self.dir.join("document-name")).unwrap();
                        assert!(
                            matches!(browser_url,
                            sister_core::model::BrowserUrlState::Known(ref url) if url.ends_with(&name)),
                            "owned browser URL must be known: {browser_url:?}"
                        );
                    }
                    return permit;
                }
            }
            assert!(
                Instant::now() < deadline,
                "native UIA did not observe {expected:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::write(self.dir.join("request"), "stop");
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if self.child.try_wait().ok().flatten().is_none()
            && let Ok(pid) = std::fs::read_to_string(self.dir.join("provider-pid"))
            && let Ok(pid) = pid.parse::<u32>()
        {
            // A stalled native UIA call must not leave this fixture's isolated
            // browser alive after its broker is killed. Never target by exe name.
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn text(blocks: &[AssistiveBlock], role: &str) -> String {
    assert!(
        blocks
            .iter()
            .all(|block| block.role == role && block.bbox.is_none())
    );
    blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore = "owns the Windows foreground; CI runs this target in a separate process"]
fn native_uia_reads_visible_edits_and_documents_and_rejects_excluded_text() {
    let mut fixture = Fixture::start("uia-visible-text.ps1");
    fixture.show("edit");
    let mut focus = WindowsFocus::new();
    let permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    let initial = text(&focus.assistive_text(permit), "edit");
    assert!(
        initial.contains("電話 0800-123-456"),
        "native UIA did not read the visible Chinese phone line"
    );
    assert!(initial.contains("visible@example.test"));
    assert!(
        !initial.contains("OFFSCREEN-SENTINEL"),
        "scrolled-out text leaked into the captured frame"
    );

    // Same HWND, title and control; values must be read again instead of cached.
    fixture.show("changed");
    let changed = text(&focus.assistive_text(permit), "edit");
    assert!(changed.contains("CHANGED-SENTINEL 02-2233-4455"));
    assert!(!changed.contains("0800-123-456"));

    fixture.show("document");
    let document_permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    let document = text(&focus.assistive_text(document_permit), "document");
    assert!(document.contains("文件 0800-222-333"));
    assert!(document.contains("DOCUMENT-SECOND-PARAGRAPH"));
    assert!(!document.contains("DOCUMENT-BOTTOM"));

    let mut recorder = native_recorder(fixture.dir.clone());
    let reads_before_pause = (
        recorder.timings().grab.calls,
        recorder.timings().ocr.calls,
        recorder.timings().assistive.calls,
    );
    assert!(recorder.set_paused(true, 2000).unwrap());

    assert_eq!(recorder.tick(3000).unwrap(), Tick::Paused);
    assert_eq!(
        (
            recorder.timings().grab.calls,
            recorder.timings().ocr.calls,
            recorder.timings().assistive.calls
        ),
        reads_before_pause
    );
    assert!(recorder.set_paused(false, 4000).unwrap());
    let first_frame = retained(recorder.tick(5000).unwrap());
    assert_native_screenshot(
        &mut recorder,
        &fixture.dir,
        first_frame,
        "0800-222-333",
        "02-9988-7766",
        FixtureDocument::Wpf,
    );
    let top_blocks = text(
        &recorder.db().assistive_blocks(first_frame).unwrap(),
        "document",
    );
    assert!(top_blocks.contains("0800-222-333"));
    assert!(!top_blocks.contains("02-9988-7766"));
    fixture.show("document-scrolled");
    let scrolled = text(&focus.assistive_text(document_permit), "document");
    assert!(scrolled.contains("DOCUMENT-BOTTOM 02-9988-7766"));
    assert!(!scrolled.contains("0800-222-333"));
    assert!(!scrolled.contains("DOCUMENT-SECOND-PARAGRAPH"));

    let bottom_frame = retained(recorder.tick(6000).unwrap());
    assert_ne!(first_frame, bottom_frame);
    assert_native_screenshot(
        &mut recorder,
        &fixture.dir,
        bottom_frame,
        "02-9988-7766",
        "0800-222-333",
        FixtureDocument::Wpf,
    );
    let bottom_blocks = text(
        &recorder.db().assistive_blocks(bottom_frame).unwrap(),
        "document",
    );
    assert!(bottom_blocks.contains("02-9988-7766"));
    assert!(!bottom_blocks.contains("0800-222-333"));
    let reads_before_password = (
        recorder.timings().grab.calls,
        recorder.timings().ocr.calls,
        recorder.timings().assistive.calls,
    );

    fixture.show("password");
    assert!(!focus.is_current(document_permit).unwrap());
    assert!(focus.assistive_text(document_permit).is_empty());
    assert!(!focus.is_current(permit).unwrap());
    assert!(focus.assistive_text(permit).is_empty());
    let password_permit = fixture.observe(&mut focus, SensitiveFieldState::Focused);
    assert!(focus.assistive_text(password_permit).is_empty());

    assert!(!matches!(recorder.tick(7000).unwrap(), Tick::Kept { .. }));
    assert_eq!(
        (
            recorder.timings().grab.calls,
            recorder.timings().ocr.calls,
            recorder.timings().assistive.calls
        ),
        reads_before_password
    );
    assert_eq!(
        recorder
            .db()
            .conn()
            .query_row("SELECT COUNT(*) FROM frames", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );

    fixture.show("button");
    let button_permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    assert!(
        focus.assistive_text(button_permit).is_empty(),
        "button names are not visible edit text"
    );

    fixture.show("other");
    assert!(!focus.is_current(button_permit).unwrap());
    assert!(focus.assistive_text(button_permit).is_empty());
    let other_permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    assert!(text(&focus.assistive_text(other_permit), "edit").contains("OTHER-WINDOW-SENTINEL"));
    println!(
        "SISTER-UIA: VERIFIED visible-chinese fresh-text document-paragraphs document-scroll native-screenshots native-ocr same-frame-rag pause-resume no-offscreen no-password no-button stale-window-denied"
    );
}

// Native browser text + synthetic pixels/system state exercise the real recorder
// and RAG without claiming this 8x8 image is a screenshot of Edge.
struct FixturePixels;
impl ScreenSource for FixturePixels {
    fn grab(&mut self, ts: i64) -> anyhow::Result<Option<RawFrame>> {
        Ok(Some(RawFrame::from_rgba(ts, 0, 8, 8, vec![255; 8 * 8 * 4])))
    }
}
struct FixtureSystem;
impl SystemSource for FixtureSystem {
    fn poll(&mut self, _: i64) -> anyhow::Result<SystemObservation> {
        Ok(SystemObservation::active())
    }
}
fn browser_recorder(dir: PathBuf) -> Recorder<impl Backend> {
    let mut config = Config::default();
    config.capture.ocr = false;
    config.capture.image_min_interval_ms = 0;
    Recorder::new(
        CompositeBackend {
            name: "Edge text / synthetic frame fixture".into(),
            system: FixtureSystem,
            screen: FixturePixels,
            focus: WindowsFocus::new(),
            clipboard: NullClipboard,
            input: NullInput,
            ocr: NullOcr,
        },
        Db::open_in_memory().unwrap(),
        config,
        Some(dir),
        MasterStopSource::NotApplicable,
    )
    .unwrap()
}
fn native_recorder(dir: PathBuf) -> Recorder<impl Backend> {
    let mut config = Config::default();
    config.capture.image_min_interval_ms = 0;
    let ocr = WindowsOcr::new(&["en-US".into()]);
    assert!(
        ocr.is_available(),
        "native screenshot verification requires OCR"
    );
    // Only system activity, clipboard and input are synthetic here. Both the
    // screen pixels and OCR come from the production Windows implementations.
    Recorder::new(
        CompositeBackend {
            name: "native screenshot and OCR fixture".into(),
            system: FixtureSystem,
            screen: WindowsScreen::new(),
            focus: WindowsFocus::new(),
            clipboard: NullClipboard,
            input: NullInput,
            ocr: ChangedRegionOcr::new(ocr),
        },
        Db::open_in_memory().unwrap(),
        config,
        Some(dir),
        MasterStopSource::NotApplicable,
    )
    .unwrap()
}

fn retained(tick: Tick) -> i64 {
    match tick {
        Tick::Kept { frame_id, .. } => frame_id,
        other => panic!("expected retained browser text: {other:?}"),
    }
}

#[test]
#[ignore = "owns an isolated Edge profile and Windows foreground; CI runs alone"]
fn native_edge_reader_visible_paragraphs_scroll_and_privacy() {
    let mut fixture = Fixture::start("uia-edge-reader.ps1");
    fixture.show("top");
    let mut focus = WindowsFocus::new();
    let permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    let initial = text(&focus.assistive_text(permit), "document");
    println!(
        "Edge provider metadata: {}",
        std::fs::read_to_string(fixture.dir.join("metadata")).unwrap()
    );
    assert!(
        initial.contains("網頁電話 0800-333-444"),
        "Edge visible text: {initial:?}"
    );
    assert!(initial.contains("EDGE-SECOND-PARAGRAPH"));
    assert!(!initial.contains("EDGE-BOTTOM"));
    assert!(!initial.contains("HIDDEN-SENTINEL"));
    assert!(!initial.contains("SIBLING-SENTINEL"));
    let mut recorder = browser_recorder(fixture.dir.clone());
    let top_frame = retained(recorder.tick(1000).unwrap());

    fixture.show("bottom");
    // The fixture changes its title to acknowledge the scroll. A fresh observation
    // is required; the old capture context must not authorize this new state.
    assert!(!focus.is_current(permit).unwrap());
    assert!(focus.assistive_text(permit).is_empty());
    let scrolled_permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    let bottom = text(&focus.assistive_text(scrolled_permit), "document");
    assert!(
        bottom.contains("EDGE-BOTTOM 02-7766-5544"),
        "Edge scrolled text: {bottom:?}"
    );
    assert!(!bottom.contains("0800-333-444"));
    assert!(!bottom.contains("EDGE-SECOND-PARAGRAPH"));
    assert!(!bottom.contains("SIBLING-SENTINEL"));
    let bottom_frame = retained(recorder.tick(2000).unwrap());
    assert_ne!(top_frame, bottom_frame);
    assert_browser_sources(
        &mut recorder,
        &fixture.dir,
        "reader.html",
        "document",
        &[
            (top_frame, "0800-333-444", "02-7766-5544"),
            (bottom_frame, "02-7766-5544", "0800-333-444"),
        ],
    );

    fixture.show("group");
    let unsupported_group = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    assert!(
        focus.assistive_text(unsupported_group).is_empty(),
        "a group must not skip its direct Document parent to borrow an outer provider"
    );

    fixture.show("password");
    assert!(!focus.is_current(permit).unwrap());
    assert!(focus.assistive_text(permit).is_empty());
    let password = fixture.observe(&mut focus, SensitiveFieldState::Focused);
    assert!(focus.assistive_text(password).is_empty());
    assert!(!matches!(recorder.tick(3000).unwrap(), Tick::Kept { .. }));
    assert_eq!(recorder.timings().assistive.calls, 2);

    fixture.show("address");
    let observation = focus.context(0).unwrap();
    assert!(
        matches!(
            observation,
            PrivacyObservation::Known {
                context: PrivacyContext::Known {
                    browser_url: sister_core::model::BrowserUrlState::Unknown,
                    ..
                },
                ..
            } | PrivacyObservation::Unknown
        ),
        "editing address must never authorize a page"
    );
    assert!(focus.assistive_text(permit).is_empty());
    assert!(!matches!(recorder.tick(4000).unwrap(), Tick::Kept { .. }));
    assert_eq!(recorder.timings().assistive.calls, 2);
    let count: i64 = recorder
        .db()
        .conn()
        .query_row("SELECT COUNT(*) FROM frames", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    println!(
        "SISTER-EDGE-UIA: VERIFIED visible-chinese scroll same-frame-rag no-hidden no-password address-unknown"
    );
}

fn assert_browser_sources(
    recorder: &mut Recorder<impl Backend>,
    dir: &std::path::Path,
    document: &str,
    role: &str,
    records: &[(i64, &str, &str)],
) {
    for &(id, phone, excluded) in records {
        let blocks = recorder.db().assistive_blocks(id).unwrap();
        let body = text(&blocks, role);
        assert!(body.contains(phone));
        assert!(!body.contains(excluded));
        let image_path: String = recorder
            .db()
            .conn()
            .query_row("SELECT image_path FROM frames WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            image::open(dir.join(image_path))
                .unwrap()
                .to_rgba8()
                .as_raw(),
            &vec![255; 8 * 8 * 4]
        );
    }
    let got = RetrievalProfile::TextAndFacts
        .retrieve(recorder.db_mut(), "phone", 10)
        .unwrap();
    let rag = grounded_answer::prepare("phone", &[], &got.answers, &got.hits, 3000)
        .unwrap()
        .unwrap();
    // A literal "phone" in the PDF can match both its fact and its text chunk.
    // Count distinct frames, and verify every source, including either kind.
    assert!(!rag.sources.is_empty());
    assert!(rag.sources.iter().all(|s| s.origin.as_str() == "assistive"));
    let mut ids: Vec<_> = rag.sources.iter().map(|s| s.frame_id.unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids, records.iter().map(|r| r.0).collect::<Vec<_>>());
    for source in &rag.sources {
        let &(_, phone, excluded) = records
            .iter()
            .find(|r| source.frame_id == Some(r.0))
            .expect("every source must refer to one of the observed frames");
        assert!(source.text.contains(phone));
        assert!(!source.text.contains(excluded));
        assert!(
            source
                .url
                .as_deref()
                .is_some_and(|url| url.ends_with(document))
        );
    }
}

#[test]
#[ignore = "owns an isolated Edge PDF reader and Windows foreground; CI runs alone"]
fn native_edge_pdf_scroll_keeps_uia_ocr_and_screenshot_evidence_together() {
    let mut fixture = Fixture::start_with_args("uia-edge-reader.ps1", &["-Pdf"]);
    fixture.show("top");
    let mut focus = WindowsFocus::new();
    let permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    let first = text(&focus.assistive_text(permit), "document-region");
    println!(
        "Edge PDF first-page provider: {}",
        std::fs::read_to_string(fixture.dir.join("metadata")).unwrap()
    );
    assert!(
        first.contains("PDF-FIRST phone 0800-444-555"),
        "PDF first page: {first:?}"
    );
    assert!(
        !first.contains("PDF-SECOND"),
        "offscreen PDF page: {first:?}"
    );
    let mut recorder = native_recorder(fixture.dir.clone());
    let first_frame = retained(recorder.tick(1000).unwrap());

    assert_native_screenshot(
        &mut recorder,
        &fixture.dir,
        first_frame,
        "0800-444-555",
        "02-6655-4433",
        FixtureDocument::Pdf,
    );
    assert!(
        text(
            &recorder.db().assistive_blocks(first_frame).unwrap(),
            "document-region"
        )
        .contains("0800-444-555")
    );

    fixture.show("bottom");
    let bottom_permit = fixture.observe(&mut focus, SensitiveFieldState::Clear);
    assert!(
        focus.assistive_text(bottom_permit).is_empty(),
        "the offscreen first-page focus must not authorize second-page UIA text"
    );
    let bottom_frame = retained(recorder.tick(6000).unwrap());
    assert_ne!(first_frame, bottom_frame);
    assert!(
        recorder
            .db()
            .assistive_blocks(bottom_frame)
            .unwrap()
            .is_empty()
    );
    assert_native_screenshot(
        &mut recorder,
        &fixture.dir,
        bottom_frame,
        "02-6655-4433",
        "0800-444-555",
        FixtureDocument::Pdf,
    );

    let ocr_calls = recorder.timings().ocr.calls;
    let grab_calls = recorder.timings().grab.calls;
    fixture.show("address");
    assert!(!focus.is_current(permit).unwrap());
    assert!(focus.assistive_text(permit).is_empty());
    assert!(!matches!(recorder.tick(7000).unwrap(), Tick::Kept { .. }));
    assert_eq!(recorder.timings().assistive.calls, 2);
    assert!(ocr_calls >= 2);
    assert_eq!(recorder.timings().ocr.calls, ocr_calls);
    assert_eq!(recorder.timings().grab.calls, grab_calls);
    assert_eq!(
        recorder
            .db()
            .conn()
            .query_row("SELECT COUNT(*) FROM frames", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    println!(
        "SISTER-PDF-UIA: VERIFIED native-screenshots native-ocr scroll old-focus-denied same-frame-rag source-url address-denied"
    );
}

enum FixtureDocument {
    Pdf,
    Wpf,
}
impl FixtureDocument {
    fn assert_source(&self, title: Option<&str>, url: Option<&str>) {
        match self {
            Self::Pdf => {
                assert!(title.is_some_and(|title| title.contains("reader.pdf")));
                assert!(url.is_some_and(|url| url.ends_with("reader.pdf")));
            }
            Self::Wpf => {
                assert_eq!(title, Some("AI-Sister UIA native fixture"));
                assert_eq!(url, None, "WPF evidence must not borrow a browser URL");
            }
        }
    }
}

// Check stored OCR, the actual saved screenshot, and every returned RAG source.
// Reading the saved PNG again catches a source attached to the previous page's
// picture even if the DB text itself is correct.
fn assert_native_screenshot(
    recorder: &mut Recorder<impl Backend>,
    dir: &std::path::Path,
    frame_id: i64,
    phone: &str,
    excluded: &str,
    document: FixtureDocument,
) {
    let body = recorder
        .db()
        .conn()
        .prepare("SELECT text FROM ocr_blocks WHERE frame_id=?1 ORDER BY id")
        .unwrap()
        .query_map([frame_id], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(body.contains(phone), "screenshot OCR: {body:?}");
    assert!(
        !body.contains(excluded),
        "previous/hidden page OCR: {body:?}"
    );
    let context = recorder.db().frame_context(frame_id).unwrap().unwrap();
    document.assert_source(context.window_title.as_deref(), context.url.as_deref());
    let path = context
        .image_path
        .expect("this frame must have its own screenshot");
    let pixels = image::open(dir.join(path)).unwrap().to_rgba8();
    let (width, height) = pixels.dimensions();
    assert!(
        width > 100 && height > 100,
        "real screen dimensions required"
    );
    let saved = RawFrame::from_rgba(context.ts, 0, width, height, pixels.into_raw());
    let blocks = WindowsOcr::new(&["en-US".into()])
        .recognize(&saved)
        .unwrap();
    let visible = blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains(phone), "saved screenshot: {visible:?}");
    assert!(
        !visible.contains(excluded),
        "saved screenshot belongs to another page"
    );

    let got = RetrievalProfile::TextAndFacts
        .retrieve(recorder.db_mut(), phone, 10)
        .unwrap();
    let rag = grounded_answer::prepare(phone, &[], &got.answers, &got.hits, 3000)
        .unwrap()
        .unwrap();
    assert!(!rag.sources.is_empty());
    assert!(
        rag.sources.iter().any(|s| s.origin.as_str() == "ocr"),
        "OCR provenance required"
    );
    if matches!(document, FixtureDocument::Wpf) {
        assert!(
            rag.sources.iter().any(|s| s.origin.as_str() == "assistive"),
            "WPF UIA provenance required"
        );
    }
    for source in &rag.sources {
        assert_eq!(source.frame_id, Some(frame_id));
        assert!(matches!(source.origin.as_str(), "ocr" | "assistive"));
        assert!(source.text.contains(phone));
        assert!(!source.text.contains(excluded));
        document.assert_source(source.title.as_deref(), source.url.as_deref());
    }
}
