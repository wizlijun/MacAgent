//! macOS Keychain Generic Password 助手。
//!
//! 用 com.hemory.macagent service，account 即 key 名。

use anyhow::{Context, Result};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

const SERVICE: &str = "com.hemory.macagent";

pub fn save(key: &str, data: &[u8]) -> Result<()> {
    set_generic_password(SERVICE, key, data).context("keychain save")
}

pub fn load(key: &str) -> Result<Option<Vec<u8>>> {
    match get_generic_password(SERVICE, key) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code() == -25300 => Ok(None), // errSecItemNotFound
        Err(e) => Err(e).context("keychain load"),
    }
}

pub fn delete(key: &str) -> Result<()> {
    match delete_generic_password(SERVICE, key) {
        Ok(_) => Ok(()),
        Err(e) if e.code() == -25300 => Ok(()),
        Err(e) => Err(e).context("keychain delete"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_load_delete() {
        let key = "test.macagent.unitkeychain";
        save(key, b"hello").unwrap();
        assert_eq!(load(key).unwrap().as_deref(), Some(&b"hello"[..]));
        delete(key).unwrap();
        assert!(load(key).unwrap().is_none());
    }
}
