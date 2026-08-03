//! TLS acceptor + lab self-signed cert generation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use crate::fs_perms::{create_private_file, ensure_private_dir, ensure_private_file};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

pub fn load_acceptor(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor> {
    ensure_private_file(key_path)?;
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
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("open cert {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse certs {}", path.display()))?;
    if certs.is_empty() {
        bail!("no certificates in {}", path.display());
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .with_context(|| format!("parse private key {}", path.display()))
}

/// Write a lab self-signed cert+key under `out_dir` (`cert.pem` / `key.pem`).
pub fn gen_self_signed(out_dir: &Path, common_name: &str) -> Result<(PathBuf, PathBuf)> {
    ensure_private_dir(out_dir)?;
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

    {
        use std::io::Write;
        let mut f = create_private_file(&cert_path)?;
        f.write_all(cert.pem().as_bytes())?;
        let mut f = create_private_file(&key_path)?;
        f.write_all(key_pair.serialize_pem().as_bytes())?;
    }

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
