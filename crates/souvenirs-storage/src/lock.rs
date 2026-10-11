use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};

use crate::catalog::CatalogLayout;
use crate::durability::{sync_file, sync_parent};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockHolder {
    pub host: String,
    pub machine_id: String,
    pub boot_id: String,
    pub process_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_start_id: Option<String>,
    pub started_at_unix_seconds: u64,
    pub nonce: String,
}

pub trait LockEnvironment {
    fn host_name(&self) -> Result<String, LockError>;
    fn machine_id(&self) -> Result<String, LockError>;
    fn boot_id(&self) -> Result<String, LockError>;
    fn process_id(&self) -> u32;
    fn process_is_running(&self, process_id: u32) -> Result<bool, LockError>;
    fn process_start_id(&self, process_id: u32) -> Result<Option<String>, LockError>;
    fn unix_seconds(&self) -> Result<u64, LockError>;
    fn nonce(&self) -> Result<String, LockError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLockEnvironment;

impl LockEnvironment for SystemLockEnvironment {
    fn host_name(&self) -> Result<String, LockError> {
        let output =
            Command::new("hostname")
                .output()
                .map_err(|source| LockError::Environment {
                    action: "read host name",
                    source,
                })?;
        if !output.status.success() {
            return Err(LockError::HostNameUnavailable);
        }
        let host = String::from_utf8(output.stdout)
            .map_err(|_| LockError::HostNameUnavailable)?
            .trim()
            .to_owned();
        if host.is_empty() {
            return Err(LockError::HostNameUnavailable);
        }
        Ok(host)
    }

    fn machine_id(&self) -> Result<String, LockError> {
        system_machine_id()
    }

    fn boot_id(&self) -> Result<String, LockError> {
        system_boot_id()
    }

    fn process_id(&self) -> u32 {
        std::process::id()
    }

    fn process_is_running(&self, process_id: u32) -> Result<bool, LockError> {
        let output = Command::new("ps")
            .args(["-p", &process_id.to_string(), "-o", "pid="])
            .output()
            .map_err(|source| LockError::Environment {
                action: "inspect lock process",
                source,
            })?;
        match (output.status.code(), output.stdout.is_empty()) {
            (Some(0), false) => Ok(true),
            (Some(1), true) => Ok(false),
            _ => Err(LockError::Environment {
                action: "inspect lock process",
                source: io::Error::other(format!("ps exited with {}", output.status)),
            }),
        }
    }

    fn process_start_id(&self, process_id: u32) -> Result<Option<String>, LockError> {
        system_process_start_id(process_id)
    }

    fn unix_seconds(&self) -> Result<u64, LockError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|source| LockError::Clock {
                details: source.to_string(),
            })
    }

    fn nonce(&self) -> Result<String, LockError> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|source| LockError::Clock {
                details: source.to_string(),
            })?;
        Ok(format!(
            "{}-{}-{}",
            std::process::id(),
            duration.as_secs(),
            duration.subsec_nanos()
        ))
    }
}

#[derive(Debug)]
pub struct CatalogLock {
    path: PathBuf,
    holder: LockHolder,
    released: bool,
}

impl CatalogLock {
    pub fn acquire(layout: &CatalogLayout) -> Result<Self, LockError> {
        Self::acquire_with(layout, &SystemLockEnvironment)
    }

    pub fn acquire_with(
        layout: &CatalogLayout,
        environment: &impl LockEnvironment,
    ) -> Result<Self, LockError> {
        Self::acquire_paths(layout.lock_path(), layout.lock_takeover_path(), environment)
    }

    pub(crate) fn acquire_paths(
        path: PathBuf,
        takeover_path: PathBuf,
        environment: &impl LockEnvironment,
    ) -> Result<Self, LockError> {
        Self::acquire_paths_with_takeover(path, takeover_path, environment).map(|(lock, _)| lock)
    }

