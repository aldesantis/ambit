//! `UniFFI` bindings to the ambit engine, linked into the macOS app as a static library.
//!
//! The library never reads process state: the app builds the full environment and passes it in
//! through [`EngineConfig`]. Every export blocks until done; the app calls them off the main
//! thread.
//!
//! Conventions every module here follows:
//!
//! - Records and enums more than one module returns live in [`records`].
//! - Every fallible export returns [`EngineError`] and wraps its body in [`errors::guard`], so a
//!   panic becomes an error instead of unwinding into Swift.
//! - Core errors convert through [`Engine::error`] when an engine is reachable, which also removes
//!   the GitHub token from the text, and through `EngineError::from` otherwise.
//! - Per-session state lives behind [`SetupSession::with_state`], keyed by the module's own type.

uniffi::setup_scaffolding!("ambit_ffi");

pub mod browse;
pub mod control;
pub mod edit;
pub mod engine;
pub mod errors;
pub mod git;
pub mod records;
pub mod review;
pub mod status;

pub use engine::{Engine, EngineConfig, SetupSession, describe_source, supported_agent_tools};
pub use errors::{EngineError, NetworkKind};
