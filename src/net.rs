use reqwest::blocking::ClientBuilder;

/// Android: проверка сертификатов по встроенному списку корневых CA (webpki-roots),
/// чтобы не требовалась Java-часть rustls-platform-verifier. На остальных ОС — без изменений.
#[cfg(target_os = "android")]
pub fn tls(b: ClientBuilder) -> ClientBuilder {
    use std::sync::Arc;
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("tls versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    b.tls_backend_preconfigured(cfg)
}

#[cfg(not(target_os = "android"))]
pub fn tls(b: ClientBuilder) -> ClientBuilder {
    b
}
