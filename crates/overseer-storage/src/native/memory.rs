//! A credential store in memory for tests (feature `test-support`). [`install`] makes it the
//! default credential builder of `keyring`, so that a test never touches the OS credential
//! store of the user.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once, PoisonError};

use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};

/// The secrets of the in-memory credential store, by service and key.
type Secrets = Arc<Mutex<HashMap<(String, String), Vec<u8>>>>;

/// Makes [`MemoryStore`] the default credential builder of `keyring` for this process. Call it
/// before the first secret operation. More calls do nothing, so the secrets stay.
pub fn install() {
    static MEMORY: Once = Once::new();
    MEMORY.call_once(|| {
        keyring::set_default_credential_builder(Box::new(MemoryStore::default()));
    });
}

/// A credential store in memory that keeps its secrets across entries (the mock store of
/// `keyring` keeps a secret in its entry only).
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: Secrets,
}

/// One credential of [`MemoryStore`].
#[derive(Debug)]
struct MemoryCredential {
    secrets: Secrets,
    id: (String, String),
}

impl CredentialApi for MemoryCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.id.clone(), secret.to_vec());
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.id)
            .cloned()
            .ok_or(keyring::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id)
            .map(drop)
            .ok_or(keyring::Error::NoEntry)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CredentialBuilderApi for MemoryStore {
    fn build(
        &self,
        _target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<Credential>> {
        Ok(Box::new(MemoryCredential {
            secrets: Arc::clone(&self.secrets),
            id: (service.to_owned(), user.to_owned()),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
