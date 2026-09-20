//! Actual recorder -> SQLite/PNG -> retrieval/RAG/export/forget, with synthetic
//! pixels and independent assistive text. This does not claim a live Windows UIA run.
use anyhow::Result;
use sister_capture::{MasterStopSource, PauseSignal, Recorder, Tick, traits::*};
use sister_core::{
    config::Config,
    db::Db,
    grounded_answer,
    model::{AssistiveBlock, BrowserUrlState, FocusSnapshot, PrivacyContext, SensitiveFieldState},
    retrieval::RetrievalProfile,
};
use std::{cell::Cell, rc::Rc};

struct Focus {
    sensitive: SensitiveFieldState,
    calls: Rc<Cell<usize>>,
    paused: Rc<Cell<bool>>,
    pause_on_read: bool,
    change_on_read: bool,
    app: &'static str,
    role: &'static str,
}
impl FocusSource for Focus {
    fn context(&mut self, _: i64) -> Result<PrivacyObservation> {
        Ok(PrivacyObservation::known(
            PrivacyContext::known(
                FocusSnapshot {
                    app_id: Some(self.app.into()),
                    ..Default::default()
                },
                self.sensitive,
                BrowserUrlState::NotApplicable,
            ),
            CapturePermit::backend_local(1),
        ))
    }
    fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
        Ok(permit == CapturePermit::backend_local(1)
            && !(self.change_on_read && self.calls.get() > 0))
    }
    fn assistive_text(&mut self, _: CapturePermit) -> Vec<AssistiveBlock> {
        self.calls.set(self.calls.get() + 1);
        if self.pause_on_read {
            self.paused.set(true);
        }
        vec![AssistiveBlock {
            text: format!("assistive receipt {}\nphone 0800-123-456", self.calls.get()),
            role: self.role.into(),
            bbox: None,
        }]
    }
}
struct Screen;
impl ScreenSource for Screen {
    fn grab(&mut self, ts: i64) -> Result<Option<RawFrame>> {
        Ok(Some(RawFrame::from_rgba(ts, 0, 8, 8, vec![255; 8 * 8 * 4])))
    }
}
struct System;
impl SystemSource for System {
    fn poll(&mut self, _: i64) -> Result<SystemObservation> {
        Ok(SystemObservation::active())
    }
}
fn focus() -> Focus {
    Focus {
        sensitive: SensitiveFieldState::Clear,
        calls: Rc::new(Cell::new(0)),
        paused: Rc::new(Cell::new(false)),
        pause_on_read: false,
        change_on_read: false,
        app: "notes.exe",
        role: "edit",
    }
}
fn recorder(
    focus: Focus,
    config: Config,
    root: Option<std::path::PathBuf>,
) -> Recorder<impl Backend> {
    Recorder::new(
        CompositeBackend {
            name: "assistive fixture".into(),
            system: System,
            screen: Screen,
            focus,
            clipboard: NullClipboard,
            input: NullInput,
            ocr: NullOcr,
        },
        Db::open_in_memory().unwrap(),
        config,
        root,
        MasterStopSource::NotApplicable,
    )
    .unwrap()
}

#[test]
fn assistive_only_text_reaches_rag_with_its_own_png_and_survives_reopen_then_forget() {
    roundtrip("edit");
}

#[test]
fn document_paragraphs_reach_rag_with_their_own_png_and_survive_reopen_then_forget() {
    roundtrip("document");
}

