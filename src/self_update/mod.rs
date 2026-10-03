//! Replacing the running binary with a released one, and noticing that one exists.
//!
//! Named `self_update` because `self` is a keyword; the TypeScript module was `src/self/`.

pub mod notice;
pub mod platform;
pub mod release;
pub mod update;

#[cfg(test)]
pub mod fake_http;
