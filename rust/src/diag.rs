use std::ops::Range;

/// A byte range into the source, used both for AST provenance and for rendering
/// diagnostics with `ariadne`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// The span covering both `self` and `other` and everything between them.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

impl From<Span> for Range<usize> {
    fn from(s: Span) -> Range<usize> {
        s.start..s.end
    }
}

impl From<Range<usize>> for Span {
    fn from(r: Range<usize>) -> Span {
        Span::new(r.start, r.end)
    }
}

/// A collected diagnostic. Unlike the Go frontend, a parse error does not abort:
/// the parser records a `Diagnostic`, recovers to the next statement boundary, and
/// keeps going, so a single run can surface many errors at once.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    /// Optional secondary hint rendered as a note beneath the primary label.
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

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Render this diagnostic to a string against `source` using `ariadne`.
    pub fn render(&self, filename: &str, source: &str) -> String {
        use ariadne::{Color, Label, Report, ReportKind, Source};

        let mut buf = Vec::new();
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
        report
            .finish()
            .write((filename, Source::from(source)), &mut buf)
            .expect("writing an ariadne report to an in-memory buffer never fails");
        String::from_utf8_lossy(&buf).into_owned()
    }
}
