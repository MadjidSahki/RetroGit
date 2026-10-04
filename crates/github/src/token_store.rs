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

/// The token read from an older item is the user's, whatever re-saving it did: a failure
/// is only logged (the migration is tried again at the next start).
pub fn keep_after_migration(token: String, resaved: Result<(), TokenStoreError>) -> String {
    if let Err(e) = resaved {
        log::warn!("token kept, but not moved to the security tool yet: {e}");
    }
    token
}

/// Comment set on the Keychain items RetroGit writes through `/usr/bin/security`.
pub const SECURITY_MARKER: &str = "retrogit-security";

fn security_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Command for `security -i` (read from its standard input) adding the item: the token is
/// hex-encoded (`-X`), so it never appears in a process list.
pub fn security_add_command(service: &str, account: &str, token: &str) -> String {
    let hex: String = token.bytes().map(|b| format!("{b:02x}")).collect();
    format!(
        "add-generic-password -U -s {} -a {} -j {} -X {}\n",
        security_quote(service),
        security_quote(account),
        security_quote(SECURITY_MARKER),
        security_quote(&hex)
    )
}

/// The `icmt` (comment) attribute in `security find-generic-password` output.
pub fn parse_security_comment(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let v = l.trim().strip_prefix("\"icmt\"<blob>=")?;
        let v = v.strip_prefix('"')?.strip_suffix('"')?;
        Some(v.to_string())
    })
}

/// macOS Keychain (through `/usr/bin/security`) / Windows Credential Manager.
///
/// On macOS, items are created and read by Apple's `security` tool rather than by RetroGit:
/// RetroGit has no Apple Team ID, so the Keychain would tie its items to the exact build and
/// ask again after every update. Like the GitHub CLI, the tool's stable identity avoids that
/// (any program of the user can read the token the same way). Items written by older
/// versions are read once more (one prompt), then re-created through the tool.
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
}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::{KeyringStore, SECURITY_MARKER, TokenStoreError, parse_security_comment};

    const SECURITY: &str = "/usr/bin/security";
    /// `security` exit code for "item not found".
    const NOT_FOUND: i32 = 44;

    fn err(e: impl std::fmt::Display) -> TokenStoreError {
        TokenStoreError(e.to_string())
    }

    impl KeyringStore {
        fn run(&self, args: &[&str]) -> Result<std::process::Output, TokenStoreError> {
            Command::new(SECURITY)
                .args(args)
                .args(["-s", &self.service, "-a", &self.account])
                .stdin(Stdio::null())
                .output()
                .map_err(err)
        }

        /// The item's comment: `None` if there is no item.
        fn comment(&self) -> Result<Option<Option<String>>, TokenStoreError> {
            let out = self.run(&["find-generic-password"])?;
            match out.status.code() {
                Some(0) => Ok(Some(parse_security_comment(&String::from_utf8_lossy(
                    &out.stdout,
                )))),
                Some(NOT_FOUND) => Ok(None),
                _ => Err(err(String::from_utf8_lossy(&out.stderr).trim())),
            }
        }

        pub(super) fn mac_load(&self) -> Result<Option<String>, TokenStoreError> {
            let Some(comment) = self.comment()? else {
                return Ok(None);
            };
            let out = self.run(&["find-generic-password", "-w"])?;
            if !out.status.success() {
                return Err(err(String::from_utf8_lossy(&out.stderr).trim()));
            }
            let token = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
            if comment.as_deref() != Some(SECURITY_MARKER) {
                // Written by an older RetroGit: re-create it through the tool.
                return Ok(Some(super::keep_after_migration(
                    token.clone(),
                    self.mac_save(&token),
                )));
            }
            Ok(Some(token))
        }

        pub(super) fn mac_save(&self, token: &str) -> Result<(), TokenStoreError> {
            // Our own items are updated in place (`-U`); an older one is replaced, so that it
            // belongs to the tool. A failed add after that delete is tried once more.
            let ours = self.comment()?.flatten().as_deref() == Some(SECURITY_MARKER);
            if !ours {
                self.mac_clear()?;
            }
            self.add(token).or_else(|_| self.add(token))
        }

        fn add(&self, token: &str) -> Result<(), TokenStoreError> {
            let mut child = Command::new(SECURITY)
                .arg("-i")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(err)?;
            let cmd = super::security_add_command(&self.service, &self.account, token);
            child
                .stdin
                .take()
                .ok_or_else(|| err("no stdin"))?
                .write_all(cmd.as_bytes())
                .map_err(err)?;
            let out = child.wait_with_output().map_err(err)?;
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() && stderr.trim().is_empty() {
                Ok(())
            } else {
                Err(err(stderr.trim()))
            }
        }

        pub(super) fn mac_clear(&self) -> Result<(), TokenStoreError> {
            let out = self.run(&["delete-generic-password"])?;
            match out.status.code() {
                Some(0) | Some(NOT_FOUND) => Ok(()),
                _ => Err(err(String::from_utf8_lossy(&out.stderr).trim())),
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl KeyringStore {
    fn entry(&self) -> Result<keyring::Entry, TokenStoreError> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| TokenStoreError(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        #[cfg(target_os = "macos")]
        return self.mac_load();
        #[cfg(not(target_os = "macos"))]
        match self.entry()?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(TokenStoreError(e.to_string())),
        }
    }

    fn save(&self, token: &str) -> Result<(), TokenStoreError> {
        #[cfg(target_os = "macos")]
        return self.mac_save(token);
        #[cfg(not(target_os = "macos"))]
        self.entry()?
            .set_password(token)
            .map_err(|e| TokenStoreError(e.to_string()))
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        #[cfg(target_os = "macos")]
        return self.mac_clear();
        #[cfg(not(target_os = "macos"))]
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
