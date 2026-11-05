//! Byte-preserving HTTP/1 framing. Entity chunks become scatter/gather spans.
use crate::{Frame, MAX_FRAME};
use anyhow::{bail, ensure};
use std::ops::Range;
fn end_headers(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|x| x == b"\r\n\r\n")
        .map(|p| p + 4)
}
pub(crate) fn frame(
    bytes: &[u8],
    response: bool,
    eof: bool,
    method: Option<&str>,
) -> anyhow::Result<Option<Frame>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let Some(end) = end_headers(bytes) else {
        ensure!(bytes.len() <= 16384, "HTTP header limit");
        return Ok(None);
    };
    ensure!(end <= 16384, "HTTP header limit");
    let header = std::str::from_utf8(&bytes[..end])?;
    let mut lines = header.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let parts: Vec<_> = first.split(' ').collect();
    ensure!(parts.len() >= 3, "invalid HTTP start line");
    let mut regions = Vec::new();
    let mut request_method = None;
    let mut informational = false;
    let mut no_body = false;
    if response {
        ensure!(
            parts[0] == "HTTP/1.1" || parts[0] == "HTTP/1.0",
            "unsupported HTTP response"
        );
        let status = parts[1].parse::<u16>()?;
        ensure!(
            (100..=599).contains(&status) && status != 101,
            "unsupported HTTP response status/upgrade"
        );
        informational = status < 200;
        no_body = informational || status == 204 || status == 304 || method == Some("HEAD");
    } else {
        ensure!(
            parts.len() == 3
                && parts[2] == "HTTP/1.1"
                && parts[0].bytes().all(|b| b.is_ascii_uppercase())
                && parts[1].starts_with('/'),
            "unsupported HTTP request line"
        );
        ensure!(
            parts[0] != "CONNECT",
            "HTTP tunnels cannot rewrite plaintext flags"
        );
        request_method = Some(parts[0].to_owned());
        let start = parts[0].len() + 1;
        regions.push(vec![start..start + parts[1].len()]);
    }
    let mut length = None;
    let mut chunked = false;
    let mut offset = first.len() + 2;
    let mut count = 0;
    for line in lines.filter(|line| !line.is_empty()) {
        count += 1;
        ensure!(count <= 128, "HTTP header count limit");
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid HTTP header"))?;
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                && value.bytes().all(|b| b >= 32 || b == 9),
            "invalid HTTP header"
        );
        let key = name.to_ascii_lowercase();
        let normalized = value.trim();
        match key.as_str() {
            "content-length" => {
                ensure!(
                    length.is_none()
                        && !normalized.is_empty()
                        && normalized.bytes().all(|b| b.is_ascii_digit()),
                    "ambiguous HTTP Content-Length"
                );
                length = Some(normalized.parse::<usize>()?);
            }
            "transfer-encoding" => {
                ensure!(
                    !chunked && normalized.eq_ignore_ascii_case("chunked"),
                    "unsupported HTTP transfer coding"
                );
                chunked = true;
            }
            "content-encoding" => ensure!(
                normalized.eq_ignore_ascii_case("identity"),
                "encoded HTTP bodies require a rewrite adapter"
            ),
            "upgrade" | "expect" | "content-md5" | "digest" | "signature" => {
                bail!("HTTP upgrade, Expect and integrity headers require a rewrite adapter")
            }
            "connection" => ensure!(
                !normalized.to_ascii_lowercase().contains("upgrade"),
                "HTTP upgrades require a rewrite adapter"
            ),
            _ => regions.push(vec![offset + name.len() + 1..offset + line.len()]),
        }
        offset += line.len() + 2;
    }
    ensure!(!(chunked && length.is_some()), "ambiguous HTTP framing");
    let total = if no_body {
        end
    } else if chunked {
        let Some((total, spans, trailers)) = chunks(bytes, end)? else {
            return Ok(None);
        };
        regions.push(spans);
        regions.extend(trailers);
        total
    } else if let Some(length) = length {
        let total = end
            .checked_add(length)
            .ok_or_else(|| anyhow::anyhow!("HTTP size overflow"))?;
        ensure!(total <= MAX_FRAME, "HTTP message exceeds64KiB");
        if bytes.len() < total {
            return Ok(None);
        };
        regions.push(vec![end..total]);
        total
    } else if response {
        if !eof {
            return Ok(None);
        };
        regions.push(vec![end..bytes.len()]);
        bytes.len()
    } else {
        end
    };
    ensure!(total <= MAX_FRAME, "HTTP message exceeds64KiB");
    Ok(Some(Frame {
        wire: bytes[..total].to_vec(),
        regions,
        method: request_method,
        informational,
    }))
}
type Chunks = (usize, Vec<Range<usize>>, Vec<Vec<Range<usize>>>);
fn chunks(bytes: &[u8], mut at: usize) -> anyhow::Result<Option<Chunks>> {
    let mut spans = Vec::new();
    let mut trailers = Vec::new();
    loop {
        let Some(n) = bytes[at..].windows(2).position(|x| x == b"\r\n") else {
            return Ok(None);
        };
        ensure!(n <= 1024, "HTTP chunk size line limit");
        let line = std::str::from_utf8(&bytes[at..at + n])?;
        let size = line.split(';').next().unwrap_or_default();
        ensure!(
            !size.is_empty() && size.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid HTTP chunk size"
        );
        let size = usize::from_str_radix(size, 16)?;
        at += n + 2;
        if size == 0 {
            loop {
                let Some(n) = bytes[at..].windows(2).position(|x| x == b"\r\n") else {
                    return Ok(None);
                };
                if n == 0 {
                    return Ok(Some((at + 2, spans, trailers)));
                }
                ensure!(trailers.len() < 128, "HTTP trailer count limit");
                let line = std::str::from_utf8(&bytes[at..at + n])?;
                let (name, value) = line
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("invalid HTTP trailer"))?;
                ensure!(
                    !name.is_empty() && value.bytes().all(|b| b >= 32 || b == 9),
                    "invalid HTTP trailer"
                );
                ensure!(
                    ![
                        "content-length",
                        "transfer-encoding",
                        "content-encoding",
                        "digest",
                        "signature",
                        "content-md5"
                    ]
                    .contains(&name.to_ascii_lowercase().as_str()),
                    "unsupported HTTP trailer"
                );
                trailers.push(vec![at + name.len() + 1..at + n]);
                at += n + 2;
            }
        }
        ensure!(
            size <= MAX_FRAME && at.checked_add(size + 2).is_some_and(|end| end <= MAX_FRAME),
            "HTTP chunk exceeds64KiB"
        );
        if bytes.len() < at + size + 2 {
            return Ok(None);
        };
        ensure!(
            &bytes[at + size..at + size + 2] == b"\r\n",
            "invalid HTTP chunk terminator"
        );
        spans.push(at..at + size);
        at += size + 2;
    }
}
