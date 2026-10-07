const rupiah = n => "Rp" + n.toLocaleString("id-ID");
const $ = id => document.getElementById(id);

const state = { products: [], category: "Semua", query: "" };

async function api(path) {
  const res = await fetch(path);
  const body = await res.json();
  if (!res.ok) throw new Error(body.error || "Terjadi kesalahan");
  return body;
}

async function loadProducts() {
  state.products = await api("/api/products");
  if (!live.on) renderCatalogStats();
  renderChips();
  renderGrid();
}

function renderChips() {
  const cats = ["Semua", ...new Set(state.products.map(p => p.category))];
  $("chips").innerHTML = cats.map(c =>
    `<button class="chip ${c === state.category ? "active" : ""}" data-cat="${c}">${c}</button>`).join("");
}

function renderGrid() {
  const q = state.query.toLowerCase();
  const items = state.products.filter(p =>
    (state.category === "Semua" || p.category === state.category) &&
    (!q || p.name.toLowerCase().includes(q) || p.description.toLowerCase().includes(q)));

  $("grid").innerHTML = items.length ? items.map(p => `
      <article class="card">
        <div class="thumb">${p.emoji}</div>
        <div class="body">
          <div class="cat">${p.category}</div>
          <div class="name">${p.name}</div>
          <div class="desc">${p.description}</div>
          <div class="meta"><span>⭐ ${p.rating.toFixed(1)}</span></div>
          <div class="row">
            <span class="price">${rupiah(p.price)}</span>
            <button class="add" data-reco="${p.id}">Lihat mirip</button>
          </div>
        </div>
      </article>`).join("") : `<div class="empty">Produk tidak ditemukan 🔍</div>`;
  $("grid").querySelectorAll(".card").forEach((card, i) => { card.style.animationDelay = `${i * 40}ms`; });
}

// Rekomendasi dihitung di server (beban CPU yang dipakai untuk demo autoscaling).
async function showRecommendations(id) {
  const base = state.products.find(p => p.id === id);
  $("panelTitle").textContent = `Mirip ${base.name}`;
  $("lines").innerHTML = `<div class="empty">Menghitung rekomendasi…</div>`;
  togglePanel(true);
  try {
    const started = performance.now();
    const recs = await api(`/api/recommendations?product_id=${id}`);
    $("lines").innerHTML = recs.map(r => `
      <div class="line">
        <div class="e">${r.emoji}</div>
        <div><div class="name">${r.name}</div><small>${rupiah(r.price)}</small></div>
        <div class="qty"><b>${r.score.toFixed(2)}</b></div>
      </div>`).join("");
    toast(`✨ ${recs.length} rekomendasi dalam ${Math.round(performance.now() - started)} ms`);
  } catch (err) {
    $("lines").innerHTML = `<div class="empty">⚠️ ${err.message}</div>`;
  }
}

let toastTimer;
function toast(msg) {
  const t = $("toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 3200);
}

const togglePanel = open => $("panel").classList.toggle("open", open);

document.addEventListener("click", e => {
  const t = e.target.closest("button, [data-close]");
  if (!t) return;
  if (t.dataset.cat) { state.category = t.dataset.cat; renderChips(); renderGrid(); }
  if (t.dataset.reco) showRecommendations(+t.dataset.reco);
  if ("close" in t.dataset) togglePanel(false);
});

$("search").addEventListener("input", e => { state.query = e.target.value; renderGrid(); });
document.addEventListener("keydown", e => e.key === "Escape" && togglePanel(false));

// Footer memperlihatkan versi build dan pod yang melayani (berganti karena load balancing).
async function loadBuild() {
  const h = await api("/health");
  state.pod = h.instance;
  $("build").textContent = `build ${h.build} · pod ${h.instance}`;
  if (!live.on) renderCatalogStats();
}

// ---------- Statistik hero ----------
// Live (Prometheus): Replika · Node app-pool · Req/detik — angkanya bergerak saat stress test,
// HPA menambah pod, dan Cluster Autoscaler menambah node. Fallback: Produk · Kategori · Pod.
const live = { on: false, shown: {} };
const fmt = n => Math.round(n).toLocaleString("id-ID");

function stat(key, value, label, sub = "", cls = "") {
  return `<div class="stat ${cls}" data-key="${key}">
    <b data-value="${value ?? ""}">${value == null ? "–" : fmt(value)}</b>
    <small>${label}</small>${sub ? `<em>${sub}</em>` : ""}</div>`;
}

function renderCatalogStats() {
  const pod = state.pod ? state.pod.split("-").pop() : "…";
  $("stats").innerHTML =
    stat("products", state.products.length, "Produk") +
    stat("categories", new Set(state.products.map(p => p.category)).size, "Kategori") +
    `<div class="stat"><b>${pod}</b><small>Pod</small></div>`;
}

function renderLiveStats(s) {
  const scaling = s.desired != null && s.replicas != null && s.desired !== s.replicas;
  const replicaSub = scaling
    ? `${s.desired > s.replicas ? "▲" : "▼"} menuju ${s.desired}`
    : "pod aktif";
  const rpsSub = s.p95_ms != null ? `p95 ${s.p95_ms < 10 ? s.p95_ms.toFixed(1) : fmt(s.p95_ms)} ms` : "idle";
  const busy = (s.rps ?? 0) >= 50;

  $("stats").innerHTML =
    stat("replicas", s.replicas, "Replika", replicaSub, scaling ? "scaling" : "") +
    stat("nodes", s.nodes, "Node", "app-pool") +
    stat("rps", s.rps, "Req/detik", rpsSub, busy ? "hot" : "");

  // Animasi count-up + pulse untuk angka yang berubah sejak poll sebelumnya.
  $("stats").querySelectorAll(".stat").forEach(el => {
    const b = el.querySelector("b");
    const key = el.dataset.key, to = s[key], from = live.shown[key];
    if (to == null) return;
    if (from != null && from !== to) {
      el.classList.add("pulse");
      tween(b, from, to);
    }
    live.shown[key] = to;
  });
}

function tween(el, from, to, ms = 700) {
  const start = performance.now();
  const step = now => {
    const t = Math.min(1, (now - start) / ms), e = 1 - Math.pow(1 - t, 3);
    el.textContent = fmt(from + (to - from) * e);
    if (t < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

async function loadLive() {
  try {
    renderLiveStats(await api("/api/live"));
    live.on = true;
  } catch {
    // Prometheus belum terpasang/terjangkau (mis. sebelum ACT 3 atau saat jalan lokal).
    if (live.on) { live.on = false; live.shown = {}; }
    renderCatalogStats();
  }
}

loadProducts();
loadBuild();
loadLive();
setInterval(loadBuild, 5000);
setInterval(loadLive, 2000);
