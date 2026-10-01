//! The Cooper command-line driver library.
//!
//! The compiler frontend (`cooper-frontend`) is pure and consumes an in-memory
//! [`cooper_frontend::Project`]. This crate is the bridge from the physical world
//! to that model: it walks a source tree and manifest off the filesystem into a
//! `Project` the frontend can analyze. Keeping it out of the frontend leaves that
//! crate free of filesystem and manifest dependencies, so it ports cleanly to
//! targets without a filesystem (a browser WASM playground, say).

mod loader;

pub use loader::{load_project, LoadError};
