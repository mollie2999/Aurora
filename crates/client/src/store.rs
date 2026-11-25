//! Saved logins ("remember me").
//!
//! The account list (name, Steam ID, display name, avatar) is a file in the
//! user's config folder, readable only by them. The credentials — the refresh
//! token and Steam Guard data — are stored in the OS credential store
//! (Secret Service / GNOME Keyring / KWallet on Linux, Credential Manager on
//! Windows, Keychain on macOS), encrypted and locked with the user's login.
//!
//! On headless or minimal Linux systems without a running Secret Service
//! daemon, credentials gracefully fall back to a `0600` file
//! (`credentials.json`) inside the `0700` config directory, and are
//! automatically migrated to the OS credential store on load once one becomes
//! available. Legacy tokens stored inline in `accounts.json` are also migrated
//! automatically on first load.

use model::SteamId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedAccount {
    pub account_name: String,
    pub steam_id: SteamId,
    #[serde(default)]
    pub persona_name: String,
    #[serde(default)]
    pub avatar_hash: Option<String>,
    /// From the credential store ([`credentials`]); empty from [`load`] and
    /// [`find`]. Read from old plaintext files only to move it out of them.
    #[serde(default, skip_serializing)]
    pub refresh_token: String,
    /// Like `refresh_token`.
    #[serde(default, skip_serializing)]
    pub guard_data: Option<String>,
}

impl std::fmt::Debug for SavedAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedAccount")
            .field("account_name", &self.account_name)
            .field("steam_id", &self.steam_id)
            .field("persona_name", &self.persona_name)
            .field("avatar_hash", &self.avatar_hash)
            .field("refresh_token", &"[REDACTED]")
            .field("guard_data", &self.guard_data.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

fn fallback_config_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: `getuid()` has no preconditions and never fails.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/steaminrust-{uid}"))
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|base| PathBuf::from(base).join("SteamInRust"))
            .unwrap_or_else(|| std::env::temp_dir().join("SteamInRust"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::env::temp_dir().join("SteamInRust")
    }
}

fn resolve_config_dir(primary: Option<PathBuf>) -> PathBuf {
    let dir = primary.filter(|p| !p.as_os_str().is_empty() && p != Path::new(".")).unwrap_or_else(fallback_config_dir);
    let _ = ensure_private_dir_at(&dir, None);
    dir
}

pub fn config_dir() -> PathBuf {
    resolve_config_dir(directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").map(|d| d.config_dir().to_path_buf()))
}

pub(crate) fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    let base = directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(fallback_config_dir);
    ensure_private_dir_at(dir, Some(&base))
}

fn ensure_private_dir_at(dir: &Path, base: Option<&Path>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir)?;
        let uid = unsafe { libc::getuid() };
        let tighten = |p: &Path| -> std::io::Result<()> {
            let meta = std::fs::symlink_metadata(p)?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "config directory is a symlink or not a directory"));
            }
            if meta.uid() != uid {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "config directory owned by another user"));
            }
            if meta.permissions().mode() & 0o777 != 0o700 {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        };
        if let Some(root) = base.filter(|r| dir.starts_with(r)) {
            let mut cur = root.to_path_buf();
            if cur.exists() {
                tighten(&cur)?;
            }
            if let Ok(rel) = dir.strip_prefix(root) {
                for comp in rel.components() {
                    cur.push(comp);
                    tighten(&cur)?;
                }
            }
        } else {
            tighten(dir)?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = base;
        std::fs::create_dir_all(dir)
    }
}

fn accounts_path(dir: &Path) -> PathBuf {
    dir.join("accounts.json")
}

fn credentials_path(dir: &Path) -> PathBuf {
    dir.join("credentials.json")
}

/// The saved accounts, without credentials (see [`credentials`]).
pub fn load() -> Vec<SavedAccount> {
    load_with(&config_dir(), &secrets::OsKeyring)
}

