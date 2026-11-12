//! Conservative, direct HTTP connectivity probes. A changed response is evidence
//! of possible interception, not proof of a captive portal rather than filtering.

use crate::state::{log, HDR, OK, WARN};
use reqwest::blocking::Client;
use reqwest::header::{CACHE_CONTROL, PRAGMA};
use reqwest::redirect::Policy;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ENDPOINTS: [(&str, &str); 2] = [
    ("gstatic", "http://www.gstatic.com/generate_204"),
    ("Cloudflare", "http://cp.cloudflare.com/generate_204"),
];
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Expected,
    Suspect(u16),
    OtherStatus(u16),
    Unavailable(&'static str),
}

fn classify(status: u16) -> Outcome {
    match status {
        204 => Outcome::Expected,
        200 | 300..=399 | 511 => Outcome::Suspect(status),
        _ => Outcome::OtherStatus(status),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Expected,
    Partial,
    Suspect,
    Inconclusive,
}

fn verdict(outcomes: &[Outcome]) -> Verdict {
    let expected = outcomes.iter().filter(|o| **o == Outcome::Expected).count();
    let suspect = outcomes.iter().any(|o| matches!(o, Outcome::Suspect(_)));
    if suspect && expected > 0 {
        Verdict::Partial
    } else if suspect {
        Verdict::Suspect
    } else if !outcomes.is_empty() && expected == outcomes.len() {
        Verdict::Expected
    } else {
        Verdict::Inconclusive
    }
}

fn client(timeout: Duration) -> Result<Client, reqwest::Error> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
}

fn nonce() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{now}-{}", SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

fn probe(client: &Client, url: &str) -> Outcome {
    // Inspect only the initial status: never read bodies or follow Location.
    match client
        .get(url)
        .query(&[("_ispcheck", nonce())])
        .header(CACHE_CONTROL, "no-cache, no-store, max-age=0")
        .header(PRAGMA, "no-cache")
        .send()
    {
        Ok(response) => classify(response.status().as_u16()),
        Err(error) => Outcome::Unavailable(if error.is_timeout() {
            "тайм-аут"
        } else if error.is_connect() {
            "ошибка соединения"
        } else {
            "ошибка HTTP-запроса"
        }),
    }
}

/// Return a concise summary suitable for Summ without modifying shared state.
/// Only two fixed public HTTP endpoints are contacted, directly and in parallel.
pub fn check() -> String {
    log(HDR, "Проверка captive portal / перехвата HTTP");
    let client = match client(TIMEOUT) {
        Ok(client) => client,
        Err(_) => {
            let summary = "Проверка HTTP не определена: не удалось создать клиент";
            log(WARN, summary);
            return summary.into();
        }
    };

    let outcomes = thread::scope(|scope| {
        let workers = ENDPOINTS.map(|(_, url)| {
            let client = &client;
            scope.spawn(move || probe(client, url))
        });
        workers.map(|worker| {
            worker
                .join()
                .unwrap_or(Outcome::Unavailable("ошибка рабочего потока"))
        })
    });

    for ((name, _), outcome) in ENDPOINTS.iter().zip(outcomes.iter()) {
        match outcome {
            Outcome::Expected => log(OK, format!("{name}: HTTP 204 (ожидаемый ответ)")),
            Outcome::Suspect(status) => log(
                WARN,
                format!("{name}: HTTP {status}, возможен перехват HTTP"),
            ),
            Outcome::OtherStatus(status) => log(
                WARN,
                format!("{name}: HTTP {status}, результат не определён"),
            ),
            Outcome::Unavailable(reason) => log(
                WARN,
                format!("{name}: {reason}, результат не определён"),
            ),
        }
    }

    let result = verdict(&outcomes);
    let summary = match result {
        Verdict::Expected => "HTTP 204: оба ответа ожидаемые; признаков перехвата не обнаружено",
        Verdict::Partial => "Возможен частичный перехват HTTP (captive portal или фильтрация)",
        Verdict::Suspect => "Возможен перехват HTTP (captive portal или фильтрация)",
        Verdict::Inconclusive => "Проверка перехвата HTTP: результат не определён",
    };
    log(if result == Verdict::Expected { OK } else { WARN }, summary);
    summary.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    #[test]
    fn status_classification_is_conservative() {
        assert_eq!(classify(204), Outcome::Expected);
        for status in [200, 300, 301, 302, 303, 304, 307, 308, 399, 511] {
            assert_eq!(classify(status), Outcome::Suspect(status));
        }
        for status in [201, 202, 205, 206, 400, 401, 403, 404, 407, 429, 500, 503] {
            assert_eq!(classify(status), Outcome::OtherStatus(status));
        }
    }

    #[test]
    fn aggregate_requires_both_expected_responses() {
        use Outcome::*;
        assert_eq!(verdict(&[Expected, Expected]), Verdict::Expected);
        assert_eq!(verdict(&[Expected, Suspect(302)]), Verdict::Partial);
        assert_eq!(verdict(&[Suspect(200), Expected]), Verdict::Partial);
        assert_eq!(verdict(&[Suspect(511), Suspect(302)]), Verdict::Suspect);
        assert_eq!(verdict(&[Suspect(200), OtherStatus(503)]), Verdict::Suspect);
        assert_eq!(verdict(&[Expected, Unavailable("timeout")]), Verdict::Inconclusive);
        assert_eq!(verdict(&[OtherStatus(403), OtherStatus(500)]), Verdict::Inconclusive);
        assert_eq!(verdict(&[]), Verdict::Inconclusive);
    }

    fn init_tls() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
            assert_eq!(stream.read(&mut byte).unwrap(), 1);
            request.push(byte[0]);
        }
        String::from_utf8(request).unwrap()
    }

    #[test]
    fn loopback_status_headers_and_no_redirect_following() {
        init_tls();
        let client = client(TIMEOUT).unwrap();
        for status in [204, 200, 302, 511, 403, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/generate_204", listener.local_addr().unwrap());
            let server_listener = listener.try_clone().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = server_listener.accept().unwrap();
                let request = read_request(&mut stream);
                // A redirect targets this same listener, so an accidental follow
                // is detected without ever contacting a public network.
                write!(stream, "HTTP/1.1 {status} Test\r\nLocation: /redirect-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                request
            });
            assert_eq!(probe(&client, &url), classify(status));
            let request = server.join().unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /generate_204?_ispcheck="));
            assert!(request.contains("cache-control: no-cache, no-store, max-age=0\r\n"));
            assert!(request.contains("pragma: no-cache\r\n"));
            listener.set_nonblocking(true).unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        }
    }

    #[test]
    fn loopback_timeout_is_inconclusive() {
        init_tls();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/generate_204", listener.local_addr().unwrap());
        let (release, wait) = mpsc::channel::<()>();
        let server = thread::spawn(move || {
            // Nonblocking accept also bounds the test if the client cannot connect.
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = wait.recv_timeout(Duration::from_secs(2));
                        drop(stream);
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        thread::yield_now();
                    }
                    Err(error) => panic!("loopback accept failed: {error}"),
                }
            }
        });
        let outcome = probe(&client(Duration::from_millis(100)).unwrap(), &url);
        let _ = release.send(());
        server.join().unwrap();
        assert_eq!(outcome, Outcome::Unavailable("тайм-аут"));
        assert_eq!(verdict(&[Outcome::Expected, outcome]), Verdict::Inconclusive);
    }

    #[test]
    fn cache_busting_values_are_distinct() {
        assert_ne!(nonce(), nonce());
    }
}
