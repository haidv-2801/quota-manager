use crate::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "quota-manager";

/// Where API keys live. Keys never go into SQLite or to the UI.
pub trait Vault: Send + Sync {
    fn set(&self, account_id: &str, key: &SecretString) -> Result<()>;
    fn get(&self, account_id: &str) -> Result<Option<SecretString>>;
    fn delete(&self, account_id: &str) -> Result<()>;
}

/// Windows Credential Manager on Windows (via the `keyring` crate).
pub struct KeyringVault;

impl KeyringVault {
    fn entry(account_id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &format!("app/{account_id}")).map_err(|e| Error::Vault(e.to_string()))
    }
}

impl Vault for KeyringVault {
    fn set(&self, account_id: &str, key: &SecretString) -> Result<()> {
        Self::entry(account_id)?
            .set_password(key.expose_secret())
            .map_err(|e| Error::Vault(e.to_string()))
    }

    fn get(&self, account_id: &str) -> Result<Option<SecretString>> {
        match Self::entry(account_id)?.get_password() {
            Ok(p) => Ok(Some(SecretString::from(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Vault(e.to_string())),
        }
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        match Self::entry(account_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Vault(e.to_string())),
        }
    }
}

/// In-memory vault for tests.
#[derive(Default)]
pub struct MemoryVault(Mutex<HashMap<String, String>>);

impl Vault for MemoryVault {
    fn set(&self, account_id: &str, key: &SecretString) -> Result<()> {
        self.0.lock().unwrap().insert(account_id.into(), key.expose_secret().into());
        Ok(())
    }

    fn get(&self, account_id: &str) -> Result<Option<SecretString>> {
        Ok(self.0.lock().unwrap().get(account_id).map(|s| SecretString::from(s.clone())))
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        self.0.lock().unwrap().remove(account_id);
        Ok(())
    }
}