fn load_with(dir: &Path, backend: &impl SecretBackend) -> Vec<SavedAccount> {
    let mut list: Vec<SavedAccount> = std::fs::read_to_string(accounts_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Files from before the credential store held the tokens in plain text:
    // move them out into the OS keyring (or 0600 fallback file if unavailable),
    // then rewrite accounts.json without them.
    if list.iter().any(|a| !a.refresh_token.is_empty() || a.guard_data.is_some()) {
        for a in &mut list {
            if !a.refresh_token.is_empty() || a.guard_data.is_some() {
                save_secret_with(dir, backend, &a.account_name, &a.refresh_token, a.guard_data.as_deref());
                a.refresh_token.clear();
                a.guard_data = None;
            }
        }
        write_at(dir, &list);
    }
    // If credentials were previously saved to the 0600 fallback file, migrate
    // them into the OS credential store as soon as it becomes available.
    migrate_fallback_with(dir, backend);
    list
}

/// A saved account with its refresh token and Steam Guard data from the
/// credential store (or `0600` fallback file when no credential store is
/// available). The token is empty when neither has it: the account needs its
/// password again.
pub fn credentials(account: &str) -> Option<SavedAccount> {
    credentials_with(&config_dir(), &secrets::OsKeyring, account)
}

fn credentials_with(dir: &Path, backend: &impl SecretBackend, account: &str) -> Option<SavedAccount> {
    let mut acc = find_with(dir, backend, account)?;
    match backend.load(&acc.account_name) {
        Ok((Some(token), guard)) if !token.is_empty() => {
            acc.refresh_token = token;
            acc.guard_data = guard;
            // Clean up any stale fallback entry for this account.
            let _ = fallback_store::remove(dir, &acc.account_name);
        }
        Ok((token, guard)) => {
            if let Some(fb) = fallback_store::get(dir, &acc.account_name) {
                acc.refresh_token = fb.refresh_token;
                acc.guard_data = fb.guard_data.or(guard);
            } else {
                acc.refresh_token = token.unwrap_or_default();
                acc.guard_data = guard;
            }
        }
        Err(e) => {
            tracing::warn!("OS credential store unavailable for {account} ({e}); reading from 0600 fallback file");
            if let Some(fb) = fallback_store::get(dir, &acc.account_name) {
                acc.refresh_token = fb.refresh_token;
                acc.guard_data = fb.guard_data;
            }
        }
    }
    Some(acc)
}

/// Write the account list (credentials are skipped by serde), readable
/// only by the user.
fn write_at(dir: &Path, list: &[SavedAccount]) {
    let p = accounts_path(dir);
    match serde_json::to_string_pretty(list) {
        Ok(json) => {
            if let Err(e) = write_private(&p, json.as_bytes()) {
                tracing::error!("could not save accounts: {e}");
            }
        }
        Err(e) => tracing::error!("could not encode accounts: {e}"),
    }
}

fn save_secret_with(dir: &Path, backend: &impl SecretBackend, account: &str, refresh_token: &str, guard_data: Option<&str>) {
    match backend.save(account, refresh_token, guard_data) {
        Ok(()) => {
            let _ = fallback_store::remove(dir, account);
        }
        Err(e) => {
            tracing::warn!("OS credential store unavailable for {account} ({e}); storing credentials in 0600 fallback file");
            if let Err(fe) = fallback_store::set(dir, account, refresh_token, guard_data) {
                tracing::error!("could not save fallback credentials for {account}: {fe}");
            }
        }
    }
}

fn migrate_fallback_with(dir: &Path, backend: &impl SecretBackend) {
    let creds_file = credentials_path(dir);
    if !creds_file.exists() {
        return;
    }
    let mut map = fallback_store::load_all(dir);
    if map.is_empty() {
        let _ = std::fs::remove_file(&creds_file);
        return;
    }
    let keys: Vec<String> = map.keys().cloned().collect();
    let mut migrated = 0usize;
    for key in keys {
        let Some(secret) = map.get(&key) else { continue };
        match backend.save(&key, &secret.refresh_token, secret.guard_data.as_deref()) {
            Ok(()) => {
                tracing::info!("migrated saved sign-in for {key} from 0600 fallback file to OS credential store");
                map.remove(&key);
                migrated += 1;
            }
            Err(e) => {
                tracing::debug!("OS credential store unavailable while migrating fallback credentials: {e}");
                break;
            }
        }
    }
    if migrated > 0 {
        if let Err(e) = fallback_store::save_all(dir, &map) {
            tracing::warn!("could not update fallback credentials after migration: {e}");
        }
    }
}

fn open_unique_temp(dir: &Path, base_name: &str) -> std::io::Result<(std::fs::File, PathBuf)> {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    for _ in 0..32 {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(pid);
        h.write_u64(seq);
        let tag = h.finish();
        let candidate = dir.join(format!(".{base_name}.{pid}-{seq}-{tag:016x}.tmp"));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&candidate) {
            Ok(f) => return Ok((f, candidate)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "could not create unique temporary file"))
}

/// Replace `path` with `bytes`, mode 0600 on Unix inside a 0700 parent dir,
/// using a unique `O_EXCL` temporary file.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent directory"))?;
    ensure_private_dir(dir)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "refusing to overwrite symlink"));
        }
    }
    let base_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let (mut f, tmp) = open_unique_temp(dir, base_name)?;
    let res = (|| -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Copy `src` to `dst` with mode 0600 on Unix inside 0700 parent directories.
pub(crate) fn copy_private(src: &Path, dst: &Path) -> std::io::Result<()> {
    let dir = dst.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent directory"))?;
    ensure_private_dir(dir)?;
    let mut reader = std::fs::File::open(src)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(dst)?;
    let result = std::io::copy(&mut reader, &mut file).and_then(|_| file.sync_all());
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(dst);
    }
    result
}

