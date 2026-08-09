use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use redis_server::resp::{decode, encode, Value};

struct Server {
    child: Child,
    port: u16,
    dir: std::path::PathBuf,
}

impl Server {
    fn start() -> Server {
        let port = free_port();
        let dir = std::env::temp_dir().join(format!(
            "redis-integration-{}-{port}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dbfile = format!("db-{port}.rdb");
        let child = Command::new(env!("CARGO_BIN_EXE_redis-server"))
            .args([
                "--port",
                &port.to_string(),
                "--dir",
                dir.to_str().unwrap(),
                "--dbfilename",
                &dbfile,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn redis-server");

        let server = Server { child, port, dir };
        server.wait_until_ready();
        server
    }

    fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("server did not become ready");
    }

    fn cmd(&self, args: &[&str]) -> Value {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = Value::Array(
            args.iter()
                .map(|a| Value::Bulk(Some(a.to_string())))
                .collect(),
        );
        stream.write_all(&encode(&request)).unwrap();
        read_reply(&mut stream)
    }

    fn dbfile(&self) -> std::path::PathBuf {
        self.dir.join(format!("db-{}.rdb", self.port))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Buffered client that keeps leftover bytes between replies (pipelining).
struct Client {
    stream: TcpStream,
    buffer: Vec<u8>,
}

impl Client {
    fn connect(port: u16) -> Client {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            stream,
            buffer: Vec::new(),
        }
    }

    fn send(&mut self, args: &[&str]) {
        let request = Value::Array(
            args.iter()
                .map(|a| Value::Bulk(Some(a.to_string())))
                .collect(),
        );
        self.stream.write_all(&encode(&request)).unwrap();
    }

    fn reply(&mut self) -> Value {
        let mut chunk = [0u8; 4096];
        loop {
            match decode(&self.buffer) {
                Ok((value, used)) => {
                    self.buffer.drain(..used);
                    return value;
                }
                Err(redis_server::resp::DecodeError::Incomplete) => {
                    let n = self.stream.read(&mut chunk).expect("read failed");
                    assert!(n > 0, "connection closed while reading reply");
                    self.buffer.extend_from_slice(&chunk[..n]);
                }
                Err(redis_server::resp::DecodeError::Protocol(e)) => {
                    panic!("protocol error reading reply: {e}");
                }
            }
        }
    }
}

fn read_reply(stream: &mut TcpStream) -> Value {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match decode(&buffer) {
            Ok((value, used)) => {
                assert_eq!(used, buffer.len(), "unexpected trailing bytes");
                return value;
            }
            Err(redis_server::resp::DecodeError::Incomplete) => {
                let n = stream.read(&mut chunk).expect("read failed");
                assert!(n > 0, "connection closed while reading reply");
                buffer.extend_from_slice(&chunk[..n]);
            }
            Err(redis_server::resp::DecodeError::Protocol(e)) => {
                panic!("protocol error reading reply: {e}");
            }
        }
    }
}

#[test]
fn basic_string_flow() {
    let s = Server::start();
    assert_eq!(s.cmd(&["PING"]), Value::Simple("PONG".to_string()));
    assert_eq!(
        s.cmd(&["SET", "foo", "bar"]),
        Value::Simple("OK".to_string())
    );
    assert_eq!(s.cmd(&["GET", "foo"]), Value::Bulk(Some("bar".to_string())));
    assert_eq!(s.cmd(&["GET", "missing"]), Value::Bulk(None));
    assert_eq!(s.cmd(&["DEL", "foo"]), Value::Integer(1));
    assert_eq!(s.cmd(&["GET", "foo"]), Value::Bulk(None));
}

#[test]
fn all_data_types_over_the_wire() {
    let s = Server::start();
    assert_eq!(s.cmd(&["HSET", "user:1", "name", "alice", "age", "30"]), Value::Integer(2));
    assert_eq!(s.cmd(&["HGET", "user:1", "name"]), Value::Bulk(Some("alice".to_string())));
    assert_eq!(s.cmd(&["RPUSH", "queue", "a", "b", "c"]), Value::Integer(3));
    assert_eq!(s.cmd(&["LRANGE", "queue", "0", "-1"]), Value::Array(vec![
        Value::Bulk(Some("a".to_string())),
        Value::Bulk(Some("b".to_string())),
        Value::Bulk(Some("c".to_string())),
    ]));
    assert_eq!(s.cmd(&["INCR", "visits"]), Value::Integer(1));
    assert_eq!(s.cmd(&["INCR", "visits"]), Value::Integer(2));
    assert_eq!(s.cmd(&["TYPE", "user:1"]), Value::Simple("hash".to_string()));
    assert_eq!(s.cmd(&["TYPE", "queue"]), Value::Simple("list".to_string()));
    assert_eq!(s.cmd(&["TYPE", "visits"]), Value::Simple("string".to_string()));
}

#[test]
fn expiry_via_setex_and_ttl() {
    let s = Server::start();
    assert_eq!(s.cmd(&["SETEX", "session", "1", "abc"]), Value::Simple("OK".to_string()));
    assert_eq!(s.cmd(&["GET", "session"]), Value::Bulk(Some("abc".to_string())));
    assert_eq!(s.cmd(&["TTL", "session"]), Value::Integer(1));
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(s.cmd(&["GET", "session"]), Value::Bulk(None));
    assert_eq!(s.cmd(&["TTL", "session"]), Value::Integer(-2));
}

#[test]
fn wrong_type_and_unknown_command() {
    let s = Server::start();
    s.cmd(&["SET", "k", "v"]);
    let reply = s.cmd(&["LPUSH", "k", "x"]);
    assert!(matches!(reply, Value::Error(e) if e.starts_with("WRONGTYPE")));
    let reply = s.cmd(&["NOPE"]);
    assert!(matches!(reply, Value::Error(e) if e.starts_with("ERR unknown command")));
}

#[test]
fn multiple_clients_concurrently() {
    let s = Server::start();
    let mut handles = Vec::new();
    for i in 0..8 {
        let port = s.port;
        handles.push(thread::spawn(move || {
            let mut client = Client::connect(port);
            let key = format!("thread-{i}");
            client.send(&["SET", &key, "42"]);
            assert_eq!(client.reply(), Value::Simple("OK".to_string()));
            client.send(&["GET", &key]);
            assert_eq!(client.reply(), Value::Bulk(Some("42".to_string())));
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(s.cmd(&["DBSIZE"]), Value::Integer(8));
}

#[test]
fn pipelined_commands_in_one_packet() {
    let s = Server::start();
    let mut client = Client::connect(s.port);
    let mut packet = Vec::new();
    for args in [&["SET", "a", "1"][..], &["SET", "b", "2"], &["GET", "a"], &["GET", "b"]] {
        packet.extend(encode(&Value::Array(
            args.iter().map(|a| Value::Bulk(Some(a.to_string()))).collect(),
        )));
    }
    client.stream.write_all(&packet).unwrap();
    assert_eq!(client.reply(), Value::Simple("OK".to_string()));
    assert_eq!(client.reply(), Value::Simple("OK".to_string()));
    assert_eq!(client.reply(), Value::Bulk(Some("1".to_string())));
    assert_eq!(client.reply(), Value::Bulk(Some("2".to_string())));
}

#[test]
fn persistence_survives_restart() {
    let mut server = Server::start();
    server.cmd(&["SET", "durable", "yes"]);
    server.cmd(&["RPUSH", "history", "x", "y"]);
    assert_eq!(server.cmd(&["SAVE"]), Value::Simple("OK".to_string()));
    assert!(server.dbfile().exists());

    server.child.kill().unwrap();
    server.child.wait().unwrap();
    let port = free_port();
    let dir = server.dir.clone();
    let dbfile = server.dbfile();
    let child = Command::new(env!("CARGO_BIN_EXE_redis-server"))
        .args([
            "--port",
            &port.to_string(),
            "--dir",
            dir.to_str().unwrap(),
            "--dbfilename",
            dbfile.file_name().unwrap().to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to respawn redis-server");
    server.child = child;
    server.port = port;
    server.wait_until_ready();

    assert_eq!(server.cmd(&["GET", "durable"]), Value::Bulk(Some("yes".to_string())));
    assert_eq!(server.cmd(&["LRANGE", "history", "0", "-1"]), Value::Array(vec![
        Value::Bulk(Some("x".to_string())),
        Value::Bulk(Some("y".to_string())),
    ]));
}

#[test]
fn protocol_error_gets_error_reply() {
    let s = Server::start();
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    stream.write_all(b"!not-a-resp-type\r\n").unwrap();
    let mut reply = String::new();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("-ERR Protocol error:"), "got: {reply}");
}
