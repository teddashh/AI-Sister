use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use fs4::TryLockError;

use crate::authority::{self, Asset, Persona, Use, VoiceLine};
use crate::zip::{self, sha256_hex};
use crate::{Error, Result};

pub const CACHE_DIRECTORY: &str = "persona-assets-v1";
const OBJECTS_DIRECTORY: &str = "objects";
const RECEIPT_FILE: &str = "installed-v1.txt";
const STAGING_PREFIX: &str = ".staging-";
const RELEASE_DIRECTORY: &str = "ai-sister-media-11-voice55-2026.07.23";
const LOCK_SUFFIX: &str = ".lock-v1";
const REVOCATIONS_SUFFIX: &str = ".revocations-v1";
const EPOCH_PREFIX: &str = "epoch-";
const REVOCATION_PREFIX: &str = "revoke-";
const AUTHORIZATION_SUFFIX: &str = ".revocations-authorized-v1";
const SETTLED_SUFFIX: &str = ".revocations-settled-v1";
const REVOCATION_MARKER: &[u8] = b"schema=1\nrelease=ai-sister-media-11-voice55-2026.07.23\n";
const RECEIPT: &[u8] = b"schema=1\nrelease=ai-sister-media-11-voice55-2026.07.23\nmanifest=21e4675653ce66b50b61e91260f1623e6e3005177f900991e3a8eeadaf9e6474\npack=7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30\n";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RevocationSnapshot {
    generation: String,
    has_revocations: bool,
}

