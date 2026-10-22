//! Sentry envelopes: `header \n (item-header \n payload \n)*`
//! (<https://develop.sentry.dev/sdk/data-model/envelopes/>). An item's
//! payload is `length` bytes when the header says so, else the rest of the
//! line.

use serde_json::{Map, Value};

#[derive(Debug)]
pub struct Item {
    pub header: Map<String, Value>,
    pub payload: Vec<u8>,
}

impl Item {
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
pub struct Envelope {
    pub header: Map<String, Value>,
    pub items: Vec<Item>,
}

impl Envelope {
    pub fn header_str(&self, key: &str) -> Option<&str> {
        self.header.get(key).and_then(Value::as_str)
    }
}

/// Why an envelope can't be read.
#[derive(Debug, PartialEq, Eq)]
pub struct EnvelopeError(pub String);

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid envelope: {}", self.0)
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
        Ok(_) => Err(EnvelopeError(format!("{what} is not a JSON object"))),
        Err(e) => Err(EnvelopeError(format!("{what}: {e}"))),
    }
}

/// Parses an envelope. Blank lines between items are tolerated.
pub fn parse(data: &[u8]) -> Result<Envelope, EnvelopeError> {
    let (header_line, mut rest) = split_line(data);
    let header = json_header(header_line, "envelope header")?;
    let mut items = Vec::new();
    loop {
        while let Some((b'\n' | b'\r', r)) = rest.split_first() {
            rest = r;
        }
        if rest.is_empty() {
            break;
        }
        let (item_line, after) = split_line(rest);
        let item_header = json_header(item_line, "item header")?;
        let payload = match item_header.get("length") {
            Some(len) => {
                let len = len
                    .as_u64()
                    .and_then(|l| usize::try_from(l).ok())
                    .ok_or_else(|| EnvelopeError("item length is not a number".into()))?;
                if after.len() < len {
                    return Err(EnvelopeError(format!(
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
        items.push(Item {
            header: item_header,
            payload: payload.to_vec(),
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
        assert_eq!(env.items[0].payload, b"\xef\xbb\xbfHello\r\n");
        assert_eq!(
            env.items[2].payload,
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
}
