// k6 load / stress test NusaMart.
//
//   k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=smoke  tests/load/stress.js   # CI gate (±30 detik)
//   k6 run -e BASE_URL=http://<ip-lb>       -e PROFILE=stress tests/load/stress.js   # live demo (±4 menit)
//   k6 run -e BASE_URL=http://<ip-lb>       -e PROFILE=spike  tests/load/stress.js   # lonjakan mendadak
//
// Alur tiap virtual user meniru pembeli: buka toko → lihat katalog → cari produk → kadang checkout.

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
    { duration: "30s", target: 50 },
    { duration: "1m", target: 150 },
    { duration: "1m30s", target: 300 },
    { duration: "1m", target: 300 },
    { duration: "30s", target: 0 },
  ],
  spike: [
    { duration: "10s", target: 10 },
    { duration: "10s", target: 400 },
    { duration: "40s", target: 400 },
    { duration: "10s", target: 10 },
  ],
};

if (!PROFILES[PROFILE]) {
  throw new Error(`PROFILE tidak dikenal: ${PROFILE} (pilih: ${Object.keys(PROFILES).join(", ")})`);
}

export const options = {
  scenarios: {
    shoppers: { executor: "ramping-vus", startVUs: 0, stages: PROFILES[PROFILE], gracefulRampDown: "10s" },
  },
  thresholds: {
    // Gate kualitas: pipeline gagal jika dilanggar.
    http_req_failed: ["rate<0.01"],
    "http_req_duration{type:read}": [PROFILE === "smoke" ? "p(95)<200" : "p(95)<500"],
    "http_req_duration{type:write}": [PROFILE === "smoke" ? "p(95)<300" : "p(95)<800"],
    checks: ["rate>0.99"],
  },
  summaryTrendStats: ["avg", "med", "p(90)", "p(95)", "max"],
};

// 409 = stok habis: perilaku bisnis yang benar (tidak oversell), bukan error.
http.setResponseCallback(http.expectedStatuses(200, 201, 409));

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

    check(http.get(`${BASE_URL}/api/stats`, read), { "stats 200": (r) => r.status === 200 });
  });

  sleep(Math.random() * 1.5 + 0.5);

  // ±30% pengunjung checkout.
  if (Math.random() < 0.3) {
    group("checkout", () => {
      const productId = productIds[Math.floor(Math.random() * productIds.length)];
      const res = http.post(
        `${BASE_URL}/api/orders`,
        JSON.stringify({ customer: `k6-vu-${__VU}`, items: [{ product_id: productId, quantity: 1 }] }),
        { headers: { "Content-Type": "application/json" }, tags: { type: "write" } },
      );
      check(res, {
        "order dibuat atau stok habis": (r) => r.status === 201 || r.status === 409,
        "tidak ada server error": (r) => r.status < 500,
      });
    });
    sleep(1);
  }
}
