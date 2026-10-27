//! Request bodies: `Content-Encoding` (gzip, deflate, br, zstd) undone
//! with a cap on the decoded size, so a small zip bomb can't take memory.
//! Every decoder is read through [`read_capped`], which stops one byte past
//! the limit, so a bomb never grows beyond it; the zstd decoder also refuses
//! frames that ask for a window over [`ZSTD_WINDOW_LOG_MAX`].

use std::borrow::Cow;
use std::io::Read;

/// Largest envelope, compressed or decoded.
pub const MAX_ENVELOPE: usize = 20 * 1024 * 1024;
/// Largest event (an `event` item or a `/store/` body), decoded.
pub const MAX_EVENT: usize = 1024 * 1024;
/// Largest attachment kept, decoded (larger ones are accepted and dropped).
pub const MAX_ATTACHMENT: usize = 1024 * 1024;
/// Encodings in a `Content-Encoding` list at most.
const MAX_ENCODINGS: usize = 2;
/// The largest window a zstd frame may ask the decoder to allocate, as a
/// power of two (8 MiB). libzstd's own limit is 128 MiB, which a few bytes
/// of header can demand before any output; SDKs compress at levels whose
/// window is 8 MiB or less.
pub const ZSTD_WINDOW_LOG_MAX: u32 = 23;

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
    decode_cow(encoding, body, limit).map(Cow::into_owned)
}

/// [`decode`] without copying a body that has no encoding.
pub fn decode_cow<'a>(
    encoding: Option<&str>,
    body: &'a [u8],
    limit: usize,
) -> Result<Cow<'a, [u8]>, BodyError> {
    let encodings: Vec<String> = encoding
        .unwrap_or("")
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e != "identity")
        .collect();
    if encodings.is_empty() {
        if body.starts_with(&[0x1f, 0x8b]) {
            return decode_one("gzip", body, limit).map(Cow::Owned);
        }
        if body.len() > limit {
            return Err(BodyError::TooLarge);
        }
        return Ok(Cow::Borrowed(body));
    }
    if encodings.len() > MAX_ENCODINGS {
        return Err(BodyError::Invalid(format!(
            "more than {MAX_ENCODINGS} content-encodings"
        )));
    }
    // Encodings are listed in the order they were applied.
    let mut data = Cow::Borrowed(body);
    for enc in encodings.iter().rev() {
        data = Cow::Owned(decode_one(enc, &data, limit)?);
    }
    Ok(data)
}

