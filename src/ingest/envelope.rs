//! Sentry envelopes: `header \n (item-header \n payload \n)*`
//! (<https://develop.sentry.dev/sdk/data-model/envelopes/>). An item's
//! payload is `length` bytes when the header says so, else the rest of the
//! line.
//!
//! Envelopes come from anonymous clients, so reading one is bounded by its
//! structure as well as its size: [`Scanner`] hands out one item at a time
//! (payloads are slices of the decoded body, nothing is copied) and stops at
//! the first violation of [`MAX_ITEMS`], [`MAX_EVENT_ITEMS`] or the header
//! sizes. The caller decides per item whether to look at the payload at all.

use std::borrow::Cow;

use serde_json::{Map, Value};

/// Items in one envelope at most (SDKs send a handful).
pub const MAX_ITEMS: usize = 100;
/// `event` and `transaction` items in one envelope at most (SDKs send one).
pub const MAX_EVENT_ITEMS: usize = 20;
/// An item header's size at most, in bytes.
pub const MAX_ITEM_HEADER: usize = 16 * 1024;
/// The envelope header's size at most, in bytes (the DSN, ids, the SDK and
/// the trace's sampling context).
pub const MAX_HEADER: usize = 64 * 1024;

#[derive(Debug)]
pub struct Item<'a> {
    pub header: Map<String, Value>,
    pub payload: Cow<'a, [u8]>,
}

impl Item<'_> {
    /// The item type (`event`, `session`, `attachment`, …).
    pub fn ty(&self) -> &str {
        self.header
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    pub fn header_str(&self, key: &str) -> Option<&str> {
        self.header.get(key).and_then(Value::as_str)
    }
}

#[derive(Debug)]
pub struct Envelope<'a> {
    pub header: Map<String, Value>,
    pub items: Vec<Item<'a>>,
}

impl Envelope<'_> {
    pub fn header_str(&self, key: &str) -> Option<&str> {
        self.header.get(key).and_then(Value::as_str)
    }
}

/// Why an envelope can't be read.
#[derive(Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// Not an envelope (400).
    Invalid(String),
    /// More items or bigger headers than ingest reads (413).
    Limit(&'static str),
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvelopeError::Invalid(why) => write!(f, "invalid envelope: {why}"),
            EnvelopeError::Limit(why) => f.write_str(why),
        }
    }
}

fn split_line(data: &[u8]) -> (&[u8], &[u8]) {
    match data.iter().position(|&b| b == b'\n') {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[]),
    }
}

fn json_header(line: &[u8], what: &str) -> Result<Map<String, Value>, EnvelopeError> {
    match serde_json::from_slice(line) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(EnvelopeError::Invalid(format!(
            "{what} is not a JSON object"
        ))),
        Err(e) => Err(EnvelopeError::Invalid(format!("{what}: {e}"))),
    }
}

/// The envelope header alone: enough to find the DSN key before anything
/// else is read. Returns the header and the data after it.
pub fn header(data: &[u8]) -> Result<(Map<String, Value>, &[u8]), EnvelopeError> {
    let (line, rest) = split_line(data);
    if line.len() > MAX_HEADER {
        return Err(EnvelopeError::Limit("the envelope header is too large"));
    }
    Ok((json_header(line, "envelope header")?, rest))
}

/// Reads the items of an envelope one by one, in order.
#[derive(Debug)]
pub struct Scanner<'a> {
    rest: &'a [u8],
    items: usize,
    events: usize,
}

impl<'a> Scanner<'a> {
    /// Reads the items that follow the envelope header (the second half of
    /// [`header`]'s answer).
    pub fn new(rest: &'a [u8]) -> Self {
        Scanner {
            rest,
            items: 0,
            events: 0,
        }
    }

    /// Items handed out so far.
    pub fn count(&self) -> usize {
        self.items
    }

    /// The next item, `None` at the end. Blank lines between items are
    /// tolerated. An error ends the envelope, and so does a limit.
    pub fn next_item(&mut self) -> Result<Option<Item<'a>>, EnvelopeError> {
        let mut rest = self.rest;
        while let Some((b'\n' | b'\r', r)) = rest.split_first() {
            rest = r;
        }
        if rest.is_empty() {
            self.rest = rest;
            return Ok(None);
        }
        if self.items >= MAX_ITEMS {
            return Err(EnvelopeError::Limit("too many items in the envelope"));
        }
        let (item_line, after) = split_line(rest);
        if item_line.len() > MAX_ITEM_HEADER {
            return Err(EnvelopeError::Limit("an item header is too large"));
        }
        let item_header = json_header(item_line, "item header")?;
        if matches!(
            item_header.get("type").and_then(Value::as_str),
            Some("event" | "transaction")
        ) {
            if self.events >= MAX_EVENT_ITEMS {
                return Err(EnvelopeError::Limit("too many events in the envelope"));
            }
            self.events += 1;
        }
        let payload = match item_header.get("length") {
            Some(len) => {
                let len = len
                    .as_u64()
                    .and_then(|l| usize::try_from(l).ok())
                    .ok_or_else(|| EnvelopeError::Invalid("item length is not a number".into()))?;
                if after.len() < len {
                    return Err(EnvelopeError::Invalid(format!(
                        "item length {len} exceeds the {} bytes left",
                        after.len()
                    )));
                }
                rest = &after[len..];
                if let Some((b'\n', r)) = rest.split_first() {
                    rest = r;
                }
                &after[..len]
            }
            None => {
                let (p, r) = split_line(after);
                rest = r;
                p.strip_suffix(b"\r").unwrap_or(p)
            }
        };
        self.rest = rest;
        self.items += 1;
        Ok(Some(Item {
            header: item_header,
            payload: Cow::Borrowed(payload),
        }))
    }
}