    pub(crate) fn acquire_paths_with_takeover(
        path: PathBuf,
        takeover_path: PathBuf,
        environment: &impl LockEnvironment,
    ) -> Result<(Self, TakeoverGuard), LockError> {
        let takeover =
            TakeoverGuard::acquire(&takeover_path, environment).map_err(|error| match error {
                LockError::TakeoverBusy => report_takeover_busy(&path),
                error => error,
            })?;
        let holder = current_holder(environment)?;

        match write_holder_exclusively(&path, &holder) {
            Ok(()) => {
                return Ok((
                    Self {
                        path,
                        holder,
                        released: false,
                    },
                    takeover,
                ))
            }
            Err(LockError::Io { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }

        let existing = read_holder(&path)?;
        if holder_is_live(&existing, &holder, environment)? {
            return Err(LockError::Busy(Box::new(existing)));
        }

        let confirmed = read_holder(&path)?;
        if confirmed != existing {
            return Err(LockError::Busy(Box::new(confirmed)));
        }
        remove_and_persist(&path)?;
        if let Err(error) = write_holder_exclusively(&path, &holder) {
            if matches!(
                &error,
                LockError::Io { source, .. }
                    if source.kind() == io::ErrorKind::AlreadyExists
            ) {
                return Err(LockError::Busy(Box::new(read_holder(&path)?)));
            }
            return Err(error);
        }
        Ok((
            Self {
                path,
                holder,
                released: false,
            },
            takeover,
        ))
    }

    pub fn holder(&self) -> &LockHolder {
        &self.holder
    }

    pub(crate) fn guards(&self, layout: &CatalogLayout) -> Result<bool, LockError> {
        if self.released {
            return Ok(false);
        }

        let owned_path = fs::canonicalize(&self.path).map_err(|source| LockError::Io {
            action: "resolve owned catalog lock",
            path: self.path.clone(),
            source,
        })?;
        let requested_path =
            fs::canonicalize(layout.lock_path()).map_err(|source| LockError::Io {
                action: "resolve requested catalog lock",
                path: layout.lock_path(),
                source,
            })?;
        if owned_path != requested_path {
            return Ok(false);
        }

        Ok(read_holder(&self.path)? == self.holder)
    }

    pub fn release(mut self) -> Result<(), LockError> {
        self.release_inner()?;
        self.released = true;
        Ok(())
    }

    #[cfg(feature = "test-failpoints")]
    pub(crate) fn abandon(mut self) {
        self.released = true;
    }

    pub fn remove_existing(layout: &CatalogLayout, expected: &LockHolder) -> Result<(), LockError> {
        Self::remove_existing_with(layout, expected, &SystemLockEnvironment)
    }

    pub fn remove_existing_with(
        layout: &CatalogLayout,
        expected: &LockHolder,
        environment: &impl LockEnvironment,
    ) -> Result<(), LockError> {
        let _takeover = TakeoverGuard::acquire(&layout.lock_takeover_path(), environment)?;
        let actual = read_holder(&layout.lock_path())?;
        if &actual != expected {
            return Err(LockError::Changed {
                expected: Box::new(expected.clone()),
                actual: Box::new(actual),
            });
        }
        remove_and_persist(&layout.lock_path())
    }

    pub fn remove_corrupt_existing(
        layout: &CatalogLayout,
        expected_contents: &[u8],
    ) -> Result<(), LockError> {
        Self::remove_corrupt_existing_with(layout, expected_contents, &SystemLockEnvironment)
    }

    pub fn remove_corrupt_existing_with(
        layout: &CatalogLayout,
        expected_contents: &[u8],
        environment: &impl LockEnvironment,
    ) -> Result<(), LockError> {
        let _takeover = TakeoverGuard::acquire(&layout.lock_takeover_path(), environment)?;
        let actual = read_lock_bytes(&layout.lock_path())?;
        if actual != expected_contents {
            return Err(LockError::CorruptChanged {
                expected: expected_contents.to_vec().into_boxed_slice(),
                actual: actual.into_boxed_slice(),
            });
        }
        remove_and_persist(&layout.lock_path())
    }

    fn release_inner(&self) -> Result<(), LockError> {
        let actual = match read_holder(&self.path) {
            Ok(holder) => holder,
            Err(LockError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(())
            }
            Err(error) => return Err(error),
        };
        if actual != self.holder {
            return Err(LockError::Changed {
                expected: Box::new(self.holder.clone()),
                actual: Box::new(actual),
            });
        }
        remove_and_persist(&self.path)
    }
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.release_inner();
        }
    }
}

#[derive(Debug)]
pub(crate) struct TakeoverGuard {
    file: File,
    path: PathBuf,
    _local_claim: LocalTakeoverClaim,
}

#[derive(Debug)]
struct LocalTakeoverClaim {
    path: PathBuf,
}