/// The first `max` decoded bytes of `body`, which may be only the start of a
/// longer one (ingest reads an envelope's header from it before it reads the
/// rest). Data that stops short or turns bad after some output still gives
/// that output; the whole body is checked when it is decoded for real. Only
/// one encoding is understood here.
pub fn decode_prefix(
    encoding: Option<&str>,
    body: &[u8],
    max: usize,
) -> Result<Vec<u8>, BodyError> {
    fn prefix(reader: impl Read, max: usize) -> (Vec<u8>, Option<std::io::Error>) {
        let mut out = Vec::new();
        let err = reader.take(max as u64).read_to_end(&mut out).err();
        (out, err)
    }
    let encodings: Vec<String> = encoding
        .unwrap_or("")
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e != "identity")
        .collect();
    let encoding = match encodings.as_slice() {
        [] if body.starts_with(&[0x1f, 0x8b]) => "gzip",
        [] => return Ok(body[..body.len().min(max)].to_vec()),
        [one] => one.as_str(),
        _ => {
            return Err(BodyError::Invalid(
                "send the key in X-Sentry-Auth when the body has two content-encodings".into(),
            ));
        }
    };
    let (out, err) = match encoding {
        "gzip" | "x-gzip" => prefix(flate2::read::MultiGzDecoder::new(body), max),
        "deflate" => match prefix(flate2::read::ZlibDecoder::new(body), max) {
            (out, Some(_)) if out.is_empty() => {
                prefix(flate2::read::DeflateDecoder::new(body), max)
            }
            other => other,
        },
        "br" => prefix(brotli::Decompressor::new(body, 4096), max),
        "zstd" => {
            let mut decoder = zstd::stream::read::Decoder::new(body)
                .map_err(|e| BodyError::Invalid(format!("zstd: {e}")))?;
            decoder
                .window_log_max(ZSTD_WINDOW_LOG_MAX)
                .map_err(|e| BodyError::Invalid(format!("zstd: {e}")))?;
            prefix(decoder, max)
        }
        other => {
            return Err(BodyError::Invalid(format!(
                "unsupported content-encoding {other}"
            )));
        }
    };
    match err {
        Some(e) if out.is_empty() => Err(BodyError::Invalid(format!("{encoding}: {e}"))),
        _ => Ok(out),
    }
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
            let mut decoder = zstd::stream::read::Decoder::new(body)
                .map_err(|e| BodyError::Invalid(format!("zstd: {e}")))?;
            decoder
                .window_log_max(ZSTD_WINDOW_LOG_MAX)
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

    #[test]
    fn zstd_frames_can_not_demand_a_huge_window() {
        let frame = |window_log: u32| {
            let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
            enc.window_log(window_log).unwrap();
            enc.write_all(DATA).unwrap();
            enc.finish().unwrap()
        };
        assert_eq!(
            decode(Some("zstd"), &frame(ZSTD_WINDOW_LOG_MAX), 100).unwrap(),
            DATA
        );
        // Declared in the header: refused before any output is made.
        assert!(matches!(
            decode(Some("zstd"), &frame(27), 100),
            Err(BodyError::Invalid(_))
        ));
    }

    #[test]
    fn every_decoder_stops_at_the_limit_while_reading() {
        let data = vec![0u8; 3 * 1024 * 1024];
        let limit = 64 * 1024;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&data).unwrap();
        let mut raw = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        raw.write_all(&data).unwrap();
        let mut br = Vec::new();
        brotli::CompressorWriter::new(&mut br, 4096, 5, 22)
            .write_all(&data)
            .unwrap();
        for (name, packed) in [
            ("gzip", gzip(&data)),
            ("deflate", z.finish().unwrap()),
            ("deflate", raw.finish().unwrap()),
            ("br", br),
            ("zstd", zstd::bulk::compress(&data, 3).unwrap()),
        ] {
            assert!(packed.len() < 16 * 1024, "{name} packs the bomb small");
            assert_eq!(
                decode(Some(name), &packed, limit),
                Err(BodyError::TooLarge),
                "{name}"
            );
        }
    }

    #[test]
    fn long_encoding_lists_are_refused() {
        let twice = gzip(&gzip(DATA));
        assert_eq!(decode(Some("gzip, gzip"), &twice, 100).unwrap(), DATA);
        assert!(matches!(
            decode(Some("gzip, gzip, gzip"), &gzip(&twice), 100),
            Err(BodyError::Invalid(_))
        ));
        assert!(matches!(decode_cow(None, DATA, 100), Ok(Cow::Borrowed(_))));
    }

    #[test]
    fn a_prefix_decodes_from_a_body_cut_short() {
        let data: Vec<u8> = (0..20_000)
            .flat_map(|i| format!("{{\"dsn\":\"http://k@h/1\",\"n\":{i}}}\n").into_bytes())
            .collect();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(&data).unwrap();
        let mut raw = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        raw.write_all(&data).unwrap();
        let mut br = Vec::new();
        brotli::CompressorWriter::new(&mut br, 4096, 5, 22)
            .write_all(&data)
            .unwrap();
        for (name, packed) in [
            ("gzip", gzip(&data)),
            ("deflate", z.finish().unwrap()),
            ("deflate", raw.finish().unwrap()),
            ("br", br),
            ("zstd", zstd::bulk::compress(&data, 3).unwrap()),
        ] {
            // Half of the compressed body is enough for the first 100 bytes.
            let cut = &packed[..packed.len() / 2];
            assert_eq!(
                decode_prefix(Some(name), cut, 100).unwrap(),
                &data[..100],
                "{name}"
            );
        }
        assert_eq!(decode_prefix(None, DATA, 5).unwrap(), &DATA[..5]);
        assert_eq!(
            decode_prefix(None, &gzip(DATA), 100).unwrap(),
            DATA,
            "sniffed"
        );
        assert!(matches!(
            decode_prefix(Some("gzip"), b"not gzip", 100),
            Err(BodyError::Invalid(_))
        ));
        assert!(matches!(
            decode_prefix(Some("compress"), DATA, 100),
            Err(BodyError::Invalid(_))
        ));
        assert!(matches!(
            decode_prefix(Some("gzip, zstd"), DATA, 100),
            Err(BodyError::Invalid(_))
        ));
    }
}
