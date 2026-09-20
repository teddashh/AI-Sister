//! Bounded visible-text collection shared by native UIA and deterministic privacy tests.
use sister_core::model::AssistiveBlock;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub(crate) const READ_BUDGET: Duration = Duration::from_millis(900);
const MAX_RANGES: usize = 16;
const MAX_CHARS: usize = 8192;

#[derive(Clone)]
pub(crate) struct ReadWindow {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}
impl ReadWindow {
    pub(crate) fn new() -> Self {
        Self {
            deadline: Instant::now() + READ_BUDGET,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn active(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire) && Instant::now() < self.deadline
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextRole {
    Edit,
    Document,
    DocumentRegion,
}
impl TextRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Document => "document",
            Self::DocumentRegion => "document-region",
        }
    }
}

/// Screen coordinates, including providers whose page container extends below
/// the viewport. Every captured text rectangle must fit the current frame.
#[derive(Clone, Copy)]
pub(crate) struct TextRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl TextRect {
    fn valid(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .iter()
        .all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
    pub(crate) fn contains(self, other: Self) -> bool {
        self.valid()
            && other.valid()
            && other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }
    pub(crate) fn overlaps(self, other: Self) -> bool {
        self.valid()
            && other.valid()
            && self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// `eligible` must positively establish exact foreground ownership, unchanged focus,
/// an Edit, Document or scoped document region, IsPassword=false and
/// IsOffscreen=false. Unknown is rejection.
pub(crate) trait VisibleText {
    fn context_matches(&mut self) -> bool;
    fn role(&mut self) -> Option<TextRole>;
    fn is_password(&mut self) -> Option<bool>;
    fn is_offscreen(&mut self) -> Option<bool>;
    fn range_count(&mut self) -> Option<usize>;
    fn text(&mut self, index: usize, limit: usize) -> Option<String>;
}

fn eligible(source: &mut impl VisibleText) -> Option<TextRole> {
    if !source.context_matches()
        || source.is_password() != Some(false)
        || source.is_offscreen() != Some(false)
    {
        return None;
    }
    source.role()
}

pub(crate) fn collect(source: &mut impl VisibleText, window: &ReadWindow) -> Vec<AssistiveBlock> {
    if !window.active() {
        return Vec::new();
    }
    let Some(role) = eligible(source) else {
        return Vec::new();
    };
    if !window.active() {
        return Vec::new();
    }
    let Some(count) = source.range_count() else {
        return Vec::new();
    };
    let mut remaining = MAX_CHARS;
    let mut out = Vec::new();
    for index in 0..count.min(MAX_RANGES) {
        if remaining == 0 {
            break;
        }
        if !window.active() || eligible(source) != Some(role) || !window.active() {
            return Vec::new();
        }
        let Some(text) = source.text(index, remaining) else {
            return Vec::new();
        };
        if !window.active() {
            return Vec::new();
        }
        // Enforce our bound even if a provider ignores GetText(maxLength).
        let text: String = text.chars().take(remaining).collect();
        remaining -= text.chars().count();
        let text = text.trim();
        if !text.is_empty() {
            out.push(AssistiveBlock {
                text: text.into(),
                role: role.as_str().into(),
                bbox: None,
            });
        }
    }
    if !window.active() || eligible(source) != Some(role) || !window.active() {
        return Vec::new();
    }
    out
}

// A provider can live on an enclosing document (Chromium). Intersect each
// visible range with RangeFromChild(focused_document) before reading any text.
#[derive(Clone, Copy)]
pub(crate) enum TextEnd {
    Start,
    End,
}
pub(crate) trait TextRange {
    fn compare(&self, end: TextEnd, other: &Self, other_end: TextEnd) -> Option<i32>;
    fn move_end(&self, end: TextEnd, other: &Self, other_end: TextEnd) -> Option<()>;
}
pub(crate) fn clip_visible<T: TextRange>(range: &T, scope: &T) -> Option<bool> {
    use TextEnd::{End, Start};
    if range.compare(End, scope, Start)? <= 0 || range.compare(Start, scope, End)? >= 0 {
        return Some(false);
    }
    if range.compare(Start, scope, Start)? < 0 {
        range.move_end(Start, scope, Start)?;
    }
    if range.compare(End, scope, End)? > 0 {
        range.move_end(End, scope, End)?;
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rectangles_must_fit_the_frame_and_window() {
        let screen = TextRect {
            x: -100.0,
            y: 0.0,
            width: 100.0,
            height: 80.0,
        };
        let visible = TextRect {
            x: -90.0,
            y: 20.0,
            width: 80.0,
            height: 20.0,
        };
        assert!(screen.contains(visible));
        assert!(screen.overlaps(visible));
        assert!(!screen.overlaps(TextRect { x: 0.0, ..visible }));
        assert!(!screen.overlaps(TextRect {
            y: f64::NAN,
            ..visible
        }));
        for invalid in [
            TextRect {
                x: -110.0,
                ..visible
            },
            TextRect {
                x: -10.0,
                ..visible
            },
            TextRect { y: -1.0, ..visible },
            TextRect { y: 70.0, ..visible },
            TextRect {
                width: 0.0,
                ..visible
            },
            TextRect {
                height: -1.0,
                ..visible
            },
            TextRect {
                x: f64::NAN,
                ..visible
            },
            TextRect {
                width: f64::INFINITY,
                ..visible
            },
        ] {
            assert!(!screen.contains(invalid));
            assert!(!invalid.contains(visible));
        }
    }

    struct Range {
        start: std::cell::Cell<usize>,
        end: std::cell::Cell<usize>,
        available: bool,
    }
    impl Range {
        fn new(start: usize, end: usize) -> Self {
            Self {
                start: start.into(),
                end: end.into(),
                available: true,
            }
        }
        fn endpoint(&self, end: TextEnd) -> &std::cell::Cell<usize> {
            match end {
                TextEnd::Start => &self.start,
                TextEnd::End => &self.end,
            }
        }
        fn text<'a>(&self, page: &'a str) -> &'a str {
            &page[self.start.get()..self.end.get()]
        }
    }
    impl TextRange for Range {
        fn compare(&self, end: TextEnd, other: &Self, other_end: TextEnd) -> Option<i32> {
            self.available.then(|| {
                self.endpoint(end)
                    .get()
                    .cmp(&other.endpoint(other_end).get()) as i32
            })
        }
        fn move_end(&self, end: TextEnd, other: &Self, other_end: TextEnd) -> Option<()> {
            if !self.available {
                return None;
            }
            self.endpoint(end).set(other.endpoint(other_end).get());
            Some(())
        }
    }
    #[test]
    fn enclosing_visible_range_is_clipped_to_the_focused_document() {
        let page = "LEFTDOCUMENTRIGHT";
        let document = Range::new(4, 12);
        for (start, end, expected) in [
            (0, 17, "DOCUMENT"),
            (0, 8, "DOCU"),
            (8, 17, "MENT"),
            (6, 10, "CUME"),
        ] {
            let visible = Range::new(start, end);
            assert_eq!(clip_visible(&visible, &document), Some(true));
            assert_eq!(visible.text(page), expected);
        }
    }
    #[test]
    fn sibling_visible_ranges_and_unreadable_ranges_are_rejected() {
        let document = Range::new(4, 12);
        for (start, end) in [(0, 4), (12, 17), (1, 3), (13, 16)] {
            assert_eq!(
                clip_visible(&Range::new(start, end), &document),
                Some(false)
            );
        }
        let mut unavailable = Range::new(0, 17);
        unavailable.available = false;
        assert_eq!(clip_visible(&unavailable, &document), None);
    }

    struct Source {
        allowed: bool,
        reads: usize,
        lose_focus: bool,
        cancel: Option<ReadWindow>,
        role: Option<TextRole>,
        password: Option<bool>,
        offscreen: Option<bool>,
        change_role: bool,
    }
    impl VisibleText for Source {
        fn context_matches(&mut self) -> bool {
            self.allowed
        }
        fn role(&mut self) -> Option<TextRole> {
            self.role
        }
        fn is_password(&mut self) -> Option<bool> {
            self.password
        }
        fn is_offscreen(&mut self) -> Option<bool> {
            self.offscreen
        }
        fn range_count(&mut self) -> Option<usize> {
            Some(30)
        }
        fn text(&mut self, _: usize, _: usize) -> Option<String> {
            self.reads += 1;
            if self.change_role {
                self.role = Some(TextRole::Edit);
            }
            if self.lose_focus {
                self.allowed = false;
            }
            if let Some(window) = &self.cancel {
                window.cancel();
            }
            Some(format!("電話 0800-123-456{}", "甲".repeat(10000)))
        }
    }
    fn source() -> Source {
        Source {
            allowed: true,
            reads: 0,
            lose_focus: false,
            cancel: None,
            role: Some(TextRole::Edit),
            password: Some(false),
            offscreen: Some(false),
            change_role: false,
        }
    }
    #[test]
    fn password_offscreen_and_unknown_role_never_read() {
        for (role, value) in [TextRole::Edit, TextRole::Document, TextRole::DocumentRegion]
            .into_iter()
            .flat_map(|role| [Some(true), None].map(|value| (role, value)))
        {
            let mut s = source();
            s.role = Some(role);
            s.password = value;
            assert!(collect(&mut s, &ReadWindow::new()).is_empty());
            assert_eq!(s.reads, 0);
            let mut s = source();
            s.role = Some(role);
            s.offscreen = value;
            assert!(collect(&mut s, &ReadWindow::new()).is_empty());
            assert_eq!(s.reads, 0);
        }
        let mut s = source();
        s.role = None;
        assert!(collect(&mut s, &ReadWindow::new()).is_empty());
        assert_eq!(s.reads, 0);
    }

    #[test]
    fn denied_or_cancelled_never_reads_text() {
        let mut source = source();
        source.allowed = false;
        assert!(collect(&mut source, &ReadWindow::new()).is_empty());
        assert_eq!(source.reads, 0);
        source.allowed = true;
        let window = ReadWindow::new();
        window.cancel();
        assert!(collect(&mut source, &window).is_empty());
        assert_eq!(source.reads, 0);
    }
    #[test]
    fn late_result_or_focus_change_discards_everything() {
        for (role, cancelled) in [TextRole::Edit, TextRole::Document, TextRole::DocumentRegion]
            .into_iter()
            .flat_map(|role| [false, true].map(|cancelled| (role, cancelled)))
        {
            let window = ReadWindow::new();
            let mut source = source();
            source.role = Some(role);
            source.lose_focus = !cancelled;
            source.cancel = cancelled.then(|| window.clone());
            assert!(collect(&mut source, &window).is_empty());
            assert_eq!(source.reads, 1);
        }
    }
    #[test]
    fn document_keeps_its_role_and_rejects_a_role_change_during_read() {
        let mut s = source();
        s.role = Some(TextRole::Document);
        let out = collect(&mut s, &ReadWindow::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "document");
        assert_eq!(out[0].bbox, None);
        assert!(out[0].text.starts_with("電話 0800-123-456"));
        s.change_role = true;
        assert!(collect(&mut s, &ReadWindow::new()).is_empty());
    }
    #[test]
    fn provider_text_is_bounded_and_has_no_invented_coordinates() {
        let mut source = source();
        let out = collect(&mut source, &ReadWindow::new());
        assert_eq!(out.len(), 1);
        assert_eq!(source.reads, 1);
        assert!(out[0].text.starts_with("電話 0800-123-456"));
        assert_eq!(out[0].text.chars().count(), MAX_CHARS);
        assert_eq!(out[0].bbox, None);
        assert_eq!(out[0].role, "edit");
    }
}
