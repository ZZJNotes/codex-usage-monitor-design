//! Keychain-only credential management for managed ChatGPT/Codex accounts.
//!
//! Secrets stay in Keychain (or an injectable fake store). SQLite holds only
//! non-secret account metadata, delete intents, and aliases.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::database::Database;

pub const CREDENTIAL_SCHEMA_VERSION: u32 = 1;
pub const KEYCHAIN_SERVICE: &str = "com.codex-usage-monitor.managed-account";

/// Minimal Keychain payload — access/id tokens never persist.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialEnvelope {
    pub schema_version: u32,
    pub generation: u64,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedAccountStatus {
    PendingAuthorization,
    Active,
    ReauthorizationRequired,
    CredentialDeleted,
    Deleting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMode {
    CredentialsOnly,
    CredentialsAndHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeletePhase {
    IntentWritten,
    KeychainDeleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteIntent {
    pub mode: DeleteMode,
    pub phase: DeletePhase,
}

/// Non-secret account registry row exposed over IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAccountRecord {
    pub account_id: String,
    pub alias: String,
    pub identity_fingerprint: String,
    pub plan_type: String,
    pub status: ManagedAccountStatus,
    pub pinned: bool,
    pub delete_intent: Option<DeleteIntent>,
    pub created_at: String,
    pub updated_at: String,
}

/// Secret-free account listing DTO for the React UI.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredAccount {
    pub account_key: String,
    pub display_name: String,
    pub auth_source: String,
    pub is_managed: bool,
    pub status: Option<ManagedAccountStatus>,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialStoreError {
    NotFound,
    Conflict,
    Locked,
    Io(String),
}

impl std::fmt::Display for CredentialStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "credential not found"),
            Self::Conflict => write!(f, "credential generation conflict"),
            Self::Locked => write!(f, "keychain locked or denied"),
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

pub trait CredentialStore: Send + Sync {
    fn load(&self, account_id: &str) -> Result<CredentialEnvelope, CredentialStoreError>;
    fn compare_and_swap(
        &self,
        account_id: &str,
        expected_generation: Option<u64>,
        next: CredentialEnvelope,
    ) -> Result<(), CredentialStoreError>;
    fn delete(&self, account_id: &str) -> Result<(), CredentialStoreError>;
}

/// In-memory store for unit tests and non-macOS fallbacks.
#[derive(Default)]
pub struct InMemoryCredentialStore {
    items: Mutex<HashMap<String, CredentialEnvelope>>,
}

impl CredentialStore for InMemoryCredentialStore {
    fn load(&self, account_id: &str) -> Result<CredentialEnvelope, CredentialStoreError> {
        self.items
            .lock()
            .expect("credential store poisoned")
            .get(account_id)
            .cloned()
            .ok_or(CredentialStoreError::NotFound)
    }

    fn compare_and_swap(
        &self,
        account_id: &str,
        expected_generation: Option<u64>,
        next: CredentialEnvelope,
    ) -> Result<(), CredentialStoreError> {
        let mut items = self.items.lock().expect("credential store poisoned");
        match (expected_generation, items.get(account_id)) {
            (None, Some(_)) => Err(CredentialStoreError::Conflict),
            (None, None) => {
                items.insert(account_id.to_string(), next);
                Ok(())
            }
            (Some(expected), Some(current)) if current.generation == expected => {
                items.insert(account_id.to_string(), next);
                Ok(())
            }
            (Some(_), Some(_)) => Err(CredentialStoreError::Conflict),
            (Some(_), None) => Err(CredentialStoreError::NotFound),
        }
    }

    fn delete(&self, account_id: &str) -> Result<(), CredentialStoreError> {
        let mut items = self.items.lock().expect("credential store poisoned");
        if items.remove(account_id).is_some() {
            Ok(())
        } else {
            // Idempotent delete: missing item is success for saga recovery.
            Ok(())
        }
    }
}

/// Production Keychain-backed store (single item per account_id).
pub struct KeychainCredentialStore;

impl KeychainCredentialStore {
    fn entry(account_id: &str) -> Result<keyring::Entry, CredentialStoreError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|error| match error {
            keyring::Error::NoEntry => CredentialStoreError::NotFound,
            other => CredentialStoreError::Io(other.to_string()),
        })
    }

    fn map_error(error: keyring::Error) -> CredentialStoreError {
        match error {
            keyring::Error::NoEntry => CredentialStoreError::NotFound,
            keyring::Error::Ambiguous(_) => CredentialStoreError::Conflict,
            other => {
                let message = other.to_string().to_ascii_lowercase();
                if message.contains("locked")
                    || message.contains("denied")
                    || message.contains("auth")
                    || message.contains("user interaction")
                {
                    CredentialStoreError::Locked
                } else {
                    CredentialStoreError::Io(other.to_string())
                }
            }
        }
    }
}

