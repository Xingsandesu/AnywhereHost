use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub fn default_state_dir() -> PathBuf {
    PathBuf::from(env::var_os("HOME").expect("HOME is required"))
        .join("Library/Application Support/AnywhereHost")
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    pub relay_urls: Vec<String>,
    pub codex_home: PathBuf,
    pub codex_binary: Option<PathBuf>,
    pub control_socket: Option<PathBuf>,
    pub notifications: Option<crate::notifications::Config>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            relay_urls: vec![],
            codex_home: env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env::var_os("HOME").expect("HOME is required")).join(".codex")
                }),
            codex_binary: None,
            control_socket: None,
            notifications: None,
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
pub fn prepare(dir: &Path) -> Result<(Config, iroh::SecretKey)> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join("config.json");
    let mut config: Config = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?)?
    } else {
        let config = Config::default();
        private_write(&path, &serde_json::to_vec_pretty(&config)?)?;
        config
    };
    if !config.relay_urls.is_empty() {
        config.relay_urls = vec![];
        private_write(&path, &serde_json::to_vec_pretty(&config)?)?;
    }
    for url in &config.relay_urls {
        anywhere_transport::validate_relay(url)?;
    }
    let path = dir.join("identity.key");
    if !path.exists() {
        private_write(&path, &anywhere_transport::new_secret())?;
    }
    let bytes: [u8; 32] = fs::read(path)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid identity key"))?;
    ensure!(
        config.codex_home.is_absolute(),
        "codexHome must be absolute"
    );
    Ok((config, iroh::SecretKey::from_bytes(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_relay_replaces_saved_relays_without_changing_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (_, key) = prepare(dir.path()).unwrap();
        let path = dir.path().join("config.json");
        let mut saved: Config = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        saved.relay_urls = vec!["https://old.example.com/".into()];
        private_write(&path, &serde_json::to_vec(&saved).unwrap()).unwrap();
        let (current, same_key) = prepare(dir.path()).unwrap();
        assert_eq!(current.relay_urls, Vec::<String>::new());
        assert_eq!(key.public(), same_key.public());
        assert_eq!(current.codex_home, saved.codex_home);
        let persisted: Config = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(persisted.relay_urls, Vec::<String>::new());
    }
}
