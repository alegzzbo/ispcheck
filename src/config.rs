// ===== НАСТРОЙКИ ПРОВАЙДЕРА =====
pub const ISP_NAME: &str = "Point Online";
pub const OST_URL: &str = "https://speedtest.point.online"; // OpenSpeedTest, без / в конце
pub const SPEED_SECONDS: u64 = 12; // длительность каждого замера
pub const THREADS: usize = 8; // параллельных потоков
pub const ENABLE_IPV6: bool = false;

pub const MTR_CYCLES: u32 = 20;
pub const MTR_MAX_HOPS: usize = 30;
pub const MTR_DEFAULT_HOST: &str = "8.8.8.8";

// Автообновление: latest.json + публичный ключ ed25519 (base64)
pub const UPDATE_URL: &str = "https://support.example.com/update/latest.json";
pub const UPDATE_PUBKEY: &str = "5blDiOdrT6GAvS3STrGOYEPxnJnGXRXlpymhvqWH/3Y=";

// Приём отчётов
pub const REPORT_URL: &str = "https://support.example.com/api/report";
pub const REPORT_TOKEN: &str = "CHANGE_ME";

pub const ISP_HOSTS: &[&str] = &["speedtest.point.online", "point.online", "yandex.ru"];
pub const SITES: &[&str] = &[
    "https://www.google.com",
    "https://www.wikipedia.org",
    "https://vk.ru",
    "https://yandex.ru",
];
pub const DNS_DOMAINS: &[&str] = &["google.com", "wikipedia.org", "yandex.ru", "vk.ru"];
pub const PUBLIC_DNS: &[(&str, &str)] = &[
    ("Cloudflare 1.1.1.1", "1.1.1.1"),
    ("Google 8.8.8.8", "8.8.8.8"),
    ("Yandex 77.88.8.8", "77.88.8.8"),
];
// DoH (RFC 8484): имя, URL, IP для прямого подключения без системного DNS
pub const DOH_SERVERS: &[(&str, &str, &str)] = &[
    ("Cloudflare DoH", "https://cloudflare-dns.com/dns-query", "1.1.1.1"),
    ("Google DoH", "https://dns.google/dns-query", "8.8.8.8"),
    ("Yandex DoH", "https://common.dot.dns.yandex.net/dns-query", "77.88.8.8"),
];
