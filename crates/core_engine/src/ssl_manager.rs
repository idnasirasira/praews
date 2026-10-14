use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType,
};
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};
use tokio::process::Command;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct SslManager {
    ssl_dir: PathBuf,
    certs_dir: PathBuf,
}

impl SslManager {
    pub fn new(app_root: &Path) -> Self {
        let ssl_dir = app_root.join("conf/ssl");
        let certs_dir = ssl_dir.join("certs");
        Self { ssl_dir, certs_dir }
    }

    pub fn root_ca_cert_path(&self) -> PathBuf {
        self.ssl_dir.join("rootCA.crt")
    }

    pub fn root_ca_key_path(&self) -> PathBuf {
        self.ssl_dir.join("rootCA.key")
    }

    /// Create a local Root CA once (10-year lifetime).
    pub fn ensure_root_ca(&self) -> Result<()> {
        fs::create_dir_all(&self.certs_dir)?;
        let cert_path = self.root_ca_cert_path();
        let key_path = self.root_ca_key_path();
        if cert_path.exists() && key_path.exists() {
            return Ok(());
        }

        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::OrganizationName, "praews Local Dev CA");
        dn.push(DnType::CommonName, "praews Local Root CA");
        params.distinguished_name = dn;
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(3650);

        let key_pair = KeyPair::generate().context("failed to generate Root CA key")?;
        let cert = params
            .self_signed(&key_pair)
            .context("failed to self-sign Root CA")?;

        fs::write(&cert_path, cert.pem())?;
        fs::write(&key_path, key_pair.serialize_pem())?;
        tracing::info!(path = %cert_path.display(), "Root CA created");
        Ok(())
    }

    /// Issue a leaf cert for `domain` signed by the local Root CA (≤365 days).
    pub fn generate_domain_cert(
        &self,
        domain: &str,
        lan_ip: Ipv4Addr,
    ) -> Result<(PathBuf, PathBuf)> {
        self.ensure_root_ca()?;

        let domain_cert_path = self.certs_dir.join(format!("{domain}.crt"));
        let domain_key_path = self.certs_dir.join(format!("{domain}.key"));
        if domain_cert_path.exists() && domain_key_path.exists() {
            return Ok((domain_cert_path, domain_key_path));
        }

        let ca_cert_pem = fs::read_to_string(self.root_ca_cert_path())?;
        let ca_key_pem = fs::read_to_string(self.root_ca_key_path())?;
        let ca_key_pair = KeyPair::from_pem(&ca_key_pem)?;
        let ca_params = CertificateParams::from_ca_cert_pem(&ca_cert_pem)?;
        let ca_cert = ca_params.self_signed(&ca_key_pair)?;

        let mut leaf_params = CertificateParams::default();
        leaf_params.is_ca = IsCa::NoCa;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, domain);
        dn.push(DnType::OrganizationName, "praews Local Development");
        leaf_params.distinguished_name = dn;

        leaf_params.subject_alt_names = vec![
            SanType::DnsName(domain.try_into()?),
            SanType::DnsName(format!("*.{domain}").try_into()?),
            SanType::DnsName("localhost".try_into()?),
            SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            SanType::IpAddress(IpAddr::V4(lan_ip)),
        ];
        leaf_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

        let now = OffsetDateTime::now_utc();
        leaf_params.not_before = now - Duration::days(1);
        leaf_params.not_after = now + Duration::days(365);

        let leaf_key = KeyPair::generate()?;
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &ca_cert, &ca_key_pair)
            .context("failed to sign domain certificate")?;

        fs::write(&domain_cert_path, leaf_cert.pem())?;
        fs::write(&domain_key_path, leaf_key.serialize_pem())?;
        Ok((domain_cert_path, domain_key_path))
    }

    /// Install Root CA into the current-user trust store (optional; reversible).
    pub async fn trust_root_ca_in_os(&self) -> Result<()> {
        self.ensure_root_ca()?;
        let cert_path = self.root_ca_cert_path();

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let mut cmd = Command::new("certutil");
            cmd.args(["-user", "-addstore", "Root", &cert_path.to_string_lossy()]);
            cmd.creation_flags(CREATE_NO_WINDOW);
            let output = cmd.output().await?;
            if !output.status.success() {
                anyhow::bail!("failed to install Root CA into Windows user store");
            }
        }

        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").unwrap_or_default();
            let login_keychain = format!("{home}/Library/Keychains/login.keychain-db");
            let _ = Command::new("security")
                .args([
                    "add-trusted-cert",
                    "-r",
                    "trustRoot",
                    "-k",
                    &login_keychain,
                    &cert_path.to_string_lossy(),
                ])
                .output()
                .await?;
        }

        Ok(())
    }

    /// Remove Root CA from the OS trust store (cleanup before deleting the app folder).
    pub async fn untrust_root_ca_from_os(&self) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let mut cmd = Command::new("certutil");
            cmd.args(["-user", "-delstore", "Root", "praews Local Root CA"]);
            cmd.creation_flags(CREATE_NO_WINDOW);
            let _ = cmd.output().await;
        }

        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("security")
                .args(["delete-certificate", "-c", "praews Local Root CA"])
                .output()
                .await;
        }

        Ok(())
    }
}
