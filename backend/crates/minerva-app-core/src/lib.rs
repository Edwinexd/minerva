//! Shared, axum-free application core. See `Cargo.toml` for the
//! rationale: this crate holds the pieces both the api and the
//! standalone worker/scheduler binaries need (AppState, config, the
//! LtiKeyPair, classification, the relink scheduler, the doc-claim
//! worker loops, the Canvas sync engine + LTI NRPS client, and the
//! periodic scheduler loops) without dragging in the axum route tree.
//! The single axum touch-point (`AppError`'s `IntoResponse` impl) is
//! gated behind the `axum` feature that only the api crate turns on.

pub mod canvas;
pub mod classification;
pub mod config;
pub mod error;
pub mod feature_flags;
pub mod github_url;
pub mod llm;
pub mod lti;
pub mod lti_nrps;
pub mod memprobe;
pub mod model_capabilities;
pub mod relink_scheduler;
pub mod rules;
pub mod schedulers;
pub mod state;
pub mod system_defaults;
pub mod worker;
