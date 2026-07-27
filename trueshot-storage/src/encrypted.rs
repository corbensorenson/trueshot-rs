//! Authenticated, seekable encryption for large local assets.
//!
//! TSE2 uses a fixed-size authenticated header followed by independently
//! authenticated fixed-size chunks. Chunk offsets are computable directly,
//! so a small RAW ROI never requires decrypting the complete source file.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use anyhow::{Context, Result};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use zeroize::Zeroizing;

pub const MAGIC: [u8; 4] = *b"TSE2";
pub const VERSION: u8 = 2;
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
const HEADER_LEN: usize = 64;
const HEADER_AAD_LEN: usize = 48;
const TAG_LEN: usize = 16;
const HEADER_NONCE_COUNTER: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionStats {
    pub plaintext_bytes: u64,
    pub encrypted_bytes: u64,
    pub chunks: u32,
}

#[derive(Debug, Clone)]
struct Header {
    chunk_size: usize,
    plaintext_len: u64,
    nonce_prefix: [u8; 8],
    aad: [u8; HEADER_AAD_LEN],
}

impl Header {
    fn chunk_count(&self) -> Result<u32> {
        if self.plaintext_len == 0 {
            return Ok(0);
        }
        let chunks = self.plaintext_len.div_ceil(self.chunk_size as u64);
        u32::try_from(chunks).context("Encrypted asset exceeds the TSE2 chunk limit")
    }

    fn chunk_plaintext_len(&self, index: u32) -> Result<usize> {
        let count = self.chunk_count()?;
        if index >= count {
            anyhow::bail!("Encrypted chunk index {index} exceeds {count} chunks");
        }
        let offset = u64::from(index)
            .checked_mul(self.chunk_size as u64)
            .context("Encrypted chunk offset overflow")?;
        Ok(self
            .plaintext_len
            .saturating_sub(offset)
            .min(self.chunk_size as u64) as usize)
    }

    fn encoded_len(&self) -> Result<u64> {
        let chunks = u64::from(self.chunk_count()?);
        (HEADER_LEN as u64)
            .checked_add(self.plaintext_len)
            .and_then(|value| value.checked_add(chunks * TAG_LEN as u64))
            .context("Encrypted asset length overflow")
    }
}

/// A random-access plaintext view over a TSE2 encrypted file.
///
/// Only the chunk containing the requested position is retained in memory.
/// Authentication is verified before any byte from that chunk is returned.
pub struct SeekableEncryptedFile {
    file: File,
    cipher: Aes256Gcm,
    header: Header,
    position: u64,
    cached_index: Option<u32>,
    cached_plaintext: Zeroizing<Vec<u8>>,
}

impl SeekableEncryptedFile {
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("Open encrypted asset {}", path.display()))?;
        Self::from_file(file, key)
            .with_context(|| format!("Authenticate encrypted asset {}", path.display()))
    }

    pub fn from_file(mut file: File, key: &[u8; 32]) -> Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let (header, cipher) = read_header(&mut file, key)?;
        let actual_len = file.metadata()?.len();
        let expected_len = header.encoded_len()?;
        if actual_len != expected_len {
            anyhow::bail!(
                "Encrypted asset length mismatch: expected {expected_len} bytes, found {actual_len}"
            );
        }
        Ok(Self {
            file,
            cipher,
            header,
            position: 0,
            cached_index: None,
            cached_plaintext: Zeroizing::new(Vec::new()),
        })
    }

    pub fn plaintext_len(&self) -> u64 {
        self.header.plaintext_len
    }

    pub fn chunk_size(&self) -> usize {
        self.header.chunk_size
    }

    fn load_chunk(&mut self, index: u32) -> Result<()> {
        if self.cached_index == Some(index) {
            return Ok(());
        }
        let plain_len = self.header.chunk_plaintext_len(index)?;
        let record_stride = self.header.chunk_size as u64 + TAG_LEN as u64;
        let encoded_offset = (HEADER_LEN as u64)
            .checked_add(
                u64::from(index)
                    .checked_mul(record_stride)
                    .context("Encrypted chunk record offset overflow")?,
            )
            .context("Encrypted chunk record offset overflow")?;
        self.file.seek(SeekFrom::Start(encoded_offset))?;
        let mut ciphertext = Zeroizing::new(vec![0u8; plain_len + TAG_LEN]);
        self.file.read_exact(&mut ciphertext)?;
        let nonce = build_nonce(&self.header.nonce_prefix, index);
        let aad = chunk_aad(&self.header.aad, index, plain_len)?;
        let plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("Encrypted chunk {index} authentication failed"))?;
        self.cached_plaintext = Zeroizing::new(plaintext);
        self.cached_index = Some(index);
        Ok(())
    }
}

