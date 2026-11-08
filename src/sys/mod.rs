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
