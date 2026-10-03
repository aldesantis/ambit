//! Turning a config and the merged catalog into a bundle, and checking a whole catalog.

pub mod resolve;
// Reached by the desktop app's FFI layer and the setup review, not by the CLI binary.
#[allow(dead_code)]
pub mod routes;
pub mod validate;
