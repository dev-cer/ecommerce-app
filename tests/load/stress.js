// k6 load / stress test NusaMart.
//
//   k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=smoke  tests/load/stress.js   # CI gate (±30 detik)
//   k6 run -e BASE_URL=http://<ip-lb>       -e PROFILE=spike  tests/load/stress.js   # demo HPA cepat (±1,5 menit)
//   k6 run -e BASE_URL=http://<ip-lb>       -e PROFILE=stress tests/load/stress.js   # naik bertahap (±2 menit)
//   k6 run -e BASE_URL=http://<ip-lb>       -e PROFILE=scale  tests/load/stress.js   # autoscaling node sampai 4 (±6 menit)
//
// Alur tiap virtual user meniru pengunjung: buka toko → lihat katalog → cari produk → buka detail produk.

import http from "k6/http";
import { check, group, sleep } from "k6";

const BASE_URL = (__ENV.BASE_URL || "http://localhost:8080").replace(/\/$/, "");
const PROFILE = __ENV.PROFILE || "smoke";

const PROFILES = {
  smoke: [
    { duration: "10s", target: 10 },
    { duration: "20s", target: 10 },
  ],
  stress: [
    { duration: "20s", target: 100 },
    { duration: "40s", target: 300 },
    { duration: "40s", target: 300 },
    { duration: "20s", target: 0 },
  ],
  // Demo singkat: lonjakan langsung + rekomendasi yang berat di CPU, sehingga CPU melewati target HPA
  // dalam hitungan detik. HPA bereaksi ±15-30 detik (metrics-server + sinkronisasi HPA), pod baru siap
  // ±5 detik kemudian; puncak 1 menit cukup untuk melihat replika naik di Grafana.
  spike: [
    { duration: "5s", target: 20 },
    { duration: "10s", target: 300 },
    { duration: "1m", target: 300 },
    { duration: "10s", target: 0 },
  ],
  // Beban berat yang melebihi kapasitas app-pool: HPA menambah pod sampai ada yang Pending, lalu
  // Cluster Autoscaler menambah node Civo (±2-3 menit) sampai maxSize 4. Puncak 5 menit agar node
  // ke-3 dan ke-4 sempat bergabung dan semua replika Running sebelum beban turun.
  scale: [
    { duration: "30s", target: 600 },
    { duration: "5m", target: 600 },
    { duration: "30s", target: 0 },
  ],
};

if (!PROFILES[PROFILE]) {
  throw new Error(`PROFILE tidak dikenal: ${PROFILE} (pilih: ${Object.keys(PROFILES).join(", ")})`);
}

const DEFAULT_THRESHOLDS = {
  // Gate kualitas: pipeline gagal jika dilanggar.
  http_req_failed: ["rate<0.01"],
  "http_req_duration{type:read}": [PROFILE === "smoke" ? "p(95)<200" : "p(95)<500"],
  checks: ["rate>0.99"],
};

// Profil spike & scale sengaja mendorong aplikasi melewati kapasitas pod/node yang ada; latensi naik
// sebentar sampai pod/node baru siap, jadi batasnya lebih longgar.
const HEAVY = PROFILE === "spike" || PROFILE === "scale";
const SCALE_THRESHOLDS = {
  http_req_failed: ["rate<0.05"],
  "http_req_duration{type:read}": ["p(95)<3000"],
  "http_req_duration{type:reco}": ["p(95)<5000"],
  checks: ["rate>0.95"],
};

export const options = {
  scenarios: {
    shoppers: { executor: "ramping-vus", startVUs: 0, stages: PROFILES[PROFILE], gracefulRampDown: "10s" },
  },
  thresholds: HEAVY ? SCALE_THRESHOLDS : DEFAULT_THRESHOLDS,
  summaryTrendStats: ["avg", "med", "p(90)", "p(95)", "max"],
};

const SEARCHES = ["kopi", "batik", "keyboard", "teh", "tas", "lilin"];
const read = { tags: { type: "read" } };

export function setup() {
  const res = http.get(`${BASE_URL}/health`);
  if (res.status !== 200) throw new Error(`${BASE_URL} belum siap (status ${res.status})`);
  const products = http.get(`${BASE_URL}/api/products`).json();
  console.log(`Target ${BASE_URL} · build ${res.json("build")} · profil ${PROFILE} · ${products.length} produk`);
  return { productIds: products.map((p) => p.id) };
}

export default function ({ productIds }) {
  group("browse", () => {
    const home = http.get(`${BASE_URL}/`, read);
    check(home, { "storefront 200": (r) => r.status === 200 });

    const list = http.get(`${BASE_URL}/api/products`, read);
    check(list, { "katalog berisi produk": (r) => r.status === 200 && r.json().length > 0 });

    const q = SEARCHES[Math.floor(Math.random() * SEARCHES.length)];
    check(http.get(`${BASE_URL}/api/products?q=${q}`, read), { "search 200": (r) => r.status === 200 });

    const productId = productIds[Math.floor(Math.random() * productIds.length)];
    check(http.get(`${BASE_URL}/api/products/${productId}`, read), { "detail 200": (r) => r.status === 200 });
  });

  if (HEAVY) {
    group("rekomendasi", () => {
      for (let i = 0; i < 2; i++) {
        const productId = productIds[Math.floor(Math.random() * productIds.length)];
        const res = http.get(`${BASE_URL}/api/recommendations?product_id=${productId}`, { tags: { type: "reco" } });
        check(res, { "rekomendasi 200": (r) => r.status === 200 });
      }
    });
  }

  sleep(Math.random() * 1.5 + 0.5);
}
