//! Bounded codecs for persisted Hydra history.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result, ensure};

pub const MIN_LEVEL: i32 = 1;
pub const MAX_LEVEL: i32 = 19;
pub const DEFAULT_LEVEL: i32 = 1;
pub fn default_level() -> i32 {
    DEFAULT_LEVEL
}
static LEVEL: AtomicI32 = AtomicI32::new(DEFAULT_LEVEL);
const WINDOW_LOG: u32 = 23;

/// Current process-local compression level, applied to subsequent writes.
pub fn level() -> i32 {
    LEVEL.load(Ordering::Relaxed)
}

/// Change the compression level after validating the supported range.
pub fn set_level(value: i32) -> Result<()> {
    ensure!(
        (MIN_LEVEL..=MAX_LEVEL).contains(&value),
        "compression level must be 1..=19"
    );
    LEVEL.store(value, Ordering::Relaxed);
    Ok(())
}

/// Write one checksummed zstd frame using the current level and one thread.
/// Callers own bounds on input and compressed storage size.
pub fn encode(bytes: &[u8]) -> Result<Vec<u8>> {
    encode_at_level(bytes, level())
}

pub fn encode_at_level(bytes: &[u8], level: i32) -> Result<Vec<u8>> {
    ensure!(
        (MIN_LEVEL..=MAX_LEVEL).contains(&level),
        "compression level must be 1..=19"
    );
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), level)?;
    encoder.window_log(WINDOW_LOG)?;
    encoder.include_checksum(true)?;
    encoder.write_all(bytes)?;
    encoder.finish().context("finish zstd history")
}

/// Decode zstd or legacy gzip, validating checksums and completion.
/// The decoded output is bounded by the caller and the zstd window by 8 MiB.
/// Concatenated frames/members share the same output bound.
pub fn decode(bytes: &[u8], max_decoded_bytes: usize) -> Result<Vec<u8>> {
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        let mut decoder = zstd::stream::read::Decoder::with_buffer(bytes)?;
        decoder.window_log_max(WINDOW_LOG)?;
        read_bounded(decoder, max_decoded_bytes)
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        read_bounded(flate2::read::MultiGzDecoder::new(bytes), max_decoded_bytes)
    } else {
        anyhow::bail!("unknown compressed history format")
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        // Read one byte beyond the remaining allowance to detect overflow,
        // including when the allowance is zero; never grow the output beyond it.
        let available = limit.saturating_sub(decoded.len());
        let read_limit = available.saturating_add(1).min(buffer.len());
        let count = reader
            .read(&mut buffer[..read_limit])
            .context("decode history")?;
        if count == 0 {
            return Ok(decoded);
        }
        ensure!(count <= available, "decoded history exceeds {limit} bytes");
        decoded.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn new_history_is_zstd_with_checksum_and_roundtrips_binary_bytes() {
        let encoded = encode(b"hello\0\xff\r\n").unwrap();
        assert_eq!(&encoded[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
        assert_ne!(encoded[4] & 4, 0, "content checksum must be enabled");
        assert_eq!(decode(&encoded, 100).unwrap(), b"hello\0\xff\r\n");
    }

    #[test]
    fn legacy_gzip_remains_readable() {
        assert_eq!(
            decode(&gzip(b"legacy\0data"), 100).unwrap(),
            b"legacy\0data"
        );
    }

    #[test]
    fn decoded_size_bound_includes_zero_and_exact_limit() {
        for encoded in [gzip(b"hello"), encode(b"hello").unwrap()] {
            assert!(decode(&encoded, 0).is_err());
            assert!(decode(&encoded, 4).is_err());
            assert_eq!(decode(&encoded, 5).unwrap(), b"hello");
        }
        for encoded in [gzip(b""), encode(b"").unwrap()] {
            assert_eq!(decode(&encoded, 0).unwrap(), b"");
        }
    }

    #[test]
    fn rejects_unknown_corrupt_truncated_and_trailing_garbage() {
        for bytes in [
            b"".as_slice(),
            b"garbage",
            &[0x28, 0xb5, 0x2f],
            &[0x1f, 0x8b],
        ] {
            assert!(decode(bytes, 100).is_err());
        }
        for original in [gzip(b"valid history"), encode(b"valid history").unwrap()] {
            for end in 0..original.len() {
                assert!(
                    decode(&original[..end], 100).is_err(),
                    "accepted truncated prefix {end}"
                );
            }
            let mut corrupt = original.clone();
            *corrupt.last_mut().unwrap() ^= 1;
            assert!(decode(&corrupt, 100).is_err());
            let mut trailing = original;
            trailing.extend_from_slice(b"garbage");
            assert!(decode(&trailing, 100).is_err());
        }
    }

    #[test]
    fn concatenated_frames_share_one_decoded_limit() {
        for mut encoded in [gzip(b"abc"), encode(b"abc").unwrap()] {
            let second = encoded.clone();
            encoded.extend_from_slice(&second);
            assert_eq!(decode(&encoded, 6).unwrap(), b"abcabc");
            assert!(decode(&encoded, 5).is_err());
        }
    }

    #[test]
    fn configured_levels_remain_readable_with_bounded_window() {
        let bytes = vec![b'x'; 64 * 1024];
        for value in [1, 9, 19] {
            let encoded = encode_at_level(&bytes, value).unwrap();
            assert_eq!(decode(&encoded, bytes.len()).unwrap(), bytes);
        }
    }

    #[test]
    fn invalid_level_does_not_change_current_level() {
        let original = level();
        for value in [i32::MIN, 0, 20, i32::MAX] {
            assert!(set_level(value).is_err());
            assert_eq!(level(), original);
        }
        set_level(19).unwrap();
        assert_eq!(level(), 19);
        set_level(original).unwrap();
    }

    #[test]
    fn rejects_window_larger_than_eight_mib() {
        // Standard frame, no content-size field, 16 MiB window, final empty raw block.
        let frame = [0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x70, 0x01, 0x00, 0x00];
        assert_eq!(zstd::stream::decode_all(frame.as_slice()).unwrap(), b"");
        assert!(decode(&frame, 100).is_err());
    }
}
