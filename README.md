<div align="center">

# redis-rust

**A Redis-compatible server written in pure Rust — zero dependencies, std only.**

Minimal · Concurrent · Persisted · Tested

</div>

---

## Table of Contents

- [Features](#features)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [Commands](#commands)
- [Examples](#examples)
- [Architecture](#architecture)
- [Persistence](#persistence)
- [Testing](#testing)
- [Project Layout](#project-layout)
- [License](#license)

---

## Features

- **RESP wire protocol** — full encoder/decoder with partial-frame handling,
  pipelining, and protocol-error replies (never crashes, always answers)
- **Strings** — `SET` with `EX`/`PX`/`NX`/`XX`, `GET`, `MGET`/`MSET`,
  atomic counters, `APPEND`
- **Hashes** — `HSET`, `HGET`, `HGETALL`, `HINCRBY`, and friends
- **Lists** — `LPUSH`/`RPUSH`, `LPOP`/`RPOP` (with count), `LRANGE` (negative
  indices), `LINDEX`, `LLEN`
- **Keyspace** — `TTL`/`EXPIRE`/`PERSIST`, `KEYS` (glob `*`/`?`), `TYPE`,
  `DBSIZE`, `FLUSHALL`
- **TTL & expiry** — absolute-millisecond deadlines with a background janitor
  thread that sweeps expired keys every 100 ms
- **Persistence** — versioned, length-prefixed snapshot format; atomic writes
  (tmp + rename); loads on boot; optional time-based autosave
- **Concurrency** — thread-per-client, shared store behind a single `Mutex`,
  buffered I/O, TCP_NODELAY
- **Bundled `redis-cli`** — an interactive REPL with real Redis-style output
  (`PONG`, `"value"`, `(nil)`, `(integer) N`, numbered arrays), quote-aware
  argument parsing, and one-shot `redis-cli ping` usage — no external client,
  no Docker needed
- **Redis-compatible errors** — `WRONGTYPE`, arity errors, overflow errors,
  and integer-parsing errors match real Redis semantics
- **Fully tested** — 47 tests: unit tests per module + end-to-end TCP
  integration tests that spawn the real binary

## Quick Start

```sh
# terminal 1 — build & run the server
cargo run --release -- --port 6379

# terminal 2 — bundled redis-cli (interactive, like the real thing)
cargo run --release --bin redis-cli
```

```
$ cargo run --release --bin redis-cli
127.0.0.1:6379> ping
PONG
127.0.0.1:6379> set user:1 alice
OK
127.0.0.1:6379> get user:1
"alice"
127.0.0.1:6379> exit
```

One-shot usage — no interactive shell needed:

```sh
cargo run --release --bin redis-cli ping     # → PONG
cargo run --release --bin redis-cli get foo   # → "value" / (nil)
# custom host/port:
cargo run --release --bin redis-cli -p 6380 -h 127.0.0.1 ping
```

The bundled CLI supports quoting (`set msg "hello world"`), a
`127.0.0.1:6379>` prompt, and real redis-cli output formatting — no external
client or Docker required.

Raw RESP also works with any tool that speaks bytes:

```sh
printf '*3\r\n$3\r\nSET\r\n$5\r\nuser:1\r\n$5\r\nalice\r\n' | nc 127.0.0.1 6379
```

## Configuration

```
Usage: redis-server [OPTIONS]

Options:
  --port N            port to listen on (default: 6379)
  --dir PATH          directory for the snapshot file (default: .)
  --dbfilename NAME   snapshot file name (default: dump.rdb)
  --save-seconds N    autosave snapshot every N seconds when data changed
  --help              show this help
```

| Option | Default | Description |
| --- | --- | --- |
| `--port N` | `6379` | listen port |
| `--dir PATH` | `.` | directory for the snapshot file |
| `--dbfilename NAME` | `dump.rdb` | snapshot file name |
| `--save-seconds N` | off | autosave every N s while data is dirty |

Example with autosave every 30 seconds:

```sh
cargo run --release -- --port 6379 --dir ./data --dbfilename dump.rdb --save-seconds 30
```

## Commands

### Connection

| Command | Reply |
| --- | --- |
| `PING [msg]` | `PONG` / `msg` |
| `ECHO msg` | `msg` |
| `SELECT 0` | `OK` |
| `QUIT` | `OK`, then closes the connection |

### Strings

| Command | Reply |
| --- | --- |
| `SET key value [EX s] [PX ms] [NX] [XX]` | `OK` / `(nil)` |
| `SETNX key value` | `1` / `0` |
| `SETEX key seconds value` · `PSETEX key ms value` | `OK` |
| `GET key` | value / `(nil)` |
| `GETDEL key` | old value / `(nil)` |
| `GETSET key value` | old value / `(nil)` |
| `MGET key...` | array of values / `(nil)` |
| `MSET key value...` | `OK` |
| `APPEND key value` | new length |
| `STRLEN key` | length |
| `INCR key` · `DECR key` · `INCRBY key n` · `DECRBY key n` | new value |

### Keyspace

| Command | Reply |
| --- | --- |
| `DEL key...` | number removed |
| `EXISTS key...` | number existing |
| `TYPE key` | `string` / `hash` / `list` / `none` |
| `TTL key` · `PTTL key` | seconds / ms; `-1` no expiry; `-2` missing |
| `EXPIRE key seconds` · `PEXPIRE key ms` | `1` / `0` |
| `PERSIST key` | `1` / `0` |
| `KEYS pattern` | matching keys (`*`, `?`) |
| `DBSIZE` | key count |
| `FLUSHALL` · `FLUSHDB` | `OK` |
| `SAVE` · `BGSAVE` | `OK` |

### Hashes

| Command | Reply |
| --- | --- |
| `HSET key field value [field value...]` | number of new fields |
| `HMSET key field value ...` | `OK` |
| `HSETNX key field value` | `1` / `0` |
| `HGET key field` | value / `(nil)` |
| `HGETALL key` | flat field/value array |
| `HMGET key field...` | array of values / `(nil)` |
| `HDEL key field...` | number removed |
| `HEXISTS key field` | `1` / `0` |
| `HLEN key` | field count |
| `HKEYS key` · `HVALS key` | field/value arrays |
| `HINCRBY key field n` | new field value |

### Lists

| Command | Reply |
| --- | --- |
| `LPUSH key value...` · `RPUSH key value...` | new length |
| `LPOP key [count]` · `RPOP key [count]` | value / array |
| `LRANGE key start stop` | array (negative indices supported) |
| `LLEN key` | length |
| `LINDEX key index` | value / `(nil)` |

## Examples

All examples use the bundled CLI — the shortest path is the interactive
prompt, or use one-shot mode:

```sh
cargo run --release --bin redis-cli incr visits     # (integer) 1
cargo run --release --bin redis-cli get visits      # "1"
```

```sh
# counters
$ redis-cli incr visits
(integer) 1
$ redis-cli incrby visits 99
(integer) 100

# expiring keys
$ redis-cli setex session "60" abc123
OK
$ redis-cli ttl session
(integer) 59
$ redis-cli ttl session
(integer) -2

# hashes
$ redis-cli hset user:1 name alice age 30
(integer) 2
$ redis-cli hgetall user:1
1) "name"
2) "alice"
3) "age"
4) "30"

# lists
$ redis-cli rpush queue job-a job-b
(integer) 2
$ redis-cli lpop queue
"job-a"
$ redis-cli lrange queue 0 -1
1) "job-b"

# persistence
$ redis-cli save
OK
$ redis-cli set durable yes
OK
# ... restart the server ...
$ redis-cli get durable
"yes"
```

## Architecture

```
                        ┌──────────────────────────────┐
   redis-cli / any      │        src/server.rs          │
   RESP client  ─────►  │  accept loop (main thread)    │
                        │       │                       │
                        │       ▼  thread-per-client    │
                        │  handle_client:               │
                        │   read 8 KiB chunks           │
                        │   → resp::decode (bytes)      │
                        │   → commands::dispatch        │
                        │   → resp::encode → write+flush│
                        └───────┬──────────────────────┘
                                │ &Store (Arc)
                        ┌───────▼──────────────────────┐
                        │     src/store.rs              │
                        │  Mutex<HashMap<String, Entry>>│
                        │  Entry { Data, expires_at_ms }│
                        │  Data: String | Hash | List   │
                        └───────┬──────────────────────┘
                                │
              janitor thread ───┤── purge expired every 100 ms
              autosave thread ──┤── save snapshot when dirty
              SAVE/BGSAVE ──────┘
```

Key design points:

- **Byte-safe protocol layer** — the read buffer is `Vec<u8>`; framing is
  byte-oriented, so arbitrary payloads (including `\r\n` inside values) are
  handled correctly.
- **Lazy + active expiry** — expired keys are purged on access *and* swept by
  a background janitor thread.
- **Single-writer store** — one `Mutex` around the whole keyspace keeps every
  command atomic without per-key locking complexity.
- **Atomic persistence** — snapshots are written to a temp file and renamed,
  so a crash mid-save never corrupts the last good snapshot.
- **No panics on bad input** — malformed frames get an `-ERR Protocol error`
  reply; a client that goes away just ends its thread.

## Persistence

Snapshots use a versioned (`RS1`), line-based format with length-prefixed
fields, so values may contain arbitrary bytes including newlines:

```
RS1
<key_count>
S|<type>
<key_len>
<key bytes>
<expire_ms | -1>
<value_len>
<value bytes>
...
```

- Written atomically on `SAVE` / `BGSAVE`
- Written automatically by the autosave thread when the dataset is dirty
  (`--save-seconds N`)
- Loaded at startup (missing/corrupt files are ignored with a warning)
- TTLs survive restarts

## Testing

```sh
cargo test
```

47 tests, all green:

- **Unit** — RESP codec round-trips, partial frames, protocol errors,
  pipelined decoding; store TTL/expiry, counters, hash/list semantics,
  glob matching; command dispatch arity/errors; snapshot round-trips and
  corruption handling; bundled CLI argument tokenizing (quotes, escapes)
- **Integration** (spawns the real binaries over TCP) — basic flows, all data
  types, expiry, wrong-type errors, 8 concurrent clients, pipelined commands
  in one packet, persistence across restart, protocol-error replies, and the
  bundled `redis-cli` in both one-shot and piped/interactive modes

## Project Layout

```
.
├── Cargo.toml
├── src/
│   ├── main.rs           server entry point, exit codes
│   ├── config.rs         CLI configuration
│   ├── server.rs         TCP accept loop, per-client threads
│   ├── resp.rs           RESP wire protocol (encode/decode)
│   ├── store.rs          thread-safe store, TTL, hashes, lists
│   ├── commands.rs       command dispatch table
│   ├── snapshot.rs       persistence format
│   └── bin/
│       └── redis-cli.rs  bundled interactive client (real redis-cli style)
└── tests/
    └── integration.rs    end-to-end TCP tests
```

## License

MIT
