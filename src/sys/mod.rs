use std::net::Ipv4Addr;
use std::time::Duration;

/// Результат одной ICMP-пробы.
pub struct Echo {
    pub hop: Option<Ipv4Addr>, // кто ответил (None — таймаут)
    pub rtt: Duration,
    pub reached: bool, // ответил адресат (echo reply)
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

/// Данные о скорости линка активного адаптера (Ethernet / Wi-Fi).
#[derive(Default, Clone, serde::Serialize)]
pub struct Link {
    pub kind: String, // wifi | ethernet | vpn | other
    pub name: String,
    pub rx_mbps: f64,
    pub tx_mbps: f64,
    pub duplex: String,
    pub ssid: String,
    pub signal_pct: Option<u32>,
    pub signal_dbm: Option<i32>,
    pub phy: String,
}

impl Link {
    pub fn kind_ru(&self) -> &'static str {
        match self.kind.as_str() {
            "wifi" => "Wi-Fi",
            "ethernet" => "Ethernet",
            "vpn" => "VPN",
            _ => "другое",
        }
    }
}
