//! The Cooper command-line driver library.
//!
//! The compiler frontend (`cooper-frontend`) is pure and consumes an in-memory
//! [`cooper_frontend::Project`]. This crate is the bridge to the physical world: it
//! loads a source tree and manifest off the filesystem into a `Project`, and builds a
//! checked project into a native executable through the IR, the LLVM backend, and the
//! system C toolchain. Keeping this out of the frontend leaves that crate free of
//! filesystem and process dependencies, so it ports cleanly to targets without them.

mod compile;
mod loader;

pub use compile::{build, BuildError};
pub use loader::{load, load_project, LoadError, Loaded, Native};
