//! Request bodies: `Content-Encoding` (gzip, deflate, br, zstd) undone
//! with a cap on the decoded size, so a small zip bomb can't take memory.

use std::io::Read;

/// Largest envelope, compressed or decoded.
pub const MAX_ENVELOPE: usize = 20 * 1024 * 1024;
/// Largest event (an `event` item or a `/store/` body), decoded.
pub const MAX_EVENT: usize = 1024 * 1024;
/// Largest attachment, decoded.
pub const MAX_ATTACHMENT: usize = 10 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum BodyError {
    /// Larger than the limit once decoded (413).
    TooLarge,
    /// Not valid for its encoding, or an encoding Kestrel doesn't know (400).
    Invalid(String),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyError::TooLarge => f.write_str("the request body is too large"),
            BodyError::Invalid(why) => write!(f, "can't decode the request body: {why}"),
        }
    }
}

/// Reads `reader` to the end, failing with [`BodyError::TooLarge`] past `limit`.
fn read_capped(reader: impl Read, limit: usize, what: &str) -> Result<Vec<u8>, BodyError> {
    let mut out = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| BodyError::Invalid(format!("{what}: {e}")))?;
    if out.len() > limit {
        return Err(BodyError::TooLarge);
    }
    Ok(out)
}

/// Undoes `encoding` (the `Content-Encoding` header, possibly a list) with at
/// most `limit` decoded bytes. Without a header, a gzip magic number is
/// still recognised.
pub fn decode(encoding: Option<&str>, body: &[u8], limit: usize) -> Result<Vec<u8>, BodyError> {
    let encodings: Vec<String> = encoding
        .unwrap_or("")
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e != "identity")
        .collect();
    if encodings.is_empty() {
        if body.starts_with(&[0x1f, 0x8b]) {
            return decode_one("gzip", body, limit);
        }
        if body.len() > limit {
            return Err(BodyError::TooLarge);
        }
        return Ok(body.to_vec());
    }
    // Encodings are listed in the order they were applied.
    let mut data = body.to_vec();
    for enc in encodings.iter().rev() {
        data = decode_one(enc, &data, limit)?;
    }
    Ok(data)
}

fn decode_one(encoding: &str, body: &[u8], limit: usize) -> Result<Vec<u8>, BodyError> {
    match encoding {
        "gzip" | "x-gzip" => read_capped(flate2::read::MultiGzDecoder::new(body), limit, "gzip"),
        "deflate" => {
            // HTTP "deflate" is zlib-wrapped, but some clients send raw deflate.
            match read_capped(flate2::read::ZlibDecoder::new(body), limit, "deflate") {
                Err(BodyError::Invalid(_)) => {
                    read_capped(flate2::read::DeflateDecoder::new(body), limit, "deflate")
                }
                other => other,
            }
        }
        "br" => read_capped(brotli::Decompressor::new(body, 4096), limit, "br"),
        "zstd" => {
            let decoder = zstd::stream::read::Decoder::new(body)
                .map_err(|e| BodyError::Invalid(format!("zstd: {e}")))?;
            read_capped(decoder, limit, "zstd")
        }
        other => Err(BodyError::Invalid(format!(
            "unsupported content-encoding {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    const DATA: &[u8] = b"{\"message\":\"hello\"}\n";

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn every_encoding_decodes() {
        assert_eq!(decode(None, DATA, 100).unwrap(), DATA);
        assert_eq!(decode(Some("identity"), DATA, 100).unwrap(), DATA);
        assert_eq!(decode(Some("gzip"), &gzip(DATA), 100).unwrap(), DATA);
        assert_eq!(decode(None, &gzip(DATA), 100).unwrap(), DATA, "sniffed");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(DATA).unwrap();
        assert_eq!(
            decode(Some("deflate"), &z.finish().unwrap(), 100).unwrap(),
            DATA
        );
        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(DATA).unwrap();
        assert_eq!(
            decode(Some("deflate"), &raw.finish().unwrap(), 100).unwrap(),
            DATA
        );
        let mut br = Vec::new();
        brotli::CompressorWriter::new(&mut br, 4096, 5, 22)
            .write_all(DATA)
            .unwrap();
        assert_eq!(decode(Some("br"), &br, 100).unwrap(), DATA);
        let zs = zstd::bulk::compress(DATA, 3).unwrap();
        assert_eq!(decode(Some("ZSTD"), &zs, 100).unwrap(), DATA);
        assert_eq!(
            decode(
                Some("gzip, zstd"),
                &zstd::bulk::compress(&gzip(DATA), 3).unwrap(),
                100
            )
            .unwrap(),
            DATA
        );
    }

    #[test]
    fn limits_hold_for_bombs() {
        let bomb = gzip(&vec![0u8; 5 * 1024 * 1024]);
        assert!(bomb.len() < 10 * 1024);
        assert_eq!(
            decode(Some("gzip"), &bomb, 1024 * 1024),
            Err(BodyError::TooLarge)
        );
        assert_eq!(decode(None, &[b'x'; 101], 100), Err(BodyError::TooLarge));
        assert!(matches!(
            decode(Some("gzip"), b"not gzip", 100),
            Err(BodyError::Invalid(_))
        ));
        assert!(matches!(
            decode(Some("compress"), DATA, 100),
            Err(BodyError::Invalid(_))
        ));
    }
}
