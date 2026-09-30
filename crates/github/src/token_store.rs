use std::sync::Mutex;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("credential store error: {0}")]
pub struct TokenStoreError(pub String);

/// Where the GitHub token lives between runs.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, TokenStoreError>;
    fn save(&self, token: &str) -> Result<(), TokenStoreError>;
    /// Removing a token that does not exist is not an error.
    fn clear(&self) -> Result<(), TokenStoreError>;
}

/// macOS Keychain / Windows Credential Manager.
pub struct KeyringStore {
    service: String,
    account: String,
}

impl KeyringStore {
    pub fn new(service: &str, account: &str) -> KeyringStore {
        KeyringStore {
            service: service.to_string(),
            account: account.to_string(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, TokenStoreError> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| TokenStoreError(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        match self.entry()?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(TokenStoreError(e.to_string())),
        }
    }

    fn save(&self, token: &str) -> Result<(), TokenStoreError> {
        self.entry()?
            .set_password(token)
            .map_err(|e| TokenStoreError(e.to_string()))
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(TokenStoreError(e.to_string())),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

impl MemoryStore {
    pub fn with_token(token: &str) -> MemoryStore {
        MemoryStore(Mutex::new(Some(token.to_string())))
    }
}

impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        Ok(self
            .0
            .lock()
            .map_err(|e| TokenStoreError(e.to_string()))?
            .clone())
    }

    fn save(&self, token: &str) -> Result<(), TokenStoreError> {
        *self.0.lock().map_err(|e| TokenStoreError(e.to_string()))? = Some(token.to_string());
        Ok(())
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        *self.0.lock().map_err(|e| TokenStoreError(e.to_string()))? = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::default();
        assert_eq!(s.load(), Ok(None));
        s.save("gho_1").ok();
        assert_eq!(s.load(), Ok(Some("gho_1".into())));
        s.clear().ok();
        s.clear().ok();
        assert_eq!(s.load(), Ok(None));
    }
}
