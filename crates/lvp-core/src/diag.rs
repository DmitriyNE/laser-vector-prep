//! Diagnostics: structured, user-facing messages produced by readers, the layer graph,
//! pattern generation and exporters. Never panics, never `println!`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Informational: nothing was lost (e.g. text entities skipped).
    Info,
    /// Something was approximated or ignored that may matter.
    Warning,
    /// Geometry or configuration is unusable until fixed.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable machine-readable code, dotted namespace (e.g. `dxf.units_missing`).
    pub code: &'static str,
    pub message: String,
    /// Where it happened: layer name, entity handle, layer id... free-form but short.
    pub context: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            context: None,
        }
    }

    pub fn info(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, message)
    }

    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

/// Convenience for collections of diagnostics.
pub trait Diagnostics {
    fn has_errors(&self) -> bool;
    fn count(&self, severity: Severity) -> usize;
}

impl Diagnostics for [Diagnostic] {
    fn has_errors(&self) -> bool {
        self.iter().any(|d| d.severity == Severity::Error)
    }

    fn count(&self, severity: Severity) -> usize {
        self.iter().filter(|d| d.severity == severity).count()
    }
}
