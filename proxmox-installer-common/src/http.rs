use anyhow::Result;
use rustls::ClientConfig;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use ureq::{Agent, AgentBuilder};

/// Builds an [`Agent`] with TLS suitable set up, depending whether a custom fingerprint was
/// supplied or not. If a fingerprint was supplied, only matching certificates will be accepted.
/// Otherwise, the system certificate store is loaded.
///
/// To gather the sha256 fingerprint you can use the following command:
/// ```no_compile
/// openssl s_client -connect <host>:443 < /dev/null 2>/dev/null | openssl x509 -fingerprint -sha256  -noout -in /dev/stdin
/// ```
///
/// # Arguments
/// * `fingerprint` - SHA256 cert fingerprint if certificate pinning should be used. Optional.
fn build_agent(fingerprint: Option<&str>) -> Result<Agent> {
    if let Some(fingerprint) = fingerprint {
        let tls_config = ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(VerifyCertFingerprint::new(fingerprint)?)
            .with_no_client_auth();

        Ok(AgentBuilder::new().tls_config(Arc::new(tls_config)).build())
    } else {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs()? {
            roots.add(&rustls::Certificate(cert.0)).unwrap();
        }

        let tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(roots)
            .with_no_client_auth();

        Ok(AgentBuilder::new()
            .tls_connector(Arc::new(native_tls::TlsConnector::new()?))
            .tls_config(Arc::new(tls_config))
            .build())
    }
}

/// Issues a GET request to the specified URL and fetches the response. Optionally a SHA256
/// fingerprint can be used to check the certificate against it, instead of the regular certificate
/// validation.
///
/// To gather the sha256 fingerprint you can use the following command:
/// ```no_compile
/// openssl s_client -connect <host>:443 < /dev/null 2>/dev/null | openssl x509 -fingerprint -sha256  -noout -in /dev/stdin
/// ```
///
/// # Arguments
/// * `url` - URL to fetch
/// * `fingerprint` - SHA256 cert fingerprint if certificate pinning should be used. Optional.
pub fn get(url: &str, fingerprint: Option<&str>) -> Result<String> {
    Ok(build_agent(fingerprint)?
        .get(url)
        .timeout(std::time::Duration::from_secs(60))
        .call()?
        .into_string()?)
}

/// Issues a POST request with the payload (JSON). Optionally a SHA256 fingerprint can be used to
/// check the cert against it, instead of the regular cert validation.
/// To gather the sha256 fingerprint you can use the following command:
/// ```no_compile
/// openssl s_client -connect <host>:443 < /dev/null 2>/dev/null | openssl x509 -fingerprint -sha256  -noout -in /dev/stdin
/// ```
///
/// # Arguments
/// * `url` - URL to call
/// * `fingerprint` - SHA256 cert fingerprint if certificate pinning should be used. Optional.
/// * `payload` - The payload to send to the server. Expected to be a JSON formatted string.
pub fn post(url: &str, fingerprint: Option<&str>, payload: String) -> Result<String> {
    Ok(build_agent(fingerprint)?
        .post(url)
        .set("Content-Type", "application/json; charset=utf-8")
        .timeout(std::time::Duration::from_secs(60))
        .send_string(&payload)?
        .into_string()?)
}

struct VerifyCertFingerprint {
    cert_fingerprint: Vec<u8>,
}

impl VerifyCertFingerprint {
    fn new<S: AsRef<str>>(cert_fingerprint: S) -> Result<std::sync::Arc<Self>> {
        let cert_fingerprint = cert_fingerprint.as_ref();
        let sanitized = cert_fingerprint.replace(':', "");
        let decoded = hex::decode(sanitized)?;
        Ok(std::sync::Arc::new(Self {
            cert_fingerprint: decoded,
        }))
    }
}

impl rustls::client::ServerCertVerifier for VerifyCertFingerprint {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        let mut hasher = Sha256::new();
        hasher.update(end_entity);
        let result = hasher.finalize();

        if result.as_slice() == self.cert_fingerprint {
            Ok(rustls::client::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("Fingerprint did not match!".into()))
        }
    }
}
