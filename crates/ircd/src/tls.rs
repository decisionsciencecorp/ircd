//! TLS acceptor + lab self-signed cert generation.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

pub fn load_acceptor(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor> {
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("build rustls ServerConfig")?;
    // IRC clients are happy with TLS1.2+; ALPN unused for classic IRC.
    config.alpn_protocols = vec![];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path).with_context(|| format!("open cert {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse certs {}", path.display()))?;
    if certs.is_empty() {
        bail!("no certificates in {}", path.display());
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path).with_context(|| format!("open key {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut keys = rustls_pemfile::pkcs8_private_keys(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse PKCS8 key {}", path.display()))?;
    if let Some(key) = keys.pop() {
        return Ok(PrivateKeyDer::Pkcs8(key));
    }
    // Retry as traditional RSA PEM
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut keys = rustls_pemfile::rsa_private_keys(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse RSA key {}", path.display()))?;
    if let Some(key) = keys.pop() {
        return Ok(PrivateKeyDer::Pkcs1(key));
    }
    bail!("no private key found in {}", path.display());
}

/// Write a lab self-signed cert+key under `out_dir` (`cert.pem` / `key.pem`).
pub fn gen_self_signed(out_dir: &Path, common_name: &str) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("mkdir {}", out_dir.display()))?;
    let cert_path = out_dir.join("cert.pem");
    let key_path = out_dir.join("key.pem");

    let mut params = rcgen::CertificateParams::new(vec![
        common_name.to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "Decision Science Corp");

    let key_pair = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    std::fs::write(&cert_path, cert.pem()).with_context(|| format!("write {}", cert_path.display()))?;
    std::fs::write(&key_path, key_pair.serialize_pem())
        .with_context(|| format!("write {}", key_path.display()))?;

    Ok((cert_path, key_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn gen_and_load_acceptor() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempdir().unwrap();
        let (cert, key) = gen_self_signed(dir.path(), "tls.test").unwrap();
        load_acceptor(&cert, &key).unwrap();
    }
}
