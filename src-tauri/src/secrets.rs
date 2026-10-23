//! API keys for cloud providers. On Windows they live in Credential Manager
//! (as "Sruthi/<provider>"), never in settings.json.

use anyhow::Result;

fn target(name: &str) -> String {
    format!("Sruthi/{name}")
}

#[cfg(windows)]
mod store {
    use anyhow::{Result, anyhow};
    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use windows::core::{HSTRING, PWSTR};

    pub fn set(target: &str, secret: &str) -> Result<()> {
        let name = HSTRING::from(target);
        let blob = secret.as_bytes();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_ptr() as *mut u16),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: every pointer in `credential` outlives the call.
        unsafe { CredWriteW(&credential, 0) }.map_err(|e| anyhow!("Couldn't save the API key: {e}"))
    }

    pub fn get(target: &str) -> Option<String> {
        let name = HSTRING::from(target);
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: on success `found` points to a credential that stays valid
        // until CredFree, which runs after the blob is copied.
        unsafe {
            CredReadW(&name, CRED_TYPE_GENERIC, None, &mut found).ok()?;
            let credential = &*found;
            let blob = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            let secret = String::from_utf8(blob.to_vec()).ok();
            CredFree(found as *const _);
            secret
        }
    }

    pub fn delete(target: &str) {
        let name = HSTRING::from(target);
        // SAFETY: plain call with a valid string; a missing entry is fine.
        let _ = unsafe { CredDeleteW(&name, CRED_TYPE_GENERIC, None) };
    }
}

/// Elsewhere (until the Mac build gets Keychain support): a file next to
/// the settings, readable only by the user.
#[cfg(not(windows))]
mod store {
    use std::collections::HashMap;

    use anyhow::Result;

    fn path() -> std::path::PathBuf {
        crate::settings::app_root().join("secrets.json")
    }

    fn load() -> HashMap<String, String> {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(map: &HashMap<String, String>) -> Result<()> {
        std::fs::create_dir_all(crate::settings::app_root())?;
        std::fs::write(path(), serde_json::to_string(map)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path(), std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn set(target: &str, secret: &str) -> Result<()> {
        let mut map = load();
        map.insert(target.to_string(), secret.to_string());
        save(&map)
    }

    pub fn get(target: &str) -> Option<String> {
        load().remove(target)
    }

    pub fn delete(target: &str) {
        let mut map = load();
        if map.remove(target).is_some() {
            let _ = save(&map);
        }
    }
}

pub fn set(name: &str, secret: &str) -> Result<()> {
    store::set(&target(name), secret.trim())
}

pub fn get(name: &str) -> Option<String> {
    store::get(&target(name)).filter(|s| !s.is_empty())
}

pub fn delete(name: &str) {
    store::delete(&target(name))
}

#[cfg(test)]
mod tests {
    /// Writes, reads and deletes a throwaway entry in the real credential
    /// store: `cargo test --lib secrets -- --ignored`
    #[test]
    #[ignore]
    fn round_trip() {
        super::set("selftest", "  sk-test-123  ").unwrap();
        assert_eq!(super::get("selftest").as_deref(), Some("sk-test-123"));
        super::delete("selftest");
        assert_eq!(super::get("selftest"), None);
    }
}
