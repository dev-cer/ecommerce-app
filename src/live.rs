//! Statistik live klaster untuk hero storefront (`GET /api/live`): replika, node app-pool,
//! request/detik, dan latensi p95 — diambil dari Prometheus (kube-prometheus-stack, ACT 3).
//!
//! Sengaja tanpa HTTP client tambahan: query Prometheus cukup HTTP/1.0 GET sederhana
//! (HTTP/1.0 menjamin respons tanpa chunked encoding). Hasil di-cache singkat agar banyak
//! browser yang membuka storefront tidak membanjiri Prometheus saat stress test.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
    time::timeout,
};

/// Umur cache: cukup segar untuk animasi di panggung, ringan untuk Prometheus.
const CACHE_TTL: Duration = Duration::from_secs(2);
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct LiveStats {
    /// Replika yang available sekarang.
    pub replicas: Option<u32>,
    /// Target HPA; berbeda dari `replicas` saat sedang scale-up/down.
    pub desired: Option<u32>,
    /// Node app-pool yang terdaftar di klaster (bertambah oleh Cluster Autoscaler).
    pub nodes: Option<u32>,
    /// Request per detik seluruh replika.
    pub rps: Option<f64>,
    /// Latensi p95 seluruh replika (milidetik).
    pub p95_ms: Option<f64>,
}

pub struct LiveSource {
    host: String,
    port: u16,
    namespace: String,
    app: String,
    node_pool: String,
    cache: Mutex<Option<(Instant, LiveStats)>>,
}

impl LiveSource {
    /// `PROMETHEUS_URL` (mis. `http://kcd-monitoring-kube-promet-prometheus.monitoring:9090`).
    /// Tidak di-set → `None`, storefront kembali ke statistik katalog biasa.
    pub fn from_env() -> Option<Self> {
        let env = |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.into());
        let url = std::env::var("PROMETHEUS_URL")
            .ok()
            .filter(|v| !v.is_empty())?;
        let (host, port) = parse_url(&url)?;
        Some(Self {
            host,
            port,
            namespace: env("POD_NAMESPACE", "default"),
            app: env("APP_NAME", "ecommerce-rust"),
            node_pool: env("APP_NODE_POOL", "app-pool"),
            cache: Mutex::new(None),
        })
    }

    pub async fn stats(&self) -> Result<LiveStats, String> {
        // Mutex sekaligus single-flight: request bersamaan menunggu satu fetch yang sama.
        let mut cache = self.cache.lock().await;
        if let Some((at, stats)) = cache.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(stats.clone());
        }

        let (ns, app, pool) = (&self.namespace, &self.app, &self.node_pool);
        let replicas = format!(
            r#"sum(kube_deployment_status_replicas_available{{namespace="{ns}",deployment="{app}"}})"#
        );
        let desired = format!(
            r#"max(kube_horizontalpodautoscaler_status_desired_replicas{{namespace="{ns}",horizontalpodautoscaler="{app}"}})"#
        );
        let nodes = format!(
            r#"count(kube_node_labels{{label_kubernetes_civo_com_civo_node_pool="{pool}"}})"#
        );
        let rps = "sum(irate(nusamart_http_requests_total[1m]))";
        let p95 = "histogram_quantile(0.95, sum by (le) (rate(nusamart_http_request_duration_seconds_bucket[1m])))";

        let (replicas, desired, nodes, rps, p95) = tokio::join!(
            self.query(&replicas),
            self.query(&desired),
            self.query(&nodes),
            self.query(rps),
            self.query(p95),
        );
        // Prometheus tak terjangkau sama sekali → error, supaya frontend memakai fallback.
        if [&replicas, &desired, &nodes, &rps, &p95]
            .iter()
            .all(|r| r.is_err())
        {
            return Err(replicas.unwrap_err());
        }

        let count = |r: Result<Option<f64>, String>| r.ok().flatten().map(|v| v.round() as u32);
        let stats = LiveStats {
            replicas: count(replicas),
            desired: count(desired),
            nodes: count(nodes),
            rps: rps.ok().flatten(),
            p95_ms: p95.ok().flatten().map(|s| s * 1000.0),
        };
        *cache = Some((Instant::now(), stats.clone()));
        Ok(stats)
    }

    /// Instant query → nilai skalar pertama (None bila kosong/NaN, mis. saat belum ada traffic).
    async fn query(&self, promql: &str) -> Result<Option<f64>, String> {
        let request = format!(
            "GET /api/v1/query?query={} HTTP/1.0\r\nHost: {}\r\nAccept: application/json\r\n\r\n",
            percent_encode(promql),
            self.host
        );
        let fetch = async {
            let mut stream = TcpStream::connect((self.host.as_str(), self.port)).await?;
            stream.write_all(request.as_bytes()).await?;
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            Ok::<_, std::io::Error>(raw)
        };
        let raw = timeout(QUERY_TIMEOUT, fetch)
            .await
            .map_err(|_| "Prometheus timeout".to_string())?
            .map_err(|err| format!("Prometheus tidak terjangkau: {err}"))?;
        parse_response(&raw)
    }
}

fn parse_url(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("http://")?.trim_end_matches('/');
    let authority = rest.split('/').next()?;
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), 80)),
    }
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn parse_response(raw: &[u8]) -> Result<Option<f64>, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("respons Prometheus tidak valid")?;
    let status_ok = raw.starts_with(b"HTTP/1.1 200") || raw.starts_with(b"HTTP/1.0 200");
    if !status_ok {
        return Err("Prometheus menolak query".into());
    }
    let body: Value = serde_json::from_slice(&raw[split + 4..]).map_err(|e| e.to_string())?;
    Ok(parse_value(&body))
}

fn parse_value(body: &Value) -> Option<f64> {
    body["data"]["result"][0]["value"][1]
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn url_parsing() {
        assert_eq!(
            parse_url("http://prom.monitoring:9090/"),
            Some(("prom.monitoring".into(), 9090))
        );
        assert_eq!(parse_url("http://prom"), Some(("prom".into(), 80)));
        assert_eq!(parse_url("https://prom:9090"), None);
    }

    #[test]
    fn promql_is_encoded() {
        assert_eq!(
            percent_encode(r#"a{b="c"}[1m]"#),
            "a%7Bb%3D%22c%22%7D%5B1m%5D"
        );
    }

    #[test]
    fn prometheus_vector_and_nan() {
        let ok = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"status\":\"success\",\"data\":{\"resultType\":\"vector\",\"result\":[{\"metric\":{},\"value\":[1.7e9,\"8\"]}]}}";
        assert_eq!(parse_response(ok), Ok(Some(8.0)));
        let nan = b"HTTP/1.0 200 OK\r\n\r\n{\"data\":{\"result\":[{\"value\":[1,\"NaN\"]}]}}";
        assert_eq!(parse_response(nan), Ok(None));
        let empty = b"HTTP/1.0 200 OK\r\n\r\n{\"data\":{\"result\":[]}}";
        assert_eq!(parse_response(empty), Ok(None));
        assert!(parse_response(b"HTTP/1.0 400 Bad Request\r\n\r\n{}").is_err());
    }
}
