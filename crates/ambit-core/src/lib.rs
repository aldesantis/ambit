//! The ambit engine: a deterministic dependency manager for AI-agent capabilities.
//!
//! The `ambit` binary and the macOS app's FFI library both link this crate. The `cli` feature adds
//! the command-line surface and self-update; the FFI library builds without it.

pub mod errors;
pub mod export;
pub mod harness;
pub mod model;
pub mod project;
pub mod resolution;
pub mod util;
pub mod version;

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
pub mod self_update;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