impl CredentialStore for KeychainCredentialStore {
    fn load(&self, account_id: &str) -> Result<CredentialEnvelope, CredentialStoreError> {
        let entry = Self::entry(account_id)?;
        let secret = entry.get_password().map_err(Self::map_error)?;
        serde_json::from_str(&secret).map_err(|error| CredentialStoreError::Io(error.to_string()))
    }

    fn compare_and_swap(
        &self,
        account_id: &str,
        expected_generation: Option<u64>,
        next: CredentialEnvelope,
    ) -> Result<(), CredentialStoreError> {
        let entry = Self::entry(account_id)?;
        let current = match entry.get_password() {
            Ok(secret) => Some(
                serde_json::from_str::<CredentialEnvelope>(&secret)
                    .map_err(|error| CredentialStoreError::Io(error.to_string()))?,
            ),
            Err(keyring::Error::NoEntry) => None,
            Err(error) => return Err(Self::map_error(error)),
        };
        match (expected_generation, current.as_ref()) {
            (None, Some(_)) => return Err(CredentialStoreError::Conflict),
            (Some(expected), Some(current)) if current.generation != expected => {
                return Err(CredentialStoreError::Conflict);
            }
            (Some(_), None) => return Err(CredentialStoreError::NotFound),
            _ => {}
        }
        let payload = serde_json::to_string(&next)
            .map_err(|error| CredentialStoreError::Io(error.to_string()))?;
        entry.set_password(&payload).map_err(Self::map_error)
    }

    fn delete(&self, account_id: &str) -> Result<(), CredentialStoreError> {
        let entry = Self::entry(account_id)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Self::map_error(error)),
        }
    }
}

