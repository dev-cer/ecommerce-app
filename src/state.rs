use std::{sync::Arc, time::Instant};

use crate::{metrics::HttpMetrics, store::Store};

/// State bersama (murah di-clone). Katalog ada di PostgreSQL (PVC), jadi semua replika aplikasi
/// melihat data yang sama dan HPA bebas menambah/mengurangi pod.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub started_at: Instant,
    /// Versi build (tag image dari CI), tampil di /health dan footer storefront.
    pub build: Arc<str>,
    /// Nama pod/host: memperlihatkan load balancing antar replika.
    pub instance: Arc<str>,
    pub http: Arc<HttpMetrics>,
    /// Beratnya "model" rekomendasi (beban CPU untuk demo autoscaling).
    pub reco_work: u32,
}

impl AppState {
    pub fn new(store: Store, reco_work: u32) -> Self {
        let env = |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.into());
        Self {
            store,
            started_at: Instant::now(),
            build: env("APP_VERSION", "dev").into(),
            instance: env("HOSTNAME", "local").into(),
            http: Arc::default(),
            reco_work,
        }
    }
}
