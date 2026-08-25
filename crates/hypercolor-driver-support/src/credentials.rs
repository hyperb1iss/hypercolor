//! Encrypted credential storage for network device drivers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;
use tokio::sync::RwLock;

use hypercolor_driver_api::DriverCredentialStore;
use hypercolor_persistence::AtomicFileWriter;

const STORE_FILE_NAME: &str = "credentials.json.enc";
const SEED_FILE_NAME: &str = ".credential_seed";
const NONCE_BYTES: usize = 12;
const CREDENTIAL_FILE_MODE: u32 = 0o600;

/// Encrypted credential store rooted in Hypercolor's data directory.
pub struct CredentialStore {
    store_path: PathBuf,
    writer: AtomicFileWriter,
    cipher: Aes256Gcm,
    cache: RwLock<HashMap<String, Value>>,
}

impl CredentialStore {
    /// Open or create the credential store in `data_dir` using blocking file I/O.
    ///
    /// This is intended for synchronous initialization paths such as daemon
    /// startup and scanner defaults.
    ///
    /// # Errors
    ///
    /// Returns an error if the seed cannot be created/read, the backing file
    /// cannot be decrypted, or the JSON payload is malformed.
    pub fn open_blocking(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("failed to create credential dir {}", data_dir.display()))?;

        let seed_path = data_dir.join(SEED_FILE_NAME);
        let store_path = data_dir.join(STORE_FILE_NAME);
        let key = load_or_create_seed_blocking(&seed_path)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|error| anyhow!("failed to construct credential cipher: {error}"))?;
        let cache = load_cache_blocking(&cipher, &store_path)?;
        let writer = store_writer(&store_path)?;

        Ok(Self {
            store_path,
            writer,
            cipher,
            cache: RwLock::new(cache),
        })
    }

    /// Open or create the credential store in `data_dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if the seed cannot be created/read, the backing file
    /// cannot be decrypted, or the JSON payload is malformed.
    pub async fn open(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir)
            .await
            .with_context(|| format!("failed to create credential dir {}", data_dir.display()))?;

        let seed_path = data_dir.join(SEED_FILE_NAME);
        let store_path = data_dir.join(STORE_FILE_NAME);
        let key = load_or_create_seed(&seed_path).await?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|error| anyhow!("failed to construct credential cipher: {error}"))?;
        let cache = load_cache(&cipher, &store_path).await?;
        let writer = store_writer(&store_path)?;
        let store = Self {
            store_path,
            writer,
            cipher,
            cache: RwLock::new(cache),
        };

        Ok(store)
    }

    async fn get(&self, key: &str) -> Option<Value> {
        self.cache.read().await.get(key).cloned()
    }

    async fn get_json(&self, key: &str) -> Option<Value> {
        self.get(key).await
    }

    /// Retrieve credentials as a driver-scoped JSON payload.
    pub async fn get_driver_json(&self, driver_id: &str, key: &str) -> Option<Value> {
        self.get_json(&scoped_credential_key(driver_id, key)).await
    }

    async fn store(&self, key: &str, value: Value) -> Result<()> {
        let snapshot = {
            let mut cache = self.cache.write().await;
            cache.insert(key.to_owned(), value);
            cache.clone()
        };
        self.persist_snapshot(&snapshot)
    }

    /// Store or replace a driver-scoped JSON credential payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the encrypted payload cannot be persisted.
    pub async fn store_driver_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.store(&scoped_credential_key(driver_id, key), value)
            .await
    }

    async fn remove(&self, key: &str) -> Result<()> {
        let snapshot = {
            let mut cache = self.cache.write().await;
            cache.remove(key);
            cache.clone()
        };
        self.persist_snapshot(&snapshot)
    }

    /// Remove a driver-scoped credential payload if present.
    ///
    /// # Errors
    ///
    /// Returns an error if the encrypted payload cannot be persisted.
    pub async fn remove_driver(&self, driver_id: &str, key: &str) -> Result<()> {
        self.remove(&scoped_credential_key(driver_id, key)).await
    }

    /// List all stored credential keys in deterministic order.
    pub async fn keys(&self) -> Vec<String> {
        let mut keys: Vec<_> = self.cache.read().await.keys().cloned().collect();
        keys.sort();
        keys
    }

    fn persist_snapshot(&self, snapshot: &HashMap<String, Value>) -> Result<()> {
        let payload = encrypt_snapshot(&self.cipher, snapshot)?;

        self.writer.write(&payload).with_context(|| {
            format!(
                "failed to persist credential store {}",
                self.store_path.display()
            )
        })?;

        Ok(())
    }
}

#[async_trait]
impl DriverCredentialStore for CredentialStore {
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        Ok(self.get_driver_json(driver_id, key).await)
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.store_driver_json(driver_id, key, value).await
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        self.remove_driver(driver_id, key).await
    }
}

fn store_writer(store_path: &Path) -> Result<AtomicFileWriter> {
    AtomicFileWriter::with_file_mode(store_path, CREDENTIAL_FILE_MODE).with_context(|| {
        format!(
            "failed to prepare credential store persistence at {}",
            store_path.display()
        )
    })
}

