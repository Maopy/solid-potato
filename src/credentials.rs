//! Credential storage. Secrets never belong in SQLite, logs, or task exports.
use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Mutex};
#[cfg(target_os = "macos")]
use std::{
    io::Write,
    process::{Command, Stdio},
};

const SERVICE_NAME: &str = "io.livearchive.desktop";

pub trait CredentialStore: Send + Sync {
    fn put(&self, key: &str, secret: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// Uses Windows Credential Manager, macOS Keychain, or the platform keyring.
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    #[cfg(not(target_os = "macos"))]
    fn entry(key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE_NAME, key).context("opening system credential store")
    }
}

// The keyring crate's macOS backend can report a successful write but fail to
// find that item from the next process on some Keychain configurations. The
// system `security` tool targets the user's default login keychain reliably.
#[cfg(target_os = "macos")]
impl CredentialStore for SystemCredentialStore {
    fn put(&self, key: &str, secret: &str) -> Result<()> {
        let mut child = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                SERVICE_NAME,
                "-a",
                key,
                "-w",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting macOS Keychain command")?;
        child
            .stdin
            .take()
            .context("opening Keychain command input")?
            .write_all(secret.as_bytes())
            .context("writing session to Keychain")?;
        let output = child
            .wait_with_output()
            .context("waiting for Keychain command")?;
        if output.status.success() {
            Ok(())
        } else {
            anyhow::bail!(
                "saving credential in macOS Keychain failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        let output = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", SERVICE_NAME, "-a", key, "-w"])
            .output()
            .context("reading macOS Keychain")?;
        if output.status.success() {
            let secret =
                String::from_utf8(output.stdout).context("decoding Keychain credential")?;
            Ok(Some(secret.trim_end_matches(['\r', '\n']).to_owned()))
        } else if output.status.code() == Some(44) {
            Ok(None)
        } else {
            anyhow::bail!(
                "reading credential from macOS Keychain failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        let output = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE_NAME, "-a", key])
            .output()
            .context("deleting macOS Keychain credential")?;
        if output.status.success() || output.status.code() == Some(44) {
            Ok(())
        } else {
            anyhow::bail!(
                "deleting credential from macOS Keychain failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl CredentialStore for SystemCredentialStore {
    fn put(&self, key: &str, secret: &str) -> Result<()> {
        Self::entry(key)?
            .set_password(secret)
            .context("saving credential")
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("reading credential"),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("deleting credential"),
        }
    }
}

/// Test double and optional ephemeral storage for development only.
#[derive(Default)]
pub struct MemoryCredentialStore(Mutex<HashMap<String, String>>);

impl CredentialStore for MemoryCredentialStore {
    fn put(&self, key: &str, secret: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

pub fn credential_key(platform: crate::model::Platform, account_id: &str) -> String {
    format!("{}:{account_id}", platform.as_str())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn credentials_survive_reopening_the_native_store() {
        let key = format!("windows-test-{}", uuid::Uuid::new_v4());
        SystemCredentialStore
            .put(&key, "temporary-test-value")
            .unwrap();
        let reopened = SystemCredentialStore.get(&key);
        SystemCredentialStore.delete(&key).unwrap();
        assert_eq!(reopened.unwrap().as_deref(), Some("temporary-test-value"));
        assert_eq!(SystemCredentialStore.get(&key).unwrap(), None);
    }
}
