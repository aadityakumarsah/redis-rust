#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Simple(String),
    Error(String),
    Integer(i64),
    Bulk(Option<String>),
    Array(Vec<Value>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Incomplete,
    Protocol(String),
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Simple(s) => {
            out.push(b'+');
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        Value::Error(s) => {
            out.push(b'-');
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        Value::Integer(n) => {
            out.push(b':');
            out.extend_from_slice(n.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        Value::Bulk(Some(s)) => {
            out.push(b'$');
            out.extend_from_slice(s.len().to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        Value::Bulk(None) => out.extend_from_slice(b"$-1\r\n"),
        Value::Array(items) => {
            out.push(b'*');
            out.extend_from_slice(items.len().to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
            for item in items {
                encode_into(item, out);
            }
        }
    }
}

/// Decode a single RESP value from the head of `input`.
/// Returns the value and the number of bytes consumed. If the input does not
/// yet contain a complete value, returns `DecodeError::Incomplete`.
pub fn decode(input: &[u8]) -> Result<(Value, usize), DecodeError> {
    let Some(&first) = input.first() else {
        return Err(DecodeError::Incomplete);
    };

    match first {
        b'+' | b'-' | b':' => {
            let Some(end) = find_crlf(input, 1) else {
                return Err(DecodeError::Incomplete);
            };
            let line = str_from(&input[1..end])?;
            match first {
                b'+' => Ok((Value::Simple(line), end + 2)),
                b'-' => Ok((Value::Error(line), end + 2)),
                _ => Ok((Value::Integer(parse_int(&line)?), end + 2)),
            }
        }
        b'$' => {
            let Some(end) = find_crlf(input, 1) else {
                return Err(DecodeError::Incomplete);
            };
            let len = parse_int(&str_from(&input[1..end])?)?;
            if len < 0 {
                return Ok((Value::Bulk(None), end + 2));
            }
            let len = len as usize;
            let start = end + 2;
            let total = start + len + 2;
            if input.len() < total {
                return Err(DecodeError::Incomplete);
            }
            if input[start + len] != b'\r' || input[start + len + 1] != b'\n' {
                return Err(DecodeError::Protocol("malformed bulk string".to_string()));
            }
            let s = str_from(&input[start..start + len])?;
            Ok((Value::Bulk(Some(s)), total))
        }
        b'*' => {
            let Some(end) = find_crlf(input, 1) else {
                return Err(DecodeError::Incomplete);
            };
            let count = parse_int(&str_from(&input[1..end])?)?;
            if count < 0 {
                return Ok((Value::Array(Vec::new()), end + 2));
            }
            let mut items = Vec::with_capacity(count as usize);
            let mut pos = end + 2;
            for _ in 0..count {
                let (item, used) = decode(&input[pos..])?;
                items.push(item);
                pos += used;
            }
            Ok((Value::Array(items), pos))
        }
        other => Err(DecodeError::Protocol(format!(
            "unknown type byte: {}",
            other as char
        ))),
    }
}

fn find_crlf(bytes: &[u8], start: usize) -> Option<usize> {
    bytes[start..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|i| start + i)
}

fn str_from(bytes: &[u8]) -> Result<String, DecodeError> {
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn parse_int(line: &str) -> Result<i64, DecodeError> {
    line.trim()
        .parse()
        .map_err(|_| DecodeError::Protocol("bad number".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: Value) {
        let bytes = encode(&value);
        let (decoded, used) = decode(&bytes).expect("decode failed");
        assert_eq!(decoded, value);
        assert_eq!(used, bytes.len());
    }

    #[test]
    fn roundtrip_simple() {
        roundtrip(Value::Simple("PONG".to_string()));
    }

    #[test]
    fn roundtrip_error() {
        roundtrip(Value::Error("ERR oops".to_string()));
    }

    #[test]
    fn roundtrip_integer() {
        roundtrip(Value::Integer(42));
        roundtrip(Value::Integer(-7));
    }

    #[test]
    fn roundtrip_bulk() {
        roundtrip(Value::Bulk(Some("hello world".to_string())));
        roundtrip(Value::Bulk(Some(String::new())));
        roundtrip(Value::Bulk(None));
    }

    #[test]
    fn roundtrip_array() {
        roundtrip(Value::Array(vec![
            Value::Bulk(Some("GET".to_string())),
            Value::Bulk(Some("key".to_string())),
        ]));
        roundtrip(Value::Array(vec![]));
        roundtrip(Value::Array(vec![Value::Array(vec![Value::Integer(1)])]));
    }

    #[test]
    fn decode_empty_is_incomplete() {
        assert_eq!(decode(b""), Err(DecodeError::Incomplete));
    }

    #[test]
    fn decode_partial_bulk_is_incomplete() {
        assert_eq!(decode(b"$5\r\nab"), Err(DecodeError::Incomplete));
        assert_eq!(decode(b"$5\r\nabc"), Err(DecodeError::Incomplete));
    }

    #[test]
    fn decode_partial_array_is_incomplete() {
        assert_eq!(decode(b"*2\r\n$1\r\na"), Err(DecodeError::Incomplete));
    }

    #[test]
    fn decode_protocol_error() {
        assert!(matches!(decode(b"!oops\r\n"), Err(DecodeError::Protocol(_))));
        assert!(matches!(decode(b"$x\r\n"), Err(DecodeError::Protocol(_))));
    }

    #[test]
    fn decode_pipeline_of_two_commands() {
        let input = b"*1\r\n$4\r\nPING\r\n*1\r\n$4\r\nPING\r\n";
        let (first, used) = decode(input).unwrap();
        assert_eq!(
            first,
            Value::Array(vec![Value::Bulk(Some("PING".to_string()))])
        );
        let (second, used2) = decode(&input[used..]).unwrap();
        assert_eq!(first, second);
        assert_eq!(used + used2, input.len());
    }

    #[test]
    fn decode_empty_bulk() {
        let (value, used) = decode(b"$0\r\n\r\n").unwrap();
        assert_eq!(value, Value::Bulk(Some(String::new())));
        assert_eq!(used, 6);
    }
}
