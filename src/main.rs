//! NusaMart: storefront web + REST API katalog produk sampel dalam satu binary.

mod catalog;
mod error;
mod live;
mod metrics;
mod recommend;
mod routes;
mod security;
mod state;
mod store;
#[cfg(test)]
mod tests;

use std::net::SocketAddr;

use tokio::net::TcpListener;

use crate::{state::AppState, store::Store};

/// Putaran "model" rekomendasi per kandidat; ±3 ms CPU per request di release build.
const DEFAULT_RECO_WORK: u32 = 100_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = env_num("PORT").unwrap_or(8080);
    let store = connect_store().await?;
    let state = AppState::new(store, env_num("RECO_WORK").unwrap_or(DEFAULT_RECO_WORK));
    let build = state.build.clone();
    let storage = state.store.kind();

    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await?;
    print_banner(port, &build, storage);

    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    println!("server berhenti dengan rapi. sampai jumpa 👋");
    Ok(())
}

fn print_banner(port: u16, build: &str, storage: &str) {
    let base = format!("http://localhost:{port}");
    let rows = [
        format!("NusaMart · Demo v{} ({build})", env!("CARGO_PKG_VERSION")),
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

/// `DATABASE_URL` di-set → PostgreSQL (data di PVC); jika tidak → katalog in-memory (lokal).
async fn connect_store() -> Result<Store, tokio_postgres::Error> {
    match std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty()) {
        Some(url) => Store::postgres(&url, 30).await,
        None => {
            println!("DATABASE_URL tidak di-set: memakai katalog in-memory");
            Ok(Store::memory())
        }
    }
}

fn env_num<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

/// Menunggu Ctrl+C atau SIGTERM (dikirim Kubernetes) agar shutdown tidak memutus request.
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
    println!("sinyal shutdown diterima, menyelesaikan request yang berjalan…");
}
