// SPDX-License-Identifier: AGPL-3.0-or-later

//! Proof of process death using a process-lifetime Linux file lock.
//!
//! This is opt-in and requires a trusted, persistent LOCAL filesystem shared by
//! the processes in one recovery scope. Lock files and the scope marker must
//! never be removed, copied or replaced. Unknown scopes, absent files and any
//! verification failure retain active owners. No clock or Redis lease is proof.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use uuid::Uuid;

// Retaining every initialized scope until process exit is intentional. A registry
// can be dropped/replaced while its WebSocket still has a blocked send; that must
// NEVER release its proof-of-life lock. Static values are not dropped on exit.
static PROCESS_OWNERS: OnceLock<Mutex<HashMap<PathBuf, Arc<OwnerRecovery>>>> = OnceLock::new();

pub(super) struct OwnerRecovery {
    directory: PathBuf,
    scope: Uuid,
    process: Uuid,
    device: u64,
    inode: u64,
    _process_lock: File,
}

impl OwnerRecovery {
    pub(super) fn for_process(directory: &Path) -> io::Result<Arc<Self>> {
        let directory = directory.canonicalize()?;
        let mut owners = PROCESS_OWNERS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| io::Error::other("Connect process owner initialization poisoned"))?;
        if let Some(owner) = owners.get(&directory) {
            return Ok(owner.clone());
        }
        let owner = Arc::new(Self::create(&directory)?);
        owners.insert(directory, owner.clone());
        Ok(owner)
    }

    fn create(directory: &Path) -> io::Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(io::Error::other("Connect owner recovery requires Linux"));
        }
        // Serialize marker initialization across restarting replicas. An incomplete
        // marker after a crash is an error, never grounds to invent a new scope.
        let initialization = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("scope.lock"))?;
        initialization.lock()?;
        let scope_path = directory.join("scope");
        let scope = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&scope_path)
        {
            Ok(mut marker) => {
                let scope = Uuid::new_v4();
                write!(marker, "{scope}")?;
                marker.sync_all()?;
                File::open(directory)?.sync_all()?;
                scope
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let marker = std::fs::read_to_string(scope_path)?;
                Uuid::parse_str(&marker).map_err(io::Error::other)?
            }
            Err(error) => return Err(error),
        };
        let process = Uuid::new_v4();
        let mut process_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.join(process.to_string()))?;
        process_lock.try_lock().map_err(io::Error::other)?;
        // Content plus inode/device checks reject accidentally replaced files.
        write!(process_lock, "{process}")?;
        process_lock.sync_all()?;
        File::open(directory)?.sync_all()?;
        let (device, inode) = file_identity(&process_lock)?;
        Ok(Self {
            directory: directory.to_owned(),
            scope,
            process,
            device,
            inode,
            _process_lock: process_lock,
        })
    }

    pub(super) fn new_socket_owner(&self) -> String {
        format!(
            "lock-v1:{}:{}:{}:{}:{}",
            self.scope,
            self.process,
            self.device,
            self.inode,
            Uuid::new_v4()
        )
    }

    /// A returned guard must remain locked until exact Redis SREM completes.
    pub(super) fn prove_dead(&self, owner: &str) -> io::Result<Option<File>> {
        let fields: Vec<_> = owner.split(':').collect();
        if fields.len() != 6 || fields[0] != "lock-v1" {
            return Ok(None);
        }
        let (Ok(scope), Ok(process), Ok(device), Ok(inode), Ok(_socket)) = (
            Uuid::parse_str(fields[1]),
            Uuid::parse_str(fields[2]),
            fields[3].parse::<u64>(),
            fields[4].parse::<u64>(),
            Uuid::parse_str(fields[5]),
        ) else {
            return Ok(None);
        };
        if scope != self.scope {
            return Ok(None);
        }
        // Open an EXISTING file. Creating a missing lock would manufacture proof.
        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.directory.join(process.to_string()))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if file_identity(&file)? != (device, inode) {
            return Ok(None);
        }
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        let mut contents = String::new();
        (&mut file).take(128).read_to_string(&mut contents)?;
        if contents != process.to_string() {
            return Ok(None);
        }
        Ok(Some(file))
    }
}

#[cfg(target_os = "linux")]
fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(target_os = "linux"))]
fn file_identity(_file: &File) -> io::Result<(u64, u64)> {
    Err(io::Error::other(
        "Connect owner recovery requires Linux local filesystem locks",
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "owner_recovery_tests.rs"]
mod tests;
