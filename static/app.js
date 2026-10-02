const rupiah = n => "Rp" + n.toLocaleString("id-ID");
const compact = n => n >= 1e6 ? "Rp" + (n / 1e6).toFixed(1).replace(".", ",") + " jt" : rupiah(n);
const $ = id => document.getElementById(id);

const state = { products: [], category: "Semua", query: "", cart: new Map() };

async function api(path, options) {
  const res = await fetch(path, { headers: { "Content-Type": "application/json" }, ...options });
  const body = await res.json();
  if (!res.ok) throw new Error(body.error || "Terjadi kesalahan");
  return body;
}

async function loadProducts() {
  const products = await api("/api/products");
  if (JSON.stringify(products) === JSON.stringify(state.products)) return; // hindari animasi ulang
  state.products = products;
  renderChips();
  renderGrid();
}

async function loadStats() {
  const s = await api("/api/stats");
  $("sOrders").textContent = s.orders;
  $("sItems").textContent = s.items_sold;
  $("sRevenue").textContent = compact(s.revenue);
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

  $("grid").innerHTML = items.length ? items.map((p, i) => {
    const left = p.stock - (state.cart.get(p.id) || 0);
    return `
      <article class="card">
        <div class="thumb">${p.emoji}</div>
        <div class="body">
          <div class="cat">${p.category}</div>
          <div class="name">${p.name}</div>
          <div class="desc">${p.description}</div>
          <div class="meta"><span>⭐ ${p.rating.toFixed(1)}</span><span>${left > 0 ? `Stok ${left}` : "Habis"}</span></div>
          <div class="row">
            <span class="price">${rupiah(p.price)}</span>
            <button class="add" data-add="${p.id}" ${left > 0 ? "" : "disabled"}>+ Tambah</button>
          </div>
        </div>
      </article>`;
  }).join("") : `<div class="empty">Produk tidak ditemukan 🔍</div>`;
  $("grid").querySelectorAll(".card").forEach((card, i) => { card.style.animationDelay = `${i * 40}ms`; });
}

function renderCart() {
  const entries = [...state.cart].map(([id, qty]) => ({ p: state.products.find(p => p.id === id), qty }));
  const count = entries.reduce((n, e) => n + e.qty, 0);
  const total = entries.reduce((n, e) => n + e.p.price * e.qty, 0);

  const badge = $("badge");
  badge.textContent = count || "";
  badge.classList.remove("bump"); void badge.offsetWidth; badge.classList.add("bump");

  $("total").textContent = rupiah(total);
  $("pay").disabled = count === 0;
  $("lines").innerHTML = entries.length ? entries.map(({ p, qty }) => `
    <div class="line">
      <div class="e">${p.emoji}</div>
      <div><div class="name">${p.name}</div><small>${rupiah(p.price * qty)}</small></div>
      <div class="qty">
        <button data-dec="${p.id}">−</button><b>${qty}</b><button data-inc="${p.id}">+</button>
      </div>
    </div>`).join("") : `<div class="empty">Keranjang masih kosong</div>`;
}

function changeQty(id, delta) {
  const p = state.products.find(p => p.id === id);
  const next = Math.min((state.cart.get(id) || 0) + delta, p.stock);
  next > 0 ? state.cart.set(id, next) : state.cart.delete(id);
  renderCart();
  renderGrid();
}

let toastTimer;
function toast(msg) {
  const t = $("toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 3200);
}

const toggleCart = open => $("cart").classList.toggle("open", open);

document.addEventListener("click", e => {
  const t = e.target.closest("button, [data-close]");
  if (!t) return;
  if (t.dataset.cat) { state.category = t.dataset.cat; renderChips(); renderGrid(); }
  if (t.dataset.add) { changeQty(+t.dataset.add, 1); toast("Ditambahkan ke keranjang ✨"); }
  if (t.dataset.inc) changeQty(+t.dataset.inc, 1);
  if (t.dataset.dec) changeQty(+t.dataset.dec, -1);
  if ("close" in t.dataset) toggleCart(false);
  if (t.id === "cartBtn") toggleCart(true);
});

$("search").addEventListener("input", e => { state.query = e.target.value; renderGrid(); });
document.addEventListener("keydown", e => e.key === "Escape" && toggleCart(false));

$("checkout").addEventListener("submit", async e => {
  e.preventDefault();
  $("pay").disabled = true;
  try {
    const order = await api("/api/orders", {
      method: "POST",
      body: JSON.stringify({
        customer: $("customer").value,
        items: [...state.cart].map(([product_id, quantity]) => ({ product_id, quantity })),
      }),
    });
    state.cart.clear();
    toggleCart(false);
    toast(`🎉 Order ${order.id} berhasil · ${rupiah(order.total)}`);
    await Promise.all([loadProducts(), loadStats()]);
  } catch (err) {
    toast("⚠️ " + err.message);
  } finally {
    renderCart();
  }
});

loadProducts().then(renderCart);
async function loadBuild() {
  const h = await api("/health");
  $("build").textContent = `build ${h.build} · pod ${h.instance}`;
}

loadStats();
loadBuild();
setInterval(loadStats, 5000);
setInterval(loadBuild, 15000);
setInterval(loadProducts, 10000); // stok berubah karena pembeli lain / restock otomatis
