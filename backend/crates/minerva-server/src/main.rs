//! Api binary entrypoint. The actual startup logic lives in
//! [`minerva_server::api_main`] so it can share the module tree with
//! the new `minerva-worker` (and Phase 3.5 `minerva-scheduler`)
//! binaries without duplicating routes / AppState / classification.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    minerva_server::api_main().await
}
