//! Exit codes and the one error type every failure path carries.

use std::fmt;

/// Exit codes. Every failure path maps onto exactly one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExitCode {
    Success = 0,
    Internal = 1,
    /// Config or ownership error.
    Config = 2,
    /// Resolution error: a pattern matching nothing, a missing requirement, a cycle, a name conflict.
    Resolution = 3,
    /// Network or cache error.
    Network = 4,
    /// Drift detected (`status --check`, `install --frozen`, `export --check`).
    Drift = 5,
    /// A health check found something: `doctor` failures.
    Doctor = 6,
}

impl ExitCode {
    /// The process exit status.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// An error with a known exit code and a message already formatted for the user.
///
/// Every message names the offending file, the offending identifier, and one concrete next step.
/// `detail` lines carry the latter two, printed indented under the summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbitError {
    pub code: ExitCode,
    pub message: String,
    pub detail: Vec<String>,
}

impl AmbitError {
    pub fn new<S: Into<String>>(
        code: ExitCode,
        message: impl Into<String>,
        detail: impl IntoIterator<Item = S>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            detail: detail.into_iter().map(Into::into).collect(),
        }
    }

    /// The full multi-line rendering, without the trailing newline.
    pub fn format(&self) -> String {
        let head = format!("error: {}", self.message);

        if self.detail.is_empty() {
            return head;
        }

        std::iter::once(head)
            .chain(self.detail.iter().map(|line| format!("       {line}")))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The catch-all for a failure nothing anticipated: always a bug in ambit, exit 1.
    pub fn unexpected(cause: impl fmt::Display) -> Self {
        Self::new(
            ExitCode::Internal,
            "unexpected internal error",
            [
                cause.to_string(),
                "this is a bug in ambit; please report it".to_owned(),
            ],
        )
    }
}

impl fmt::Display for AmbitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AmbitError {}

impl From<std::io::Error> for AmbitError {
    fn from(error: std::io::Error) -> Self {
        Self::unexpected(error)
    }
}

pub type Result<T, E = AmbitError> = std::result::Result<T, E>;

/// The `(file line N)` suffix a message carries, degrading to `(file)` when nothing positioned
/// the value.
///
/// Every error must name the offending file, and the line is what makes it actionable, so the two
/// are rendered in one place rather than per call site, including for errors raised long after the
/// document was parsed.
pub fn at(file: &str, line: Option<usize>) -> String {
    match line {
        None => format!("({file})"),
        Some(line) => format!("({file} line {line})"),
    }
}

pub fn config_error<S: Into<String>>(
    message: impl Into<String>,
    detail: impl IntoIterator<Item = S>,
) -> AmbitError {
    AmbitError::new(ExitCode::Config, message, detail)
}

pub fn resolution_error<S: Into<String>>(
    message: impl Into<String>,
    detail: impl IntoIterator<Item = S>,
) -> AmbitError {
    AmbitError::new(ExitCode::Resolution, message, detail)
}

pub fn network_error<S: Into<String>>(
    message: impl Into<String>,
    detail: impl IntoIterator<Item = S>,
) -> AmbitError {
    AmbitError::new(ExitCode::Network, message, detail)
}

pub fn drift_error<S: Into<String>>(
    message: impl Into<String>,
    detail: impl IntoIterator<Item = S>,
) -> AmbitError {
    AmbitError::new(ExitCode::Drift, message, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_the_summary_alone_without_detail() {
        let error = config_error("bad thing", Vec::<String>::new());

        assert_eq!(error.format(), "error: bad thing");
    }

    #[test]
    fn indents_detail_under_the_summary() {
        let error = resolution_error("bad thing", ["why", "fix"]);

        assert_eq!(error.format(), "error: bad thing\n       why\n       fix");
        assert_eq!(error.code.as_i32(), 3);
    }

    #[test]
    fn unexpected_errors_are_internal() {
        let error = AmbitError::unexpected("boom");

        assert_eq!(error.code, ExitCode::Internal);
        assert_eq!(
            error.format(),
            "error: unexpected internal error\n       boom\n       this is a bug in ambit; please report it"
        );
    }

    #[test]
    fn positions_with_and_without_a_line() {
        assert_eq!(at("ambit.yml", None), "(ambit.yml)");
        assert_eq!(at("ambit.yml", Some(4)), "(ambit.yml line 4)");
    }
}
