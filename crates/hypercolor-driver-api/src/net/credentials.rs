//! Encrypted credential storage for network device backends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use tokio::sync::RwLock;

/// Per-process temp-file suffix counter for atomic store writes.
static SAVE_COUNTER: AtomicU64 = AtomicU64::new(0);

const STORE_FILE_NAME: &str = "credentials.json.enc";
const SEED_FILE_NAME: &str = ".credential_seed";
const NONCE_BYTES: usize = 12;

/// Stored credentials for a network device/backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// Backend identifier, for example `openrgb`.
    pub backend_id: String,
    /// Backend-defined credential payload.
    pub data: Value,
}

impl Credentials {
    /// Build stored credentials from backend-owned payload data.
    #[must_use]
    pub fn new(backend_id: impl Into<String>, data: Value) -> Self {
        Self {
            backend_id: backend_id.into(),
            data,
        }
    }

    /// Convert stored credentials to the driver-facing JSON payload.
    #[must_use]
    pub fn into_driver_json(self) -> Value {
        self.data
    }

    /// Build stored credentials from a driver-facing JSON payload.
    #[must_use]
    pub fn from_driver_json(key: &str, value: Value) -> Self {
        let backend_id = key.split(':').next().unwrap_or("custom");
        Self::new(backend_id, value)
    }
}

/// Encrypted credential store rooted in Hypercolor's data directory.
pub struct CredentialStore {
    store_path: PathBuf,
    cipher: Aes256Gcm,
    cache: RwLock<HashMap<String, Credentials>>,
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

        Ok(Self {
            store_path,
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
        let store = Self {
            store_path,
            cipher,
            cache: RwLock::new(cache),
        };

        Ok(store)
    }

    /// Retrieve credentials for one key.
    pub async fn get(&self, key: &str) -> Option<Credentials> {
        self.cache.read().await.get(key).cloned()
    }

    /// Retrieve credentials as a driver-facing JSON payload.
    pub async fn get_json(&self, key: &str) -> Option<Value> {
        self.get(key).await.map(Credentials::into_driver_json)
    }

    /// Retrieve credentials as a driver-scoped JSON payload.
    pub async fn get_driver_json(&self, driver_id: &str, key: &str) -> Option<Value> {
        self.get_json(&scoped_credential_key(driver_id, key)).await
    }

    /// Store or replace credentials for one key.
    ///
    /// # Errors
    ///
    /// Returns an error if the encrypted payload cannot be persisted.
    pub async fn store(&self, key: &str, creds: Credentials) -> Result<()> {
        let snapshot = {
            let mut cache = self.cache.write().await;
            cache.insert(key.to_owned(), creds);
            cache.clone()
        };
        self.persist_snapshot(&snapshot).await
    }

    /// Store or replace credentials from a driver-facing JSON payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the encrypted payload cannot be persisted.
    pub async fn store_json(&self, key: &str, value: Value) -> Result<()> {
        self.store(key, Credentials::from_driver_json(key, value))
            .await
    }

    /// Store or replace a driver-scoped JSON credential payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the encrypted payload cannot be persisted.
    pub async fn store_driver_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.store(
            &scoped_credential_key(driver_id, key),
            Credentials::new(driver_id, value),
        )
        .await
    }

    /// Remove credentials for one key if present.
    ///
    /// # Errors
    ///
    /// Returns an error if the encrypted payload cannot be persisted.
    pub async fn remove(&self, key: &str) -> Result<()> {
        let snapshot = {
            let mut cache = self.cache.write().await;
            cache.remove(key);
            cache.clone()
        };
        self.persist_snapshot(&snapshot).await
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

    async fn persist_snapshot(&self, snapshot: &HashMap<String, Credentials>) -> Result<()> {
        let payload = encrypt_snapshot(&self.cipher, snapshot)?;

        let tmp_path = temp_store_path(&self.store_path);
        fs::write(&tmp_path, payload).await.with_context(|| {
            format!(
                "failed to write temporary credential store {}",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, &self.store_path)
            .await
            .with_context(|| {
                format!(
                    "failed to replace credential store {}",
                    self.store_path.display()
                )
            })?;

        Ok(())
    }
}

fn scoped_credential_key(driver_id: &str, key: &str) -> String {
    format!("{driver_id}:{key}")
}

fn load_or_create_seed_blocking(path: &Path) -> Result<[u8; 32]> {
    if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read credential seed {}", path.display()))?;
        if bytes.len() != 32 {
            bail!(
                "credential seed {} must be exactly 32 bytes, found {}",
                path.display(),
                bytes.len()
            );
        }

        let mut seed = [0_u8; 32];
        seed.copy_from_slice(&bytes);
        return Ok(seed);
    }

    let seed = rand::random::<[u8; 32]>();
    std::fs::write(path, seed)
        .with_context(|| format!("failed to write credential seed {}", path.display()))?;
    Ok(seed)
}

async fn load_or_create_seed(path: &Path) -> Result<[u8; 32]> {
    if path.exists() {
        let bytes = fs::read(path)
            .await
            .with_context(|| format!("failed to read credential seed {}", path.display()))?;
        if bytes.len() != 32 {
            bail!(
                "credential seed {} must be exactly 32 bytes, found {}",
                path.display(),
                bytes.len()
            );
        }

        let mut seed = [0_u8; 32];
        seed.copy_from_slice(&bytes);
        return Ok(seed);
    }

    let seed = rand::random::<[u8; 32]>();
    fs::write(path, seed)
        .await
        .with_context(|| format!("failed to write credential seed {}", path.display()))?;
    Ok(seed)
}

fn load_cache_blocking(
    cipher: &Aes256Gcm,
    store_path: &Path,
) -> Result<HashMap<String, Credentials>> {
    if !store_path.exists() {
        return Ok(HashMap::new());
    }

    let payload = std::fs::read(store_path)
        .with_context(|| format!("failed to read credential store {}", store_path.display()))?;
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

async fn load_cache(cipher: &Aes256Gcm, store_path: &Path) -> Result<HashMap<String, Credentials>> {
    if !store_path.exists() {
        return Ok(HashMap::new());
    }

    let payload = fs::read(store_path)
        .await
        .with_context(|| format!("failed to read credential store {}", store_path.display()))?;
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

fn deserialize_cache(plaintext: &[u8], store_path: &Path) -> Result<HashMap<String, Credentials>> {
    serde_json::from_slice(plaintext).with_context(|| {
        format!(
            "failed to deserialize credential store {}",
            store_path.display()
        )
    })
}

fn encrypt_snapshot(
    cipher: &Aes256Gcm,
    snapshot: &HashMap<String, Credentials>,
) -> Result<Vec<u8>> {
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

fn temp_store_path(store_path: &Path) -> PathBuf {
    let counter = SAVE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let pid = std::process::id();
    let file_name = store_path.file_name().map_or_else(
        || STORE_FILE_NAME.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    store_path.with_file_name(format!("{file_name}.tmp-{pid}-{nanos}-{counter}"))
}
