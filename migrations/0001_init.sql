-- Skema NusaMart (idempotent, dijalankan setiap startup). Data PostgreSQL tersimpan di PVC (Civo Volume),
-- jadi katalog tetap ada walau pod database di-restart. Seed dari src/catalog.rs hanya bila tabel kosong.
CREATE TABLE IF NOT EXISTS products (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    category    TEXT    NOT NULL,
    description TEXT    NOT NULL,
    emoji       TEXT    NOT NULL,
    price       BIGINT  NOT NULL CHECK (price >= 0),
    rating      REAL    NOT NULL
);