/// Parses a whole envelope into owned items (tests and tools; ingest reads
/// it item by item with [`Scanner`]).
pub fn parse(data: &[u8]) -> Result<Envelope<'static>, EnvelopeError> {
    let (header, rest) = header(data)?;
    let mut scanner = Scanner::new(rest);
    let mut items = Vec::new();
    while let Some(item) = scanner.next_item()? {
        items.push(Item {
            header: item.header,
            payload: Cow::Owned(item.payload.into_owned()),
        });
    }
    Ok(Envelope { header, items })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENVELOPE: &[u8] = b"{\"event_id\":\"9ec79c33ec9942ab8353589fcb2e04dc\",\"dsn\":\"http://k@127.0.0.1:9000/1\"}\n\
{\"type\":\"attachment\",\"length\":10,\"filename\":\"a.txt\"}\n\
\xef\xbb\xbfHello\r\n\n\
{\"type\":\"event\",\"length\":41}\n\
{\"message\":\"hello\",\"level\":\"error\",\"x\":1}\n\
{\"type\":\"session\"}\n\
{\"started\":\"2020-02-07T14:16:00Z\"}\n";

    #[test]
    fn parses_items_with_and_without_length() {
        let env = parse(ENVELOPE).unwrap();
        assert_eq!(
            env.header_str("event_id"),
            Some("9ec79c33ec9942ab8353589fcb2e04dc")
        );
        let types: Vec<&str> = env.items.iter().map(Item::ty).collect();
        assert_eq!(types, ["attachment", "event", "session"]);
        assert_eq!(&*env.items[0].payload, b"\xef\xbb\xbfHello\r\n");
        assert_eq!(
            &*env.items[2].payload,
            b"{\"started\":\"2020-02-07T14:16:00Z\"}"
        );
        assert!(parse(b"{}").unwrap().items.is_empty());
    }

    #[test]
    fn refuses_broken_envelopes() {
        assert!(parse(b"{}\n{\"type\":\"event\",\"length\":99}\n{}").is_err());
        assert!(parse(b"not json\n").is_err());
        assert!(parse(b"[]\n").is_err());
        assert!(parse(b"{}\n{\"type\":\"event\",\"length\":-1}\n{}").is_err());
    }

    #[test]
    fn the_scanner_borrows_and_the_header_comes_first() {
        let (header, rest) = header(ENVELOPE).unwrap();
        assert!(header.contains_key("dsn"));
        let mut scanner = Scanner::new(rest);
        let first = scanner.next_item().unwrap().unwrap();
        assert!(matches!(first.payload, Cow::Borrowed(_)), "no copy");
        assert_eq!(first.ty(), "attachment");
        while scanner.next_item().unwrap().is_some() {}
        assert_eq!(scanner.count(), 3);
    }

    #[test]
    fn items_events_and_headers_are_limited() {
        // 100 items are fine, 101 stop at the 101st.
        let tiny = "{\"type\":\"x\"}\n\n";
        let ok = format!("{{}}\n{}", tiny.repeat(MAX_ITEMS));
        assert_eq!(parse(ok.as_bytes()).unwrap().items.len(), MAX_ITEMS);
        let too_many = format!("{{}}\n{}", tiny.repeat(MAX_ITEMS + 1));
        assert!(matches!(
            parse(too_many.as_bytes()),
            Err(EnvelopeError::Limit(_))
        ));
        // 20 events and transactions together are fine, the 21st is not.
        let mixed = |n: usize| {
            let mut s = String::from("{}\n");
            for i in 0..n {
                let ty = if i % 2 == 0 { "event" } else { "transaction" };
                s.push_str(&format!("{{\"type\":\"{ty}\"}}\n{{}}\n"));
            }
            s
        };
        assert_eq!(
            parse(mixed(MAX_EVENT_ITEMS).as_bytes())
                .unwrap()
                .items
                .len(),
            MAX_EVENT_ITEMS
        );
        assert!(matches!(
            parse(mixed(MAX_EVENT_ITEMS + 1).as_bytes()),
            Err(EnvelopeError::Limit(_))
        ));
        // Other items don't count as events.
        let sessions = format!("{{}}\n{}", "{\"type\":\"session\"}\n{}\n".repeat(50));
        assert_eq!(parse(sessions.as_bytes()).unwrap().items.len(), 50);
        // Header sizes.
        let big_item = format!(
            "{{}}\n{{\"type\":\"x\",\"pad\":\"{}\"}}\n\n",
            "a".repeat(MAX_ITEM_HEADER)
        );
        assert!(matches!(
            parse(big_item.as_bytes()),
            Err(EnvelopeError::Limit(_))
        ));
        let big_header = format!("{{\"pad\":\"{}\"}}\n", "a".repeat(MAX_HEADER));
        assert!(matches!(
            parse(big_header.as_bytes()),
            Err(EnvelopeError::Limit(_))
        ));
    }

    #[test]
    fn a_limit_stops_the_scan_without_reading_further() {
        // Two million tiny items would cost a pass each; the scanner stops
        // at the 101st header and never looks at the rest.
        let body = format!("{{}}\n{}", "{}\n\n".repeat(2_000_000));
        let (_, rest) = header(body.as_bytes()).unwrap();
        let mut scanner = Scanner::new(rest);
        let mut read = 0;
        let err = loop {
            match scanner.next_item() {
                Ok(Some(_)) => read += 1,
                Ok(None) => panic!("ended"),
                Err(e) => break e,
            }
        };
        assert_eq!(read, MAX_ITEMS);
        assert!(matches!(err, EnvelopeError::Limit(_)));
    }
}
