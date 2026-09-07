//! 只讀取 Persona authority 允許的 classic ZIP profile。
//!
//! pack 是 deterministic、stored-only 的 classic ZIP。這裡不接受 ZIP64、壓縮、
//! data descriptor、extra field、comment、directory、symlink 或不連續 local data；
//! 也不把 archive entry 的名字拿去 join 本機路徑。只有 authority 指到的 12 個
//! object bytes 會在驗完整包之後交給 cache 層。

use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::authority::{self, Asset, Kind};
use crate::{Error, Result};

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const UTF8_FLAG: u16 = 0x0800;
const VERSION_20: u16 = 20;
const UNIX_VERSION_20: u16 = (3 << 8) | VERSION_20;
const UNIX_REGULAR_0600: u32 = 0o100600 << 16;

#[derive(Debug)]
struct Entry<'a> {
    name: &'a str,
    crc32: u32,
    size: usize,
    local_offset: usize,
    modified_time: u16,
    modified_date: u16,
    data: &'a [u8],
}

pub(crate) struct Selected<'a> {
    entries: Vec<(&'static Asset, &'a [u8])>,
}

impl<'a> Selected<'a> {
    pub(crate) fn get(&self, asset: &'static Asset) -> Option<&'a [u8]> {
        self.entries
            .iter()
            .find_map(|(found, bytes)| (found.object == asset.object).then_some(*bytes))
    }
}

pub(crate) fn validate<'a>(bytes: &'a [u8], cancelled: &AtomicBool) -> Result<Selected<'a>> {
    ensure_not_cancelled(cancelled)?;
    if !authority::selected_rights_are_approved() {
        return Err(Error::ArchiveRejected);
    }
    let entries = parse_cancellable(
        bytes,
        authority::PACK_BYTES,
        authority::PACK_SHA256,
        authority::PACK_ENTRY_COUNT,
        authority::PACK_EXTRACTED_BYTES,
        cancelled,
    )?;

    let mut selected = Vec::with_capacity(authority::ASSETS.len());
    for asset in &authority::ASSETS {
        ensure_not_cancelled(cancelled)?;
        let Some(entry) = entries.iter().find(|entry| entry.name == asset.object) else {
            return Err(Error::ArchiveRejected);
        };
        if entry.size != asset.bytes || media_kind(entry.data) != Some(asset.kind) {
            return Err(Error::ArchiveRejected);
        }
        selected.push((asset, entry.data));
    }
    Ok(Selected { entries: selected })
}

