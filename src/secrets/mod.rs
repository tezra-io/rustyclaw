// OS keychain-backed secret store for API keys.
//
// Resolution order: Keychain → environment variable → config file.
// The keychain is the preferred storage; env vars provide backward
// compatibility and CI convenience; config file values are lowest priority.

use anyhow::Result;

/// Service name used for all keychain entries.
const SERVICE: &str = "rustyclaw";

/// Well-known API key names that can be stored in the keychain.
pub const KNOWN_KEYS: &[&str] = &[
    "RUSTYCLAW_API_KEY",
    "API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "GLM_API_KEY",
    "ZAI_API_KEY",
    "OLLAMA_API_KEY",
    "BRAVE_API_KEY",
];

/// Keychain-backed secret store with env var fallback.
pub struct KeychainStore;

impl KeychainStore {
    /// Retrieve a secret. Checks keychain first, then env var.
    /// Returns `None` if neither source has the key.
    pub fn get(key: &str) -> Option<String> {
        // 1. Keychain
        if let Some(value) = Self::get_keychain(key) {
            return Some(value);
        }
        // 2. Env var fallback
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    }

    /// Store a secret in the OS keychain.
    pub fn set(key: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, key)?;
        entry
            .set_password(value)
            .map_err(|e| anyhow::anyhow!("Failed to store secret in keychain: {e}"))
    }

    /// Delete a secret from the OS keychain.
    pub fn delete(key: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()), // already absent
            Err(e) => Err(anyhow::anyhow!(
                "Failed to delete secret from keychain: {e}"
            )),
        }
    }

    /// Check whether a key exists in the keychain (does not check env vars).
    pub fn has(key: &str) -> bool {
        Self::get_keychain(key).is_some()
    }

    /// List which of the well-known keys are present in the keychain.
    pub fn list_stored() -> Vec<&'static str> {
        KNOWN_KEYS
            .iter()
            .copied()
            .filter(|k| Self::has(k))
            .collect()
    }

    /// Read from keychain only, returning `None` on any error or missing entry.
    fn get_keychain(key: &str) -> Option<String> {
        let entry = keyring::Entry::new(SERVICE, key).ok()?;
        match entry.get_password() {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_fallback_works() {
        let unique = "RUSTYCLAW_TEST_SECRET_9f8a7b";
        // Ensure not in keychain
        let _ = KeychainStore::delete(unique);
        std::env::set_var(unique, "from-env");

        let value = KeychainStore::get(unique);
        assert_eq!(value.as_deref(), Some("from-env"));

        std::env::remove_var(unique);
    }

    #[test]
    fn empty_env_var_returns_none() {
        let unique = "RUSTYCLAW_TEST_EMPTY_ENV_c3d4e5";
        let _ = KeychainStore::delete(unique);
        std::env::set_var(unique, "   ");

        let value = KeychainStore::get(unique);
        assert!(value.is_none(), "Whitespace-only env var should be None");

        std::env::remove_var(unique);
    }

    #[test]
    fn missing_key_returns_none() {
        let unique = "RUSTYCLAW_TEST_MISSING_a1b2c3";
        let _ = KeychainStore::delete(unique);
        std::env::remove_var(unique);

        let value = KeychainStore::get(unique);
        assert!(value.is_none());
    }

    #[test]
    fn keychain_set_get_delete_cycle() {
        let unique = "RUSTYCLAW_TEST_CYCLE_d4e5f6";
        std::env::remove_var(unique);

        // set
        KeychainStore::set(unique, "keychain-value").unwrap();
        assert!(KeychainStore::has(unique));

        // get (keychain wins over env)
        let value = KeychainStore::get(unique);
        assert_eq!(value.as_deref(), Some("keychain-value"));

        // delete
        KeychainStore::delete(unique).unwrap();
        assert!(!KeychainStore::has(unique));
        assert!(KeychainStore::get(unique).is_none());
    }

    #[test]
    fn keychain_overrides_env_var() {
        let unique = "RUSTYCLAW_TEST_OVERRIDE_e5f6a7";
        std::env::set_var(unique, "from-env");
        KeychainStore::set(unique, "from-keychain").unwrap();

        let value = KeychainStore::get(unique);
        assert_eq!(
            value.as_deref(),
            Some("from-keychain"),
            "Keychain should take priority over env var"
        );

        // cleanup
        KeychainStore::delete(unique).unwrap();
        std::env::remove_var(unique);
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let unique = "RUSTYCLAW_TEST_NOEXIST_b8c9d0";
        let result = KeychainStore::delete(unique);
        assert!(result.is_ok(), "Deleting non-existent key should succeed");
    }

    #[test]
    fn list_stored_returns_only_present_keys() {
        let stored = KeychainStore::list_stored();
        // We can't assert specific contents since this depends on the host,
        // but we can verify the return type and that it's a subset of KNOWN_KEYS.
        for key in &stored {
            assert!(KNOWN_KEYS.contains(key));
        }
    }
}