pub fn find(account: &str) -> Option<SavedAccount> {
    find_with(&config_dir(), &secrets::OsKeyring, account)
}

fn find_with(dir: &Path, backend: &impl SecretBackend, account: &str) -> Option<SavedAccount> {
    load_with(dir, backend).into_iter().find(|a| a.account_name.eq_ignore_ascii_case(account))
}

/// Insert or update by account name, keeping fields the new value leaves
/// empty. Its credentials go to the OS credential store, falling back to a
/// `0600` file if no credential store daemon is available.
pub fn upsert(acc: SavedAccount) {
    upsert_with(&config_dir(), &secrets::OsKeyring, acc);
}

fn upsert_with(dir: &Path, backend: &impl SecretBackend, mut acc: SavedAccount) {
    if !acc.refresh_token.is_empty() {
        if acc.guard_data.is_none() {
            acc.guard_data = backend
                .load(&acc.account_name)
                .ok()
                .and_then(|(_, g)| g)
                .or_else(|| fallback_store::get(dir, &acc.account_name).and_then(|fb| fb.guard_data));
        }
        save_secret_with(dir, backend, &acc.account_name, &acc.refresh_token, acc.guard_data.as_deref());
        acc.refresh_token.clear();
        acc.guard_data = None;
    }
    let mut list = load_with(dir, backend);
    if let Some(old) = list.iter().find(|a| a.account_name.eq_ignore_ascii_case(&acc.account_name)) {
        if acc.persona_name.is_empty() {
            acc.persona_name = old.persona_name.clone();
        }
        if acc.avatar_hash.is_none() {
            acc.avatar_hash = old.avatar_hash.clone();
        }
    }
    list.retain(|a| !a.account_name.eq_ignore_ascii_case(&acc.account_name));
    list.insert(0, acc);
    write_at(dir, &list);
}

pub fn update(account: &str, f: impl FnOnce(&mut SavedAccount)) {
    update_with(&config_dir(), &secrets::OsKeyring, account, f);
}

fn update_with(dir: &Path, backend: &impl SecretBackend, account: &str, f: impl FnOnce(&mut SavedAccount)) {
    let mut list = load_with(dir, backend);
    if let Some(a) = list.iter_mut().find(|a| a.account_name.eq_ignore_ascii_case(account)) {
        f(a);
        write_at(dir, &list);
    }
}

/// Forget an account and its credentials.
pub fn remove(account: &str) {
    remove_with(&config_dir(), &secrets::OsKeyring, account);
}