impl Read for SeekableEncryptedFile {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.position >= self.header.plaintext_len {
            return Ok(0);
        }
        let mut written = 0usize;
        while written < output.len() && self.position < self.header.plaintext_len {
            let index_u64 = self.position / self.header.chunk_size as u64;
            let index = u32::try_from(index_u64).map_err(|_| {
                Error::new(ErrorKind::InvalidData, "Encrypted chunk index overflow")
            })?;
            self.load_chunk(index)
                .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;
            let within = (self.position % self.header.chunk_size as u64) as usize;
            let available = self.cached_plaintext.len().saturating_sub(within);
            if available == 0 {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "Authenticated chunk ended before declared plaintext length",
                ));
            }
            let count = available.min(output.len() - written);
            output[written..written + count]
                .copy_from_slice(&self.cached_plaintext[within..within + count]);
            written += count;
            self.position += count as u64;
        }
        Ok(written)
    }
}

impl Seek for SeekableEncryptedFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let target = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.header.plaintext_len) + i128::from(delta),
        };
        if target < 0 || target > i128::from(u64::MAX) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Encrypted seek is outside the representable file range",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

pub fn encrypt_file(
    input: &Path,
    output: &Path,
    key: &[u8; 32],
    chunk_size: usize,
) -> Result<EncryptionStats> {
    validate_chunk_size(chunk_size)?;
    let mut source =
        File::open(input).with_context(|| format!("Open plaintext asset {}", input.display()))?;
    let plaintext_len = source.metadata()?.len();
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .with_context(|| format!("Create encrypted asset {}", output.display()))?;
    let stats = encrypt_reader(
        &mut source,
        &mut destination,
        plaintext_len,
        key,
        chunk_size,
    )?;
    destination.sync_all()?;
    Ok(stats)
}

pub fn encrypt_bytes(
    output: &Path,
    key: &[u8; 32],
    bytes: &[u8],
    chunk_size: usize,
) -> Result<EncryptionStats> {
    validate_chunk_size(chunk_size)?;
    let mut source = std::io::Cursor::new(bytes);
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .with_context(|| format!("Create encrypted asset {}", output.display()))?;
    let stats = encrypt_reader(
        &mut source,
        &mut destination,
        bytes.len() as u64,
        key,
        chunk_size,
    )?;
    destination.sync_all()?;
    Ok(stats)
}

/// Encrypts bytes into an already-open destination.
///
/// This allows callers to keep publication authority descriptor-relative and
/// atomically reveal the completed TSE2 file only after encryption succeeds.
pub fn encrypt_bytes_to_writer<W: Write>(
    destination: &mut W,
    key: &[u8; 32],
    bytes: &[u8],
    chunk_size: usize,
) -> Result<EncryptionStats> {
    validate_chunk_size(chunk_size)?;
    let mut source = std::io::Cursor::new(bytes);
    encrypt_reader(
        &mut source,
        destination,
        bytes.len() as u64,
        key,
        chunk_size,
    )
}

pub fn encrypt_reader_to_writer<R: Read, W: Write>(
    source: &mut R,
    destination: &mut W,
    plaintext_len: u64,
    key: &[u8; 32],
    chunk_size: usize,
) -> Result<EncryptionStats> {
    validate_chunk_size(chunk_size)?;
    encrypt_reader(source, destination, plaintext_len, key, chunk_size)
}

pub fn decrypt_file(input: &Path, output: &Path, key: &[u8; 32]) -> Result<EncryptionStats> {
    let mut source = SeekableEncryptedFile::open(input, key)?;
    let plaintext_len = source.plaintext_len();
    let chunks = source.header.chunk_count()?;
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .with_context(|| format!("Create decrypted asset {}", output.display()))?;
    std::io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    Ok(EncryptionStats {
        plaintext_bytes: plaintext_len,
        encrypted_bytes: source.header.encoded_len()?,
        chunks,
    })
}

pub fn decrypt_to_vec(path: &Path, key: &[u8; 32], max_plaintext_bytes: usize) -> Result<Vec<u8>> {
    let mut source = SeekableEncryptedFile::open(path, key)?;
    let plaintext_len = usize::try_from(source.plaintext_len())
        .context("Encrypted plaintext length exceeds this platform")?;
    if plaintext_len > max_plaintext_bytes {
        anyhow::bail!(
            "Encrypted plaintext exceeds the {} byte read limit",
            max_plaintext_bytes
        );
    }
    let mut output = Vec::with_capacity(plaintext_len);
    source.read_to_end(&mut output)?;
    Ok(output)
}