/// One fixed cache transaction. The persistent sibling lock is never removed: unlinking a lock
/// path lets a second process lock a new inode while the first still owns the old one.
pub(crate) struct InstallTransaction {
    cache_root: PathBuf,
    _lock: File,
    revocations: RevocationSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    Available,
    RepairNeeded,
    Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalOutcome {
    Removed,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Portrait {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceMetadata {
    pub line: VoiceLine,
    pub duration_ms: u32,
    pub spoken_text: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub line: VoiceLine,
    pub duration_ms: u32,
    pub bytes: Vec<u8>,
}

/// 真正留在 cache 的是 4 張立繪與 8 條無條件聲音，不保留 73 MB omnibus archive。
pub const INSTALLED_ASSET_BYTES: usize = installed_asset_bytes();

const fn installed_asset_bytes() -> usize {
    let mut total = 0;
    let mut index = 0;
    while index < authority::ASSETS.len() {
        total += authority::ASSETS[index].bytes;
        index += 1;
    }
    total
}

pub fn cache_state(cache_root: &Path) -> Result<CacheState> {
    cache_state_result(cache_root)
}

pub fn install_from_bytes(cache_root: &Path, archive: &[u8]) -> Result<InstallOutcome> {
    let transaction = begin_install(cache_root)?;
    let cancelled = AtomicBool::new(false);
    install_from_bytes_in_transaction(&transaction, archive, &cancelled)
}

pub(crate) fn begin_install(cache_root: &Path) -> Result<InstallTransaction> {
    ensure_revocation_journal(cache_root)?;
    // This snapshot is the install click's ordering point. A remove that appends after it always
    // invalidates this transaction, including the narrow window before the lock is acquired.
    let revocations = revocation_snapshot(cache_root)?;
    if !revocations_are_settled(cache_root, &revocations)? {
        return Err(Error::CacheRevoked);
    }
    let lock = mutation_lock(cache_root, false)?;
    let transaction = InstallTransaction {
        cache_root: cache_root.to_path_buf(),
        _lock: lock,
        revocations,
    };
    if revocation_snapshot(cache_root)? != transaction.revocations {
        return Err(Error::CacheRevoked);
    }
    Ok(transaction)
}

impl InstallTransaction {
    pub(crate) fn checkpoint(&self, cancelled: &AtomicBool) -> Result<()> {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if revocation_snapshot(&self.cache_root)? != self.revocations {
            return Err(Error::CacheRevoked);
        }
        Ok(())
    }

    /// Called before the only GET. A second process therefore returns `CacheBusy` before it can
    /// create another request, and an already installed release creates no request at all.
    #[cfg(feature = "download")]
    pub(crate) fn installed_before_download(&self, cancelled: &AtomicBool) -> Result<bool> {
        self.checkpoint(cancelled)?;
        match inspect_root(&self.cache_root) {
            Ok(RootState::Empty) => Ok(false),
            Ok(RootState::Installed) => Ok(revocations_are_authorized(
                &self.cache_root,
                &self.revocations,
            )?),
            Err(Error::CacheRejected) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn install_from_bytes_in_transaction(
    transaction: &InstallTransaction,
    archive: &[u8],
    cancelled: &AtomicBool,
) -> Result<InstallOutcome> {
    transaction.checkpoint(cancelled)?;
    // 所有不受信 bytes 都先走完整包驗證；cancel 也會在 digest 與每一個 entry 間檢查。
    let selected = zip::validate(archive, cancelled)?;
    transaction.checkpoint(cancelled)?;
    if prepare_install_root(transaction, cancelled)? {
        return Ok(InstallOutcome::AlreadyInstalled);
    }

    transaction.checkpoint(cancelled)?;
    let stage = new_stage(&transaction.cache_root)?;
    let result = (|| {
        let objects = stage.join(OBJECTS_DIRECTORY);
        create_private_directory(&objects)?;
        for asset in &authority::ASSETS {
            transaction.checkpoint(cancelled)?;
            let bytes = selected.get(asset).ok_or(Error::ArchiveRejected)?;
            write_private_file(&asset_path_in(&stage, asset), bytes)?;
        }
        transaction.checkpoint(cancelled)?;
        // marker 最後寫；沒有它的資料夾永遠不會被 resolver 當成 installed。
        write_private_file(&stage.join(RECEIPT_FILE), RECEIPT)?;
        validate_release_directory(&stage, Some(cancelled))?;
        transaction.checkpoint(cancelled)?;

        let destination = release_directory(&transaction.cache_root);
        match fs::symlink_metadata(&destination) {
            Ok(_) => remove_known_release_directory(&destination)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        transaction.checkpoint(cancelled)?;
        fs::rename(&stage, &destination)?;
        let committed = (|| {
            validate_release_directory(&destination, Some(cancelled))?;
            transaction.checkpoint(cancelled)?;
            // This is the commit point. Cancellation after this durable write is ordered after a
            // successful install; a concurrent remove writes a new generation and disables it.
            authorize_revocations(&transaction.cache_root, &transaction.revocations)
        })();
        if let Err(error) = committed {
            // The old authorization was durably invalidated before mutation, and the exclusive
            // lock prevents a resolver from observing this window. Even failed cleanup therefore
            // leaves a repairable-but-inactive release, not enabled bytes.
            let _ = remove_known_release_directory(&destination);
            return Err(error);
        }
        Ok(InstallOutcome::Installed)
    })();

    if result.is_err() {
        // 只刪我們在 exact staging path 建的 allowlisted 檔案。若其中出現未知項目，
        // 不遞迴追著刪；下一次狀態會明講 RepairNeeded。
        let _ = remove_known_release_directory(&stage);
    }
    result
}

/// Repair removes only the exact managed release/staging shapes. Unknown siblings or unknown
/// entries remain untouched and block the operation.
fn prepare_install_root(transaction: &InstallTransaction, cancelled: &AtomicBool) -> Result<bool> {
    let cache_root = &transaction.cache_root;
    ensure_root(cache_root)?;
    transaction.checkpoint(cancelled)?;
    match inspect_root(cache_root) {
        Ok(RootState::Installed)
            if revocations_are_authorized(cache_root, &transaction.revocations)? =>
        {
            return Ok(true);
        }
        Ok(RootState::Empty) => {
            invalidate_authorization(cache_root)?;
            return Ok(false);
        }
        Ok(RootState::Installed) | Err(Error::CacheRejected) => {}
        Err(error) => return Err(error),
    }

    // A prior successful install may have left an authorization for this same revocation
    // generation. Invalidate it before deleting/replacing any bytes so a crash cannot commit the
    // new release with the old authorization.
    invalidate_authorization(cache_root)?;
    for entry in fs::read_dir(cache_root)? {
        transaction.checkpoint(cancelled)?;
        let entry = entry?;
        let name = entry.file_name();
        if name == OsStr::new(RELEASE_DIRECTORY) || is_staging_name(&name) {
            remove_known_release_directory(&entry.path())?;
        } else {
            return Err(Error::CacheHasUnknownEntries);
        }
    }
    transaction.checkpoint(cancelled)?;
    match inspect_root(cache_root)? {
        RootState::Empty => Ok(false),
        RootState::Installed => Err(Error::CacheRejected),
    }
}

pub fn remove(cache_root: &Path) -> Result<RemovalOutcome> {
    // Revocation is the linearization point, before waiting for an installer/reader. The journal
    // is outside the directory being removed, so a crash or lock error cannot resurrect bytes.
    append_revocation_marker(cache_root)?;
    let _lock = mutation_lock(cache_root, true)?;
    let removal = (|| {
        let mut found = false;
        match fs::symlink_metadata(cache_root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RemovalOutcome::Absent);
            }
            Err(error) => return Err(error.into()),
            Ok(metadata) if !metadata.file_type().is_dir() => return Err(Error::CacheRejected),
            Ok(_) => {}
        }

        for entry in fs::read_dir(cache_root)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == OsStr::new(RELEASE_DIRECTORY) || is_staging_name(&name) {
                found = true;
                remove_known_release_directory(&entry.path())?;
            } else {
                return Err(Error::CacheHasUnknownEntries);
            }
        }
        match fs::remove_dir(cache_root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                return Err(Error::CacheHasUnknownEntries);
            }
            Err(error) => return Err(error.into()),
        }
        Ok(if found {
            RemovalOutcome::Removed
        } else {
            RemovalOutcome::Absent
        })
    })();
    let settled = settle_revocations(cache_root);
    match (removal, settled) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

pub fn read_portrait(cache_root: &Path, persona: Persona) -> Result<Portrait> {
    with_installed_cache(cache_root, || {
        let asset = authority::portrait(persona);
        Ok(Portrait {
            bytes: read_asset(&release_directory(cache_root), asset, None)?,
        })
    })
}

pub fn voice_metadata(cache_root: &Path, persona: Persona) -> Result<Vec<VoiceMetadata>> {
    with_installed_cache(cache_root, || {
        Ok(authority::ASSETS
            .iter()
            .filter_map(|asset| match asset.use_as {
                Use::Voice(line) if line.persona() == persona => Some(VoiceMetadata {
                    line,
                    duration_ms: asset.duration_ms.expect("voice duration is embedded"),
                    spoken_text: line.spoken_text(),
                }),
                _ => None,
            })
            .collect())
    })
}

pub fn read_voice(cache_root: &Path, line: VoiceLine) -> Result<Voice> {
    with_installed_cache(cache_root, || {
        let asset = authority::voice(line);
        Ok(Voice {
            line,
            duration_ms: asset.duration_ms.expect("voice duration is embedded"),
            bytes: read_asset(&release_directory(cache_root), asset, None)?,
        })
    })
}

fn with_installed_cache<T>(cache_root: &Path, read: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = read_lock(cache_root)?;
    let before = revocation_snapshot(cache_root)?;
    match inspect_root(cache_root) {
        Ok(RootState::Installed) => {}
        Ok(RootState::Empty) | Err(Error::CacheRejected | Error::CacheHasUnknownEntries) => {
            return Err(Error::CacheRejected);
        }
        Err(error) => return Err(error),
    }
    if !revocations_are_authorized(cache_root, &before)? {
        return Err(Error::CacheRejected);
    }
    let value = read()?;
    let after = revocation_snapshot(cache_root)?;
    if before != after || !revocations_are_authorized(cache_root, &after)? {
        return Err(Error::CacheRevoked);
    }
    Ok(value)
}

fn cache_state_result(cache_root: &Path) -> Result<CacheState> {
    let _lock = read_lock(cache_root)?;
    let root = match inspect_root(cache_root) {
        Ok(root) => root,
        Err(Error::CacheRejected | Error::CacheHasUnknownEntries) => {
            return Ok(CacheState::RepairNeeded);
        }
        Err(error) => return Err(error),
    };
    if matches!(root, RootState::Empty) {
        return cache_state_for_empty_root(cache_root);
    }
    cache_state_for_validated_release(cache_root)
}

fn cache_state_for_empty_root(cache_root: &Path) -> Result<CacheState> {
    let journal = sibling_path(cache_root, REVOCATIONS_SUFFIX)?;
    match fs::symlink_metadata(journal) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CacheState::Available);
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let state = (|| {
        let snapshot = revocation_snapshot(cache_root)?;
        Ok(if revocations_are_settled(cache_root, &snapshot)? {
            CacheState::Available
        } else {
            CacheState::RepairNeeded
        })
    })();
    match state {
        Err(Error::CacheRejected | Error::CacheHasUnknownEntries | Error::CacheRevoked) => {
            Ok(CacheState::RepairNeeded)
        }
        other => other,
    }
}

fn cache_state_for_validated_release(cache_root: &Path) -> Result<CacheState> {
    match validated_release_control_state(cache_root) {
        Ok(state) => Ok(state),
        Err(Error::CacheRejected | Error::CacheHasUnknownEntries | Error::CacheRevoked) => {
            Ok(CacheState::RepairNeeded)
        }
        Err(error) => Err(error),
    }
}

fn validated_release_control_state(cache_root: &Path) -> Result<CacheState> {
    let before = revocation_snapshot(cache_root)?;
    let authorized = revocations_are_authorized(cache_root, &before)?;
    let after = revocation_snapshot(cache_root)?;
    Ok(if before == after && authorized {
        CacheState::Installed
    } else {
        CacheState::RepairNeeded
    })
}

fn mutation_lock(cache_root: &Path, wait: bool) -> Result<File> {
    let file = open_lock_file(cache_root)?;
    if wait {
        fs4::FileExt::lock(&file)?;
    } else {
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(Error::CacheBusy),
            Err(TryLockError::Error(error)) => return Err(Error::Io(error)),
        }
    }
    Ok(file)
}

fn read_lock(cache_root: &Path) -> Result<File> {
    let file = open_lock_file(cache_root)?;
    match fs4::FileExt::try_lock_shared(&file) {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Error::CacheBusy),
        Err(TryLockError::Error(error)) => Err(Error::Io(error)),
    }
}

fn open_lock_file(cache_root: &Path) -> Result<File> {
    ensure_sibling_parent(cache_root)?;
    let path = sibling_path(cache_root, LOCK_SUFFIX)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    let path_metadata = fs::symlink_metadata(&path)?;
    if !path_metadata.file_type().is_file() || path_metadata.file_type().is_symlink() {
        return Err(Error::CacheRejected);
    }
    Ok(file)
}

fn append_revocation_marker(cache_root: &Path) -> Result<()> {
    let directory = ensure_revocation_journal(cache_root)?;

    for _ in 0..16u8 {
        let marker = directory.join(random_name(REVOCATION_PREFIX)?);
        match write_private_file(&marker, REVOCATION_MARKER) {
            Ok(()) => {
                sync_directory(&directory)?;
                return Ok(());
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(Error::CacheRejected)
}

fn ensure_revocation_journal(cache_root: &Path) -> Result<PathBuf> {
    ensure_sibling_parent(cache_root)?;
    let directory = sibling_path(cache_root, REVOCATIONS_SUFFIX)?;
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(Error::CacheRejected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match create_private_directory(&directory) {
                Ok(()) => sync_directory(directory.parent().ok_or(Error::CacheRejected)?)?,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error.into()),
    }
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err(Error::CacheRejected);
    }

    // An opaque epoch makes deleting/recreating an empty journal a new generation. Empty files
    // are valid immediately at create_new, so a crash cannot leave a half-written epoch token.
    let mut saw_epoch = false;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_epoch_name(&name) {
            if !regular_file_with_size(&entry.path(), 0)? {
                return Err(Error::CacheRejected);
            }
            saw_epoch = true;
        } else if !is_revocation_name(&name)
            || !regular_file_with_size(&entry.path(), REVOCATION_MARKER.len())?
            || fs::read(entry.path())? != REVOCATION_MARKER
        {
            return Err(Error::CacheRejected);
        }
    }
    if !saw_epoch {
        let mut created = false;
        for _ in 0..16u8 {
            let epoch = directory.join(random_name(EPOCH_PREFIX)?);
            match write_private_file(&epoch, b"") {
                Ok(()) => {
                    sync_directory(&directory)?;
                    created = true;
                    break;
                }
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        if !created {
            return Err(Error::CacheRejected);
        }
    }
    Ok(directory)
}

fn revocation_snapshot(cache_root: &Path) -> Result<RevocationSnapshot> {
    let directory = sibling_path(cache_root, REVOCATIONS_SUFFIX)?;
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::CacheRejected);
        }
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_dir() => return Err(Error::CacheRejected),
        Ok(_) => {}
    }

    let mut names = Vec::new();
    let mut saw_epoch = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_epoch_name(&name) {
            if !regular_file_with_size(&entry.path(), 0)? {
                return Err(Error::CacheRejected);
            }
            saw_epoch = true;
        } else if !is_revocation_name(&name)
            || !regular_file_with_size(&entry.path(), REVOCATION_MARKER.len())?
            || fs::read(entry.path())? != REVOCATION_MARKER
        {
            return Err(Error::CacheRejected);
        }
        names.push(name.into_string().map_err(|_| Error::CacheRejected)?);
    }
    if !saw_epoch {
        return Err(Error::CacheRejected);
    }
    names.sort_unstable();
    let mut canonical = Vec::new();
    for name in &names {
        canonical.extend_from_slice(name.as_bytes());
        canonical.push(b'\n');
    }
    Ok(RevocationSnapshot {
        generation: sha256_hex(&canonical),
        has_revocations: names.iter().any(|name| name.starts_with(REVOCATION_PREFIX)),
    })
}

