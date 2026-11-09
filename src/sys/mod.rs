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
    pub freq_mhz: u32,
    pub channel: u32,
    pub band: String, // "2.4" | "5" | "6"
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

impl Link {
    /// По частоте (МГц) определяет диапазон и номер канала.
    pub fn set_freq(&mut self, mhz: u32) {
        if mhz == 0 {
            return;
        }
        self.freq_mhz = mhz;
        let (band, ch) = if mhz < 2500 {
            ("2.4", if mhz == 2484 { 14 } else { mhz.saturating_sub(2407) / 5 })
        } else if mhz < 5900 {
            ("5", mhz.saturating_sub(5000) / 5)
        } else {
            ("6", mhz.saturating_sub(5950) / 5)
        };
        self.band = band.into();
        self.channel = ch;
    }

    /// «5 ГГц, канал 36»
    pub fn band_text(&self) -> String {
        if self.band.is_empty() {
            return String::new();
        }
        format!("{} ГГц, канал {}", self.band.replace('.', ","), self.channel)
    }
}

#[cfg(test)]
mod tests {
    use super::Link;

    #[test]
    fn bands() {
        for (mhz, band, ch) in [(2412, "2.4", 1), (2437, "2.4", 6), (2484, "2.4", 14), (5180, "5", 36), (5745, "5", 149), (5955, "6", 1)] {
            let mut l = Link::default();
            l.set_freq(mhz);
            assert_eq!((l.band.as_str(), l.channel), (band, ch), "{mhz}");
        }
        let mut l = Link::default();
        l.set_freq(5180);
        assert_eq!(l.band_text(), "5 ГГц, канал 36");
    }
}