fn encrypt_reader<R: Read, W: Write>(
    source: &mut R,
    destination: &mut W,
    plaintext_len: u64,
    key: &[u8; 32],
    chunk_size: usize,
) -> Result<EncryptionStats> {
    let mut file_id = [0u8; 16];
    let mut nonce_prefix = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut file_id);
    rand::thread_rng().fill_bytes(&mut nonce_prefix);
    let cipher = derive_file_cipher(key, &file_id)?;
    let (header, encoded_header) =
        build_header(chunk_size, plaintext_len, file_id, nonce_prefix, &cipher)?;
    destination.write_all(&encoded_header)?;

    let mut buffer = Zeroizing::new(vec![0u8; chunk_size]);
    let mut consumed = 0u64;
    let chunk_count = header.chunk_count()?;
    for index in 0..chunk_count {
        let plain_len = header.chunk_plaintext_len(index)?;
        source
            .read_exact(&mut buffer[..plain_len])
            .with_context(|| format!("Read plaintext chunk {index}"))?;
        let nonce = build_nonce(&nonce_prefix, index);
        let aad = chunk_aad(&header.aad, index, plain_len)?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &buffer[..plain_len],
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("Encrypt plaintext chunk {index} failed"))?;
        destination.write_all(&ciphertext)?;
        consumed = consumed.saturating_add(plain_len as u64);
    }
    if consumed != plaintext_len {
        anyhow::bail!(
            "Plaintext changed during encryption: expected {plaintext_len} bytes, read {consumed}"
        );
    }
    let mut trailing = [0u8; 1];
    if source.read(&mut trailing)? != 0 {
        anyhow::bail!("Plaintext grew during encryption");
    }
    Ok(EncryptionStats {
        plaintext_bytes: plaintext_len,
        encrypted_bytes: header.encoded_len()?,
        chunks: chunk_count,
    })
}

fn build_header(
    chunk_size: usize,
    plaintext_len: u64,
    file_id: [u8; 16],
    nonce_prefix: [u8; 8],
    cipher: &Aes256Gcm,
) -> Result<(Header, [u8; HEADER_LEN])> {
    validate_chunk_size(chunk_size)?;
    let mut encoded = [0u8; HEADER_LEN];
    encoded[..4].copy_from_slice(&MAGIC);
    encoded[4] = VERSION;
    encoded[5] = 0;
    encoded[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    encoded[8..12].copy_from_slice(&(chunk_size as u32).to_le_bytes());
    encoded[12..20].copy_from_slice(&plaintext_len.to_le_bytes());
    encoded[20..36].copy_from_slice(&file_id);
    encoded[36..44].copy_from_slice(&nonce_prefix);
    let mut aad = [0u8; HEADER_AAD_LEN];
    aad.copy_from_slice(&encoded[..HEADER_AAD_LEN]);
    let nonce = build_nonce(&nonce_prefix, HEADER_NONCE_COUNTER);
    let tag = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &[],
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("Authenticate encrypted header failed"))?;
    if tag.len() != TAG_LEN {
        anyhow::bail!("Unexpected encrypted header tag length");
    }
    encoded[HEADER_AAD_LEN..].copy_from_slice(&tag);
    Ok((
        Header {
            chunk_size,
            plaintext_len,
            nonce_prefix,
            aad,
        },
        encoded,
    ))
}

fn read_header(file: &mut File, key: &[u8; 32]) -> Result<(Header, Aes256Gcm)> {
    let mut encoded = [0u8; HEADER_LEN];
    file.read_exact(&mut encoded)?;
    if encoded[..4] != MAGIC {
        anyhow::bail!("Invalid TSE2 encryption magic");
    }
    if encoded[4] != VERSION {
        anyhow::bail!("Unsupported TSE2 encryption version {}", encoded[4]);
    }
    if encoded[5] != 0 {
        anyhow::bail!("Unsupported TSE2 encryption flags");
    }
    if u16::from_le_bytes([encoded[6], encoded[7]]) as usize != HEADER_LEN {
        anyhow::bail!("Invalid TSE2 header length");
    }
    if encoded[44..48] != [0u8; 4] {
        anyhow::bail!("Unsupported TSE2 reserved header fields");
    }
    let chunk_size =
        u32::from_le_bytes(encoded[8..12].try_into().expect("fixed header slice")) as usize;
    validate_chunk_size(chunk_size)?;
    let plaintext_len = u64::from_le_bytes(encoded[12..20].try_into().expect("fixed header slice"));
    let mut file_id = [0u8; 16];
    file_id.copy_from_slice(&encoded[20..36]);
    let cipher = derive_file_cipher(key, &file_id)?;
    let mut nonce_prefix = [0u8; 8];
    nonce_prefix.copy_from_slice(&encoded[36..44]);
    let mut aad = [0u8; HEADER_AAD_LEN];
    aad.copy_from_slice(&encoded[..HEADER_AAD_LEN]);
    let nonce = build_nonce(&nonce_prefix, HEADER_NONCE_COUNTER);
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &encoded[HEADER_AAD_LEN..],
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("Encrypted header authentication failed"))?;
    if !plaintext.is_empty() {
        anyhow::bail!("Encrypted header authentication produced unexpected plaintext");
    }
    let header = Header {
        chunk_size,
        plaintext_len,
        nonce_prefix,
        aad,
    };
    header.chunk_count()?;
    Ok((header, cipher))
}