fn remove_with(dir: &Path, backend: &impl SecretBackend, account: &str) {
    if let Err(e) = backend.delete(account) {
        tracing::warn!("couldn't delete the saved sign-in of {account} from OS credential store: {e}");
    }
    if let Err(e) = fallback_store::remove(dir, account) {
        tracing::warn!("couldn't delete fallback credentials for {account}: {e}");
    }
    let mut list = load_with(dir, backend);
    list.retain(|a| !a.account_name.eq_ignore_ascii_case(account));
    write_at(dir, &list);
}

trait SecretBackend {
    fn load(&self, account: &str) -> Result<(Option<String>, Option<String>), String>;
    fn save(&self, account: &str, refresh_token: &str, guard_data: Option<&str>) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// The OS credential store. Each account has two entries (refresh token,
/// Steam Guard data), keyed by the lowercase account name.
mod secrets {
    use super::SecretBackend;

    const SERVICE: &str = "dev.SteamInRust.SteamInRust";

    pub struct OsKeyring;

    fn entry(kind: &str, account: &str) -> keyring::Result<keyring::Entry> {
        keyring::Entry::new(&format!("{SERVICE}.{kind}"), &account.to_lowercase())
    }

    fn get(kind: &str, account: &str) -> keyring::Result<Option<String>> {
        match entry(kind, account)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn set(kind: &str, account: &str, value: Option<&str>) -> keyring::Result<()> {
        let e = entry(kind, account)?;
        match value {
            Some(v) if !v.is_empty() => e.set_password(v),
            _ => match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(err),
            },
        }
    }

    impl SecretBackend for OsKeyring {
        fn load(&self, account: &str) -> Result<(Option<String>, Option<String>), String> {
            let token = get("refresh-token", account).map_err(|e| e.to_string())?;
            let guard = get("guard-data", account).map_err(|e| e.to_string())?;
            Ok((token, guard))
        }

