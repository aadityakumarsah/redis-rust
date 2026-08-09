use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::TcpStream;

use redis_server::resp::{self, DecodeError, Value};

fn main() {
    let mut host = String::from("127.0.0.1");
    let mut port: u16 = 6379;
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--host" => {
                host = args
                    .get(i + 1)
                    .cloned()
                    .unwrap_or_else(|| usage_and_exit("missing host after -h"));
                i += 2;
            }
            "-p" | "--port" => {
                let Some(p) = args.get(i + 1).and_then(|s| s.parse().ok()) else {
                    usage_and_exit("invalid port");
                };
                port = p;
                i += 2;
            }
            _ => break,
        }
    }
    let inline_args = &args[i..];

    let addr = format!("{host}:{port}");
    let stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Could not connect to Redis at {addr}: {e}");
            std::process::exit(1);
        }
    };
    let interactive = io::stdin().is_terminal();

    if inline_args.is_empty() {
        let mut stdin = io::stdin().lock();
        loop {
            if interactive {
                print!("{addr}> ");
                let _ = io::stdout().flush();
            }
            let mut line = String::new();
            if stdin.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if matches!(trimmed.to_ascii_lowercase().as_str(), "quit" | "exit") {
                break;
            }
            let tokens = tokenize(trimmed);
            let reply = run_command(&stream, &tokens);
            print_reply(&reply, 0);
        }
    } else {
        let reply = run_command(&stream, inline_args);
        print_reply(&reply, 0);
    }
}

fn run_command(stream: &TcpStream, args: &[String]) -> Value {
    let request = Value::Array(
        args.iter()
            .map(|a| Value::Bulk(Some(a.clone())))
            .collect(),
    );
    let mut writer = stream.try_clone().expect("clone failed");
    writer
        .write_all(&resp::encode(&request))
        .expect("write failed");
    let mut reader = stream.try_clone().expect("clone failed");
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match resp::decode(&buffer) {
            Ok((value, used)) => {
                buffer.drain(..used);
                return value;
            }
            Err(DecodeError::Incomplete) => {
                let n = reader.read(&mut chunk).expect("read failed");
                if n == 0 {
                    panic!("connection closed");
                }
                buffer.extend_from_slice(&chunk[..n]);
            }
            Err(DecodeError::Protocol(e)) => {
                panic!("protocol error: {e}");
            }
        }
    }
}

/// Format replies the way the real redis-cli does.
fn print_reply(value: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match value {
        Value::Simple(s) => println!("{s}"),
        Value::Error(e) => println!("{pad}(error) {e}"),
        Value::Integer(n) => println!("{pad}(integer) {n}"),
        Value::Bulk(Some(s)) => println!("{pad}\"{s}\""),
        Value::Bulk(None) => println!("{pad}(nil)"),
        Value::Array(items) => {
            if items.is_empty() {
                println!("{pad}(empty array)");
                return;
            }
            for (i, item) in items.iter().enumerate() {
                match item {
                    Value::Array(_) => {
                        println!("{pad}{})", i + 1);
                        print_reply(item, indent + 2);
                    }
                    Value::Simple(s) => println!("{pad}{}) {s}", i + 1),
                    Value::Error(e) => println!("{pad}{}) (error) {e}", i + 1),
                    Value::Integer(n) => println!("{pad}{}) (integer) {n}", i + 1),
                    Value::Bulk(Some(s)) => println!("{pad}{}) \"{s}\"", i + 1),
                    Value::Bulk(None) => println!("{pad}{}) (nil)", i + 1),
                }
            }
        }
    }
}

/// Split a command line into arguments, honoring single/double quotes.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match in_quote {
            Some(q) => {
                if c == '\\' {
                    if let Some(&next) = chars.peek() {
                        current.push(next);
                        chars.next();
                    }
                } else if c == q {
                    in_quote = None;
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '"' | '\'' => in_quote = Some(c),
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            },
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn usage_and_exit(msg: &str) -> ! {
    eprintln!("redis-cli: {msg}");
    eprintln!("Usage: redis-cli [-h HOST] [-p PORT] [command args...]");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_plain() {
        assert_eq!(tokenize("set foo bar"), vec!["set", "foo", "bar"]);
        assert_eq!(tokenize("  ping  "), vec!["ping"]);
        assert_eq!(tokenize(""), Vec::<String>::new());
    }

    #[test]
    fn tokenize_quoted() {
        assert_eq!(
            tokenize(r#"set msg "hello world""#),
            vec!["set", "msg", "hello world"]
        );
        assert_eq!(
            tokenize(r#"set msg 'it'"'"'s here'"#),
            vec!["set", "msg", "it's here"]
        );
        assert_eq!(
            tokenize(r#"set msg "say \"hi\"""#),
            vec!["set", "msg", r#"say "hi""#]
        );
        assert_eq!(tokenize("set msg a   b"), vec!["set", "msg", "a", "b"]);
    }
}
