//! Scheduler binary entrypoint. See
//! [`minerva_server::scheduler_main`] for the actual lifecycle.
//! Phase 3.5 of the microservices split.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    minerva_server::scheduler_main().await
}
