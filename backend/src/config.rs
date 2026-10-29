use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    net::SocketAddr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub codex_home: PathBuf,
    pub codex_binary: Option<PathBuf>,
    pub control_socket: Option<PathBuf>,
    pub frpc_binary: Option<PathBuf>,
    pub apns: Option<crate::apns::Config>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:9443".parse().unwrap(),
            codex_home: env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env::var_os("HOME").expect("HOME is required")).join(".codex")
                }),
            codex_binary: None,
            control_socket: None,
            frpc_binary: None,
            apns: None,
        }
    }
}
pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub fn prepare(dir: &Path) -> Result<(Config, String, String)> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join("config.json");
    let config = if path.exists() {
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        let config = Config::default();
        private_write(&path, &serde_json::to_vec_pretty(&config)?)?;
        config
    };
    let token_path = dir.join("access-token");
    if !token_path.exists() {
        let mut bytes = [0u8; 32];
        use std::io::Read;
        fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&token_path)?;
        file.write_all(hex::encode(bytes).as_bytes())?;
        file.sync_all()?;
    }
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))?;
    let token = fs::read_to_string(token_path)?.trim().to_owned();
    ensure!(
        token.len() == 64 && token.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid access-token"
    );
    let cert = dir.join("host-cert.pem");
    let key = dir.join("host-key.pem");
    ensure!(
        cert.exists() == key.exists(),
        "certificate and key must exist together"
    );
    if !cert.exists() {
        let temporary = tempfile::tempdir_in(dir)?;
        let openssl_config = temporary.path().join("openssl.cnf");
        fs::write(
            &openssl_config,
            "[req]\ndistinguished_name=dn\nx509_extensions=v3\nprompt=no\n[dn]\nCN=AnywhereHost\n[v3]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n",
        )?;
        let status = std::process::Command::new("/usr/bin/openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:3072",
                "-sha256",
                "-days",
                "3650",
                "-nodes",
                "-subj",
                "/CN=AnywhereHost",
            ])
            .arg("-config")
            .arg(&openssl_config)
            .arg("-keyout")
            .arg(temporary.path().join("key.pem"))
            .arg("-out")
            .arg(temporary.path().join("cert.pem"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        ensure!(status.success(), "TLS certificate generation failed");
        private_write(&key, &fs::read(temporary.path().join("key.pem"))?)?;
        private_write(&cert, &fs::read(temporary.path().join("cert.pem"))?)?;
    }
    let output = std::process::Command::new("/usr/bin/openssl")
        .args(["x509", "-outform", "DER", "-in"])
        .arg(cert)
        .output()?;
    ensure!(output.status.success(), "invalid TLS certificate");
    let fingerprint = hex::encode(Sha256::digest(&output.stdout));
    Ok((config, token, fingerprint))
}
