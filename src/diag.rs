//! Source spans and compiler diagnostics.
//!
//! Every diagnostic carries the [`Span`] of the offending source range, so the
//! whole pipeline can surface many precisely located errors from a single run
//! rather than aborting at the first one. Rendering is delegated to [`ariadne`],
//! which draws the underlined snippet and any attached note.

use std::ops::Range;

use ariadne::{Color, Label, Report, ReportKind, Source};

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

impl From<Span> for Range<usize> {
    fn from(span: Span) -> Range<usize> {
        span.start..span.end
    }
}

impl From<Range<usize>> for Span {
    fn from(range: Range<usize>) -> Span {
        Span::new(range.start, range.end)
    }
}

/// A located compiler error with an optional explanatory note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub note: Option<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            span,
            message: message.into(),
            note: None,
        }
    }

    /// Attach a secondary note, rendered beneath the primary label.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Render this diagnostic against `source`, labelling the file as `filename`.
    pub fn render(&self, filename: &str, source: &str) -> String {
        let range: Range<usize> = self.span.into();
        let mut report = Report::build(ReportKind::Error, (filename, range.clone()))
            .with_message(&self.message)
            .with_label(
                Label::new((filename, range))
                    .with_message(&self.message)
                    .with_color(Color::Red),
            );
        if let Some(note) = &self.note {
            report = report.with_note(note);
        }

        let mut buf = Vec::new();
        report
            .finish()
            .write((filename, Source::from(source)), &mut buf)
            .expect("writing a report to an in-memory buffer cannot fail");
        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// Render every diagnostic in `diagnostics` against `source`, concatenated in
/// order.
pub fn render_all(diagnostics: &[Diagnostic], filename: &str, source: &str) -> String {
    diagnostics
        .iter()
        .map(|d| d.render(filename, source))
        .collect()
}
