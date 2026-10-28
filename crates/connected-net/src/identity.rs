use std::fs;
use std::io::Write;
use std::path::Path;

use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::error::{NetError, Result};
use crate::fingerprint::Fingerprint;

const CERT_FILE: &str = "identity.cert.der";
const KEY_FILE: &str = "identity.key.der";
const NAME_FILE: &str = "identity.name";

/// This device: a name people recognize and a self-signed certificate that proves it.
///
/// The certificate never expires and names no host. Peers pin its fingerprint instead of
/// validating it against anything, so only the key pair matters.
pub struct Identity {
    name: String,
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    fingerprint: Fingerprint,
}

impl Identity {
    pub fn generate(name: &str) -> Result<Self> {
        let cert_error = |err: rcgen::Error| NetError::Certificate(err.to_string());

        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(cert_error)?;
        let mut params =
            CertificateParams::new(vec!["connected.local".to_string()]).map_err(cert_error)?;
        params.distinguished_name.push(DnType::CommonName, name);
        let cert = params.self_signed(&key_pair).map_err(cert_error)?;

        let cert = cert.der().clone();
        let fingerprint = Fingerprint::of_certificate(&cert);
        Ok(Self {
            name: name.to_owned(),
            key: PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
            cert,
            fingerprint,
        })
    }

    /// Loads the identity stored in `dir`, creating and saving one on first use.
    ///
    /// `default_name` only matters on that first use; afterwards the stored name wins.
    pub fn load_or_create(dir: &Path, default_name: &str) -> Result<Self> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);

        if cert_path.exists() && key_path.exists() {
            let cert = CertificateDer::from(fs::read(&cert_path)?);
            let key = PrivatePkcs8KeyDer::from(fs::read(&key_path)?);
            let name = fs::read_to_string(dir.join(NAME_FILE))
                .map(|name| name.trim().to_owned())
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| default_name.to_owned());
            let fingerprint = Fingerprint::of_certificate(&cert);
            return Ok(Self { name, cert, key, fingerprint });
        }

        let identity = Self::generate(default_name)?;
        fs::create_dir_all(dir)?;
        write_private(&key_path, identity.key.secret_pkcs8_der())?;
        fs::write(&cert_path, &identity.cert)?;
        fs::write(dir.join(NAME_FILE), &identity.name)?;
        Ok(identity)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub(crate) fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert.clone()]
    }

    pub(crate) fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

/// Writes a secret so that only the owner can read it.
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "connected-identity-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn two_identities_differ() {
        let a = Identity::generate("a").unwrap();
        let b = Identity::generate("b").unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn identity_is_created_once_and_then_reloaded() {
        let dir = temp_dir("reload");
        let first = Identity::load_or_create(&dir, "laptop").unwrap();
        let second = Identity::load_or_create(&dir, "ignored").unwrap();

        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(second.name(), "laptop");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn private_key_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("perms");
        Identity::load_or_create(&dir, "laptop").unwrap();
        let mode = fs::metadata(dir.join(KEY_FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "key file mode is {mode:o}");
        let _ = fs::remove_dir_all(&dir);
    }
}
