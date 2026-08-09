# redis-rust

A Redis-compatible server written in pure Rust (std only, zero dependencies).

## Features

- RESP protocol codec (encode/decode, partial frames, pipelining)
- Strings: SET (EX/PX/NX/XX), GET, GETDEL, GETSET, MGET/MSET, APPEND, STRLEN, INCR/DECR/INCRBY/DECRBY
- Hashes: HSET, HGET, HGETALL, HMGET, HDEL, HEXISTS, HLEN, HKEYS, HVALS, HINCRBY, HSETNX
- Lists: LPUSH/RPUSH, LPOP/RPOP, LRANGE, LLEN, LINDEX
- Keyspace: DEL, EXISTS, TYPE, TTL/PTTL, EXPIRE/PEXPIRE, PERSIST, KEYS, DBSIZE, FLUSHALL
- TTL/expiry with a background janitor thread
- Snapshot persistence (SAVE/BGSAVE, autosave, atomic writes, load on boot)
- Thread-per-client concurrency, buffered I/O, protocol error handling

## Usage

```sh
cargo run --release -- --port 6379 --save-seconds 10
```

| Option | Default | Description |
| --- | --- | --- |
| `--port N` | `6379` | port to listen on |
| `--dir PATH` | `.` | directory for the snapshot file |
| `--dbfilename NAME` | `dump.rdb` | snapshot file name |
| `--save-seconds N` | off | autosave when data changed |

## Testing

```sh
cargo test
```

Unit tests cover the RESP codec, store, commands, and snapshot format;
integration tests spawn the real binary and exercise it over TCP
(pipelining, concurrency, persistence across restart).

## Project layout

```
src/
  main.rs       entry point
  config.rs     CLI configuration
  server.rs     TCP server, accept loop, per-client threads
  resp.rs       RESP wire protocol
  store.rs      thread-safe data store with TTL support
  commands.rs   command dispatch
  snapshot.rs   persistence format
tests/
  integration.rs  end-to-end TCP tests
```
