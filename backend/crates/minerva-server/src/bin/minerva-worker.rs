//! Worker binary entrypoint. See [`minerva_server::worker_main`] for
//! the actual lifecycle. Phase 3 of the microservices split.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    minerva_server::worker_main().await
}