fn scoped_credential_key(driver_id: &str, key: &str) -> String {
    format!("{driver_id}:{key}")
}

fn seed_from_bytes(path: &Path, bytes: &[u8]) -> Result<[u8; 32]> {
    let seed: [u8; 32] = bytes.try_into().map_err(|_| {
        anyhow!(
            "credential seed {} must be exactly 32 bytes, found {}",
            path.display(),
            bytes.len()
        )
    })?;
    Ok(seed)
}

fn load_or_create_seed_blocking(path: &Path) -> Result<[u8; 32]> {
    if let Some(bytes) = read_secret_file_blocking(path)? {
        return seed_from_bytes(path, &bytes);
    }

    let seed = rand::random::<[u8; 32]>();
    write_secret_file_blocking(path, &seed)
        .with_context(|| format!("failed to write credential seed {}", path.display()))?;
    Ok(seed)
}

async fn load_or_create_seed(path: &Path) -> Result<[u8; 32]> {
    if let Some(bytes) = read_secret_file(path.to_path_buf()).await? {
        return seed_from_bytes(path, &bytes);
    }

    let seed = rand::random::<[u8; 32]>();
    write_secret_file(path.to_path_buf(), seed.to_vec())
        .await
        .with_context(|| format!("failed to write credential seed {}", path.display()))?;
    Ok(seed)
}

fn load_cache_blocking(cipher: &Aes256Gcm, store_path: &Path) -> Result<HashMap<String, Value>> {
    match read_secret_file_blocking(store_path)? {
        Some(payload) => decrypt_cache(cipher, store_path, &payload),
        None => Ok(HashMap::new()),
    }
}

async fn load_cache(cipher: &Aes256Gcm, store_path: &Path) -> Result<HashMap<String, Value>> {
    match read_secret_file(store_path.to_path_buf()).await? {
        Some(payload) => decrypt_cache(cipher, store_path, &payload),
        None => Ok(HashMap::new()),
    }
}

fn decrypt_cache(
    cipher: &Aes256Gcm,
    store_path: &Path,
    payload: &[u8],
) -> Result<HashMap<String, Value>> {
    if payload.is_empty() {
        return Ok(HashMap::new());
    }
    if payload.len() <= NONCE_BYTES {
        bail!("credential store {} is truncated", store_path.display());
    }

    let nonce = Nonce::from_slice(&payload[..NONCE_BYTES]);
    let plaintext = cipher
        .decrypt(nonce, &payload[NONCE_BYTES..])
        .map_err(|error| anyhow!("failed to decrypt credential store: {error}"))?;

    deserialize_cache(&plaintext, store_path)
}

fn deserialize_cache(plaintext: &[u8], store_path: &Path) -> Result<HashMap<String, Value>> {
    serde_json::from_slice(plaintext).with_context(|| {
        format!(
            "failed to deserialize credential store {}",
            store_path.display()
        )
    })
}

fn encrypt_snapshot(cipher: &Aes256Gcm, snapshot: &HashMap<String, Value>) -> Result<Vec<u8>> {
    let plaintext =
        serde_json::to_vec_pretty(snapshot).context("failed to serialize credentials")?;
    let nonce_bytes = rand::random::<[u8; NONCE_BYTES]>();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|error| anyhow!("failed to encrypt credential store: {error}"))?;

    let mut payload = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);
    Ok(payload)
}

/// Create `path` as a new private file holding `payload`.
///
/// Creation, mode, and sync all go through `hypercolor-platform-fs`, so a
/// pre-existing file or symlink at `path` is refused rather than replaced.
fn write_secret_file_blocking(path: &Path, payload: &[u8]) -> Result<()> {
    hypercolor_platform_fs::write_secret(path, payload)
        .with_context(|| format!("failed to create secret file {}", path.display()))
}

async fn write_secret_file(path: PathBuf, payload: Vec<u8>) -> Result<()> {
    tokio::task::spawn_blocking(move || write_secret_file_blocking(&path, &payload))
        .await
        .context("secret file write task was cancelled")?
}

/// Read a secret file without following a final symlink.
///
/// Returns `None` when the file does not exist. On Unix the open handle is
/// tightened to `0600` before reading so a loosened mode is repaired without
/// a path-based chmod that a symlink swap could redirect.
fn read_secret_file_blocking(path: &Path) -> Result<Option<Vec<u8>>> {
    use std::io::Read as _;

    let mut file = match hypercolor_platform_fs::open_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to open secret file {}", path.display()));
        }
    };
    restrict_file_permissions(&file)
        .with_context(|| format!("failed to restrict secret file {}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read secret file {}", path.display()))?;
    Ok(Some(bytes))
}

async fn read_secret_file(path: PathBuf) -> Result<Option<Vec<u8>>> {
    tokio::task::spawn_blocking(move || read_secret_file_blocking(&path))
        .await
        .context("secret file read task was cancelled")?
}

#[cfg(unix)]
fn restrict_file_permissions(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = file.metadata()?.permissions();
    if permissions.mode() & 0o777 == CREDENTIAL_FILE_MODE {
        return Ok(());
    }
    permissions.set_mode(CREDENTIAL_FILE_MODE);
    file.set_permissions(permissions)
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn restrict_file_permissions(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}