fn revocations_are_settled(cache_root: &Path, snapshot: &RevocationSnapshot) -> Result<bool> {
    if !snapshot.has_revocations {
        return Ok(true);
    }
    let path = sibling_path(cache_root, SETTLED_SUFFIX)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(Error::CacheRejected);
        }
        Ok(_) => {}
    }
    Ok(fs::read(path)? == settlement_bytes(snapshot))
}

fn settle_revocations(cache_root: &Path) -> Result<()> {
    let snapshot = revocation_snapshot(cache_root)?;
    let path = sibling_path(cache_root, SETTLED_SUFFIX)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(Error::CacheRejected);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(&settlement_bytes(&snapshot))?;
    file.sync_all()?;
    sync_directory(path.parent().ok_or(Error::CacheRejected)?)
}

fn settlement_bytes(snapshot: &RevocationSnapshot) -> Vec<u8> {
    format!(
        "schema=1\nrelease={RELEASE_DIRECTORY}\nsettled-generation={}\n",
        snapshot.generation
    )
    .into_bytes()
}

fn revocations_are_authorized(cache_root: &Path, snapshot: &RevocationSnapshot) -> Result<bool> {
    let path = sibling_path(cache_root, AUTHORIZATION_SUFFIX)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(Error::CacheRejected);
        }
        Ok(_) => {}
    }
    Ok(fs::read(path)? == authorization_bytes(snapshot))
}

