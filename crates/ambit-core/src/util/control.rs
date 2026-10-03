//! Progress reporting and cancellation for long operations driven by a library caller.
//!
//! A [`Control`] travels by value inside the contexts an operation already takes. The default
//! reports nowhere and never cancels, which is what the CLI uses. It compares equal to every other
//! `Control`, so the structs that carry it keep their derived `PartialEq`, `Eq` and `Debug`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::errors::{AmbitError, ExitCode, Result};

/// The step of an operation a [`Progress`] report belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    LoadingCatalogs,
    Fetching,
    Resolving,
    Planning,
    CheckingOwnership,
    SavingConfig,
    WritingFiles,
    RemovingFiles,
    WritingRecords,
}

/// One progress report. `total` is 0 when the amount of work is not known in advance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    /// What the step is working on: a catalog name, a path.
    pub subject: String,
    pub current: u32,
    pub total: u32,
}

/// Receives progress reports. Called on the thread running the operation.
pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: &Progress);
}

/// A cancellation flag and a progress sink, both optional.
#[derive(Clone, Default)]
pub struct Control {
    cancel: Option<Arc<AtomicBool>>,
    sink: Option<Arc<dyn ProgressSink>>,
}

impl Control {
    /// A control that observes `cancel` and reports to `sink`.
    pub fn new(cancel: Option<Arc<AtomicBool>>, sink: Option<Arc<dyn ProgressSink>>) -> Self {
        Self { cancel, sink }
    }

    /// Sends `progress` to the sink, if there is one.
    pub fn report(&self, progress: &Progress) {
        if let Some(sink) = &self.sink {
            sink.report(progress);
        }
    }

    /// Whether the caller has asked the operation to stop.
    pub fn is_canceled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    /// A point where the operation may stop.
    ///
    /// # Errors
    ///
    /// [`canceled`] (exit [`ExitCode::Canceled`]) once the caller has set the cancel flag.
    pub fn check(&self) -> Result<()> {
        if self.is_canceled() {
            return Err(canceled());
        }

        Ok(())
    }
}

impl PartialEq for Control {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Control {}

impl fmt::Debug for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Control")
            .field("cancelable", &self.cancel.is_some())
            .field("reporting", &self.sink.is_some())
            .finish()
    }
}

/// The error an operation returns when it stops because the caller canceled it.
pub fn canceled() -> AmbitError {
    AmbitError::new(
        ExitCode::Canceled,
        "the operation was canceled",
        Vec::<String>::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_never_cancels() {
        let control = Control::default();

        assert!(!control.is_canceled());
        assert_eq!(control.check(), Ok(()));
    }

    #[test]
    fn checks_fail_once_the_flag_is_set() {
        let flag = Arc::new(AtomicBool::new(false));
        let control = Control::new(Some(Arc::clone(&flag)), None);

        assert_eq!(control.check(), Ok(()));

        flag.store(true, Ordering::Relaxed);

        assert_eq!(control.check().unwrap_err().code, ExitCode::Canceled);
    }
}
