use sema_pkg::{build_router, AppState};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = sema_pkg::config::Config::from_env();

    // Fail closed on insecure production secrets before accepting any traffic.
    if let Err(e) = config.check_production_secrets() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    // Ensure data directories exist before connecting
    std::fs::create_dir_all(&config.blob_dir).expect("Failed to create blob dir");
    std::fs::create_dir_all("data").ok();

    let db = sema_pkg::db::connect(&config.database_url).await;

    let state = Arc::new(AppState { db, config });
    let addr = format!("{}:{}", state.config.host, state.config.port);

    let app = build_router(state.clone());

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!(
                "Port {} is already in use, finding an available port...",
                state.config.port
            );
            let fallback = format!("{}:0", state.config.host);
            match tokio::net::TcpListener::bind(&fallback).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Error: failed to bind to a fallback port: {e}");
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Error: failed to bind to {addr}: {e}");
            std::process::exit(1);
        }
    };

    let local_addr = listener.local_addr().expect("listener has local addr");
    println!("sema-pkg listening on http://{local_addr}");
    tracing::info!("sema-pkg listening on http://{}", local_addr);

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("Error: server exited unexpectedly: {e}");
        std::process::exit(1);
    }
}
