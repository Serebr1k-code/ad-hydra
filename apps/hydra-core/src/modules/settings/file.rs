//! Bounded private configuration persistence, independent of immutable runtime snapshots.
use super::config::{EditableConfig, validate};
use crate::infrastructure::operations;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub const MAX_BYTES: u64 = 65536;
#[derive(Clone)]
pub struct ConfigFile {
    pub path: Option<PathBuf>,
    pub running_revision: Option<String>,
}
pub struct SavedConfig {
    pub settings: operations::Settings,
    pub revision: String,
}
#[derive(Debug)]
pub enum SaveError {
    Conflict,
    Invalid(String),
    Unavailable,
}
impl ConfigFile {
    pub fn new(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let running_revision = path.as_deref().map(read).transpose()?.map(|c| c.revision);
        Ok(Self {
            path,
            running_revision,
        })
    }
    pub fn read(&self) -> anyhow::Result<Option<SavedConfig>> {
        self.path.as_deref().map(read).transpose()
    }
    pub fn readonly_reason(&self) -> Option<String> {
        let Some(path) = &self.path else {
            return Some(
                "Core was started without --operations; no editable file is configured".into(),
            );
        };
        let Some(parent) = path.parent().map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        }) else {
            return Some("Configuration directory is unavailable".into());
        };
        let Ok(metadata) = std::fs::metadata(parent) else {
            return Some("Configuration directory is unavailable".into());
        };
        let uid = nix::unistd::geteuid().as_raw();
        let groups = nix::unistd::getgroups().unwrap_or_default();
        let mask = if uid == metadata.uid() {
            0o300
        } else if nix::unistd::getegid().as_raw() == metadata.gid()
            || groups.iter().any(|g| g.as_raw() == metadata.gid())
        {
            0o030
        } else {
            0o003
        };
        (uid != 0 && metadata.mode() & mask != mask).then(|| "Configuration directory is not writable by Core; provision a dedicated Core-owned directory".into())
    }
    pub fn save(&self, revision: &str, config: &EditableConfig) -> Result<SavedConfig, SaveError> {
        let path = self.path.as_deref().ok_or(SaveError::Unavailable)?;
        let previous = read(path).map_err(|_| SaveError::Unavailable)?;
        if previous.revision != revision {
            return Err(SaveError::Conflict);
        }
        let settings = config
            .resolve(&previous.settings)
            .map_err(|e| SaveError::Invalid(e.to_string()))?;
        validate(&settings).map_err(|e| SaveError::Invalid(e.to_string()))?;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|_| SaveError::Unavailable)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(SaveError::Invalid("Configuration exceeds 64 KiB".into()));
        }
        // Validate certificate loading and checker initialization before replacing the live file.
        atomic_write(path, revision, &bytes).map_err(|e| {
            if e.to_string() == "configuration changed" {
                SaveError::Conflict
            } else {
                SaveError::Unavailable
            }
        })?;
        Ok(SavedConfig {
            settings,
            revision: digest(&bytes),
        })
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn read(path: &Path) -> anyhow::Result<SavedConfig> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() == 1
            && metadata.len() <= MAX_BYTES,
        "operations config must be private regular file <=64 KiB"
    );
    let file = File::open(path)?;
    let opened = file.metadata()?;
    anyhow::ensure!(
        opened.ino() == metadata.ino() && opened.dev() == metadata.dev(),
        "configuration changed while opening"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "operations config exceeds 64 KiB"
    );
    Ok(SavedConfig {
        settings: serde_json::from_slice(&bytes)?,
        revision: digest(&bytes),
    })
}
fn atomic_write(path: &Path, revision: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        })
        .ok_or_else(|| anyhow::anyhow!("missing configuration directory"))?;
    let original = std::fs::symlink_metadata(path)?;
    let temp = parent.join(format!(".hydra-settings-{}", uuid::Uuid::new_v4()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(temp.clone());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let created = file.metadata()?;
    if original.uid() != created.uid() || original.gid() != created.gid() {
        std::os::unix::fs::chown(&temp, Some(original.uid()), Some(original.gid()))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    // Loading the temporary file resolves external CA material exactly as startup does.
    let loaded = operations::Settings::load(Some(&temp))?;
    if let Some(checker) = loaded.flags().checker {
        checker.validate()?;
    }
    anyhow::ensure!(read(path)?.revision == revision, "configuration changed");
    std::fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