impl TakeoverGuard {
    pub(crate) fn acquire(
        path: &Path,
        environment: &impl LockEnvironment,
    ) -> Result<Self, LockError> {
        let holder = current_holder(environment)?;
        let path = canonical_sibling_path(path)?;
        let local_claim = LocalTakeoverClaim::acquire(&path)?;
        let mut file = loop {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|source| LockError::Io {
                    action: "open lock takeover guard",
                    path: path.clone(),
                    source,
                })?;
            if let Err(source) =
                rustix::fs::fcntl_lock(&file, FlockOperation::NonBlockingLockExclusive)
            {
                return Err(if source == rustix::io::Errno::WOULDBLOCK {
                    LockError::TakeoverBusy
                } else {
                    LockError::Io {
                        action: "acquire lock takeover guard",
                        path,
                        source: io::Error::from(source),
                    }
                });
            }
            if file_identifies_path(&file, &path)? {
                break file;
            }
        };
        file.set_len(0).map_err(|source| LockError::Io {
            action: "truncate lock takeover guard",
            path: path.clone(),
            source,
        })?;
        write_holder(&mut file, &path, &holder)?;
        sync_parent(&path).map_err(|source| LockError::Io {
            action: "persist lock takeover guard",
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            file,
            path,
            _local_claim: local_claim,
        })
    }

    pub(crate) fn remove(self) -> Result<(), LockError> {
        if !file_identifies_path(&self.file, &self.path)? {
            return Err(LockError::Io {
                action: "remove owned lock takeover guard",
                path: self.path.clone(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "lock takeover path no longer identifies the owned file",
                ),
            });
        }
        remove_and_persist(&self.path)
    }
}

impl LocalTakeoverClaim {
    fn acquire(path: &Path) -> Result<Self, LockError> {
        claim_local_takeover(path)?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for LocalTakeoverClaim {
    fn drop(&mut self) {
        release_local_takeover(&self.path);
    }
}

fn holder_is_live(
    existing: &LockHolder,
    contender: &LockHolder,
    environment: &impl LockEnvironment,
) -> Result<bool, LockError> {
    if existing.machine_id != contender.machine_id {
        return Ok(true);
    }
    if existing.boot_id != contender.boot_id {
        return Ok(false);
    }

    if !environment.process_is_running(existing.process_id)? {
        return Ok(false);
    }
    let Some(recorded_start_id) = existing.process_start_id.as_deref() else {
        return Ok(true);
    };
    let Some(observed_start_id) = environment.process_start_id(existing.process_id)? else {
        return Ok(true);
    };
    Ok(observed_start_id == recorded_start_id)
}

fn current_holder(environment: &impl LockEnvironment) -> Result<LockHolder, LockError> {
    let process_id = environment.process_id();
    Ok(LockHolder {
        host: environment.host_name()?,
        machine_id: environment.machine_id()?,
        boot_id: environment.boot_id()?,
        process_id,
        process_start_id: environment.process_start_id(process_id)?,
        started_at_unix_seconds: environment.unix_seconds()?,
        nonce: environment.nonce()?,
    })
}

fn canonical_sibling_path(path: &Path) -> Result<PathBuf, LockError> {
    let parent = path.parent().ok_or_else(|| LockError::Io {
        action: "resolve lock takeover parent",
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "lock path has no parent"),
    })?;
    let parent = fs::canonicalize(parent).map_err(|source| LockError::Io {
        action: "canonicalize lock takeover directory",
        path: parent.to_path_buf(),
        source,
    })?;
    let name = path.file_name().ok_or_else(|| LockError::Io {
        action: "resolve lock takeover name",
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "lock path has no file name"),
    })?;
    Ok(parent.join(name))
}

fn local_takeovers() -> &'static Mutex<HashSet<PathBuf>> {
    static TAKEOVERS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    TAKEOVERS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn claim_local_takeover(path: &Path) -> Result<(), LockError> {
    let mut takeovers = local_takeovers()
        .lock()
        .map_err(|_| LockError::TakeoverStatePoisoned)?;
    if !takeovers.insert(path.to_path_buf()) {
        return Err(LockError::TakeoverBusy);
    }
    Ok(())
}

fn release_local_takeover(path: &Path) {
    if let Ok(mut takeovers) = local_takeovers().lock() {
        takeovers.remove(path);
    }
}

fn report_takeover_busy(path: &Path) -> LockError {
    read_holder(path)
        .map(|holder| LockError::Busy(Box::new(holder)))
        .unwrap_or(LockError::TakeoverBusy)
}

