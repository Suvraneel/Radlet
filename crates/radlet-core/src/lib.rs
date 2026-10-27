//! Radlet's conversion engine.
//!
//! The engine is UI-agnostic. The Mac app, the `radlet` CLI and future
//! Windows/Linux shells all drive it the same way:
//!
//! 1. [`format::detect`] each selected file and build the [`catalog::wheel`]
//!    of possible conversions, tools and recipes.
//! 2. Turn the chosen slot's [`ops::Action`] into units with [`job::plan`].
//! 3. Run them with [`job::run_batch`], inline or in isolated worker
//!    processes, and stream [`job::Event`]s back to the UI.

pub mod assemble;
pub mod catalog;
pub mod engines;
pub mod error;
pub mod format;
pub mod job;
pub mod naming;
pub mod ops;
pub mod pipeline;
pub mod privacy;
pub mod recipe;

pub use catalog::{Capabilities, Slot, Wheel};
pub use error::{Error, ErrorInfo, Result};
pub use format::{Category, Format};
pub use job::{Event, Executor, Outcome, RunOptions, Status, Summary, Unit};
pub use naming::OutputLocation;
pub use ops::{Action, CompressLevel, Step};
pub use recipe::Recipe;

/// Engine version, reported to shells over the protocol.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
