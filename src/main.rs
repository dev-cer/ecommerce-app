//! Demo e-commerce service: storefront web + REST API dalam satu binary.

mod catalog;
mod error;
mod metrics;
mod routes;
mod security;
mod state;
mod store;
#[cfg(test)]
mod tests;

use std::{net::SocketAddr, time::Duration};

use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;

use crate::{
    state::AppState,
    store::{MemoryStore, PgStore, Store},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let store = connect_store().await?;
    let storage = store.kind();
    let state = AppState::new(store);
    spawn_restocker(state.clone());
    let app = routes::router(state).layer(trace);

    let listener = TcpListener::bind(addr).await?;
    print_banner(port, storage);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("server berhenti dengan rapi. sampai jumpa 👋");
    Ok(())
}

fn print_banner(port: u16, storage: &str) {
    let base = format!("http://localhost:{port}");
    let rows = [
        format!("NusaMart · E-commerce Demo v{}", env!("CARGO_PKG_VERSION")),
        String::new(),
        format!("Storefront  →  {base}"),
        format!("API         →  {base}/api/products"),
        format!("Health      →  {base}/health"),
        format!("Storage     →  {storage}"),
    ];
    let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) + 4;
    println!("\n  ╭{}╮", "─".repeat(width));
    for row in &rows {
        println!("  │  {row:<w$}│", w = width - 2);
    }
    println!("  ╰{}╯\n", "─".repeat(width));
}

/// `DATABASE_URL` di-set → PostgreSQL (wajib untuk multi-replika); jika tidak → in-memory.
async fn connect_store() -> Result<Store, sqlx::Error> {
    let Some(url) = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty()) else {
        tracing::warn!(
            "DATABASE_URL tidak di-set: memakai penyimpanan in-memory (hanya untuk 1 replika)"
        );
        return Ok(Store::Memory(MemoryStore::new()));
    };
    let max_connections = std::env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let store = PgStore::connect(&url, max_connections).await?;
    tracing::info!(
        max_connections,
        "terhubung ke PostgreSQL, migrasi & seed selesai"
    );
    Ok(Store::Postgres(store))
}

/// Gudang otomatis: isi ulang stok berkala supaya demo tetap bisa dipakai setelah stress test.
/// Set `RESTOCK_INTERVAL_SECS=0` untuk mematikan. Aman dijalankan oleh banyak replika sekaligus.
fn spawn_restocker(state: AppState) {
    let secs: u64 = std::env::var("RESTOCK_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    if secs == 0 {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(secs));
        interval.tick().await;
        loop {
            interval.tick().await;
            match state.store.restock().await {
                Ok(0) => {}
                Ok(units) => tracing::info!(units, "gudang restock otomatis"),
                Err(err) => tracing::warn!(?err, "restock gagal"),
            }
        }
    });
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