fn validate_chunk_size(chunk_size: usize) -> Result<()> {
    if !(64 * 1024..=16 * 1024 * 1024).contains(&chunk_size) || !chunk_size.is_power_of_two() {
        anyhow::bail!("TSE2 chunk size must be a power of two between 64 KiB and 16 MiB");
    }
    Ok(())
}

fn build_nonce(prefix: &[u8; 8], counter: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(prefix);
    nonce[8..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

fn derive_file_cipher(master_key: &[u8; 32], file_id: &[u8; 16]) -> Result<Aes256Gcm> {
    let hkdf = Hkdf::<Sha256>::new(Some(file_id), master_key);
    let mut file_key = Zeroizing::new([0u8; 32]);
    hkdf.expand(b"trueshot-tse2-aes256gcm-file-key-v1", &mut *file_key)
        .map_err(|_| anyhow::anyhow!("Derive TSE2 per-file encryption key failed"))?;
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&file_key[..])))
}

fn chunk_aad(header_aad: &[u8; HEADER_AAD_LEN], index: u32, plain_len: usize) -> Result<Vec<u8>> {
    let plain_len = u32::try_from(plain_len).context("Encrypted chunk length exceeds u32")?;
    let mut aad = Vec::with_capacity(HEADER_AAD_LEN + 8);
    aad.extend_from_slice(header_aad);
    aad.extend_from_slice(&index.to_le_bytes());
    aad.extend_from_slice(&plain_len.to_le_bytes());
    Ok(aad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypted_fixture(
        bytes: &[u8],
        chunk_size: usize,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("asset.nef.enc");
        encrypt_bytes(&path, &[0x5au8; 32], bytes, chunk_size).unwrap();
        (directory, path)
    }

    #[test]
    fn random_seek_crosses_authenticated_chunks() {
        let bytes = (0..400_000u32)
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let (_directory, path) = encrypted_fixture(&bytes, 64 * 1024);
        let mut reader = SeekableEncryptedFile::open(&path, &[0x5au8; 32]).unwrap();
        let mut output = vec![0u8; 90_000];
        reader.seek(SeekFrom::Start(61_000)).unwrap();
        reader.read_exact(&mut output).unwrap();
        assert_eq!(output, bytes[61_000..151_000]);
        reader.seek(SeekFrom::End(-37)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, bytes[bytes.len() - 37..]);
    }

    #[test]
    fn header_tampering_is_rejected_before_reads() {
        let (_directory, path) = encrypted_fixture(b"measured pixels", 64 * 1024);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[12] ^= 0x40;
        std::fs::write(&path, bytes).unwrap();
        assert!(SeekableEncryptedFile::open(&path, &[0x5au8; 32]).is_err());
    }

    #[test]
    fn chunk_swapping_is_rejected_by_position_bound_aad() {
        let bytes = vec![0x31u8; 3 * 64 * 1024];
        let (_directory, path) = encrypted_fixture(&bytes, 64 * 1024);
        let mut encoded = std::fs::read(&path).unwrap();
        let stride = 64 * 1024 + TAG_LEN;
        let first = encoded[HEADER_LEN..HEADER_LEN + stride].to_vec();
        let second = encoded[HEADER_LEN + stride..HEADER_LEN + 2 * stride].to_vec();
        encoded[HEADER_LEN..HEADER_LEN + stride].copy_from_slice(&second);
        encoded[HEADER_LEN + stride..HEADER_LEN + 2 * stride].copy_from_slice(&first);
        std::fs::write(&path, encoded).unwrap();
        let mut reader = SeekableEncryptedFile::open(&path, &[0x5au8; 32]).unwrap();
        let mut output = [0u8; 1];
        assert!(reader.read_exact(&mut output).is_err());
    }

    #[test]
    fn truncation_and_append_are_rejected_at_open() {
        let (_directory, path) = encrypted_fixture(&vec![7u8; 80_000], 64 * 1024);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(SeekableEncryptedFile::open(&path, &[0x5au8; 32]).is_err());
        let mut appended = original;
        appended.push(0);
        std::fs::write(&path, appended).unwrap();
        assert!(SeekableEncryptedFile::open(&path, &[0x5au8; 32]).is_err());
    }

    #[test]
    fn wrong_key_is_rejected_by_header_authentication() {
        let (_directory, path) = encrypted_fixture(b"pixels", 64 * 1024);
        assert!(SeekableEncryptedFile::open(&path, &[0x7bu8; 32]).is_err());
    }
}