pub fn identity_fingerprint(openai_account_id: &str) -> String {
    let digest = Sha256::digest(openai_account_id.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn new_account_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    // UUID-ish stable local key (not derived from email).
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub struct CredentialService {
    database: Database,
    store: Arc<dyn CredentialStore>,
    account_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl CredentialService {
    pub fn new(database: Database, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            database,
            store,
            account_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn production(database: Database) -> Self {
        Self::new(database, Arc::new(KeychainCredentialStore))
    }

    pub fn store(&self) -> Arc<dyn CredentialStore> {
        self.store.clone()
    }

    fn lock_for(&self, account_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.account_locks.lock().expect("account locks poisoned");
        locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub fn list_managed(&self) -> Result<Vec<ManagedAccountRecord>, String> {
        self.database.list_managed_accounts()
    }

    pub fn discover_accounts(&self) -> Vec<DiscoveredAccount> {
        self.list_managed()
            .unwrap_or_default()
            .into_iter()
            .filter(|account| account.status != ManagedAccountStatus::Deleting)
            .map(|account| DiscoveredAccount {
                account_key: account.account_id.clone(),
                display_name: account.alias.clone(),
                auth_source: "managed".to_string(),
                is_managed: true,
                status: Some(account.status),
                pinned: account.pinned,
            })
            .collect()
    }

    pub fn begin_pending_account(&self, alias: &str) -> Result<ManagedAccountRecord, String> {
        let account_id = new_account_id();
        let now = Utc::now().to_rfc3339();
        let record = ManagedAccountRecord {
            account_id: account_id.clone(),
            alias: if alias.trim().is_empty() {
                format!("Account · {}", &account_id[..8])
            } else {
                alias.trim().to_string()
            },
            identity_fingerprint: String::new(),
            plan_type: "unknown".to_string(),
            status: ManagedAccountStatus::PendingAuthorization,
            pinned: false,
            delete_intent: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.database.upsert_managed_account(&record)?;
        Ok(record)
    }

    pub fn complete_authorization(
        &self,
        account_id: &str,
        openai_account_id: &str,
        alias: &str,
        plan_type: &str,
        refresh_token: &str,
    ) -> Result<ManagedAccountRecord, String> {
        let lock = self.lock_for(account_id);
        let _guard = lock.lock().expect("account lock poisoned");
        let fingerprint = identity_fingerprint(openai_account_id);
        let envelope = CredentialEnvelope {
            schema_version: CREDENTIAL_SCHEMA_VERSION,
            generation: 1,
            refresh_token: refresh_token.to_string(),
        };
        self.store
            .compare_and_swap(account_id, None, envelope)
            .map_err(|error| error.to_string())?;
        let mut record = self
            .database
            .get_managed_account(account_id)?
            .ok_or_else(|| format!("managed account '{account_id}' not found"))?;
        record.identity_fingerprint = fingerprint;
        if !alias.trim().is_empty() {
            record.alias = alias.trim().to_string();
        }
        if !plan_type.trim().is_empty() {
            record.plan_type = plan_type.trim().to_string();
        }
        record.status = ManagedAccountStatus::Active;
        record.updated_at = Utc::now().to_rfc3339();
        self.database.upsert_managed_account(&record)?;
        Ok(record)
    }

    pub fn mark_reauthorization_required(&self, account_id: &str) -> Result<(), String> {
        let mut record = self
            .database
            .get_managed_account(account_id)?
            .ok_or_else(|| format!("managed account '{account_id}' not found"))?;
        record.status = ManagedAccountStatus::ReauthorizationRequired;
        record.updated_at = Utc::now().to_rfc3339();
        self.database.upsert_managed_account(&record)
    }

    pub fn rotate_refresh_token(
        &self,
        account_id: &str,
        expected_generation: u64,
        refresh_token: &str,
    ) -> Result<CredentialEnvelope, String> {
        let lock = self.lock_for(account_id);
        let _guard = lock.lock().expect("account lock poisoned");
        let next = CredentialEnvelope {
            schema_version: CREDENTIAL_SCHEMA_VERSION,
            generation: expected_generation.saturating_add(1),
            refresh_token: refresh_token.to_string(),
        };
        match self
            .store
            .compare_and_swap(account_id, Some(expected_generation), next.clone())
        {
            Ok(()) => Ok(next),
            Err(CredentialStoreError::Conflict) => Err("credential generation conflict".into()),
            Err(CredentialStoreError::Locked) => Err("keychain locked or denied".into()),
            Err(error) => {
                // Fail-closed: upstream may have rotated; require reauthorization.
                let _ = self.mark_reauthorization_required(account_id);
                Err(error.to_string())
            }
        }
    }

    pub fn load_envelope(
        &self,
        account_id: &str,
    ) -> Result<CredentialEnvelope, CredentialStoreError> {
        self.store.load(account_id)
    }

    pub fn set_alias(&self, account_id: &str, alias: &str) -> Result<ManagedAccountRecord, String> {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            return Err("alias must not be empty".into());
        }
        let mut record = self
            .database
            .get_managed_account(account_id)?
            .ok_or_else(|| format!("managed account '{account_id}' not found"))?;
        record.alias = trimmed.to_string();
        record.updated_at = Utc::now().to_rfc3339();
        self.database.upsert_managed_account(&record)?;
        Ok(record)
    }

    pub fn set_pinned(
        &self,
        account_id: &str,
        pinned: bool,
    ) -> Result<ManagedAccountRecord, String> {
        if pinned {
            self.database.clear_managed_account_pins()?;
        }
        let mut record = self
            .database
            .get_managed_account(account_id)?
            .ok_or_else(|| format!("managed account '{account_id}' not found"))?;
        record.pinned = pinned;
        record.updated_at = Utc::now().to_rfc3339();
        self.database.upsert_managed_account(&record)?;
        Ok(record)
    }

    /// Delete saga: intent → Keychain → metadata/history cleanup.
    pub fn delete_account(&self, account_id: &str, mode: DeleteMode) -> Result<(), String> {
        let lock = self.lock_for(account_id);
        let _guard = lock.lock().expect("account lock poisoned");
        let mut record = self
            .database
            .get_managed_account(account_id)?
            .ok_or_else(|| format!("managed account '{account_id}' not found"))?;

        record.status = ManagedAccountStatus::Deleting;
        record.pinned = false;
        record.delete_intent = Some(DeleteIntent {
            mode,
            phase: DeletePhase::IntentWritten,
        });
        record.updated_at = Utc::now().to_rfc3339();
        self.database.upsert_managed_account(&record)?;

        self.store
            .delete(account_id)
            .map_err(|error| error.to_string())?;

        record.delete_intent = Some(DeleteIntent {
            mode,
            phase: DeletePhase::KeychainDeleted,
        });
        record.updated_at = Utc::now().to_rfc3339();
        self.database.upsert_managed_account(&record)?;

        match mode {
            DeleteMode::CredentialsOnly => {
                record.status = ManagedAccountStatus::CredentialDeleted;
                record.delete_intent = None;
                record.updated_at = Utc::now().to_rfc3339();
                self.database.upsert_managed_account(&record)?;
            }
            DeleteMode::CredentialsAndHistory => {
                // History cleanup is performed by the caller via DataGovernanceService
                // after this returns Ok, then the managed row is removed.
                record.delete_intent = None;
                record.updated_at = Utc::now().to_rfc3339();
                self.database.upsert_managed_account(&record)?;
                self.database.delete_managed_account(account_id)?;
            }
        }
        Ok(())
    }

    pub fn reconcile_on_startup(&self) -> Result<(), String> {
        for account in self.list_managed()? {
            if account.status == ManagedAccountStatus::PendingAuthorization {
                match self.store.load(&account.account_id) {
                    Ok(_) => {
                        let mut record = account;
                        record.status = ManagedAccountStatus::Active;
                        record.updated_at = Utc::now().to_rfc3339();
                        self.database.upsert_managed_account(&record)?;
                    }
                    Err(CredentialStoreError::NotFound) => {
                        self.database.delete_managed_account(&account.account_id)?;
                    }
                    Err(_) => {
                        let mut record = account;
                        record.status = ManagedAccountStatus::ReauthorizationRequired;
                        record.updated_at = Utc::now().to_rfc3339();
                        self.database.upsert_managed_account(&record)?;
                    }
                }
            } else if account.status == ManagedAccountStatus::Deleting {
                if let Some(intent) = account.delete_intent.clone() {
                    let _ = self.delete_account(&account.account_id, intent.mode);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_creates_and_rotates_isolated_accounts() {
        let store = InMemoryCredentialStore::default();
        let first = CredentialEnvelope {
            schema_version: 1,
            generation: 1,
            refresh_token: "rt-a".into(),
        };
        store.compare_and_swap("a", None, first.clone()).unwrap();
        assert_eq!(store.load("a").unwrap(), first);
        let rotated = CredentialEnvelope {
            schema_version: 1,
            generation: 2,
            refresh_token: "rt-a2".into(),
        };
        store
            .compare_and_swap("a", Some(1), rotated.clone())
            .unwrap();
        assert_eq!(store.load("a").unwrap(), rotated);
        assert!(matches!(
            store.compare_and_swap("a", Some(1), rotated.clone()),
            Err(CredentialStoreError::Conflict)
        ));
        assert!(matches!(
            store.load("b"),
            Err(CredentialStoreError::NotFound)
        ));
    }

    #[test]
    fn delete_is_idempotent() {
        let store = InMemoryCredentialStore::default();
        store.delete("missing").unwrap();
        store
            .compare_and_swap(
                "a",
                None,
                CredentialEnvelope {
                    schema_version: 1,
                    generation: 1,
                    refresh_token: "rt".into(),
                },
            )
            .unwrap();
        store.delete("a").unwrap();
        store.delete("a").unwrap();
    }

    #[test]
    fn managed_account_registry_round_trips_without_secrets() {
        let database = Database::in_memory().unwrap();
        let service =
            CredentialService::new(database, Arc::new(InMemoryCredentialStore::default()));
        let pending = service.begin_pending_account("Work").unwrap();
        let completed = service
            .complete_authorization(
                &pending.account_id,
                "openai-user-1",
                "Work",
                "plus",
                "refresh-secret",
            )
            .unwrap();
        assert_eq!(completed.status, ManagedAccountStatus::Active);
        assert!(!completed.identity_fingerprint.is_empty());
        let listed = service.discover_accounts();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].display_name, "Work");
        let dto = serde_json::to_string(&listed[0]).unwrap();
        assert!(!dto.contains("refresh-secret"));
        assert!(!dto.contains("token"));
    }

    #[test]
    fn credentials_only_delete_keeps_stub() {
        let database = Database::in_memory().unwrap();
        let service =
            CredentialService::new(database, Arc::new(InMemoryCredentialStore::default()));
        let pending = service.begin_pending_account("A").unwrap();
        service
            .complete_authorization(&pending.account_id, "oa-1", "A", "plus", "rt")
            .unwrap();
        service
            .delete_account(&pending.account_id, DeleteMode::CredentialsOnly)
            .unwrap();
        let record = service
            .list_managed()
            .unwrap()
            .into_iter()
            .find(|row| row.account_id == pending.account_id)
            .unwrap();
        assert_eq!(record.status, ManagedAccountStatus::CredentialDeleted);
        assert!(matches!(
            service.load_envelope(&pending.account_id),
            Err(CredentialStoreError::NotFound)
        ));
    }
}