fn file_identifies_path(file: &File, path: &Path) -> Result<bool, LockError> {
    let open = file.metadata().map_err(|source| LockError::Io {
        action: "inspect open lock takeover guard",
        path: path.to_path_buf(),
        source,
    })?;
    let current = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Err(LockError::Io {
                action: "inspect current lock takeover guard",
                path: path.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "lock takeover path is not a regular file",
                ),
            })
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(LockError::Io {
                action: "inspect current lock takeover guard",
                path: path.to_path_buf(),
                source,
            })
        }
    };
    Ok(open.dev() == current.dev() && open.ino() == current.ino())
}

fn write_holder_exclusively(path: &Path, holder: &LockHolder) -> Result<(), LockError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| LockError::Io {
            action: "create lock exclusively",
            path: path.to_path_buf(),
            source,
        })?;

    let result = write_holder(&mut file, path, holder);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result?;
    sync_parent(path).map_err(|source| LockError::Io {
        action: "persist lock directory",
        path: path.to_path_buf(),
        source,
    })
}

fn write_holder(file: &mut File, path: &Path, holder: &LockHolder) -> Result<(), LockError> {
    serde_json::to_writer(&mut *file, holder).map_err(|source| LockError::Serialize {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(b"\n")
        .and_then(|()| sync_file(file))
        .map_err(|source| LockError::Io {
            action: "write and persist lock",
            path: path.to_path_buf(),
            source,
        })
}

fn read_holder(path: &Path) -> Result<LockHolder, LockError> {
    let bytes = read_lock_bytes(path)?;
    serde_json::from_slice(&bytes).map_err(|source| LockError::Corrupt {
        path: path.to_path_buf(),
        contents: bytes.into_boxed_slice(),
        source,
    })
}

fn read_lock_bytes(path: &Path) -> Result<Vec<u8>, LockError> {
    fs::read(path).map_err(|source| LockError::Io {
        action: "read lock",
        path: path.to_path_buf(),
        source,
    })
}

fn remove_and_persist(path: &Path) -> Result<(), LockError> {
    fs::remove_file(path).map_err(|source| LockError::Io {
        action: "remove lock",
        path: path.to_path_buf(),
        source,
    })?;
    sync_parent(path).map_err(|source| LockError::Io {
        action: "persist lock removal",
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug)]
pub enum LockError {
    Busy(Box<LockHolder>),
    BootIdUnavailable,
    Changed {
        expected: Box<LockHolder>,
        actual: Box<LockHolder>,
    },
    Clock {
        details: String,
    },
    Corrupt {
        path: PathBuf,
        contents: Box<[u8]>,
        source: serde_json::Error,
    },
    CorruptChanged {
        expected: Box<[u8]>,
        actual: Box<[u8]>,
    },
    Environment {
        action: &'static str,
        source: io::Error,
    },
    HostNameUnavailable,
    MachineIdUnavailable,
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Serialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    TakeoverBusy,
    TakeoverStatePoisoned,
}

impl fmt::Display for LockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(holder) => write!(
                formatter,
                "catalog is busy: {} process {} holds the lock since {}",
                holder.host, holder.process_id, holder.started_at_unix_seconds
            ),
            Self::BootIdUnavailable => formatter.write_str("boot identifier is unavailable"),
            Self::Changed { expected, actual } => write!(
                formatter,
                "catalog lock changed from {} process {} to {} process {}; refusing removal",
                expected.host, expected.process_id, actual.host, actual.process_id
            ),
            Self::Clock { details } => {
                write!(formatter, "failed to read the system clock: {details}")
            }
            Self::Corrupt { path, source, .. } => {
                write!(
                    formatter,
                    "catalog lock {} is corrupt: {source}",
                    path.display()
                )
            }
            Self::CorruptChanged { .. } => formatter
                .write_str("catalog lock contents changed; refusing removal of the corrupt lock"),
            Self::Environment { action, source } => write!(formatter, "{action}: {source}"),
            Self::HostNameUnavailable => formatter.write_str("host name is unavailable"),
            Self::MachineIdUnavailable => {
                formatter.write_str("stable machine identifier is unavailable")
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Serialize { path, source } => {
                write!(
                    formatter,
                    "failed to serialize lock {}: {source}",
                    path.display()
                )
            }
            Self::TakeoverBusy => formatter.write_str("lock takeover is busy"),
            Self::TakeoverStatePoisoned => {
                formatter.write_str("local lock takeover state is unavailable")
            }
        }
    }
}

impl Error for LockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Corrupt { source, .. } | Self::Serialize { source, .. } => Some(source),
            Self::Environment { source, .. } | Self::Io { source, .. } => Some(source),
            Self::Busy(_)
            | Self::BootIdUnavailable
            | Self::Changed { .. }
            | Self::Clock { .. }
            | Self::CorruptChanged { .. }
            | Self::HostNameUnavailable
            | Self::MachineIdUnavailable
            | Self::TakeoverBusy
            | Self::TakeoverStatePoisoned => None,
        }
    }
}

