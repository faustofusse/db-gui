//! dbear shared core.
//!
//! Everything that isn't UI lives here: models, drivers, value decoding, SQL handling.
//! - The GPUI (Linux) app depends on this crate directly.
//! - The SwiftUI (macOS) app uses it through `dbcore-ffi` (UniFFI).

mod config;
mod connection;
pub mod driver;
pub mod highlight;
pub mod mock;
pub mod model;
pub mod postgres;
pub mod store;

pub use connection::Connection;
pub use driver::{Driver, Error, Result};
pub use model::*;
pub use store::ConnectionStore;