fn parse_cancellable<'a>(
    bytes: &'a [u8],
    expected_bytes: usize,
    expected_sha256: &str,
    expected_entries: usize,
    expected_extracted_bytes: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<Entry<'a>>> {
    ensure_not_cancelled(cancelled)?;
    if bytes.len() != expected_bytes || sha256_hex_cancellable(bytes, cancelled)? != expected_sha256
    {
        return Err(Error::ArchiveRejected);
    }
    if bytes.len() < 22 || expected_entries == 0 || expected_entries >= u16::MAX as usize {
        return Err(Error::ArchiveRejected);
    }

    let eocd_offset = bytes.len() - 22;
    let eocd = take(bytes, eocd_offset, 22)?;
    if le_u32(eocd, 0)? != EOCD_SIGNATURE
        || le_u16(eocd, 4)? != 0
        || le_u16(eocd, 6)? != 0
        || le_u16(eocd, 8)? as usize != expected_entries
        || le_u16(eocd, 10)? as usize != expected_entries
        || le_u16(eocd, 20)? != 0
    {
        return Err(Error::ArchiveRejected);
    }
    if eocd_offset >= 20 && le_u32(bytes, eocd_offset - 20).ok() == Some(ZIP64_LOCATOR_SIGNATURE) {
        return Err(Error::ArchiveRejected);
    }
    let central_size = usize::try_from(le_u32(eocd, 12)?).map_err(|_| Error::ArchiveRejected)?;
    let central_offset = usize::try_from(le_u32(eocd, 16)?).map_err(|_| Error::ArchiveRejected)?;
    if central_size == 0
        || central_offset == 0
        || central_offset.checked_add(central_size) != Some(eocd_offset)
    {
        return Err(Error::ArchiveRejected);
    }

    let mut entries = Vec::with_capacity(expected_entries);
    let mut local_offset = 0usize;
    let mut extracted = 0usize;
    let mut previous_name: Option<&str> = None;
    while local_offset < central_offset {
        ensure_not_cancelled(cancelled)?;
        let header = take(bytes, local_offset, 30)?;
        if le_u32(header, 0)? != LOCAL_SIGNATURE
            || le_u16(header, 4)? != VERSION_20
            || le_u16(header, 6)? != UTF8_FLAG
            || le_u16(header, 8)? != 0
            || le_u16(header, 28)? != 0
        {
            return Err(Error::ArchiveRejected);
        }
        let compressed =
            usize::try_from(le_u32(header, 18)?).map_err(|_| Error::ArchiveRejected)?;
        let uncompressed =
            usize::try_from(le_u32(header, 22)?).map_err(|_| Error::ArchiveRejected)?;
        let name_len = usize::from(le_u16(header, 26)?);
        if compressed != uncompressed || name_len == 0 {
            return Err(Error::ArchiveRejected);
        }
        let name_offset = local_offset.checked_add(30).ok_or(Error::ArchiveRejected)?;
        let name_bytes = take(bytes, name_offset, name_len)?;
        let name = std::str::from_utf8(name_bytes).map_err(|_| Error::ArchiveRejected)?;
        let object_sha = safe_object_sha(name).ok_or(Error::ArchiveRejected)?;
        if previous_name.is_some_and(|previous| previous >= name) {
            return Err(Error::ArchiveRejected);
        }
        let data_offset = name_offset
            .checked_add(name_len)
            .ok_or(Error::ArchiveRejected)?;
        let data = take(bytes, data_offset, compressed)?;
        if sha256_hex_cancellable(data, cancelled)? != object_sha || media_kind(data).is_none() {
            return Err(Error::ArchiveRejected);
        }
        let next = data_offset
            .checked_add(compressed)
            .ok_or(Error::ArchiveRejected)?;
        if next > central_offset {
            return Err(Error::ArchiveRejected);
        }
        extracted = extracted
            .checked_add(uncompressed)
            .ok_or(Error::ArchiveRejected)?;
        if extracted > expected_extracted_bytes || entries.len() >= expected_entries {
            return Err(Error::ArchiveRejected);
        }
        entries.push(Entry {
            name,
            crc32: le_u32(header, 14)?,
            size: uncompressed,
            local_offset,
            modified_time: le_u16(header, 10)?,
            modified_date: le_u16(header, 12)?,
            data,
        });
        previous_name = Some(name);
        local_offset = next;
    }
    if local_offset != central_offset
        || entries.len() != expected_entries
        || extracted != expected_extracted_bytes
    {
        return Err(Error::ArchiveRejected);
    }

    let mut central = central_offset;
    for entry in &entries {
        ensure_not_cancelled(cancelled)?;
        let header = take(bytes, central, 46)?;
        if le_u32(header, 0)? != CENTRAL_SIGNATURE
            || le_u16(header, 4)? != UNIX_VERSION_20
            || le_u16(header, 6)? != VERSION_20
            || le_u16(header, 8)? != UTF8_FLAG
            || le_u16(header, 10)? != 0
            || le_u16(header, 12)? != entry.modified_time
            || le_u16(header, 14)? != entry.modified_date
            || le_u32(header, 16)? != entry.crc32
            || usize::try_from(le_u32(header, 20)?).ok() != Some(entry.size)
            || usize::try_from(le_u32(header, 24)?).ok() != Some(entry.size)
            || le_u16(header, 30)? != 0
            || le_u16(header, 32)? != 0
            || le_u16(header, 34)? != 0
            || le_u16(header, 36)? != 0
            || le_u32(header, 38)? != UNIX_REGULAR_0600
            || usize::try_from(le_u32(header, 42)?).ok() != Some(entry.local_offset)
        {
            return Err(Error::ArchiveRejected);
        }
        let name_len = usize::from(le_u16(header, 28)?);
        let name_offset = central.checked_add(46).ok_or(Error::ArchiveRejected)?;
        let central_name = take(bytes, name_offset, name_len)?;
        if central_name != entry.name.as_bytes() {
            return Err(Error::ArchiveRejected);
        }
        central = name_offset
            .checked_add(name_len)
            .ok_or(Error::ArchiveRejected)?;
    }
    if central != eocd_offset {
        return Err(Error::ArchiveRejected);
    }
    ensure_not_cancelled(cancelled)?;
    Ok(entries)
}

