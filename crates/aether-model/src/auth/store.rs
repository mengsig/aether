//! Credential stores: in-memory for tests, a locked 0600 JSON file for real use.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::types::{AuthError, Credential, CredentialInfo, CredentialStore, ModifyFn};

#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    inner: Mutex<BTreeMap<String, Credential>>,
}

impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn read(&self, provider: &str) -> Result<Option<Credential>, AuthError> {
        Ok(self.inner.lock().await.get(provider).cloned())
    }

    async fn list(&self) -> Result<Vec<CredentialInfo>, AuthError> {
        Ok(self
            .inner
            .lock()
            .await
            .iter()
            .map(|(p, c)| info(p, c))
            .collect())
    }

    async fn modify(&self, provider: &str, f: ModifyFn) -> Result<Option<Credential>, AuthError> {
        let mut map = self.inner.lock().await;
        let current = map.get(provider).cloned();
        let next = f(current.clone()).await?;
        if let Some(cred) = &next {
            map.insert(provider.to_owned(), cred.clone());
        }
        Ok(next.or(current))
    }

    async fn delete(&self, provider: &str) -> Result<(), AuthError> {
        self.inner.lock().await.remove(provider);
        Ok(())
    }
}

fn info(provider: &str, cred: &Credential) -> CredentialInfo {
    CredentialInfo {
        provider: provider.to_owned(),
        kind: cred.kind(),
        expires_at: cred.expires_at(),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, Credential>,
}

const FORMAT_VERSION: u32 = 1;

/// `credentials.json` with mode 0600, written atomically, guarded by an
/// in-process mutex and an advisory lock file shared across processes.
#[derive(Debug)]
pub struct FileCredentialStore {
    path: PathBuf,
    guard: Mutex<()>,
}

impl FileCredentialStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            guard: Mutex::new(()),
        }
    }

    pub fn in_config_dir(dir: &Path) -> Self {
        Self::new(dir.join("credentials.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock_path(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".lock");
        PathBuf::from(p)
    }

    /// Acquire the cross-process lock. The returned file holds it until dropped.
    async fn lock_file(&self) -> Result<File, AuthError> {
        let path = self.lock_path();
        tokio::task::spawn_blocking(move || {
            if let Some(dir) = path.parent() {
                create_private_dir(dir)?;
            }
            let file = open_lock_file(&path)?;
            file.lock()
                .map_err(|e| AuthError::Store(format!("lock {}: {e}", path.display())))?;
            Ok(file)
        })
        .await
        .map_err(|e| AuthError::Store(format!("lock task: {e}")))?
    }

    fn load(&self) -> Result<FileFormat, AuthError> {
        match fs::read(&self.path) {
            Ok(bytes) if bytes.is_empty() => Ok(FileFormat::default()),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| AuthError::Store(format!("parse {}: {e}", self.path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileFormat::default()),
            Err(e) => Err(AuthError::Store(format!(
                "read {}: {e}",
                self.path.display()
            ))),
        }
    }

    fn save(&self, data: &FileFormat) -> Result<(), AuthError> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| AuthError::Store("credentials path has no parent".into()))?;
        create_private_dir(dir)?;
        let bytes = serde_json::to_vec_pretty(data)
            .map_err(|e| AuthError::Store(format!("serialize: {e}")))?;
        // NamedTempFile uses exclusive, unpredictable creation in this same
        // directory and persists with the platform's replace-existing rename.
        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .map_err(|e| AuthError::Store(format!("create temporary credentials file: {e}")))?;
        let tmp_path = tmp.path().to_path_buf();
        tmp.write_all(&bytes)
            .map_err(|e| AuthError::Store(format!("write {}: {e}", tmp_path.display())))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| AuthError::Store(format!("sync {}: {e}", tmp_path.display())))?;
        tmp.persist(&self.path).map_err(|e| {
            AuthError::Store(format!(
                "persist {} -> {}: {}",
                tmp_path.display(),
                self.path.display(),
                e.error
            ))
        })?;
        Ok(())
    }
}

