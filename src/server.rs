use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::commands::{self, Context};
use crate::config::Config;
use crate::resp::{self, DecodeError, Value};
use crate::store::Store;

pub fn run(config: Config) -> std::io::Result<()> {
    let db_path = config.db_path();
    let store = Arc::new(Store::load_from(&db_path));

    let addr = format!("127.0.0.1:{}", config.port);
    let listener = TcpListener::bind(&addr)?;
    println!("redis-server 0.1.0 starting...");
    println!("* Listening on {addr}");
    println!(
        "* Snapshot: {} (loaded on start, SAVE/BGSAVE on demand)",
        db_path.display()
    );
    if let Some(secs) = config.save_seconds {
        println!("* Autosave: every {secs}s when data changed");
        store.spawn_autosave(db_path.clone(), Duration::from_secs(secs));
    }
    store.spawn_janitor(Duration::from_millis(100));
    println!("Ready to accept connections (redis-cli ping)");

    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        let store = Arc::clone(&store);
        let db_path = db_path.clone();
        std::thread::spawn(move || handle_client(stream, store, db_path));
    }
    Ok(())
}

fn handle_client(stream: TcpStream, store: Arc<Store>, db_path: PathBuf) {
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream.try_clone().expect("clone failed"));
    let mut writer = BufWriter::new(stream.try_clone().expect("clone failed"));
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let ctx = Context {
        store: &store,
        db_path: &db_path,
    };

    loop {
        match resp::decode(&buffer) {
            Ok((value, used)) => {
                buffer.drain(..used);
                let reply = match &value {
                    Value::Array(items) => commands::dispatch(items, &ctx),
                    _ => Value::Error("ERR expected array".to_string()),
                };
                let is_quit = commands::is_quit(&value);
                let encoded = resp::encode(&reply);
                if writer.write_all(&encoded).and_then(|_| writer.flush()).is_err() {
                    break;
                }
                if is_quit {
                    break;
                }
            }
            Err(DecodeError::Incomplete) => match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            },
            Err(DecodeError::Protocol(e)) => {
                let reply = resp::encode(&Value::Error(format!("ERR Protocol error: {e}")));
                let _ = writer.write_all(&reply);
                let _ = writer.flush();
                break;
            }
        }
    }
}