fn roundtrip(role: &'static str) {
    let dir = std::env::temp_dir().join(format!("sister-assistive-{role}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = Config::default();
    config.capture.ocr = false;
    config.capture.image_min_interval_ms = 0;
    let mut source = focus();
    source.role = role;
    let mut rec = recorder(source, config, Some(dir.clone()));
    let id = match rec.tick(1000).unwrap() {
        Tick::Kept { frame_id, .. } => frame_id,
        other => panic!("{other:?}"),
    };
    assert_eq!(rec.timings().assistive.calls, 1);
    assert_eq!(rec.timings().focus_check.calls, 3);
    let ranked = rec.timings().ranked();
    assert!(ranked.iter().any(|(label, _)| *label == "輔助讀字"));
    assert!(ranked.iter().any(|(label, _)| *label == "脈絡核對"));
    let got = RetrievalProfile::TextAndFacts
        .retrieve(rec.db_mut(), "phone", 10)
        .unwrap();
    assert_eq!(got.hits.len(), 1);
    assert_eq!(got.answers.len(), 1);
    let rag = grounded_answer::prepare("phone", &[], &got.answers, &got.hits, 2000)
        .unwrap()
        .unwrap();
    assert_eq!(rag.sources.len(), 2);
    for source in rag.sources {
        assert_eq!(source.origin.as_str(), "assistive");
        assert_eq!(source.frame_id, Some(id));
    }
    let images = rec.db().frames_with_image(&[id]).unwrap();
    assert_eq!(images.len(), 1);
    let image_path: String = rec
        .db()
        .conn()
        .query_row("SELECT image_path FROM frames WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    let image = image::open(dir.join(image_path)).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (8, 8));
    assert_eq!(image.as_raw(), &vec![255; 8 * 8 * 4]);
    assert_eq!(rec.db().assistive_blocks(id).unwrap()[0].bbox, None);
    assert_eq!(rec.db().assistive_blocks(id).unwrap()[0].role, role);
    // Same pixels, new accessible text: don't silently discard it as a dHash duplicate.
    assert!(matches!(rec.tick(2000).unwrap(), Tick::Kept { .. }));
    assert_eq!(rec.db().search("receipt", 10).unwrap().len(), 2);
    let backup = dir.join("backup.db");
    rec.db().export_to(&backup).unwrap();
    let mut reopened = Db::open(&backup).unwrap();
    assert_eq!(reopened.assistive_blocks(id).unwrap().len(), 1);
    assert_eq!(reopened.assistive_blocks(id).unwrap()[0].role, role);
    let draft = reopened
        .export_replay("assistive", 0, 3000)
        .unwrap()
        .into_inner();
    let json = serde_json::to_string(&draft).unwrap();
    assert!(
        !json.contains("0800-123-456"),
        "new source must also be deidentified"
    );
    let mut imported = Db::open_in_memory().unwrap();
    imported.import_replay(&draft, 0).unwrap();
    let hits = imported.search("receipt", 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.source_kind.as_str() == "assistive"));
    reopened.forget(0, 3000, Some(&dir)).unwrap();
    assert!(reopened.assistive_blocks(id).unwrap().is_empty());
    assert!(reopened.search("receipt", 10).unwrap().is_empty());
    assert!(reopened.fact_sightings("phone", 10).unwrap().is_empty());
    drop(reopened);
    drop(rec);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blocked_context_disabled_capture_and_disabled_assistive_never_call_the_source() {
    for (role, case) in ["edit", "document"]
        .into_iter()
        .flat_map(|r| (0..6).map(move |c| (r, c)))
    {
        let mut focus = focus();
        focus.role = role;
        let calls = focus.calls.clone();
        let mut config = Config::default();
        config.capture.store_images = false;
        match case {
            0 => focus.sensitive = SensitiveFieldState::Focused,
            1 => focus.sensitive = SensitiveFieldState::Unknown,
            2 => focus.app = "1password.exe",
            3 => config.capture.enabled = false,
            4 => config.capture.assistive = false,
            _ => {}
        }
        let mut rec = recorder(focus, config, None);
        rec.tick_with_pause_probe(1000, || {
            if case == 5 {
                PauseSignal::Paused { observed_at: 1000 }
            } else {
                PauseSignal::Recording
            }
        })
        .unwrap();
        assert_eq!(calls.get(), 0, "case {case}");
        assert_eq!(rec.timings().assistive.calls, 0, "case {case}");
        assert!(rec.db().search("receipt", 10).unwrap().is_empty());
    }
}

#[test]
fn pause_or_context_change_during_assistive_read_discards_text_and_frame() {
    for (role, change) in ["edit", "document"]
        .into_iter()
        .flat_map(|r| [false, true].map(|c| (r, c)))
    {
        let mut focus = focus();
        focus.role = role;
        focus.pause_on_read = !change;
        focus.change_on_read = change;
        let paused = focus.paused.clone();
        let calls = focus.calls.clone();
        let mut rec = recorder(focus, Config::default(), None);
        let tick = rec
            .tick_with_pause_probe(1000, || {
                if paused.get() {
                    PauseSignal::Paused { observed_at: 1000 }
                } else {
                    PauseSignal::Recording
                }
            })
            .unwrap();
        assert_eq!(
            tick,
            if change {
                Tick::ContextChanged
            } else {
                Tick::Paused
            }
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(
            rec.timings().assistive.calls,
            1,
            "discarded reads still cost time"
        );
        assert_eq!(rec.timings().focus_check.calls, if change { 3 } else { 2 });
        assert!(rec.db().search("receipt", 10).unwrap().is_empty());
        let frames: i64 = rec
            .db()
            .conn()
            .query_row("SELECT COUNT(*) FROM frames", [], |r| r.get(0))
            .unwrap();
        assert_eq!(frames, 0);
    }
}

#[test]
fn losing_assistive_text_refreshes_ocr_once_even_when_the_image_hash_matches() {
    struct DisappearingText(Focus);
    impl FocusSource for DisappearingText {
        fn context(&mut self, ts: i64) -> Result<PrivacyObservation> {
            self.0.context(ts)
        }
        fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
            self.0.is_current(permit)
        }
        fn assistive_text(&mut self, permit: CapturePermit) -> Vec<AssistiveBlock> {
            if self.0.calls.get() == 0 {
                self.0.assistive_text(permit)
            } else {
                Vec::new()
            }
        }
    }
    struct PageOcr;
    impl Ocr for PageOcr {
        fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<sister_core::model::OcrBlock>> {
            Ok(vec![sister_core::model::OcrBlock {
                text: if frame.ts == 1000 {
                    "phone 0800-123-456"
                } else {
                    "phone 02-6655-4433"
                }
                .into(),
                x: 0,
                y: 0,
                w: 8,
                h: 8,
                confidence: -1.0,
            }])
        }
    }
    let mut rec = Recorder::new(
        CompositeBackend {
            name: "assistive disappearance / synthetic OCR".into(),
            system: System,
            screen: Screen,
            focus: DisappearingText(focus()),
            clipboard: NullClipboard,
            input: NullInput,
            ocr: PageOcr,
        },
        Db::open_in_memory().unwrap(),
        Config::default(),
        None,
        MasterStopSource::NotApplicable,
    )
    .unwrap();
    assert!(matches!(rec.tick(1000).unwrap(), Tick::Kept { .. }));
    let next = match rec.tick(2000).unwrap() {
        Tick::Kept { frame_id, .. } => frame_id,
        other => panic!("lost UIA must give OCR a fresh chance: {other:?}"),
    };
    assert!(rec.db().assistive_blocks(next).unwrap().is_empty());
    let got = RetrievalProfile::TextAndFacts
        .retrieve(rec.db_mut(), "02-6655-4433", 10)
        .unwrap();
    let rag = grounded_answer::prepare("02-6655-4433", &[], &got.answers, &got.hits, 2000)
        .unwrap()
        .unwrap();
    assert!(!rag.sources.is_empty());
    for source in rag.sources {
        assert_eq!(source.origin.as_str(), "ocr");
        assert_eq!(source.frame_id, Some(next));
        assert!(source.text.contains("02-6655-4433"));
        assert!(!source.text.contains("0800-123-456"));
    }
    for ts in [3000, 4000, 5000] {
        assert!(matches!(rec.tick(ts).unwrap(), Tick::Duplicate { .. }));
    }
    assert_eq!(
        rec.timings().ocr.calls,
        2,
        "continued UIA absence must not keep forcing OCR"
    );
    assert_eq!(
        rec.db()
            .conn()
            .query_row("SELECT COUNT(*) FROM frames", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}
