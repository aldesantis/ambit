//! Cancellation and progress for long engine calls, bridged to core's [`Control`].
//!
//! The app creates a [`CancelToken`] per operation and passes it with an optional
//! [`ProgressListener`]; [`control_of`] turns the pair into the `Control` an engine call threads
//! through. Reports arrive on the thread running the call, never the main thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ambit_core::util::control::{Control, Progress, ProgressSink};

use crate::records::ProgressEvent;

/// A cancel flag the app sets from any thread. An operation checks it between steps and stops a
/// running git; once it has started writing to the project it finishes instead.
#[derive(Debug, Default, uniffi::Object)]
pub struct CancelToken(Arc<AtomicBool>);

#[uniffi::export]
impl CancelToken {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Receives progress reports, implemented in Swift.
#[uniffi::export(with_foreign)]
pub trait ProgressListener: Send + Sync {
    fn on_progress(&self, event: ProgressEvent);
}

struct ListenerSink(Arc<dyn ProgressListener>);

impl ProgressSink for ListenerSink {
    fn report(&self, progress: &Progress) {
        self.0.on_progress(progress.into());
    }
}

/// The core `Control` for an engine call's optional token and listener.
pub fn control_of(
    cancel: Option<&Arc<CancelToken>>,
    progress: Option<Arc<dyn ProgressListener>>,
) -> Control {
    Control::new(
        cancel.map(|token| Arc::clone(&token.0)),
        progress.map(|listener| Arc::new(ListenerSink(listener)) as Arc<dyn ProgressSink>),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ambit_core::errors::ExitCode;
    use ambit_core::util::control;

    use super::*;
    use crate::records::Stage;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<ProgressEvent>>);

    impl ProgressListener for Recorder {
        fn on_progress(&self, event: ProgressEvent) {
            self.0.lock().expect("unpoisoned").push(event);
        }
    }

    #[test]
    fn a_token_cancels_the_control_built_from_it() {
        let token = CancelToken::new();
        let control = control_of(Some(&token), None);

        assert_eq!(control.check(), Ok(()));

        token.cancel();

        assert!(token.is_canceled());
        assert_eq!(control.check().unwrap_err().code, ExitCode::Canceled);
    }

    #[test]
    fn reports_reach_the_listener() {
        let recorder = Arc::new(Recorder::default());
        let control = control_of(
            None,
            Some(Arc::clone(&recorder) as Arc<dyn ProgressListener>),
        );

        control.report(&Progress {
            stage: control::Stage::Fetching,
            subject: "catalog \"company\"".to_owned(),
            current: 3,
            total: 7,
        });

        let events = recorder.0.lock().expect("unpoisoned");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, Stage::Fetching);
        assert_eq!(events[0].subject, "catalog \"company\"");
        assert_eq!((events[0].current, events[0].total), (3, 7));
    }
}
