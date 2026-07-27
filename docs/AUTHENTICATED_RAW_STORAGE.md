# Authenticated Seekable RAW Storage

TrueShot's `TSE2` container protects large local RAW assets without forcing a
whole-file decrypt or a plaintext staging file.

## Format And Cryptography

- AES-256-GCM authenticates the header and every data chunk.
- A random 128-bit file identity and HKDF-SHA256 derive an independent file key
  from the project key.
- The 64-byte fixed header binds version, flags, chunk size, plaintext length,
  file identity, and nonce prefix.
- Chunk AAD binds the authenticated header, zero-based chunk index, and exact
  plaintext length.
- Fixed records make encrypted offsets directly computable. The final chunk
  length follows from the authenticated total plaintext length.
- Opening validates the exact encoded file length. Truncation and appended data
  fail before processing.

The default chunk is 1 MiB. A reader caches one authenticated plaintext chunk,
supports standard `Read + Seek`, and zeroizes cached plaintext on replacement
or drop.

## NEF Behavior

- Plain NEFs retain the existing read-only `mmap` path.
- Encrypted TIFF, EXIF, MakerNote, and preview parsing use authenticated seeks.
- Encrypted preview extraction does not use the full-file JPEG-marker scan
  fallback.
- RAW decode reads only TIFF strips intersecting the requested ROI. Nikon
  compression may still require decoding the selected compressed strip; TSE2
  does not falsely claim arbitrary entropy-code seekability.
- One preview-derived crop remains shared across an HDR/focus group.
- Encrypted noise, sensor-correction, and lens-PSF profiles use bounded
  authenticated reads and preserve their plaintext SHA-256 replay identity.

## Publication And Recovery

Writers create a same-directory unique partial file, sync it, and publish with a
create-if-absent hard link. Plaintext is removed only after the final encrypted
asset authenticates and its declared plaintext length matches the source.
Wrong keys, damaged headers, swapped chunks, partial writes, and length changes
cannot authorize cleanup.

`TSE1` remains readable for bounded legacy reports and explicit decryption. It
is not accepted as authenticated-seekable RAW and must be migrated before
encrypted refusion.

## Processor Boundary

The local server preflights every encrypted RAW before launching the packaged
processor. The zeroizing project key is serialized only into the existing
anonymous child stdin pipe together with the bounded replay envelope. It is
never placed in argv, an environment variable, a project file, or a plaintext
temporary.

## Qualification

- Storage unit gates cover random cross-chunk seeks, wrong keys, authenticated
  header mutation, chunk swapping, truncation, and appended bytes.
- Server gates cover bounded `TSE1` compatibility, bounded `TSE2` reads, atomic
  no-replace publication, concurrent publication, and authenticated crash
  recovery.
- `trueshot-core/tests/encrypted_nef_parity.rs` is a retained-fixture gate. With
  `TRUESHOT_REAL_NEF` set, it encrypts a real Z9 NEF and requires exact native
  Bayer equality for the same plaintext/encrypted ROI without a plaintext
  sibling.
- Five release runs on the private 47.3 MB fixture measured encrypted parse
  p50/p95 at 0.018/0.019 seconds and encrypted parse+512x512 ROI decode at
  0.555/0.556 seconds. The redacted record is
  `docs/benchmarks/encrypted_nef_parity_2026-07-27.json`.

This is storage and decode-integrity evidence. It is not a claim that encrypted
processing has passed full-sensor throughput gates on every Apple Silicon
generation.
