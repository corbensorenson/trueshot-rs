use std::path::PathBuf;
use trueshot_core::nef::parser::Z9NefParser;
use trueshot_core::nef::raw_data::Roi;

#[test]
#[ignore = "requires TRUESHOT_REAL_NEF pointing to a retained Nikon Z9 NEF"]
fn authenticated_tse2_roi_matches_plaintext_nef_exactly() {
    let total_started = std::time::Instant::now();
    let source = PathBuf::from(
        std::env::var_os("TRUESHOT_REAL_NEF")
            .expect("TRUESHOT_REAL_NEF must identify a retained Z9 NEF"),
    );
    let directory = tempfile::tempdir().unwrap();
    let encrypted = directory.path().join("qualification.NEF.enc");
    let key = [0xabu8; 32];
    let encryption_started = std::time::Instant::now();
    let encryption = trueshot_storage::encrypted::encrypt_file(
        &source,
        &encrypted,
        &key,
        trueshot_storage::encrypted::DEFAULT_CHUNK_SIZE,
    )
    .unwrap();
    let encryption_seconds = encryption_started.elapsed().as_secs_f64();

    let plaintext_started = std::time::Instant::now();
    let mut plain = Z9NefParser::new(&source);
    plain.parse().unwrap();
    let plaintext_parse_seconds = plaintext_started.elapsed().as_secs_f64();
    let metadata = plain.get_metadata().unwrap().clone();
    let width = metadata.width.min(512);
    let height = metadata.height.min(512);
    let roi = Roi::new(
        (metadata.width - width) / 2,
        (metadata.height - height) / 2,
        width,
        height,
    );
    let plaintext_decode_started = std::time::Instant::now();
    let expected = plain.load_roi(&roi, None).unwrap();
    let plaintext_decode_seconds = plaintext_decode_started.elapsed().as_secs_f64();
    let plaintext_seconds = plaintext_started.elapsed().as_secs_f64();

    let encrypted_started = std::time::Instant::now();
    let mut protected = Z9NefParser::new_encrypted(&encrypted, key);
    protected.parse().unwrap();
    let encrypted_parse_seconds = encrypted_started.elapsed().as_secs_f64();
    let encrypted_decode_started = std::time::Instant::now();
    let observed = protected.load_roi(&roi, None).unwrap();
    let encrypted_decode_seconds = encrypted_decode_started.elapsed().as_secs_f64();
    let encrypted_seconds = encrypted_started.elapsed().as_secs_f64();

    assert_eq!(protected.get_metadata().unwrap().width, metadata.width);
    assert_eq!(protected.get_metadata().unwrap().height, metadata.height);
    assert_eq!(observed.data, expected.data);
    assert!(!directory.path().join("qualification.NEF").exists());
    assert_eq!(
        std::fs::read_dir(directory.path()).unwrap().count(),
        1,
        "qualification must not stage indexes or plaintext beside the encrypted NEF"
    );
    eprintln!(
        "TSE2 real-NEF parity: source={} encrypted={} chunks={} encrypt={:.3}s plaintext_parse={:.3}s plaintext_decode={:.3}s plaintext_total={:.3}s encrypted_parse={:.3}s encrypted_decode={:.3}s encrypted_total={:.3}s total={:.3}s",
        encryption.plaintext_bytes,
        encryption.encrypted_bytes,
        encryption.chunks,
        encryption_seconds,
        plaintext_parse_seconds,
        plaintext_decode_seconds,
        plaintext_seconds,
        encrypted_parse_seconds,
        encrypted_decode_seconds,
        encrypted_seconds,
        total_started.elapsed().as_secs_f64(),
    );
}