#[cfg(target_os = "linux")]
fn system_machine_id() -> Result<String, LockError> {
    let value = fs::read_to_string("/etc/machine-id").map_err(|source| LockError::Environment {
        action: "read machine identifier",
        source,
    })?;
    normalized_machine_id(value)
}

#[cfg(target_os = "macos")]
fn system_machine_id() -> Result<String, LockError> {
    let output = Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .map_err(|source| LockError::Environment {
            action: "read machine identifier",
            source,
        })?;
    if !output.status.success() {
        return Err(LockError::MachineIdUnavailable);
    }
    let output = String::from_utf8(output.stdout).map_err(|_| LockError::MachineIdUnavailable)?;
    let value = output
        .lines()
        .find_map(|line| line.split_once("\"IOPlatformUUID\" = \""))
        .and_then(|(_, value)| value.strip_suffix('"'))
        .ok_or(LockError::MachineIdUnavailable)?;
    normalized_machine_id(value.to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system_machine_id() -> Result<String, LockError> {
    SystemLockEnvironment.host_name()
}

fn normalized_machine_id(value: String) -> Result<String, LockError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(LockError::MachineIdUnavailable)
    } else {
        Ok(value)
    }
}

#[cfg(target_os = "linux")]
fn system_boot_id() -> Result<String, LockError> {
    let value = fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|source| {
        LockError::Environment {
            action: "read boot identifier",
            source,
        }
    })?;
    normalized_boot_id(value)
}

#[cfg(target_os = "macos")]
fn system_boot_id() -> Result<String, LockError> {
    let output = Command::new("sysctl")
        .args(["-n", "kern.bootsessionuuid"])
        .output()
        .map_err(|source| LockError::Environment {
            action: "read boot identifier",
            source,
        })?;
    if !output.status.success() {
        return Err(LockError::BootIdUnavailable);
    }
    let value = String::from_utf8(output.stdout).map_err(|_| LockError::BootIdUnavailable)?;
    normalized_boot_id(value)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system_boot_id() -> Result<String, LockError> {
    Err(LockError::BootIdUnavailable)
}

fn normalized_boot_id(value: String) -> Result<String, LockError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(LockError::BootIdUnavailable)
    } else {
        Ok(value)
    }
}

#[cfg(target_os = "linux")]
fn system_process_start_id(process_id: u32) -> Result<Option<String>, LockError> {
    let path = PathBuf::from(format!("/proc/{process_id}/stat"));
    let value = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LockError::Environment {
                action: "read process identity",
                source,
            })
        }
    };
    let fields = value
        .rsplit_once(") ")
        .map(|(_, fields)| fields)
        .ok_or_else(|| LockError::Environment {
            action: "read process identity",
            source: io::Error::new(io::ErrorKind::InvalidData, "invalid /proc process record"),
        })?;
    let start_ticks = fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| LockError::Environment {
            action: "read process identity",
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "process start field is unavailable",
            ),
        })?;
    Ok(Some(format!("linux:{start_ticks}")))
}