fn authorize_revocations(cache_root: &Path, snapshot: &RevocationSnapshot) -> Result<()> {
    let current = revocation_snapshot(cache_root)?;
    if &current != snapshot {
        return Err(Error::CacheRevoked);
    }
    let path = sibling_path(cache_root, AUTHORIZATION_SUFFIX)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(&authorization_bytes(snapshot))?;
    file.sync_all()?;
    sync_directory(path.parent().ok_or(Error::CacheRejected)?)?;
    Ok(())
}

fn invalidate_authorization(cache_root: &Path) -> Result<()> {
    let path = sibling_path(cache_root, AUTHORIZATION_SUFFIX)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(Error::CacheRejected);
        }
        Ok(_) => {}
    }
    fs::remove_file(&path)?;
    sync_directory(path.parent().ok_or(Error::CacheRejected)?)
}

fn authorization_bytes(snapshot: &RevocationSnapshot) -> Vec<u8> {
    format!(
        "schema=1\nrelease={RELEASE_DIRECTORY}\ngeneration={}\n",
        snapshot.generation
    )
    .into_bytes()
}

fn sibling_path(cache_root: &Path, suffix: &str) -> Result<PathBuf> {
    let mut name = cache_root
        .file_name()
        .ok_or(Error::CacheRejected)?
        .to_os_string();
    name.push(suffix);
    Ok(cache_root
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name))
}

fn ensure_sibling_parent(cache_root: &Path) -> Result<()> {
    let parent = cache_root.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn random_name(prefix: &str) -> Result<String> {
    let mut random = [0u8; 16];
    getrandom::getrandom(&mut random).map_err(|_| Error::SecureRandomUnavailable)?;
    let mut name = String::with_capacity(prefix.len() + random.len() * 2);
    name.push_str(prefix);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in random {
        name.push(char::from(HEX[usize::from(byte >> 4)]));
        name.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(name)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    // `FlushFileBuffers` on the marker itself is the portable Windows durability boundary here;
    // opening directories as ordinary `File`s is not supported by std on Windows.
    Ok(())
}

enum RootState {
    Empty,
    Installed,
}

fn inspect_root(cache_root: &Path) -> Result<RootState> {
    match fs::symlink_metadata(cache_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RootState::Empty);
        }
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_dir() => return Err(Error::CacheRejected),
        Ok(_) => {}
    }

    let mut release = None;
    for entry in fs::read_dir(cache_root)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == OsStr::new(RELEASE_DIRECTORY) {
            if release.replace(entry.path()).is_some() {
                return Err(Error::CacheRejected);
            }
        } else {
            // crash 留下的 staging 與任何未知檔案都不能長得像「尚未下載」。
            return Err(if is_staging_name(&name) {
                Error::CacheRejected
            } else {
                Error::CacheHasUnknownEntries
            });
        }
    }
    let Some(release) = release else {
        return Ok(RootState::Empty);
    };
    validate_release_directory(&release, None)?;
    Ok(RootState::Installed)
}