fn open_lock_file(path: &Path) -> Result<File, AuthError> {
    let mut create = OpenOptions::new();
    create.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    match create.open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)
                .map_err(|e| AuthError::Store(format!("inspect lock {}: {e}", path.display())))?;
            if !metadata.file_type().is_file() {
                return Err(AuthError::Store(format!(
                    "lock {} is not a regular file",
                    path.display()
                )));
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|e| AuthError::Store(format!("open lock {}: {e}", path.display())))?;
            // The parent is verified as non-writable by other users before
            // this open, so an untrusted process cannot replace this checked
            // entry between lstat and open.
            if !file
                .metadata()
                .map_err(|e| AuthError::Store(format!("inspect lock {}: {e}", path.display())))?
                .is_file()
            {
                return Err(AuthError::Store(format!(
                    "lock {} is not a regular file",
                    path.display()
                )));
            }
            Ok(file)
        }
        Err(error) => Err(AuthError::Store(format!(
            "open lock {}: {error}",
            path.display()
        ))),
    }
}

fn create_private_dir(dir: &Path) -> Result<(), AuthError> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) => return validate_private_dir(dir, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(AuthError::Store(format!(
                "inspect directory {}: {error}",
                dir.display()
            )));
        }
    }

    if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| AuthError::Store(format!("create {}: {e}", parent.display())))?;
    }
    #[cfg(unix)]
    let create_result = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(dir)
    };
    #[cfg(not(unix))]
    let create_result = fs::create_dir(dir);

    match create_result {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(dir).map_err(|e| {
                AuthError::Store(format!("inspect directory {}: {e}", dir.display()))
            })?;
            return validate_private_dir(dir, &metadata);
        }
        Err(error) => {
            return Err(AuthError::Store(format!(
                "create {}: {error}",
                dir.display()
            )));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| AuthError::Store(format!("secure directory {}: {e}", dir.display())))?;
    }
    Ok(())
}

/// Check the directory without reading credentials or creating a lock file.
pub fn validate_credentials_dir(dir: &Path) -> Result<(), AuthError> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) => validate_private_dir(dir, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AuthError::Store(format!(
            "inspect directory {}: {error}",
            dir.display()
        ))),
    }
}

/// A shell-quoted repair command for an existing directory.
pub fn credentials_dir_fix(dir: &Path) -> String {
    let path = dir.to_string_lossy();
    format!("chmod 700 -- '{}'", path.replace('\'', "'\\''"))
}

