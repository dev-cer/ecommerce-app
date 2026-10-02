-- Skema NusaMart. Data katalog di-seed dari kode (src/catalog.rs) saat startup.
CREATE TABLE products (
    id            INTEGER PRIMARY KEY,
    name          TEXT    NOT NULL,
    category      TEXT    NOT NULL,
    description   TEXT    NOT NULL,
    emoji         TEXT    NOT NULL,
    price         BIGINT  NOT NULL CHECK (price >= 0),
    rating        REAL    NOT NULL,
    stock         INTEGER NOT NULL CHECK (stock >= 0), -- CHECK = jaring pengaman terakhir anti-oversell
    initial_stock INTEGER NOT NULL CHECK (initial_stock >= 0)
);

CREATE TABLE orders (
    id         BIGSERIAL   PRIMARY KEY,
    customer   TEXT        NOT NULL,
    total      BIGINT      NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE order_items (
    order_id   BIGINT  NOT NULL REFERENCES orders (id) ON DELETE CASCADE,
    product_id INTEGER NOT NULL REFERENCES products (id),
    name       TEXT    NOT NULL,
    quantity   INTEGER NOT NULL CHECK (quantity > 0),
    subtotal   BIGINT  NOT NULL,
    PRIMARY KEY (order_id, product_id)
);
