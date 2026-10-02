//! Demo e-commerce service: storefront web + REST API dalam satu binary.

mod catalog;
mod error;
mod routes;
mod state;

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .with_target(false)
        .compact()
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let trace = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));
    let app = routes::router(AppState::new()).layer(trace);

    let listener = TcpListener::bind(addr).await?;
    print_banner(port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("server berhenti dengan rapi. sampai jumpa 👋");
    Ok(())
}

fn print_banner(port: u16) {
    let base = format!("http://localhost:{port}");
    let rows = [
        format!("NusaMart · E-commerce Demo v{}", env!("CARGO_PKG_VERSION")),
        String::new(),
        format!("Storefront  →  {base}"),
        format!("API         →  {base}/api/products"),
        format!("Health      →  {base}/health"),
    ];
    let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) + 4;
    println!("\n  ╭{}╮", "─".repeat(width));
    for row in &rows {
        println!("  │  {row:<w$}│", w = width - 2);
    }
    println!("  ╰{}╯\n", "─".repeat(width));
}

/// Menunggu Ctrl+C atau SIGTERM (dikirim `docker stop`) agar shutdown tidak memutus request.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("gagal memasang handler Ctrl+C");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("gagal memasang handler SIGTERM")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("sinyal shutdown diterima, menyelesaikan request yang berjalan…");
}
