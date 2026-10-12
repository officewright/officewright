use std::fmt;

/// Something in a document that Word repairs or ignores when it opens it, and so does
/// officewright: the document opens, and this says what was not as the standard has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    kind: DiagnosticKind,
    message: String,
}

/// What kind of repair a [`Diagnostic`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// An attribute has a value officewright does not understand; the attribute is read
    /// as absent.
    InvalidValue,
    /// Content where the document format has none, such as text between paragraphs; it
    /// is ignored.
    UnexpectedContent,
    /// More diagnostics than are kept; the message says how many were left out.
    Omitted,
}

impl Diagnostic {
    pub(crate) fn new(kind: DiagnosticKind, message: String) -> Self {
        Self { kind, message }
    }

    /// The kind of repair.
    pub fn kind(&self) -> DiagnosticKind {
        self.kind
    }

    /// What was repaired, naming the part and position.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The diagnostics of one document, at most [`Diagnostics::MAX`] of them, so that a
/// document full of repairs costs no more memory than its content.
#[derive(Debug, Default)]
pub(crate) struct Diagnostics {
    kept: Vec<Diagnostic>,
    omitted: u64,
}

impl Diagnostics {
    const MAX: usize = 1000;

    /// Whether no more diagnostics are kept, only counted.
    pub(crate) fn is_full(&self) -> bool {
        self.kept.len() >= Self::MAX
    }

    pub(crate) fn push(&mut self, kind: DiagnosticKind, message: impl FnOnce() -> String) {
        if self.kept.len() < Self::MAX {
            self.kept.push(Diagnostic::new(kind, message()));
        } else {
            self.omitted += 1;
        }
    }

    pub(crate) fn finish(mut self) -> Vec<Diagnostic> {
        if self.omitted > 0 {
            self.kept.push(Diagnostic::new(
                DiagnosticKind::Omitted,
                format!("{} more diagnostics are not kept", self.omitted),
            ));
        }
        self.kept
    }
}
