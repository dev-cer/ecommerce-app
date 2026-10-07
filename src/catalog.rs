use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Product {
    pub id: i32,
    pub name: String,
    pub category: String,
    pub description: String,
    pub emoji: String,
    /// Harga dalam Rupiah (tanpa desimal).
    pub price: i64,
    pub rating: f32,
}

/// Katalog sampel (statis, di memori): cukup untuk demo dan identik di semua replika.
pub fn seed() -> Vec<Product> {
    vec![
        product(
            1,
            "Kopi Gayo Arabika 250g",
            "Makanan",
            "Single origin Aceh, notes cokelat & rempah.",
            "☕",
            89_000,
            4.9,
        ),
        product(
            2,
            "Batik Tulis Pekalongan",
            "Fashion",
            "Kemeja batik tulis motif Jlamprang, katun primisima.",
            "👔",
            425_000,
            4.8,
        ),
        product(
            3,
            "Earbuds Nirkabel Pro",
            "Elektronik",
            "ANC, baterai 30 jam, tahan air IPX5.",
            "🎧",
            649_000,
            4.6,
        ),
        product(
            4,
            "Sambal Roa Manado",
            "Makanan",
            "Ikan roa asap, pedas nampol, tanpa pengawet.",
            "🌶️",
            55_000,
            4.9,
        ),
        product(
            5,
            "Tas Rotan Bali",
            "Fashion",
            "Anyaman rotan tangan, tali kulit asli.",
            "👜",
            275_000,
            4.7,
        ),
        product(
            6,
            "Smartwatch Fit 2",
            "Elektronik",
            "GPS, SpO₂, layar AMOLED 1.4 inci.",
            "⌚",
            899_000,
            4.5,
        ),
        product(
            7,
            "Teh Melati Slawi",
            "Makanan",
            "Teh hijau wangi melati, 100 kantong.",
            "🍵",
            32_000,
            4.6,
        ),
        product(
            8,
            "Sneakers Kanvas Lokal",
            "Fashion",
            "Sol karet vulkanisir, handmade Bandung.",
            "👟",
            349_000,
            4.7,
        ),
        product(
            9,
            "Keyboard Mekanik 75%",
            "Elektronik",
            "Hot-swap, switch tactile, RGB.",
            "⌨️",
            1_150_000,
            4.8,
        ),
        product(
            10,
            "Tanaman Monstera Mini",
            "Rumah",
            "Siap tanam dengan pot keramik putih.",
            "🪴",
            120_000,
            4.6,
        ),
        product(
            11,
            "Lilin Aromaterapi Sereh",
            "Rumah",
            "Soy wax, wangi sereh & kayu manis.",
            "🕯️",
            79_000,
            4.8,
        ),
        product(
            12,
            "Mug Gerabah Kasongan",
            "Rumah",
            "Gerabah glasir biru, 350 ml.",
            "🍶",
            65_000,
            4.5,
        ),
    ]
}

fn product(
    id: i32,
    name: &str,
    category: &str,
    description: &str,
    emoji: &str,
    price: i64,
    rating: f32,
) -> Product {
    Product {
        id,
        name: name.into(),
        category: category.into(),
        description: description.into(),
        emoji: emoji.into(),
        price,
        rating,
    }
}
