//! Thot MVP vertical slice — axum server.

mod api;
mod db;
mod generate;
mod glossary;
mod llm;
mod pdf;
mod web_verify;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use crate::api::{router as api_router, AppState};
use crate::db::Db;
use crate::llm::{LlmClient, LlmConfig};
use crate::web_verify::WebVerifier;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("thot=info".parse()?))
        .init();

    let data_dir = std::env::var("THOT_DATA_DIR").unwrap_or_else(|_| "data".into());
    let _ = std::fs::create_dir_all(&data_dir);
    let db_path = format!("{data_dir}/thot.db");
    let pdf_path = PathBuf::from(&data_dir).join("course.pdf");
    let db = Db::connect(&db_path).await?;

    let llm = LlmClient::new(LlmConfig::default());
    tracing::info!(
        provider = llm.provider_label(),
        groq = llm.has_groq_key(),
        "LLM client ready"
    );

    let state = AppState {
        db,
        llm,
        verifier: WebVerifier::new(),
        pdf_path,
    };

    let static_dir = resolve_static_dir();
    let index = static_dir.join("index.html");

    let app = Router::new()
        .merge(api_router(state))
        .fallback_service(
            ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index)),
        )
        .layer(CorsLayer::permissive());

    let port: u16 = std::env::var("THOT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Thot listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn resolve_static_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("static"),
        PathBuf::from("thot/static"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static"),
    ];
    for c in candidates {
        if c.join("index.html").exists() {
            return c;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}