        fn save(&self, account: &str, refresh_token: &str, guard_data: Option<&str>) -> Result<(), String> {
            set("refresh-token", account, Some(refresh_token)).map_err(|e| e.to_string())?;
            set("guard-data", account, guard_data).map_err(|e| e.to_string())
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            set("refresh-token", account, None).map_err(|e| e.to_string())?;
            set("guard-data", account, None).map_err(|e| e.to_string())
        }
    }
}

/// `0600` file fallback used only when the OS credential store is unavailable
/// (e.g. headless or minimal Linux without a Secret Service daemon).
mod fallback_store {
    use super::{BTreeMap, Path, credentials_path, write_private};
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Default, Serialize, Deserialize)]
    pub struct StoredSecret {
        #[serde(default)]
        pub refresh_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub guard_data: Option<String>,
    }

    impl std::fmt::Debug for StoredSecret {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StoredSecret")
                .field("refresh_token", &"[REDACTED]")
                .field("guard_data", &self.guard_data.as_ref().map(|_| "[REDACTED]"))
                .finish()
        }
    }

    pub fn load_all(dir: &Path) -> BTreeMap<String, StoredSecret> {
        let p = credentials_path(dir);
        match std::fs::symlink_metadata(&p) {
            Ok(meta) if meta.file_type().is_symlink() => {
                tracing::warn!("refusing to read symlinked fallback credentials file {}", p.display());
                return BTreeMap::new();
            }
            Ok(_) => {}
            Err(_) => return BTreeMap::new(),
        }
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_all(dir: &Path, map: &BTreeMap<String, StoredSecret>) -> std::io::Result<()> {
        let p = credentials_path(dir);
        if map.is_empty() {
            match std::fs::remove_file(&p) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        let json = serde_json::to_string_pretty(map)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_private(&p, json.as_bytes())
    }

    pub fn get(dir: &Path, account: &str) -> Option<StoredSecret> {
        load_all(dir).remove(&account.to_lowercase())
    }

    pub fn set(dir: &Path, account: &str, refresh_token: &str, guard_data: Option<&str>) -> std::io::Result<()> {
        let mut map = load_all(dir);
        let key = account.to_lowercase();
        let guard = guard_data.filter(|g| !g.is_empty()).map(str::to_string);
        if refresh_token.is_empty() && guard.is_none() {
            map.remove(&key);
        } else {
            map.insert(key, StoredSecret { refresh_token: refresh_token.to_string(), guard_data: guard });
        }
        save_all(dir, &map)
    }

    pub fn remove(dir: &Path, account: &str) -> std::io::Result<()> {
        let p = credentials_path(dir);
        if !p.exists() {
            return Ok(());
        }
        let mut map = load_all(dir);
        if map.remove(&account.to_lowercase()).is_some() {
            save_all(dir, &map)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_impls_redact_passwords_codes_keys_and_tokens() {
        let login = crate::Command::LoginPassword { account: "alice".into(), password: "super_secret_pw".into() };
        let login_dbg = format!("{login:?}");
        assert!(login_dbg.contains("alice"));
        assert!(!login_dbg.contains("super_secret_pw"), "password leaked in Command Debug: {login_dbg}");

        let code = crate::Command::SubmitCode("987654".into());
        assert!(!format!("{code:?}").contains("987654"), "Steam Guard code leaked in Command Debug");

        let key = crate::Command::ActivateKey { key: "AAAAA-BBBBB-CCCCC".into() };
        assert!(!format!("{key:?}").contains("AAAAA-BBBBB-CCCCC"), "CD key leaked in Command Debug");

        let saved = SavedAccount {
            account_name: "alice".into(),
            steam_id: 76561198000000001,
            persona_name: "Alice".into(),
            avatar_hash: None,
            refresh_token: "eyJhbGciOi.secret_refresh".into(),
            guard_data: Some("guard_machine_secret".into()),
        };
        let saved_dbg = format!("{saved:?}");
        assert!(saved_dbg.contains("alice"));
        assert!(!saved_dbg.contains("secret_refresh"), "refresh_token leaked in SavedAccount Debug: {saved_dbg}");
        assert!(!saved_dbg.contains("guard_machine_secret"), "guard_data leaked in SavedAccount Debug: {saved_dbg}");
    }

    #[cfg(unix)]
    #[test]
    fn write_private_uses_excl_temp_and_0700_0600_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sir-store-sec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("cfg").join("offline").join("123");
        std::fs::create_dir_all(&sub).unwrap();
        // Start with loose 0755 permissions to verify write_private tightens it to 0700!
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let target = sub.join("account.json");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "untouched").unwrap();
        // Plant the old predictable `.tmp` symlink (`account.tmp`).
        std::os::unix::fs::symlink(&victim, target.with_extension("tmp")).unwrap();

        write_private(&target, b"{\"ok\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched", "predictable .tmp symlink was followed!");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"ok\":true}");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777, 0o700);

        // Verify copy_private (used by cloud-backups) sets 0600 even from a 0644 source!
        let src_save = dir.join("save.dat");
        std::fs::write(&src_save, b"save-bytes").unwrap();
        std::fs::set_permissions(&src_save, std::fs::Permissions::from_mode(0o644)).unwrap();
        let bkp = dir.join("cfg").join("cloud-backups").join("570").join("99").join("save.dat");
        copy_private(&src_save, &bkp).unwrap();
        assert_eq!(std::fs::metadata(&bkp).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(bkp.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);

        // Verify offline::write sets 0600!
        let off = dir.join("cfg").join("offline").join("123").join("playtime.json");
        crate::offline::write(&off, &vec![1u32, 2, 3]);
        assert_eq!(std::fs::metadata(&off).unwrap().permissions().mode() & 0o777, 0o600);

        // Verify config_dir fallback never returns "." and enforces 0700 `/tmp/steaminrust-<uid>`!
        let fb = resolve_config_dir(None);
        let uid = unsafe { libc::getuid() };
        assert_eq!(fb, PathBuf::from(format!("/tmp/steaminrust-{uid}")));
        assert_eq!(std::fs::metadata(&fb).unwrap().permissions().mode() & 0o777, 0o700);

        let _ = std::fs::remove_dir_all(&dir);
    }

    struct MockKeyring {
        available: std::sync::atomic::AtomicBool,
        entries: std::sync::Mutex<BTreeMap<String, (String, Option<String>)>>,
    }

    impl MockKeyring {
        fn available() -> Self {
            Self {
                available: std::sync::atomic::AtomicBool::new(true),
                entries: std::sync::Mutex::new(BTreeMap::new()),
            }
        }

        fn unavailable() -> Self {
            Self {
                available: std::sync::atomic::AtomicBool::new(false),
                entries: std::sync::Mutex::new(BTreeMap::new()),
            }
        }

        fn set_available(&self, on: bool) {
            self.available.store(on, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl SecretBackend for MockKeyring {
        fn load(&self, account: &str) -> Result<(Option<String>, Option<String>), String> {
            if !self.available.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("no secret service daemon".into());
            }
            let guard = self.entries.lock().unwrap();
            match guard.get(&account.to_lowercase()) {
                Some((t, g)) => Ok((Some(t.clone()), g.clone())),
                None => Ok((None, None)),
            }
        }

        fn save(&self, account: &str, refresh_token: &str, guard_data: Option<&str>) -> Result<(), String> {
            if !self.available.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("no secret service daemon".into());
            }
            let mut guard = self.entries.lock().unwrap();
            guard.insert(
                account.to_lowercase(),
                (refresh_token.to_string(), guard_data.filter(|g| !g.is_empty()).map(str::to_string)),
            );
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            if !self.available.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("no secret service daemon".into());
            }
            self.entries.lock().unwrap().remove(&account.to_lowercase());
            Ok(())
        }
    }

    fn temp_store_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sir-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_account(name: &str, token: &str, guard: Option<&str>) -> SavedAccount {
        SavedAccount {
            account_name: name.into(),
            steam_id: 76561198000000001,
            persona_name: "Alice".into(),
            avatar_hash: Some("deadbeef".into()),
            refresh_token: token.into(),
            guard_data: guard.map(str::to_string),
        }
    }

    #[test]
    fn stored_secret_debug_redacts_tokens() {
        let secret = fallback_store::StoredSecret {
            refresh_token: "super_secret_refresh".into(),
            guard_data: Some("super_secret_guard".into()),
        };
        let dbg = format!("{secret:?}");
        assert!(!dbg.contains("super_secret_refresh"), "refresh_token leaked in StoredSecret Debug: {dbg}");
        assert!(!dbg.contains("super_secret_guard"), "guard_data leaked in StoredSecret Debug: {dbg}");
    }

    #[test]
    fn keyring_store_roundtrip_never_writes_tokens_to_disk() {
        let dir = temp_store_dir("keyring-roundtrip");
        let keyring = MockKeyring::available();

        upsert_with(&dir, &keyring, sample_account("Alice", "tok-1", Some("guard-1")));

        // accounts.json must not contain secrets, and no fallback file should exist.
        let accounts_raw = std::fs::read_to_string(accounts_path(&dir)).unwrap();
        assert!(!accounts_raw.contains("tok-1"));
        assert!(!accounts_raw.contains("guard-1"));
        assert!(!credentials_path(&dir).exists());

        // load() returns metadata without secrets; credentials() fetches from keyring (case-insensitive).
        let loaded = load_with(&dir, &keyring);
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].refresh_token.is_empty());
        assert!(loaded[0].guard_data.is_none());

        let creds = credentials_with(&dir, &keyring, "alice").unwrap();
        assert_eq!(creds.refresh_token, "tok-1");
        assert_eq!(creds.guard_data.as_deref(), Some("guard-1"));

        // Updating refresh token without guard_data preserves existing guard_data.
        upsert_with(&dir, &keyring, sample_account("Alice", "tok-2", None));
        let creds2 = credentials_with(&dir, &keyring, "ALICE").unwrap();
        assert_eq!(creds2.refresh_token, "tok-2");
        assert_eq!(creds2.guard_data.as_deref(), Some("guard-1"));

        // remove() clears both accounts.json and keyring.
        remove_with(&dir, &keyring, "alice");
        assert!(load_with(&dir, &keyring).is_empty());
        assert!(keyring.entries.lock().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fallback_file_used_when_keyring_unavailable_and_migrated_when_available() {
        let dir = temp_store_dir("keyring-fallback");
        let keyring = MockKeyring::unavailable();

        upsert_with(&dir, &keyring, sample_account("Alice", "fallback-tok", Some("fallback-guard")));

        // accounts.json stays token-free; credentials.json holds the fallback secret with 0600 perms.
        let accounts_raw = std::fs::read_to_string(accounts_path(&dir)).unwrap();
        assert!(!accounts_raw.contains("fallback-tok"));
        let creds_file = credentials_path(&dir);
        assert!(creds_file.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&creds_file).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // Reading credentials while keyring is still unavailable succeeds via fallback file.
        let creds = credentials_with(&dir, &keyring, "alice").unwrap();
        assert_eq!(creds.refresh_token, "fallback-tok");
        assert_eq!(creds.guard_data.as_deref(), Some("fallback-guard"));

        // Once the OS keyring becomes available, load() migrates tokens into keyring and deletes credentials.json.
        keyring.set_available(true);
        let _ = load_with(&dir, &keyring);
        assert!(!creds_file.exists(), "fallback credentials.json should be removed once migrated");
        let migrated = credentials_with(&dir, &keyring, "alice").unwrap();
        assert_eq!(migrated.refresh_token, "fallback-tok");
        assert_eq!(migrated.guard_data.as_deref(), Some("fallback-guard"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_accounts_json_migrates_to_keyring_or_fallback() {
        // Case 1: OS keyring available -> legacy inline token moves into keyring.
        let dir_kr = temp_store_dir("legacy-to-keyring");
        let legacy_json = r#"[{
            "account_name": "LegacyUser",
            "steam_id": 76561198000000002,
            "persona_name": "Legacy",
            "avatar_hash": null,
            "refresh_token": "legacy-refresh-1",
            "guard_data": "legacy-guard-1"
        }]"#;
        write_private(&accounts_path(&dir_kr), legacy_json.as_bytes()).unwrap();
        let keyring = MockKeyring::available();

        let list = load_with(&dir_kr, &keyring);
        assert_eq!(list.len(), 1);
        assert!(list[0].refresh_token.is_empty());
        assert!(!std::fs::read_to_string(accounts_path(&dir_kr)).unwrap().contains("legacy-refresh-1"));
        assert!(!credentials_path(&dir_kr).exists());
        let creds = credentials_with(&dir_kr, &keyring, "legacyuser").unwrap();
        assert_eq!(creds.refresh_token, "legacy-refresh-1");
        assert_eq!(creds.guard_data.as_deref(), Some("legacy-guard-1"));
        let _ = std::fs::remove_dir_all(&dir_kr);

        // Case 2: OS keyring unavailable -> legacy inline token moves into 0600 fallback file (not lost!).
        let dir_fb = temp_store_dir("legacy-to-fallback");
        write_private(&accounts_path(&dir_fb), legacy_json.as_bytes()).unwrap();
        let no_keyring = MockKeyring::unavailable();

        let list_fb = load_with(&dir_fb, &no_keyring);
        assert_eq!(list_fb.len(), 1);
        assert!(!std::fs::read_to_string(accounts_path(&dir_fb)).unwrap().contains("legacy-refresh-1"));
        assert!(credentials_path(&dir_fb).exists());
        let creds_fb = credentials_with(&dir_fb, &no_keyring, "legacyuser").unwrap();
        assert_eq!(creds_fb.refresh_token, "legacy-refresh-1");
        assert_eq!(creds_fb.guard_data.as_deref(), Some("legacy-guard-1"));

        // Removing the account deletes the fallback file too.
        remove_with(&dir_fb, &no_keyring, "legacyuser");
        assert!(!credentials_path(&dir_fb).exists());
        let _ = std::fs::remove_dir_all(&dir_fb);
    }
}