fn ensure_root(cache_root: &Path) -> Result<()> {
    match fs::symlink_metadata(cache_root) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(Error::CacheRejected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = cache_root.parent() {
                fs::create_dir_all(parent)?;
            }
            create_private_directory(cache_root)
        }
        Err(error) => Err(error.into()),
    }
}

fn new_stage(cache_root: &Path) -> Result<PathBuf> {
    for _ in 0..16u8 {
        let candidate = cache_root.join(random_name(STAGING_PREFIX)?);
        match create_private_directory(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(Error::CacheRejected)
}

fn validate_release_directory(directory: &Path, cancelled: Option<&AtomicBool>) -> Result<()> {
    check_cancel(cancelled)?;
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() {
        return Err(Error::CacheRejected);
    }
    let mut saw_objects = false;
    let mut saw_receipt = false;
    for entry in fs::read_dir(directory)? {
        check_cancel(cancelled)?;
        let entry = entry?;
        match entry.file_name().to_str() {
            Some(OBJECTS_DIRECTORY) if !saw_objects => saw_objects = true,
            Some(RECEIPT_FILE) if !saw_receipt => saw_receipt = true,
            _ => return Err(Error::CacheHasUnknownEntries),
        }
    }
    if !saw_objects || !saw_receipt {
        return Err(Error::CacheRejected);
    }
    let receipt_path = directory.join(RECEIPT_FILE);
    check_cancel(cancelled)?;
    if !regular_file_with_size(&receipt_path, RECEIPT.len())? || fs::read(receipt_path)? != RECEIPT
    {
        return Err(Error::CacheRejected);
    }

    let objects = directory.join(OBJECTS_DIRECTORY);
    if !fs::symlink_metadata(&objects)?.file_type().is_dir() {
        return Err(Error::CacheRejected);
    }
    let mut seen = vec![false; authority::ASSETS.len()];
    for entry in fs::read_dir(&objects)? {
        check_cancel(cancelled)?;
        let entry = entry?;
        let Some(index) = authority::ASSETS
            .iter()
            .position(|asset| object_file_name(asset) == entry.file_name())
        else {
            return Err(Error::CacheHasUnknownEntries);
        };
        if seen[index] {
            return Err(Error::CacheRejected);
        }
        read_asset(directory, &authority::ASSETS[index], cancelled)?;
        seen[index] = true;
    }
    if seen.iter().any(|found| !found) {
        return Err(Error::CacheRejected);
    }
    Ok(())
}

fn read_asset(directory: &Path, asset: &Asset, cancelled: Option<&AtomicBool>) -> Result<Vec<u8>> {
    check_cancel(cancelled)?;
    let path = asset_path_in(directory, asset);
    if !regular_file_with_size(&path, asset.bytes)? {
        return Err(Error::CacheRejected);
    }
    let bytes = fs::read(path)?;
    check_cancel(cancelled)?;
    let digest = match cancelled {
        Some(cancelled) => zip::sha256_hex_cancellable(&bytes, cancelled)?,
        None => sha256_hex(&bytes),
    };
    if digest != object_sha(asset) {
        return Err(Error::CacheRejected);
    }
    Ok(bytes)
}

fn regular_file_with_size(path: &Path, bytes: usize) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.file_type().is_file() && usize::try_from(metadata.len()).ok() == Some(bytes))
}

fn object_file_name(asset: &Asset) -> &OsStr {
    Path::new(asset.object)
        .file_name()
        .expect("embedded object has a file name")
}

fn object_sha(asset: &Asset) -> &str {
    Path::new(asset.object)
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("embedded object has an ASCII hash name")
}

fn asset_path_in(directory: &Path, asset: &Asset) -> PathBuf {
    directory
        .join(OBJECTS_DIRECTORY)
        .join(object_file_name(asset))
}

fn release_directory(cache_root: &Path) -> PathBuf {
    cache_root.join(RELEASE_DIRECTORY)
}

fn is_staging_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.strip_prefix(STAGING_PREFIX).is_some_and(|tail| {
            tail.len() == 32
                && tail
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    })
}

fn is_revocation_name(name: &OsStr) -> bool {
    is_random_name(name, REVOCATION_PREFIX)
}

fn is_epoch_name(name: &OsStr) -> bool {
    is_random_name(name, EPOCH_PREFIX)
}

