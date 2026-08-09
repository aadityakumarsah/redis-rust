use std::collections::{HashMap, VecDeque};
use std::io::Write;

use crate::store::{Data, Entry};

const MAGIC: &[u8] = b"RS1\n";

/// Line-based, length-prefixed snapshot format:
///
/// ```text
/// RS1
/// <key_count>
/// S|H|L
/// <name_len>
/// <name bytes>
/// <expire_ms | -1>
/// <payload...>
/// ```
///
/// String:   `<val_len>\n<val bytes>`
/// Hash:     `<field_count>\n` then per field: `<flen>\n<f bytes>\n<vlen>\n<v bytes>`
/// List:     `<item_count>\n` then per item: `<ilen>\n<i bytes>`
///
/// Values are length-prefixed, so they may contain arbitrary bytes including
/// newlines.
pub fn serialize(entries: &HashMap<String, Entry>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    write_line(&mut out, entries.len());
    for (key, entry) in entries {
        match &entry.data {
            Data::String(value) => {
                out.extend_from_slice(b"S\n");
                write_bytes(&mut out, key);
                write_expire(&mut out, entry);
                write_bytes(&mut out, value);
            }
            Data::Hash(hash) => {
                out.extend_from_slice(b"H\n");
                write_bytes(&mut out, key);
                write_expire(&mut out, entry);
                write_line(&mut out, hash.len());
                for (field, value) in hash {
                    write_bytes(&mut out, field);
                    write_bytes(&mut out, value);
                }
            }
            Data::List(list) => {
                out.extend_from_slice(b"L\n");
                write_bytes(&mut out, key);
                write_expire(&mut out, entry);
                write_line(&mut out, list.len());
                for item in list {
                    write_bytes(&mut out, item);
                }
            }
        }
    }
    out
}

fn write_line(out: &mut Vec<u8>, n: usize) {
    let _ = writeln!(out, "{n}");
}

fn write_bytes(out: &mut Vec<u8>, value: &str) {
    let _ = writeln!(out, "{}", value.len());
    out.extend_from_slice(value.as_bytes());
}

fn write_expire(out: &mut Vec<u8>, entry: &Entry) {
    match entry.expires_at {
        Some(ms) => {
            let _ = writeln!(out, "{ms}");
        }
        None => {
            let _ = writeln!(out, "-1");
        }
    }
}

pub fn deserialize(bytes: &[u8]) -> Result<HashMap<String, Entry>, String> {
    if !bytes.starts_with(MAGIC) {
        return Err("bad magic header".to_string());
    }
    let mut cur = Cursor {
        bytes,
        pos: MAGIC.len(),
    };

    let count = cur.line_num()? as usize;
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let ty = cur.line_str()?;
        let key = cur.bytes_line()?;
        let expires_at = match cur.line_num()? {
            n if n < 0 => None,
            n => Some(n as u128),
        };
        let data = match ty {
            "S" => {
                let value = cur.bytes_line()?;
                Data::String(value)
            }
            "H" => {
                let fields = cur.line_num()? as usize;
                let mut hash = HashMap::with_capacity(fields);
                for _ in 0..fields {
                    let field = cur.bytes_line()?;
                    let value = cur.bytes_line()?;
                    hash.insert(field, value);
                }
                Data::Hash(hash)
            }
            "L" => {
                let items = cur.line_num()? as usize;
                let mut list = VecDeque::with_capacity(items);
                for _ in 0..items {
                    list.push_back(cur.bytes_line()?);
                }
                Data::List(list)
            }
            other => return Err(format!("unknown type marker: {other}")),
        };
        entries.insert(key, Entry { data, expires_at });
    }
    Ok(entries)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Read one length-prefixed byte payload.
    fn bytes_line(&mut self) -> Result<String, String> {
        let len = self.line_num()?;
        if len < 0 {
            return Err("negative length".to_string());
        }
        let len = len as usize;
        if self.pos + len > self.bytes.len() {
            return Err("truncated snapshot".to_string());
        }
        let payload = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        String::from_utf8(payload.to_vec())
            .map_err(|_| "non-utf8 payload in snapshot".to_string())
    }

    fn line_num(&mut self) -> Result<i64, String> {
        let line = self.line_str()?;
        line.trim()
            .parse()
            .map_err(|_| format!("bad number in snapshot: {line}"))
    }

    fn line_str(&mut self) -> Result<&'a str, String> {
        let rest = &self.bytes[self.pos..];
        let idx = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("truncated snapshot")?;
        self.pos += idx + 1;
        std::str::from_utf8(&rest[..idx]).map_err(|_| "non-utf8 in snapshot".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with(expires_at: Option<u128>) -> Entry {
        Entry {
            data: Data::String("x".to_string()),
            expires_at,
        }
    }

    #[test]
    fn roundtrip_all_types() {
        let mut entries = HashMap::new();
        entries.insert(
            "str".to_string(),
            Entry {
                data: Data::String("hello\nworld".to_string()),
                expires_at: Some(1_700_000_000_000),
            },
        );
        let mut hash = HashMap::new();
        hash.insert("f1".to_string(), "v1".to_string());
        hash.insert("f2".to_string(), "v2".to_string());
        entries.insert("hash".to_string(), Entry {
            data: Data::Hash(hash),
            expires_at: None,
        });
        let mut list = VecDeque::new();
        list.push_back("a".to_string());
        list.push_back("b".to_string());
        entries.insert("list".to_string(), Entry {
            data: Data::List(list),
            expires_at: None,
        });
        entries.insert("expired-ish".to_string(), entry_with(Some(42)));

        let bytes = serialize(&entries);
        let loaded = deserialize(&bytes).unwrap();
        assert_eq!(loaded.len(), entries.len());
        for (key, entry) in &entries {
            assert_eq!(loaded.get(key).unwrap(), entry);
        }
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(deserialize(b"NOPE\n").is_err());
    }

    #[test]
    fn rejects_truncated() {
        let entries = HashMap::from([("k".to_string(), entry_with(None))]);
        let bytes = serialize(&entries);
        assert!(deserialize(&bytes[..bytes.len() - 3]).is_err());
    }

    #[test]
    fn roundtrip_empty() {
        let bytes = serialize(&HashMap::new());
        let loaded = deserialize(&bytes).unwrap();
        assert!(loaded.is_empty());
    }
}
