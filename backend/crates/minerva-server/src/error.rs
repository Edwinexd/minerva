//! `AppError` and friends moved to the axum-free `minerva-app-core` so the
//! Canvas sync engine + LTI NRPS client (which return `AppError`) could
//! move there too, lettng the `minerva-worker` / `minerva-scheduler`
//! binaries link no axum. Re-exported here so the many `crate::error::*`
//! paths across the route tree keep resolving unchanged.
//!
//! The api crate enables `minerva-app-core`'s `axum` feature (see this
//! crate's `Cargo.toml`), so `AppError`'s `IntoResponse` impl is present
//! here even though the type is defined upstream.
pub use minerva_app_core::error::{AppError, ErrorParams, LocalizedMessage};
