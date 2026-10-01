//! Several GitHub accounts: the registry, their tokens in the system credential store, and
//! which account a repository uses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::token_store::{KeyringStore, TokenStore, TokenStoreError};

/// A signed-in GitHub account.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    pub token: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("login", &self.login)
            .field("token", &"***")
            .finish()
    }
}

/// What the UI shows about an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatus {
    pub login: String,
    pub name: Option<String>,
    /// `false`: GitHub refused its token (or it could not be read): sign in again.
    pub valid: bool,
}

#[derive(Debug, Clone)]
struct Entry {
    status: AccountStatus,
    token: Option<String>,
    /// Organizations the account belongs to (lowercase), for choosing accounts.
    orgs: Vec<String>,
}

/// The accounts, in the order they were added, shared between threads.
#[derive(Debug, Clone, Default)]
pub struct Accounts(Arc<RwLock<Vec<Entry>>>);

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Accounts {
    /// Usable accounts (valid, with a token), in order.
    pub fn list(&self) -> Vec<Account> {
        self.0
            .read()
            .map(|v| {
                v.iter()
                    .filter(|e| e.status.valid)
                    .filter_map(|e| {
                        e.token.as_ref().map(|t| Account {
                            login: e.status.login.clone(),
                            token: t.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get(&self, login: &str) -> Option<Account> {
        self.list().into_iter().find(|a| same(&a.login, login))
    }

    /// Every account, valid or not, in order.
    pub fn statuses(&self) -> Vec<AccountStatus> {
        self.0
            .read()
            .map(|v| v.iter().map(|e| e.status.clone()).collect())
            .unwrap_or_default()
    }

    /// Logins of every account (what the config remembers).
    pub fn logins(&self) -> Vec<String> {
        self.statuses().into_iter().map(|s| s.login).collect()
    }

    /// Add `account`, or replace the token of the same login (keeps its place).
    pub fn upsert(&self, account: Account, name: Option<String>) {
        if let Ok(mut v) = self.0.write() {
            let status = AccountStatus {
                login: account.login.clone(),
                name,
                valid: true,
            };
            match v.iter_mut().find(|e| same(&e.status.login, &account.login)) {
                Some(e) => {
                    e.status = status;
                    e.token = Some(account.token);
                }
                None => v.push(Entry {
                    status,
                    token: Some(account.token),
                    orgs: Vec::new(),
                }),
            }
        }
    }

    /// Known account whose token could not be checked or read: listed as invalid.
    pub fn add_invalid(&self, login: &str) {
        if let Ok(mut v) = self.0.write()
            && !v.iter().any(|e| same(&e.status.login, login))
        {
            v.push(Entry {
                status: AccountStatus {
                    login: login.to_string(),
                    name: None,
                    valid: false,
                },
                token: None,
                orgs: Vec::new(),
            });
        }
    }

    /// GitHub refused this account's token: keep it listed, unusable.
    pub fn invalidate(&self, login: &str) {
        if let Ok(mut v) = self.0.write()
            && let Some(e) = v.iter_mut().find(|e| same(&e.status.login, login))
        {
            e.status.valid = false;
            e.token = None;
        }
    }

    pub fn remove(&self, login: &str) {
        if let Ok(mut v) = self.0.write() {
            v.retain(|e| !same(&e.status.login, login));
        }
    }

    pub fn set_orgs(&self, login: &str, orgs: Vec<String>) {
        if let Ok(mut v) = self.0.write()
            && let Some(e) = v.iter_mut().find(|e| same(&e.status.login, login))
        {
            e.orgs = orgs.into_iter().map(|o| o.to_lowercase()).collect();
        }
    }

    /// Valid logins that are `owner` or a member of organization `owner`.
    pub fn owners_accounts(&self, owner: &str) -> Vec<String> {
        let owner = owner.to_lowercase();
        self.0
            .read()
            .map(|v| {
                v.iter()
                    .filter(|e| e.status.valid && e.token.is_some())
                    .filter(|e| e.status.login.to_lowercase() == owner || e.orgs.contains(&owner))
                    .map(|e| e.status.login.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Account of a repository, as remembered in the config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoAccount {
    pub login: String,
    /// Chosen by the user (kept even if it stops working); otherwise learned.
    pub manual: bool,
}

/// Steps 1 to 3 of choosing the account of a repository of `owner`: the user's choice,
/// then the account seen to have access, then an account that is the owner or a member of
/// it. `None`: try the accounts one by one (step 4). Only usable accounts are returned.
pub fn choose_account(
    owner: &str,
    accounts: &Accounts,
    remembered: Option<&RepoAccount>,
) -> Option<String> {
    if let Some(r) = remembered
        && let Some(a) = accounts.get(&r.login)
    {
        return Some(a.login);
    }
    accounts.owners_accounts(owner).into_iter().next()
}

/// Where account tokens live: one credential per login.
pub trait AccountStore: Send + Sync {
    fn load(&self, login: &str) -> Result<Option<String>, TokenStoreError>;
    fn save(&self, login: &str, token: &str) -> Result<(), TokenStoreError>;
    /// Removing a token that does not exist is not an error.
    fn clear(&self, login: &str) -> Result<(), TokenStoreError>;
    /// The single token of RetroGit versions before multiple accounts, if still there.
    fn load_legacy(&self) -> Result<Option<String>, TokenStoreError>;
    fn clear_legacy(&self) -> Result<(), TokenStoreError>;
}

/// macOS Keychain / Windows Credential Manager: service `RetroGit`, account
/// `github.com:<login>` (legacy: `github.com`).
pub struct KeyringAccounts {
    service: String,
}

impl KeyringAccounts {
    pub fn new(service: &str) -> KeyringAccounts {
        KeyringAccounts {
            service: service.to_string(),
        }
    }

    fn entry(&self, login: &str) -> KeyringStore {
        KeyringStore::new(
            &self.service,
            &format!("github.com:{}", login.to_lowercase()),
        )
    }

    fn legacy(&self) -> KeyringStore {
        KeyringStore::new(&self.service, "github.com")
    }
}

impl AccountStore for KeyringAccounts {
    fn load(&self, login: &str) -> Result<Option<String>, TokenStoreError> {
        self.entry(login).load()
    }
    fn save(&self, login: &str, token: &str) -> Result<(), TokenStoreError> {
        self.entry(login).save(token)
    }
    fn clear(&self, login: &str) -> Result<(), TokenStoreError> {
        self.entry(login).clear()
    }
    fn load_legacy(&self) -> Result<Option<String>, TokenStoreError> {
        self.legacy().load()
    }
    fn clear_legacy(&self) -> Result<(), TokenStoreError> {
        self.legacy().clear()
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryAccounts {
    tokens: Mutex<HashMap<String, String>>,
    legacy: Mutex<Option<String>>,
}

impl MemoryAccounts {
    /// As left by a RetroGit version with a single account.
    pub fn with_legacy(token: &str) -> MemoryAccounts {
        MemoryAccounts {
            legacy: Mutex::new(Some(token.to_string())),
            ..MemoryAccounts::default()
        }
    }

    pub fn with(login: &str, token: &str) -> MemoryAccounts {
        let m = MemoryAccounts::default();
        let _ = m.save(login, token);
        m
    }
}

fn poisoned<E: std::fmt::Display>(e: E) -> TokenStoreError {
    TokenStoreError(e.to_string())
}

impl AccountStore for MemoryAccounts {
    fn load(&self, login: &str) -> Result<Option<String>, TokenStoreError> {
        Ok(self
            .tokens
            .lock()
            .map_err(poisoned)?
            .get(&login.to_lowercase())
            .cloned())
    }
    fn save(&self, login: &str, token: &str) -> Result<(), TokenStoreError> {
        self.tokens
            .lock()
            .map_err(poisoned)?
            .insert(login.to_lowercase(), token.to_string());
        Ok(())
    }
    fn clear(&self, login: &str) -> Result<(), TokenStoreError> {
        self.tokens
            .lock()
            .map_err(poisoned)?
            .remove(&login.to_lowercase());
        Ok(())
    }
    fn load_legacy(&self) -> Result<Option<String>, TokenStoreError> {
        Ok(self.legacy.lock().map_err(poisoned)?.clone())
    }
    fn clear_legacy(&self) -> Result<(), TokenStoreError> {
        *self.legacy.lock().map_err(poisoned)? = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(login: &str) -> Account {
        Account {
            login: login.into(),
            token: format!("gho_{login}"),
        }
    }

    #[test]
    fn the_registry_keeps_order_replaces_and_invalidates() {
        let a = Accounts::default();
        a.upsert(acc("ada"), None);
        a.upsert(acc("bob"), Some("Bob".into()));
        a.upsert(
            Account {
                login: "ADA".into(),
                token: "gho_new".into(),
            },
            None,
        );
        assert_eq!(a.logins(), ["ADA", "bob"], "same login replaced in place");
        assert_eq!(a.get("ada").map(|x| x.token).as_deref(), Some("gho_new"));
        a.invalidate("bob");
        assert_eq!(a.list().len(), 1);
        assert!(!a.statuses()[1].valid);
        assert_eq!(a.get("bob"), None);
        a.add_invalid("carol");
        a.add_invalid("ada");
        assert_eq!(a.logins(), ["ADA", "bob", "carol"]);
        a.remove("Bob");
        assert_eq!(a.logins(), ["ADA", "carol"]);
    }

    #[test]
    fn debug_hides_tokens() {
        assert!(!format!("{:?}", acc("ada")).contains("gho_"));
        let a = Accounts::default();
        a.upsert(acc("ada"), None);
        assert!(!format!("{:?}", a.list()).contains("gho_ada"));
    }

    #[test]
    fn the_account_is_the_remembered_one_then_the_owner_or_a_member() {
        let a = Accounts::default();
        a.upsert(acc("perso"), None);
        a.upsert(acc("pro1"), None);
        a.upsert(acc("pro2"), None);
        a.set_orgs("pro1", vec!["Corp".into()]);
        a.set_orgs("pro2", vec!["corp".into()]);
        let manual = RepoAccount {
            login: "pro2".into(),
            manual: true,
        };
        assert_eq!(
            choose_account("Corp", &a, Some(&manual)).as_deref(),
            Some("pro2")
        );
        assert_eq!(
            choose_account("CORP", &a, None).as_deref(),
            Some("pro1"),
            "first member"
        );
        assert_eq!(
            choose_account("Perso", &a, None).as_deref(),
            Some("perso"),
            "owner"
        );
        assert_eq!(choose_account("someone", &a, None), None, "try them all");
        // A remembered account that is gone (removed, invalid): the other rules apply.
        let gone = RepoAccount {
            login: "old".into(),
            manual: false,
        };
        assert_eq!(
            choose_account("corp", &a, Some(&gone)).as_deref(),
            Some("pro1")
        );
        a.invalidate("pro1");
        assert_eq!(choose_account("corp", &a, None).as_deref(), Some("pro2"));
    }

    #[test]
    fn memory_store_keeps_one_token_per_login_and_the_legacy_one() {
        let m = MemoryAccounts::with_legacy("gho_old");
        assert_eq!(m.load_legacy(), Ok(Some("gho_old".into())));
        m.save("Ada", "gho_a").ok();
        assert_eq!(m.load("ada"), Ok(Some("gho_a".into())));
        assert_eq!(m.load("bob"), Ok(None));
        m.clear("ADA").ok();
        m.clear("ada").ok();
        assert_eq!(m.load("ada"), Ok(None));
        m.clear_legacy().ok();
        assert_eq!(m.load_legacy(), Ok(None));
    }
}
