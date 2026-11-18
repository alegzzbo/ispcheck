use objc2_core_wlan::{CWChannelBand, CWPHYMode, CWWiFiClient};

/// Текущее Wi-Fi-подключение через CoreWLAN.
pub struct MacWifi {
    pub iface: String,
    pub ssid: String, // пусто, если у процесса нет доступа к геолокации (macOS 14+)
    pub rssi: i32,    // дБм
    pub rate: f64,    // Мбит/с
    pub channel: u32,
    pub band: &'static str, // "2.4" | "5" | "6" | ""
    pub phy: &'static str,
}

pub fn wifi() -> Option<MacWifi> {
    unsafe {
        let iface = CWWiFiClient::sharedWiFiClient().interface()?;
        if !iface.powerOn() {
            return None;
        }
        let name = iface.interfaceName()?.to_string();
        let (rssi, rate) = (iface.rssiValue() as i32, iface.transmitRate());
        if rssi == 0 && rate == 0.0 {
            return None; // не подключён к сети
        }
        let (channel, band) = match iface.wlanChannel() {
            Some(ch) => {
                let b = ch.channelBand();
                let band = if b == CWChannelBand::Band2GHz {
                    "2.4"
                } else if b == CWChannelBand::Band5GHz {
                    "5"
                } else if b == CWChannelBand::Band6GHz {
                    "6"
                } else {
                    ""
                };
                (ch.channelNumber() as u32, band)
            }
            None => (0, ""),
        };
        let p = iface.activePHYMode();
        let phy = if p == CWPHYMode::Mode11ax {
            "802.11ax (Wi-Fi 6)"
        } else if p == CWPHYMode::Mode11ac {
            "802.11ac (Wi-Fi 5)"
        } else if p == CWPHYMode::Mode11n {
            "802.11n (Wi-Fi 4)"
        } else if p == CWPHYMode::Mode11g {
            "802.11g"
        } else if p == CWPHYMode::Mode11a {
            "802.11a"
        } else if p == CWPHYMode::Mode11b {
            "802.11b"
        } else {
            ""
        };
        Some(MacWifi {
            iface: name,
            ssid: iface.ssid().map(|s| s.to_string()).unwrap_or_default(),
            rssi,
            rate,
            channel,
            band,
            phy,
        })
    }
}