fn is_random_name(name: &OsStr, prefix: &str) -> bool {
    name.to_str().is_some_and(|name| {
        name.strip_prefix(prefix).is_some_and(|tail| {
            tail.len() == 32
                && tail
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    })
}

fn check_cancel(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Acquire)) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn remove_known_release_directory(directory: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || metadata.file_type().is_file() {
        fs::remove_file(directory)?;
        return Ok(());
    }
    if !metadata.file_type().is_dir() {
        return Err(Error::CacheRejected);
    }

    let objects = directory.join(OBJECTS_DIRECTORY);
    match fs::symlink_metadata(&objects) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.file_type().is_file() => {
            fs::remove_file(&objects)?;
        }
        Ok(metadata) if metadata.file_type().is_dir() => {
            for asset in &authority::ASSETS {
                let path = asset_path_in(directory, asset);
                match fs::symlink_metadata(&path) {
                    Ok(metadata)
                        if metadata.file_type().is_file() || metadata.file_type().is_symlink() =>
                    {
                        fs::remove_file(path)?;
                    }
                    Ok(_) => return Err(Error::CacheHasUnknownEntries),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            match fs::remove_dir(&objects) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                    return Err(Error::CacheHasUnknownEntries);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(_) => return Err(Error::CacheHasUnknownEntries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let receipt = directory.join(RECEIPT_FILE);
    match fs::symlink_metadata(&receipt) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(receipt)?;
        }
        Ok(_) => return Err(Error::CacheHasUnknownEntries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match fs::remove_dir(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            Err(Error::CacheHasUnknownEntries)
        }
        Err(error) => Err(error.into()),
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn cleanup_test_sidecars(cache_root: &Path) -> Result<()> {
    for suffix in [LOCK_SUFFIX, AUTHORIZATION_SUFFIX, SETTLED_SUFFIX] {
        let path = sibling_path(cache_root, suffix)?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let journal = sibling_path(cache_root, REVOCATIONS_SUFFIX)?;
    match fs::read_dir(&journal) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if !is_epoch_name(&entry.file_name()) && !is_revocation_name(&entry.file_name()) {
                    return Err(Error::CacheHasUnknownEntries);
                }
                fs::remove_file(entry.path())?;
            }
            fs::remove_dir(journal)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ai-sister-assets-test-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    fn state(cache_root: &Path) -> CacheState {
        cache_state(cache_root).expect("read cache state")
    }

    fn cleanup_sidecars(cache_root: &Path) {
        cleanup_test_sidecars(cache_root).expect("cleanup test sidecars");
    }

    fn delete_known_journal(cache_root: &Path) {
        let journal = sibling_path(cache_root, REVOCATIONS_SUFFIX).expect("journal path");
        for entry in fs::read_dir(&journal).expect("read journal") {
            let entry = entry.expect("journal entry");
            assert!(
                is_epoch_name(&entry.file_name()) || is_revocation_name(&entry.file_name()),
                "test helper refuses unknown journal entry"
            );
            fs::remove_file(entry.path()).expect("remove journal entry");
        }
        fs::remove_dir(journal).expect("remove journal");
    }

    #[test]
    fn a_missing_cache_is_available_not_installed() {
        let root = temp_root("missing");
        assert_eq!(state(&root), CacheState::Available);
        assert_eq!(
            remove(&root).expect("remove absent"),
            RemovalOutcome::Absent
        );
        cleanup_sidecars(&root);
    }

    #[test]
    fn a_marker_without_objects_is_repair_needed() {
        let root = temp_root("marker-only");
        fs::create_dir(&root).expect("root");
        let release = release_directory(&root);
        fs::create_dir(&release).expect("release");
        fs::write(release.join(RECEIPT_FILE), RECEIPT).expect("receipt");
        assert_eq!(state(&root), CacheState::RepairNeeded);
        assert_eq!(
            remove(&root).expect("precise remove"),
            RemovalOutcome::Removed
        );
        cleanup_sidecars(&root);
    }

    #[test]
    fn unknown_files_are_not_recursively_deleted() {
        let root = temp_root("unknown");
        fs::create_dir(&root).expect("root");
        let release = release_directory(&root);
        fs::create_dir(&release).expect("release");
        fs::write(release.join("not-ours.txt"), b"keep me").expect("foreign file");
        assert_eq!(state(&root), CacheState::RepairNeeded);
        assert!(matches!(remove(&root), Err(Error::CacheHasUnknownEntries)));
        assert!(release.join("not-ours.txt").exists());
        fs::remove_file(release.join("not-ours.txt")).expect("test cleanup file");
        fs::remove_dir(release).expect("test cleanup release");
        fs::remove_dir(&root).expect("test cleanup root");
        cleanup_sidecars(&root);
    }

    #[test]
    fn an_unknown_root_entry_blocks_install_instead_of_reporting_a_false_success() {
        let root = temp_root("unknown-root");
        fs::create_dir(&root).expect("root");
        fs::write(root.join("not-ours.txt"), b"keep me").expect("foreign file");

        // Validation happens before cache inspection in production. An empty byte slice is
        // rejected first, so exercise the state transition directly: clearing known staging
        // must not turn an unknown sibling into an installable empty root.
        let transaction = begin_install(&root).expect("transaction");
        let cancelled = AtomicBool::new(false);
        assert!(matches!(
            prepare_install_root(&transaction, &cancelled),
            Err(Error::CacheHasUnknownEntries)
        ));
        drop(transaction);
        assert_eq!(state(&root), CacheState::RepairNeeded);
        assert!(root.join("not-ours.txt").exists());

        fs::remove_file(root.join("not-ours.txt")).expect("test cleanup file");
        fs::remove_dir(&root).expect("test cleanup root");
        cleanup_sidecars(&root);
    }

    #[test]
    fn two_handles_contend_and_state_reports_busy_without_claiming_corruption() {
        let root = temp_root("two-handles");
        let first = begin_install(&root).expect("first transaction");
        assert!(matches!(begin_install(&root), Err(Error::CacheBusy)));
        assert!(matches!(cache_state(&root), Err(Error::CacheBusy)));

        drop(first);
        drop(begin_install(&root).expect("lock released for second transaction"));
        cleanup_sidecars(&root);
    }

    #[test]
    fn remove_revokes_a_live_transaction_before_waiting_then_deletes_after_drop() {
        use std::thread;
        use std::time::{Duration, Instant};

        let root = temp_root("remove-live");
        let transaction = begin_install(&root).expect("live transaction");
        let remove_root = root.clone();
        let remover = thread::spawn(move || remove(&remove_root));

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if revocation_snapshot(&root).is_ok_and(|snapshot| snapshot.has_revocations) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "remove did not publish revocation"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let cancelled = AtomicBool::new(false);
        assert!(matches!(
            transaction.checkpoint(&cancelled),
            Err(Error::CacheRevoked)
        ));
        assert!(matches!(cache_state(&root), Err(Error::CacheBusy)));

        drop(transaction);
        assert_eq!(
            remover.join().expect("remover thread").expect("remove"),
            RemovalOutcome::Absent
        );
        assert_eq!(state(&root), CacheState::Available);
        assert!(
            sibling_path(&root, LOCK_SUFFIX)
                .expect("lock path")
                .exists()
        );
        assert!(
            sibling_path(&root, REVOCATIONS_SUFFIX)
                .expect("journal path")
                .exists()
        );
        cleanup_sidecars(&root);
    }

    #[test]
    fn a_settled_revocation_allows_explicit_repair_but_a_new_one_cancels_it() {
        let root = temp_root("settled-repair");
        assert_eq!(
            remove(&root).expect("settled remove"),
            RemovalOutcome::Absent
        );

        let transaction = begin_install(&root).expect("repair transaction");
        assert!(
            !revocations_are_authorized(&root, &transaction.revocations)
                .expect("authorization state")
        );
        authorize_revocations(&root, &transaction.revocations).expect("explicit repair commit");
        assert!(
            revocations_are_authorized(&root, &transaction.revocations)
                .expect("authorization state")
        );
        drop(transaction);

        append_revocation_marker(&root).expect("new revocation");
        assert!(matches!(begin_install(&root), Err(Error::CacheRevoked)));
        settle_revocations(&root).expect("completed removal permits a later repair");
        drop(begin_install(&root).expect("repair after settled remove"));
        cleanup_sidecars(&root);
    }

    #[test]
    fn an_empty_root_is_repair_needed_until_its_revocation_is_settled() {
        let root = temp_root("empty-unsettled");
        drop(begin_install(&root).expect("initialize controls"));
        assert_eq!(state(&root), CacheState::Available);

        append_revocation_marker(&root).expect("interrupted remove marker");
        assert_eq!(state(&root), CacheState::RepairNeeded);
        settle_revocations(&root).expect("finish removal protocol");
        assert_eq!(state(&root), CacheState::Available);
        cleanup_sidecars(&root);
    }

    #[test]
    fn cancellation_and_errors_release_the_cross_process_lock() {
        let root = temp_root("cancel-release");
        let transaction = begin_install(&root).expect("transaction");
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            transaction.checkpoint(&cancelled),
            Err(Error::Cancelled)
        ));
        drop(transaction);
        drop(begin_install(&root).expect("cancelled transaction released lock"));
        cleanup_sidecars(&root);
    }

    #[test]
    fn corrupt_known_release_and_hex_staging_are_repairable() {
        let root = temp_root("known-repair");
        fs::create_dir(&root).expect("root");
        let release = release_directory(&root);
        fs::create_dir(&release).expect("release");
        fs::write(release.join(RECEIPT_FILE), RECEIPT).expect("receipt");
        let stage = root.join(".staging-abcdef0123456789abcdef0123456789");
        fs::create_dir(&stage).expect("crashed stage");

        let transaction = begin_install(&root).expect("repair transaction");
        let cancelled = AtomicBool::new(false);
        assert!(!prepare_install_root(&transaction, &cancelled).expect("repair known paths"));
        assert!(!release.exists());
        assert!(!stage.exists());
        drop(transaction);
        fs::remove_dir(&root).expect("empty cache root");
        cleanup_sidecars(&root);
    }

    #[test]
    fn stale_authorization_is_invalidated_before_repair_mutates_bytes() {
        let root = temp_root("stale-auth");
        let old = begin_install(&root).expect("old transaction");
        authorize_revocations(&root, &old.revocations).expect("old authorization");
        drop(old);

        let repair = begin_install(&root).expect("repair transaction");
        let cancelled = AtomicBool::new(false);
        assert!(!prepare_install_root(&repair, &cancelled).expect("prepare empty root"));
        assert!(
            !sibling_path(&root, AUTHORIZATION_SUFFIX)
                .expect("authorization path")
                .exists(),
            "a crash after this point must not reuse an old commit"
        );
        drop(repair);
        fs::remove_dir(&root).expect("empty cache root");
        cleanup_sidecars(&root);
    }

    #[test]
    fn recreating_a_deleted_journal_cannot_reuse_an_old_authorization() {
        let root = temp_root("journal-epoch");
        let old = begin_install(&root).expect("old transaction");
        let old_snapshot = old.revocations.clone();
        authorize_revocations(&root, &old_snapshot).expect("old authorization");
        drop(old);
        delete_known_journal(&root);

        let replacement = begin_install(&root).expect("replacement journal");
        assert_ne!(replacement.revocations, old_snapshot);
        assert!(
            !revocations_are_authorized(&root, &replacement.revocations)
                .expect("replacement authorization state")
        );
        drop(replacement);
        cleanup_sidecars(&root);
    }

    #[test]
    fn a_validated_release_maps_missing_or_corrupt_controls_to_repair_needed() {
        let root = temp_root("control-repair-state");
        let transaction = begin_install(&root).expect("initialize controls");
        authorize_revocations(&root, &transaction.revocations).expect("authorization");
        drop(transaction);
        assert_eq!(
            cache_state_for_validated_release(&root).expect("valid controls"),
            CacheState::Installed
        );

        delete_known_journal(&root);
        assert_eq!(
            cache_state_for_validated_release(&root).expect("missing journal is repairable"),
            CacheState::RepairNeeded
        );
        ensure_revocation_journal(&root).expect("replacement journal");
        assert_eq!(
            cache_state_for_validated_release(&root).expect("stale authorization is repairable"),
            CacheState::RepairNeeded
        );

        let authorization = sibling_path(&root, AUTHORIZATION_SUFFIX).expect("authorization path");
        fs::remove_file(&authorization).expect("remove stale authorization");
        fs::create_dir(&authorization).expect("malformed authorization");
        assert_eq!(
            cache_state_for_validated_release(&root).expect("malformed control is repairable"),
            CacheState::RepairNeeded
        );
        fs::remove_dir(authorization).expect("test cleanup malformed authorization");
        cleanup_sidecars(&root);
    }

    #[test]
    fn malformed_journal_is_rejected_and_never_deleted_by_remove() {
        let root = temp_root("foreign-journal");
        drop(begin_install(&root).expect("initialize journal"));
        let journal = sibling_path(&root, REVOCATIONS_SUFFIX).expect("journal path");
        let foreign = journal.join("not-ours.txt");
        fs::write(&foreign, b"keep me").expect("foreign journal entry");

        assert!(matches!(begin_install(&root), Err(Error::CacheRejected)));
        assert!(matches!(remove(&root), Err(Error::CacheRejected)));
        assert_eq!(
            fs::read(&foreign).expect("foreign entry preserved"),
            b"keep me"
        );

        fs::remove_file(foreign).expect("test cleanup foreign entry");
        cleanup_sidecars(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_can_never_become_an_installed_object() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        fs::create_dir(&root).expect("root");
        let release = release_directory(&root);
        fs::create_dir(&release).expect("release");
        fs::create_dir(release.join(OBJECTS_DIRECTORY)).expect("objects");
        fs::write(release.join(RECEIPT_FILE), RECEIPT).expect("receipt");
        let first = &authority::ASSETS[0];
        symlink("somewhere-else", asset_path_in(&release, first)).expect("symlink");
        assert_eq!(state(&root), CacheState::RepairNeeded);
        remove(&root).expect("known symlink is removed without following it");
        cleanup_sidecars(&root);
    }

    #[test]
    #[ignore = "release/manual：需要 AI_SISTER_ASSET_PACK 指到 73 MB 公開 pack"]
    fn public_pack_installs_resolves_offline_and_is_precisely_removed() {
        let pack = std::env::var_os("AI_SISTER_ASSET_PACK").expect("set AI_SISTER_ASSET_PACK");
        let archive = fs::read(pack).expect("read public pack");
        let root = temp_root("public-pack");
        fs::create_dir(&root).expect("test parent");
        let cache = root.join(CACHE_DIRECTORY);

        assert_eq!(
            install_from_bytes(&cache, &archive).expect("install exact pack"),
            InstallOutcome::Installed
        );
        assert_eq!(state(&cache), CacheState::Installed);
        for persona in [
            Persona::Chatgpt,
            Persona::Claude,
            Persona::Gemini,
            Persona::Grok,
        ] {
            assert!(
                !read_portrait(&cache, persona)
                    .expect("portrait")
                    .bytes
                    .is_empty()
            );
            assert_eq!(
                voice_metadata(&cache, persona)
                    .expect("voice catalog")
                    .len(),
                2
            );
        }
        for line in [
            VoiceLine::ChatgptGreeting,
            VoiceLine::ClaudeQuiet,
            VoiceLine::GeminiQuiet,
            VoiceLine::GrokGreeting,
        ] {
            assert_eq!(read_voice(&cache, line).expect("voice").line, line);
        }

        delete_known_journal(&cache);
        assert_eq!(state(&cache), CacheState::RepairNeeded);
        assert_eq!(
            install_from_bytes(&cache, &archive).expect("repair missing journal"),
            InstallOutcome::Installed
        );
        let authorization = sibling_path(&cache, AUTHORIZATION_SUFFIX).expect("authorization path");
        fs::write(&authorization, b"broken").expect("corrupt authorization");
        assert_eq!(state(&cache), CacheState::RepairNeeded);
        assert_eq!(
            install_from_bytes(&cache, &archive).expect("repair corrupt authorization"),
            InstallOutcome::Installed
        );

        let portrait = authority::portrait(Persona::Chatgpt);
        let portrait_path = asset_path_in(&release_directory(&cache), portrait);
        let mut corrupt = fs::read(&portrait_path).expect("read installed portrait");
        corrupt[12] ^= 1;
        fs::write(&portrait_path, corrupt).expect("corrupt installed portrait");
        assert_eq!(state(&cache), CacheState::RepairNeeded);
        assert!(read_portrait(&cache, Persona::Chatgpt).is_err());
        assert_eq!(
            install_from_bytes(&cache, &archive).expect("repair corrupt exact release"),
            InstallOutcome::Installed
        );
        assert_eq!(state(&cache), CacheState::Installed);
        assert_eq!(
            remove(&cache).expect("remove pack"),
            RemovalOutcome::Removed
        );
        assert_eq!(state(&cache), CacheState::Available);
        assert_eq!(
            install_from_bytes(&cache, &archive).expect("explicit reinstall clears revocation"),
            InstallOutcome::Installed
        );
        assert_eq!(state(&cache), CacheState::Installed);
        remove(&cache).expect("final remove");
        cleanup_sidecars(&cache);
        fs::remove_dir(root).expect("empty test parent");
    }
}
