//! Thot MVP vertical slice — desktop / LAN axum binary.

use thot::server::{self, ServerConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::run(ServerConfig::default()).await
}
