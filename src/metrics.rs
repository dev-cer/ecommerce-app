//! Metrik Prometheus tanpa dependency tambahan: counter bisnis + request HTTP + histogram latensi.

use std::{
    fmt::Write,
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::Instant,
};

use axum::{extract::Request, extract::State, middleware::Next, response::Response};

use crate::state::AppState;

/// Batas atas bucket histogram latensi (detik).
const BUCKETS: [f64; 10] = [0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0];
const CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

#[derive(Default)]
pub struct HttpMetrics {
    requests: [AtomicU64; CLASSES.len()],
    /// Hitungan per bucket (non-kumulatif); `+Inf` adalah slot terakhir.
    buckets: [AtomicU64; BUCKETS.len() + 1],
    duration_micros: AtomicU64,
}

impl HttpMetrics {
    fn observe(&self, status: u16, seconds: f64) {
        let class = usize::from(status / 100).clamp(1, 5) - 1;
        self.requests[class].fetch_add(1, Relaxed);
        let bucket = BUCKETS
            .iter()
            .position(|&le| seconds <= le)
            .unwrap_or(BUCKETS.len());
        self.buckets[bucket].fetch_add(1, Relaxed);
        self.duration_micros
            .fetch_add((seconds * 1e6) as u64, Relaxed);
    }
}

/// Middleware: catat status & latensi setiap request (kecuali scrape `/metrics` sendiri).
pub async fn track(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/metrics" {
        return next.run(req).await;
    }
    let started = Instant::now();
    let response = next.run(req).await;
    state
        .http
        .observe(response.status().as_u16(), started.elapsed().as_secs_f64());
    response
}

pub fn render(state: &AppState) -> String {
    let stats = state.stats();
    let mut out = String::new();

    let mut metric = |name: &str, kind: &str, help: &str, samples: &[(String, String)]| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (labels, value) in samples {
            let _ = writeln!(out, "{name}{labels} {value}");
        }
    };
    let plain = |v: String| [(String::new(), v)];

    metric(
        "nusamart_orders_total",
        "counter",
        "Jumlah order yang berhasil.",
        &plain(stats.orders.to_string()),
    );
    metric(
        "nusamart_items_sold_total",
        "counter",
        "Jumlah item terjual.",
        &plain(stats.items_sold.to_string()),
    );
    metric(
        "nusamart_revenue_rupiah_total",
        "counter",
        "Total omzet dalam Rupiah.",
        &plain(stats.revenue.to_string()),
    );
    metric(
        "nusamart_stock_units",
        "gauge",
        "Total unit stok tersisa.",
        &plain(stats.stock_units.to_string()),
    );
    metric(
        "nusamart_uptime_seconds",
        "gauge",
        "Lama proses berjalan.",
        &plain(state.started_at.elapsed().as_secs().to_string()),
    );
    metric(
        "nusamart_build_info",
        "gauge",
        "Build yang sedang berjalan.",
        &[(
            format!(
                "{{build=\"{}\",version=\"{}\"}}",
                state.build,
                env!("CARGO_PKG_VERSION")
            ),
            "1".into(),
        )],
    );

    let http = &state.http;
    let requests: Vec<_> = CLASSES
        .iter()
        .zip(&http.requests)
        .map(|(class, n)| {
            (
                format!("{{class=\"{class}\"}}"),
                n.load(Relaxed).to_string(),
            )
        })
        .collect();
    metric(
        "nusamart_http_requests_total",
        "counter",
        "Request HTTP per kelas status.",
        &requests,
    );

    let mut cumulative = 0;
    let mut histogram: Vec<_> = BUCKETS
        .iter()
        .chain(std::iter::once(&f64::INFINITY))
        .zip(&http.buckets)
        .map(|(le, n)| {
            cumulative += n.load(Relaxed);
            let le = if le.is_infinite() {
                "+Inf".to_string()
            } else {
                le.to_string()
            };
            (format!("_bucket{{le=\"{le}\"}}"), cumulative.to_string())
        })
        .collect();
    histogram.push((
        "_sum".into(),
        (http.duration_micros.load(Relaxed) as f64 / 1e6).to_string(),
    ));
    histogram.push(("_count".into(), cumulative.to_string()));
    metric(
        "nusamart_http_request_duration_seconds",
        "histogram",
        "Latensi request HTTP.",
        &histogram,
    );

    out
}
