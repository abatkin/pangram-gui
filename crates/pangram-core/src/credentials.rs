//! API key storage. The desktop uses the Linux Secret Service; if it is unavailable the key is
//! kept for the session only. Keys are never written to plaintext files.

use std::collections::HashMap;
use std::sync::Mutex;

use secret_service::{EncryptionType, SecretService};

use crate::api::ApiKey;

const ATTR_APP: (&str, &str) = ("application", "net.batkin.pangram-desktop");
/// Value of the `kind` attribute identifying the app's key item.
pub const DEFAULT_KIND: &str = "pangram-api-key";
const LABEL: &str = "Pangram API key";

#[derive(Debug, Clone, thiserror::Error)]
#[error("Secret Service unavailable: {0}")]
pub struct CredentialError(pub String);

fn attrs(kind: &str) -> HashMap<&str, &str> {
    HashMap::from([ATTR_APP, ("kind", kind)])
}

fn err(e: impl std::fmt::Display) -> CredentialError {
    CredentialError(e.to_string())
}

pub enum CredentialStore {
    /// Linux Secret Service item identified by `kind` (normally [`DEFAULT_KIND`]).
    SecretService { kind: String },
    /// For tests and environments without a keyring.
    Memory(Mutex<Option<String>>),
    /// A store whose every operation fails with this message (for tests).
    Unavailable(String),
}

impl CredentialStore {
    pub async fn load(&self) -> Result<Option<ApiKey>, CredentialError> {
        match self {
            Self::Memory(m) => Ok(m.lock().unwrap().as_deref().and_then(ApiKey::new)),
            Self::Unavailable(msg) => Err(CredentialError(msg.clone())),
            Self::SecretService { kind } => {
                let ss = SecretService::connect(EncryptionType::Dh)
                    .await
                    .map_err(err)?;
                let found = ss.search_items(attrs(kind)).await.map_err(err)?;
                let item = match (found.unlocked.first(), found.locked.first()) {
                    (Some(item), _) => item,
                    (None, Some(item)) => {
                        item.unlock().await.map_err(err)?;
                        item
                    }
                    (None, None) => return Ok(None),
                };
                let secret = item.get_secret().await.map_err(err)?;
                Ok(String::from_utf8(secret)
                    .ok()
                    .as_deref()
                    .and_then(ApiKey::new))
            }
        }
    }

    pub async fn save(&self, key: &ApiKey) -> Result<(), CredentialError> {
        match self {
            Self::Memory(m) => {
                *m.lock().unwrap() = Some(key.expose().to_owned());
                Ok(())
            }
            Self::Unavailable(msg) => Err(CredentialError(msg.clone())),
            Self::SecretService { kind } => {
                let ss = SecretService::connect(EncryptionType::Dh)
                    .await
                    .map_err(err)?;
                let collection = ss.get_default_collection().await.map_err(err)?;
                collection.ensure_unlocked().await.map_err(err)?;
                collection
                    .create_item(
                        LABEL,
                        attrs(kind),
                        key.expose().as_bytes(),
                        true,
                        "text/plain",
                    )
                    .await
                    .map_err(err)?;
                Ok(())
            }
        }
    }

    pub async fn delete(&self) -> Result<(), CredentialError> {
        match self {
            Self::Memory(m) => {
                *m.lock().unwrap() = None;
                Ok(())
            }
            Self::Unavailable(msg) => Err(CredentialError(msg.clone())),
            Self::SecretService { kind } => {
                let ss = SecretService::connect(EncryptionType::Dh)
                    .await
                    .map_err(err)?;
                let found = ss.search_items(attrs(kind)).await.map_err(err)?;
                for item in found.unlocked.iter().chain(found.locked.iter()) {
                    item.delete().await.map_err(err)?;
                }
                Ok(())
            }
        }
    }
}

impl CredentialStore {
    pub fn secret_service() -> Self {
        Self::SecretService {
            kind: DEFAULT_KIND.to_owned(),
        }
    }
}