#[cfg(test)]
fn parse<'a>(
    bytes: &'a [u8],
    expected_bytes: usize,
    expected_sha256: &str,
    expected_entries: usize,
    expected_extracted_bytes: usize,
) -> Result<Vec<Entry<'a>>> {
    let cancelled = AtomicBool::new(false);
    parse_cancellable(
        bytes,
        expected_bytes,
        expected_sha256,
        expected_entries,
        expected_extracted_bytes,
        &cancelled,
    )
}

fn safe_object_sha(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("objects/")?;
    let (sha, extension) = rest.rsplit_once('.')?;
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !matches!(extension, "webp" | "wav")
    {
        return None;
    }
    Some(sha)
}

fn media_kind(bytes: &[u8]) -> Option<Kind> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        Some(Kind::Wav)
    } else {
        None
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest_hex(digest)
}

pub(crate) fn sha256_hex_cancellable(bytes: &[u8], cancelled: &AtomicBool) -> Result<String> {
    let mut digest = Sha256::new();
    for chunk in bytes.chunks(64 * 1024) {
        ensure_not_cancelled(cancelled)?;
        digest.update(chunk);
    }
    ensure_not_cancelled(cancelled)?;
    Ok(digest_hex(digest.finalize()))
}

fn digest_hex(digest: impl IntoIterator<Item = u8>) -> String {
    let mut text = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn take(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset.checked_add(length).ok_or(Error::ArchiveRejected)?;
    bytes.get(offset..end).ok_or(Error::ArchiveRejected)
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let raw: [u8; 2] = take(bytes, offset, 2)?
        .try_into()
        .map_err(|_| Error::ArchiveRejected)?;
    Ok(u16::from_le_bytes(raw))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw: [u8; 4] = take(bytes, offset, 4)?
        .try_into()
        .map_err(|_| Error::ArchiveRejected)?;
    Ok(u32::from_le_bytes(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_u16(to: &mut Vec<u8>, value: u16) {
        to.extend(value.to_le_bytes());
    }

    fn push_u32(to: &mut Vec<u8>, value: u32) {
        to.extend(value.to_le_bytes());
    }

    fn fixture(data: &[u8], extension: &str) -> (Vec<u8>, String) {
        let sha = sha256_hex(data);
        let name = format!("objects/{sha}.{extension}");
        let name_bytes = name.as_bytes();
        let mut zip = Vec::new();
        push_u32(&mut zip, LOCAL_SIGNATURE);
        push_u16(&mut zip, VERSION_20);
        push_u16(&mut zip, UTF8_FLAG);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u32(&mut zip, 0x1234_5678);
        push_u32(&mut zip, data.len() as u32);
        push_u32(&mut zip, data.len() as u32);
        push_u16(&mut zip, name_bytes.len() as u16);
        push_u16(&mut zip, 0);
        zip.extend(name_bytes);
        zip.extend(data);
        let central_offset = zip.len();
        push_u32(&mut zip, CENTRAL_SIGNATURE);
        push_u16(&mut zip, UNIX_VERSION_20);
        push_u16(&mut zip, VERSION_20);
        push_u16(&mut zip, UTF8_FLAG);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u32(&mut zip, 0x1234_5678);
        push_u32(&mut zip, data.len() as u32);
        push_u32(&mut zip, data.len() as u32);
        push_u16(&mut zip, name_bytes.len() as u16);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u32(&mut zip, UNIX_REGULAR_0600);
        push_u32(&mut zip, 0);
        zip.extend(name_bytes);
        let central_size = zip.len() - central_offset;
        push_u32(&mut zip, EOCD_SIGNATURE);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 0);
        push_u16(&mut zip, 1);
        push_u16(&mut zip, 1);
        push_u32(&mut zip, central_size as u32);
        push_u32(&mut zip, central_offset as u32);
        push_u16(&mut zip, 0);
        let digest = sha256_hex(&zip);
        (zip, digest)
    }

    #[test]
    fn exact_stored_zip_is_accepted() {
        let data = b"RIFF\x04\0\0\0WEBP";
        let (zip, digest) = fixture(data, "webp");
        let entries = parse(&zip, zip.len(), &digest, 1, data.len()).expect("valid fixture");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data, data);
    }

    #[test]
    fn whole_pack_hash_and_length_are_both_required() {
        let data = b"RIFF\x04\0\0\0WAVE";
        let (mut zip, digest) = fixture(data, "wav");
        zip[10] ^= 1;
        assert!(matches!(
            parse(&zip, zip.len(), &digest, 1, data.len()),
            Err(Error::ArchiveRejected)
        ));
        let (_, digest) = fixture(data, "wav");
        let (zip, _) = fixture(data, "wav");
        assert!(parse(&zip, zip.len() + 1, &digest, 1, data.len()).is_err());
    }

    #[test]
    fn unsafe_or_non_content_addressed_names_are_rejected() {
        let data = b"RIFF\x04\0\0\0WEBP";
        let (mut zip, _) = fixture(data, "webp");
        let first_name = 30;
        zip[first_name] = b'x';
        let digest = sha256_hex(&zip);
        assert!(parse(&zip, zip.len(), &digest, 1, data.len()).is_err());
    }

    #[test]
    fn compression_flags_extra_fields_and_file_kind_are_fail_closed() {
        let data = b"RIFF\x04\0\0\0WEBP";
        for offset in [6usize, 8, 28] {
            let (mut zip, _) = fixture(data, "webp");
            zip[offset] ^= 1;
            let digest = sha256_hex(&zip);
            assert!(
                parse(&zip, zip.len(), &digest, 1, data.len()).is_err(),
                "local header offset {offset}"
            );
        }

        let bad = b"this is not media";
        let (zip, digest) = fixture(bad, "webp");
        assert!(parse(&zip, zip.len(), &digest, 1, bad.len()).is_err());
    }

    #[test]
    fn central_directory_must_describe_the_same_regular_file() {
        let data = b"RIFF\x04\0\0\0WAVE";
        let (original, _) = fixture(data, "wav");
        let central = 30 + 76 + data.len();
        for offset in [central + 8, central + 10, central + 38, central + 42] {
            let mut zip = original.clone();
            zip[offset] ^= 1;
            let digest = sha256_hex(&zip);
            assert!(
                parse(&zip, zip.len(), &digest, 1, data.len()).is_err(),
                "central header offset {offset}"
            );
        }
    }

    #[test]
    #[ignore = "release/manual：需要 AI_SISTER_ASSET_PACK 指到 73 MB 公開 pack"]
    fn pinned_public_pack_passes_the_same_runtime_validator() {
        let path = std::env::var_os("AI_SISTER_ASSET_PACK").expect("set AI_SISTER_ASSET_PACK");
        let bytes = std::fs::read(path).expect("read public pack");
        let cancelled = AtomicBool::new(false);
        let selected = validate(&bytes, &cancelled).expect("pinned public pack must validate");
        for asset in &authority::ASSETS {
            assert_eq!(selected.get(asset).map(<[u8]>::len), Some(asset.bytes));
        }
    }
}
