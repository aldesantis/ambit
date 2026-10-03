//! `UniFFI` bindings to the ambit engine, linked into the macOS app as a static library.
//!
//! The library never reads process state: the app builds the full environment and passes it in.

use std::sync::Arc;

uniffi::setup_scaffolding!("ambit_ffi");

/// The engine handle the app holds for its lifetime.
#[derive(uniffi::Object)]
pub struct Engine {}

#[uniffi::export]
impl Engine {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }
}