fn validate_private_dir(dir: &Path, metadata: &fs::Metadata) -> Result<(), AuthError> {
    if metadata.file_type().is_symlink() {
        return Err(AuthError::Store(format!(
            "credentials parent {} must not be a symlink",
            dir.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(AuthError::Store(format!(
            "credentials parent {} is not a directory",
            dir.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o022 != 0 {
            return Err(AuthError::Store(format!(
                "credentials parent {} is group- or world-writable; run `{}`",
                dir.display(),
                credentials_dir_fix(dir)
            )));
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl CredentialStore for FileCredentialStore {
    async fn read(&self, provider: &str) -> Result<Option<Credential>, AuthError> {
        let _guard = self.guard.lock().await;
        let _lock = self.lock_file().await?;
        Ok(self.load()?.providers.get(provider).cloned())
    }

    async fn list(&self) -> Result<Vec<CredentialInfo>, AuthError> {
        let _guard = self.guard.lock().await;
        let _lock = self.lock_file().await?;
        Ok(self
            .load()?
            .providers
            .iter()
            .map(|(p, c)| info(p, c))
            .collect())
    }

    async fn modify(&self, provider: &str, f: ModifyFn) -> Result<Option<Credential>, AuthError> {
        let _guard = self.guard.lock().await;
        let _lock = self.lock_file().await?;
        let mut data = self.load()?;
        let current = data.providers.get(provider).cloned();
        let next = f(current.clone()).await?;
        if let Some(cred) = &next {
            data.providers.insert(provider.to_owned(), cred.clone());
            data.version = FORMAT_VERSION;
            self.save(&data)?;
        }
        Ok(next.or(current))
    }

    async fn delete(&self, provider: &str) -> Result<(), AuthError> {
        let _guard = self.guard.lock().await;
        let _lock = self.lock_file().await?;
        let mut data = self.load()?;
        if data.providers.remove(provider).is_some() {
            data.version = FORMAT_VERSION;
            self.save(&data)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::types::AuthKind;

    fn key(k: &str) -> Credential {
        Credential::ApiKey {
            key: k.into(),
            env: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn file_store_roundtrip_and_permissions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileCredentialStore::in_config_dir(&dir.path().join("aether"));
        assert!(store.read("anthropic").await.expect("read").is_none());

        let stored = store
            .modify(
                "anthropic",
                Box::new(|_| Box::pin(async { Ok(Some(key("sk-1"))) })),
            )
            .await
            .expect("modify");
        assert_eq!(stored, Some(key("sk-1")));
        assert_eq!(
            store.read("anthropic").await.expect("read"),
            Some(key("sk-1"))
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path())
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Unchanged when the callback returns None.
        let same = store
            .modify(
                "anthropic",
                Box::new(|cur| {
                    Box::pin(async move {
                        assert_eq!(cur, Some(key("sk-1")));
                        Ok(None)
                    })
                }),
            )
            .await
            .expect("modify");
        assert_eq!(same, Some(key("sk-1")));

        let list = store.list().await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, AuthKind::ApiKey);

        store.delete("anthropic").await.expect("delete");
        assert!(store.read("anthropic").await.expect("read").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_does_not_follow_the_old_predictable_temp_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileCredentialStore::in_config_dir(dir.path());
        let target = dir.path().join("attacker-controlled-target");
        fs::write(&target, b"keep this file unchanged").expect("target");
        let planted = dir
            .path()
            .join(format!(".credentials.{}.tmp", std::process::id()));
        symlink(&target, &planted).expect("plant old predictable temp symlink");

        store
            .modify(
                "anthropic",
                Box::new(|_| Box::pin(async { Ok(Some(key("secret-value"))) })),
            )
            .await
            .expect("save credential");

        assert_eq!(
            fs::read(&target).expect("target bytes"),
            b"keep this file unchanged"
        );
        assert_eq!(
            store.read("anthropic").await.expect("read credential"),
            Some(key("secret-value"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_an_existing_group_or_world_writable_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("shared");
        fs::create_dir(&config).expect("config dir");
        fs::set_permissions(&config, fs::Permissions::from_mode(0o777)).expect("make permissive");
        let store = FileCredentialStore::in_config_dir(&config);
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_called = called.clone();
        let result = store
            .modify(
                "anthropic",
                Box::new(move |_| {
                    Box::pin(async move {
                        callback_called.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(Some(key("secret-value")))
                    })
                }),
            )
            .await;

        assert!(result.is_err());
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!store.path().exists());
        assert_eq!(
            fs::metadata(&config)
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o777
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_a_symlink_credentials_parent() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let actual = dir.path().join("actual");
        fs::create_dir(&actual).expect("actual directory");
        let alias = dir.path().join("alias");
        symlink(&actual, &alias).expect("directory symlink");
        let store = FileCredentialStore::in_config_dir(&alias);

        assert!(store.read("anthropic").await.is_err());
        assert!(!store.path().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_a_preexisting_lock_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileCredentialStore::in_config_dir(dir.path());
        let target = dir.path().join("target");
        fs::write(&target, b"do not change").expect("target");
        symlink(&target, store.lock_path()).expect("lock symlink");

        assert!(store.read("anthropic").await.is_err());
        assert_eq!(fs::read(&target).expect("target bytes"), b"do not change");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn newly_created_config_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("aether");
        let store = FileCredentialStore::in_config_dir(&config);
        store
            .modify(
                "anthropic",
                Box::new(|_| Box::pin(async { Ok(Some(key("secret-value"))) })),
            )
            .await
            .expect("save credential");
        assert_eq!(
            fs::metadata(config)
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[tokio::test]
    async fn callback_errors_do_not_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileCredentialStore::in_config_dir(dir.path());
        let err = store
            .modify(
                "x",
                Box::new(|_| Box::pin(async { Err(AuthError::Flow("nope".into())) })),
            )
            .await
            .unwrap_err();
        assert_eq!(err, AuthError::Flow("nope".into()));
        assert!(!store.path().exists());
    }

    #[tokio::test]
    async fn concurrent_modifies_serialize() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = std::sync::Arc::new(FileCredentialStore::in_config_dir(dir.path()));
        let mut handles = Vec::new();
        for i in 0..8 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                store
                    .modify(
                        &format!("p{i}"),
                        Box::new(move |_| Box::pin(async move { Ok(Some(key(&format!("k{i}")))) })),
                    )
                    .await
                    .expect("modify");
            }));
        }
        for h in handles {
            h.await.expect("join");
        }
        assert_eq!(store.list().await.expect("list").len(), 8);
    }

    #[tokio::test]
    async fn memory_store_behaves_like_file_store() {
        let store = MemoryCredentialStore::new();
        store
            .modify("a", Box::new(|_| Box::pin(async { Ok(Some(key("1"))) })))
            .await
            .expect("modify");
        assert_eq!(store.read("a").await.expect("read"), Some(key("1")));
        store.delete("a").await.expect("delete");
        assert!(store.list().await.expect("list").is_empty());
    }
}