#[cfg(target_os = "macos")]
fn system_process_start_id(process_id: u32) -> Result<Option<String>, LockError> {
    use proc_pidinfo::{proc_pidinfo, Pid, ProcBSDInfo};

    proc_pidinfo::<ProcBSDInfo>(Pid(process_id))
        .map_err(|source| LockError::Environment {
            action: "read process identity",
            source,
        })
        .map(|info| {
            info.map(|info| {
                format!(
                    "macos:{}:{:06}",
                    info.pbi_start_tvsec, info.pbi_start_tvusec
                )
            })
        })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system_process_start_id(_process_id: u32) -> Result<Option<String>, LockError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{
        file_identifies_path, CatalogLock, LockEnvironment, LockError, LockHolder, TakeoverGuard,
    };
    use crate::catalog::{CatalogConfig, CatalogLayout};
    use crate::test_support::TempDirectory;
    use std::fs;

    #[derive(Clone)]
    struct FakeEnvironment {
        boot_id: &'static str,
        process_id: u32,
        processes: Vec<(u32, Option<&'static str>)>,
        nonce: &'static str,
    }

    impl LockEnvironment for FakeEnvironment {
        fn host_name(&self) -> Result<String, LockError> {
            Ok("computer".to_owned())
        }

        fn machine_id(&self) -> Result<String, LockError> {
            Ok("machine-a".to_owned())
        }

        fn boot_id(&self) -> Result<String, LockError> {
            Ok(self.boot_id.to_owned())
        }

        fn process_id(&self) -> u32 {
            self.process_id
        }

        fn process_is_running(&self, process_id: u32) -> Result<bool, LockError> {
            Ok(self
                .processes
                .iter()
                .any(|(candidate, _)| *candidate == process_id))
        }

        fn process_start_id(&self, process_id: u32) -> Result<Option<String>, LockError> {
            match self
                .processes
                .iter()
                .find(|(candidate, _)| *candidate == process_id)
            {
                Some((_, Some(start_id))) => Ok(Some((*start_id).to_owned())),
                Some((_, None)) | None => Ok(None),
            }
        }

        fn unix_seconds(&self) -> Result<u64, LockError> {
            Ok(1_700_000_000)
        }

        fn nonce(&self) -> Result<String, LockError> {
            Ok(self.nonce.to_owned())
        }
    }

    fn environment(
        boot_id: &'static str,
        process_id: u32,
        nonce: &'static str,
        processes: &[(u32, Option<&'static str>)],
    ) -> FakeEnvironment {
        FakeEnvironment {
            boot_id,
            process_id,
            processes: processes.to_vec(),
            nonce,
        }
    }

    fn holder(boot_id: &str, process_id: u32, process_start_id: Option<&str>) -> LockHolder {
        LockHolder {
            host: "computer".to_owned(),
            machine_id: "machine-a".to_owned(),
            boot_id: boot_id.to_owned(),
            process_id,
            process_start_id: process_start_id.map(str::to_owned),
            started_at_unix_seconds: 1,
            nonce: "holder".to_owned(),
        }
    }

    fn write_holder(layout: &CatalogLayout, holder: &LockHolder) {
        fs::write(
            layout.lock_path(),
            format!("{}\n", serde_json::to_string(holder).unwrap()),
        )
        .unwrap();
    }

    fn initialized_catalog(name: &str) -> (TempDirectory, CatalogLayout) {
        let temporary = TempDirectory::new(name);
        let layout =
            CatalogLayout::initialize(temporary.path(), &CatalogConfig::default()).unwrap();
        (temporary, layout)
    }

    #[test]
    fn exclusive_lock_reports_the_current_holder() {
        let (_temporary, layout) = initialized_catalog("lock-busy");
        let first_environment = environment("boot-a", 41, "first", &[(41, Some("process-41"))]);
        let contender_environment = environment(
            "boot-a",
            42,
            "second",
            &[(41, Some("process-41")), (42, Some("process-42"))],
        );
        let first = CatalogLock::acquire_with(&layout, &first_environment).unwrap();

        let error = CatalogLock::acquire_with(&layout, &contender_environment).unwrap_err();

        assert!(matches!(
            error,
            LockError::Busy(holder) if *holder == *first.holder()
        ));
    }

    #[test]
    fn removes_stale_lock_from_dead_process_on_same_host() {
        let (_temporary, layout) = initialized_catalog("lock-stale");
        write_holder(&layout, &holder("boot-a", 41, Some("process-41")));
        let environment = environment("boot-a", 42, "replacement", &[(42, Some("process-42"))]);

        let acquired = CatalogLock::acquire_with(&layout, &environment).unwrap();

        assert_eq!(acquired.holder().process_id, 42);
        assert_eq!(acquired.holder().nonce, "replacement");
    }

    #[test]
    fn removes_same_machine_lock_from_an_earlier_boot_when_pid_is_reused() {
        let (_temporary, layout) = initialized_catalog("lock-reboot");
        write_holder(
            &layout,
            &holder("boot-before-restart", 41, Some("original-process")),
        );
        let environment = environment(
            "boot-after-restart",
            42,
            "replacement",
            &[
                (41, Some("reused-process")),
                (42, Some("contender-process")),
            ],
        );

        let acquired = CatalogLock::acquire_with(&layout, &environment).unwrap();

        assert_eq!(acquired.holder().boot_id, "boot-after-restart");
        assert_eq!(acquired.holder().process_id, 42);
    }

    #[test]
    fn removes_same_boot_lock_when_pid_is_reused() {
        let (_temporary, layout) = initialized_catalog("lock-pid-reused");
        write_holder(&layout, &holder("boot-a", 41, Some("original-process")));
        let environment = environment(
            "boot-a",
            42,
            "replacement",
            &[
                (41, Some("reused-process")),
                (42, Some("contender-process")),
            ],
        );

        let acquired = CatalogLock::acquire_with(&layout, &environment).unwrap();

        assert_eq!(
            acquired.holder().process_start_id.as_deref(),
            Some("contender-process")
        );
    }

    #[test]
    fn live_original_process_reports_the_recorded_holder() {
        let (_temporary, layout) = initialized_catalog("lock-original-live");
        let original = holder("boot-a", 41, Some("original-process"));
        write_holder(&layout, &original);
        let environment = environment(
            "boot-a",
            42,
            "contender",
            &[
                (41, Some("original-process")),
                (42, Some("contender-process")),
            ],
        );

        let error = CatalogLock::acquire_with(&layout, &environment).unwrap_err();

        assert!(matches!(error, LockError::Busy(actual) if *actual == original));
    }

    #[test]
    fn unavailable_same_boot_identity_refuses_automatic_removal() {
        let (_temporary, layout) = initialized_catalog("lock-identity-unavailable");
        let original = holder("boot-a", 41, Some("original-process"));
        write_holder(&layout, &original);
        let environment = environment(
            "boot-a",
            42,
            "contender",
            &[(41, None), (42, Some("contender-process"))],
        );

        let error = CatalogLock::acquire_with(&layout, &environment).unwrap_err();

        assert!(matches!(error, LockError::Busy(actual) if *actual == original));
        assert!(layout.lock_path().exists());
    }

    #[test]
    fn legacy_live_lock_without_process_identity_remains_busy() {
        let (_temporary, layout) = initialized_catalog("lock-legacy-live");
        let legacy = holder("boot-a", 41, None);
        write_holder(&layout, &legacy);
        let environment = environment(
            "boot-a",
            42,
            "contender",
            &[
                (41, Some("unknown-live-process")),
                (42, Some("contender-process")),
            ],
        );

        let error = CatalogLock::acquire_with(&layout, &environment).unwrap_err();

        assert!(matches!(error, LockError::Busy(actual) if *actual == legacy));
    }

    #[test]
    fn takeover_guard_serializes_stale_lock_removal() {
        let (_temporary, layout) = initialized_catalog("takeover-guard");
        let first_environment = environment("boot-a", 41, "first", &[(41, Some("process-41"))]);
        let second_environment = environment(
            "boot-a",
            42,
            "second",
            &[(41, Some("process-41")), (42, Some("process-42"))],
        );
        let _first =
            TakeoverGuard::acquire(&layout.lock_takeover_path(), &first_environment).unwrap();

        let error =
            TakeoverGuard::acquire(&layout.lock_takeover_path(), &second_environment).unwrap_err();

        assert!(matches!(error, LockError::TakeoverBusy));
        let original = holder("boot-a", 41, Some("process-41"));
        write_holder(&layout, &original);
        let error = CatalogLock::acquire_with(&layout, &second_environment).unwrap_err();
        assert!(matches!(error, LockError::Busy(actual) if *actual == original));
    }

    #[test]
    fn takeover_guard_detects_an_unlinked_open_inode() {
        let temporary = TempDirectory::new("takeover-inode");
        let path = temporary.path().join("takeover");
        fs::write(&path, b"old").unwrap();
        let open = fs::File::open(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"new").unwrap();

        assert!(!file_identifies_path(&open, &path).unwrap());
    }

    #[test]
    fn foreign_lock_requires_explicit_exact_removal() {
        let (_temporary, layout) = initialized_catalog("lock-foreign");
        let foreign = LockHolder {
            host: "other-computer".to_owned(),
            machine_id: "machine-b".to_owned(),
            boot_id: "boot-b".to_owned(),
            process_id: 41,
            process_start_id: Some("foreign-process".to_owned()),
            started_at_unix_seconds: 1,
            nonce: "foreign".to_owned(),
        };
        write_holder(&layout, &foreign);
        let environment = environment("boot-a", 42, "local", &[(42, Some("process-42"))]);

        let error = CatalogLock::acquire_with(&layout, &environment).unwrap_err();
        assert!(matches!(error, LockError::Busy(actual) if *actual == foreign));

        CatalogLock::remove_existing_with(&layout, &foreign, &environment).unwrap();
        assert!(!layout.lock_path().exists());
    }

    #[test]
    fn explicit_removal_works_when_current_process_identity_is_unavailable() {
        let (_temporary, layout) = initialized_catalog("lock-remove-without-identity");
        let foreign = LockHolder {
            host: "other-computer".to_owned(),
            machine_id: "machine-b".to_owned(),
            boot_id: "boot-b".to_owned(),
            process_id: 41,
            process_start_id: Some("foreign-process".to_owned()),
            started_at_unix_seconds: 1,
            nonce: "foreign".to_owned(),
        };
        write_holder(&layout, &foreign);
        let environment = environment("boot-a", 42, "local", &[(42, None)]);

        CatalogLock::remove_existing_with(&layout, &foreign, &environment).unwrap();

        assert!(!layout.lock_path().exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_start_identity_is_stable_for_the_current_process() {
        let process_id = std::process::id();

        let first = super::system_process_start_id(process_id).unwrap();
        let second = super::system_process_start_id(process_id).unwrap();

        assert!(first.is_some());
        assert_eq!(first, second);
    }

    #[test]
    fn explicit_removal_refuses_changed_lock() {
        let (_temporary, layout) = initialized_catalog("lock-changed");
        let expected = LockHolder {
            host: "other-computer".to_owned(),
            machine_id: "machine-b".to_owned(),
            boot_id: "boot-b".to_owned(),
            process_id: 41,
            process_start_id: Some("foreign-process".to_owned()),
            started_at_unix_seconds: 1,
            nonce: "expected".to_owned(),
        };
        let actual = LockHolder {
            nonce: "actual".to_owned(),
            ..expected.clone()
        };
        fs::write(
            layout.lock_path(),
            format!("{}\n", serde_json::to_string(&actual).unwrap()),
        )
        .unwrap();
        let environment = environment("boot-a", 42, "local", &[(42, Some("process-42"))]);

        let error =
            CatalogLock::remove_existing_with(&layout, &expected, &environment).unwrap_err();

        assert!(matches!(
            error,
            LockError::Changed {
                expected: observed_expected,
                actual: observed_actual,
            } if *observed_expected == expected && *observed_actual == actual
        ));
        assert!(layout.lock_path().exists());
    }

    #[test]
    fn release_removes_only_the_owned_lock() {
        let (_temporary, layout) = initialized_catalog("lock-release");
        let environment = environment("boot-a", 42, "owned", &[(42, Some("process-42"))]);
        let acquired = CatalogLock::acquire_with(&layout, &environment).unwrap();

        acquired.release().unwrap();

        assert!(!layout.lock_path().exists());
    }

    #[test]
    fn corrupt_lock_can_be_removed_only_with_exact_confirmed_contents() {
        let (_temporary, layout) = initialized_catalog("lock-corrupt");
        let corrupt = b"{partial";
        fs::write(layout.lock_path(), corrupt).unwrap();
        let environment = environment("boot-a", 42, "local", &[(42, Some("process-42"))]);

        let error = CatalogLock::acquire_with(&layout, &environment).unwrap_err();
        assert!(matches!(
            error,
            LockError::Corrupt { contents, .. } if contents.as_ref() == corrupt
        ));

        CatalogLock::remove_corrupt_existing_with(&layout, corrupt, &environment).unwrap();
        assert!(!layout.lock_path().exists());
    }

    #[test]
    fn corrupt_lock_removal_refuses_changed_contents() {
        let (_temporary, layout) = initialized_catalog("lock-corrupt-changed");
        fs::write(layout.lock_path(), b"new contents").unwrap();
        let environment = environment("boot-a", 42, "local", &[(42, Some("process-42"))]);

        let error =
            CatalogLock::remove_corrupt_existing_with(&layout, b"old contents", &environment)
                .unwrap_err();

        assert!(matches!(error, LockError::CorruptChanged { .. }));
        assert!(layout.lock_path().exists());
    }
}
