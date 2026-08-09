use std::path::Path;

use crate::resp::Value;
use crate::store::{Data, Store, StoreError, NOT_INTEGER_ERR, OVERFLOW_ERR, WRONGTYPE_ERR};

pub struct Context<'a> {
    pub store: &'a Store,
    pub db_path: &'a Path,
}

pub fn dispatch(items: &[Value], ctx: &Context) -> Value {
    let Some(name) = command_name(items) else {
        return Value::Error("ERR empty command".to_string());
    };
    match name.as_str() {
        "PING" => cmd_ping(items),
        "ECHO" => cmd_echo(items),
        "QUIT" => Value::Simple("OK".to_string()),
        "SELECT" => cmd_select(items),
        "SET" => cmd_set(items, ctx.store),
        "SETNX" => cmd_setnx(items, ctx.store),
        "SETEX" | "PSETEX" => cmd_setex(items, ctx.store, name == "PSETEX"),
        "GET" => cmd_get(items, ctx.store),
        "GETDEL" => cmd_getdel(items, ctx.store),
        "GETSET" => cmd_getset(items, ctx.store),
        "MGET" => cmd_mget(items, ctx.store),
        "MSET" => cmd_mset(items, ctx.store),
        "APPEND" => cmd_append(items, ctx.store),
        "STRLEN" => cmd_strlen(items, ctx.store),
        "INCR" => cmd_incr_by(items, ctx.store, 1),
        "DECR" => cmd_incr_by(items, ctx.store, -1),
        "INCRBY" => cmd_incr_by_arg(items, ctx.store, 1),
        "DECRBY" => cmd_incr_by_arg(items, ctx.store, -1),
        "DEL" => cmd_del(items, ctx.store),
        "EXISTS" => cmd_exists(items, ctx.store),
        "TYPE" => cmd_type(items, ctx.store),
        "TTL" | "PTTL" => cmd_ttl(items, ctx.store, name == "PTTL"),
        "EXPIRE" | "PEXPIRE" => cmd_expire(items, ctx.store, name == "PEXPIRE"),
        "PERSIST" => cmd_persist(items, ctx.store),
        "KEYS" => cmd_keys(items, ctx.store),
        "DBSIZE" => Value::Integer(ctx.store.len() as i64),
        "FLUSHALL" | "FLUSHDB" => {
            ctx.store.flush();
            Value::Simple("OK".to_string())
        }
        "SAVE" | "BGSAVE" => match ctx.store.save(ctx.db_path) {
            Ok(()) => Value::Simple("OK".to_string()),
            Err(e) => Value::Error(format!("ERR saving snapshot: {e}")),
        },
        "HSET" => cmd_hset(items, ctx.store),
        "HMSET" => cmd_hmset(items, ctx.store),
        "HSETNX" => cmd_hsetnx(items, ctx.store),
        "HGET" => cmd_hget(items, ctx.store),
        "HGETALL" => cmd_hgetall(items, ctx.store),
        "HMGET" => cmd_hmget(items, ctx.store),
        "HDEL" => cmd_hdel(items, ctx.store),
        "HEXISTS" => cmd_hexists(items, ctx.store),
        "HLEN" => cmd_hlen(items, ctx.store),
        "HKEYS" => cmd_hkeys(items, ctx.store),
        "HVALS" => cmd_hvals(items, ctx.store),
        "HINCRBY" => cmd_hincrby(items, ctx.store),
        "LPUSH" | "RPUSH" => cmd_push(items, ctx.store, name == "RPUSH"),
        "LPOP" | "RPOP" => cmd_pop(items, ctx.store, name == "RPOP"),
        "LRANGE" => cmd_lrange(items, ctx.store),
        "LLEN" => cmd_llen(items, ctx.store),
        "LINDEX" => cmd_lindex(items, ctx.store),
        other => Value::Error(format!("ERR unknown command '{}'", other)),
    }
}

pub fn is_quit(value: &Value) -> bool {
    matches!(value, Value::Array(items) if command_name(items).as_deref() == Some("QUIT"))
}

// ---- helpers ----

fn command_name(items: &[Value]) -> Option<String> {
    match items.first()? {
        Value::Bulk(Some(s)) => Some(s.to_uppercase()),
        Value::Simple(s) => Some(s.to_uppercase()),
        _ => None,
    }
}

fn str_arg<'a>(items: &'a [Value], i: usize) -> Option<&'a str> {
    match items.get(i) {
        Some(Value::Bulk(Some(s))) => Some(s),
        Some(Value::Simple(s)) => Some(s),
        _ => None,
    }
}

fn int_arg(items: &[Value], i: usize) -> Option<i64> {
    str_arg(items, i)?.parse().ok()
}

fn all_str_args<'a>(items: &'a [Value], from: usize) -> Vec<&'a str> {
    items[from..]
        .iter()
        .filter_map(|v| match v {
            Value::Bulk(Some(s)) => Some(s.as_str()),
            Value::Simple(s) => Some(s.as_str()),
            _ => None,
        })
        .collect()
}

fn wrong_arity(name: &str) -> Value {
    Value::Error(format!(
        "ERR wrong number of arguments for '{}' command",
        name
    ))
}

fn store_err(e: StoreError) -> Value {
    match e {
        StoreError::WrongType => Value::Error(WRONGTYPE_ERR.to_string()),
        StoreError::NotAnInteger => Value::Error(NOT_INTEGER_ERR.to_string()),
        StoreError::Overflow => Value::Error(OVERFLOW_ERR.to_string()),
    }
}

// ---- connection ----

fn cmd_ping(items: &[Value]) -> Value {
    match items.len() {
        1 => Value::Simple("PONG".to_string()),
        2 => match str_arg(items, 1) {
            Some(msg) => Value::Bulk(Some(msg.to_string())),
            None => wrong_arity("ping"),
        },
        _ => wrong_arity("ping"),
    }
}

fn cmd_echo(items: &[Value]) -> Value {
    if items.len() != 2 {
        return wrong_arity("echo");
    }
    match str_arg(items, 1) {
        Some(msg) => Value::Bulk(Some(msg.to_string())),
        None => wrong_arity("echo"),
    }
}

fn cmd_select(items: &[Value]) -> Value {
    match str_arg(items, 1) {
        Some("0") => Value::Simple("OK".to_string()),
        Some(_) => Value::Error("ERR DB index is out of range".to_string()),
        None => wrong_arity("select"),
    }
}

// ---- strings ----

fn cmd_set(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(value)) = (str_arg(items, 1), str_arg(items, 2)) else {
        return wrong_arity("set");
    };

    let mut ttl_ms: Option<u64> = None;
    let mut nx = false;
    let mut xx = false;
    let mut i = 3;
    while i < items.len() {
        let Some(opt) = str_arg(items, i) else {
            return wrong_arity("set");
        };
        match opt.to_ascii_uppercase().as_str() {
            "EX" | "PX" => {
                let Some(n) = str_arg(items, i + 1).and_then(|s| s.parse::<u64>().ok()) else {
                    return Value::Error("ERR invalid expire time in 'set' command".to_string());
                };
                if n == 0 {
                    return Value::Error("ERR invalid expire time in 'set' command".to_string());
                }
                ttl_ms = Some(if opt.eq_ignore_ascii_case("EX") {
                    n.saturating_mul(1000)
                } else {
                    n
                });
                i += 2;
            }
            "NX" => {
                nx = true;
                i += 1;
            }
            "XX" => {
                xx = true;
                i += 1;
            }
            _ => return wrong_arity("set"),
        }
    }
    if nx && xx {
        return Value::Error("ERR syntax error".to_string());
    }
    if (nx && store.exists(key)) || (xx && !store.exists(key)) {
        return Value::Bulk(None);
    }
    store.set(key, Data::String(value.to_string()), ttl_ms);
    Value::Simple("OK".to_string())
}

fn cmd_setnx(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(value)) = (str_arg(items, 1), str_arg(items, 2)) else {
        return wrong_arity("setnx");
    };
    let created = store.set_if_absent(key, Data::String(value.to_string()));
    Value::Integer(created as i64)
}

fn cmd_setex(items: &[Value], store: &Store, is_ms: bool) -> Value {
    let (Some(key), Some(ttl), Some(value)) =
        (str_arg(items, 1), int_arg(items, 2), str_arg(items, 3))
    else {
        return wrong_arity(if is_ms { "psetex" } else { "setex" });
    };
    if ttl <= 0 {
        return Value::Error(format!(
            "ERR invalid expire time in '{}' command",
            if is_ms { "psetex" } else { "setex" }
        ));
    }
    let ttl_ms = if is_ms { ttl as u64 } else { ttl as u64 * 1000 };
    store.set(key, Data::String(value.to_string()), Some(ttl_ms));
    Value::Simple("OK".to_string())
}

fn cmd_get(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("get");
    };
    match store.get(key) {
        Some(Data::String(s)) => Value::Bulk(Some(s)),
        Some(_) => Value::Error(WRONGTYPE_ERR.to_string()),
        None => Value::Bulk(None),
    }
}

fn cmd_getdel(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("getdel");
    };
    match store.get(key) {
        Some(Data::String(s)) => {
            store.remove(key);
            Value::Bulk(Some(s))
        }
        Some(_) => Value::Error(WRONGTYPE_ERR.to_string()),
        None => Value::Bulk(None),
    }
}

fn cmd_getset(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(value)) = (str_arg(items, 1), str_arg(items, 2)) else {
        return wrong_arity("getset");
    };
    let old = match store.get(key) {
        Some(Data::String(s)) => Value::Bulk(Some(s)),
        Some(_) => return Value::Error(WRONGTYPE_ERR.to_string()),
        None => Value::Bulk(None),
    };
    store.set(key, Data::String(value.to_string()), None);
    old
}

fn cmd_mget(items: &[Value], store: &Store) -> Value {
    if items.len() < 2 {
        return wrong_arity("mget");
    }
    let keys = all_str_args(items, 1);
    let values = keys
        .iter()
        .map(|k| match store.get(k) {
            Some(Data::String(s)) => Value::Bulk(Some(s)),
            _ => Value::Bulk(None),
        })
        .collect();
    Value::Array(values)
}

fn cmd_mset(items: &[Value], store: &Store) -> Value {
    if items.len() < 3 || (items.len() - 1) % 2 != 0 {
        return wrong_arity("mset");
    }
    let mut i = 1;
    while i + 1 < items.len() {
        let (Some(k), Some(v)) = (str_arg(items, i), str_arg(items, i + 1)) else {
            return wrong_arity("mset");
        };
        store.set(k, Data::String(v.to_string()), None);
        i += 2;
    }
    Value::Simple("OK".to_string())
}

fn cmd_append(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(suffix)) = (str_arg(items, 1), str_arg(items, 2)) else {
        return wrong_arity("append");
    };
    match store.append(key, suffix) {
        Ok(n) => Value::Integer(n as i64),
        Err(e) => store_err(e),
    }
}

fn cmd_strlen(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("strlen");
    };
    match store.get(key) {
        Some(Data::String(s)) => Value::Integer(s.len() as i64),
        Some(_) => Value::Error(WRONGTYPE_ERR.to_string()),
        None => Value::Integer(0),
    }
}

fn cmd_incr_by(items: &[Value], store: &Store, delta: i64) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity(if delta == 1 { "incr" } else { "decr" });
    };
    match store.incrby(key, delta) {
        Ok(n) => Value::Integer(n),
        Err(e) => store_err(e),
    }
}

fn cmd_incr_by_arg(items: &[Value], store: &Store, sign: i64) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity(if sign == 1 { "incrby" } else { "decrby" });
    };
    let Some(delta) = int_arg(items, 2) else {
        return wrong_arity(if sign == 1 { "incrby" } else { "decrby" });
    };
    match store.incrby(key, sign * delta) {
        Ok(n) => Value::Integer(n),
        Err(e) => store_err(e),
    }
}

// ---- keyspace ----

fn cmd_del(items: &[Value], store: &Store) -> Value {
    if items.len() < 2 {
        return wrong_arity("del");
    }
    let keys: Vec<&str> = all_str_args(items, 1);
    Value::Integer(store.remove_many(&keys) as i64)
}

fn cmd_exists(items: &[Value], store: &Store) -> Value {
    if items.len() < 2 {
        return wrong_arity("exists");
    }
    let keys = all_str_args(items, 1);
    let count = keys.iter().filter(|k| store.exists(k)).count();
    Value::Integer(count as i64)
}

fn cmd_type(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("type");
    };
    match store.type_of(key) {
        Some(t) => Value::Simple(t.to_string()),
        None => Value::Simple("none".to_string()),
    }
}

fn cmd_ttl(items: &[Value], store: &Store, ms: bool) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity(if ms { "pttl" } else { "ttl" });
    };
    match store.ttl_ms(key) {
        None => Value::Integer(-2),
        Some(-1) => Value::Integer(-1),
        Some(remaining) if ms => Value::Integer(remaining),
        Some(remaining) => Value::Integer((remaining + 999) / 1000),
    }
}

fn cmd_expire(items: &[Value], store: &Store, ms: bool) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity(if ms { "pexpire" } else { "expire" });
    };
    let Some(ttl) = int_arg(items, 2) else {
        return wrong_arity(if ms { "pexpire" } else { "expire" });
    };
    if ttl <= 0 {
        return Value::Error(format!(
            "ERR invalid expire time in '{}' command",
            if ms { "pexpire" } else { "expire" }
        ));
    }
    let ttl_ms = if ms { ttl as u64 } else { ttl as u64 * 1000 };
    Value::Integer(store.set_expiry_relative(key, ttl_ms) as i64)
}

fn cmd_persist(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("persist");
    };
    Value::Integer(store.persist(key) as i64)
}

fn cmd_keys(items: &[Value], store: &Store) -> Value {
    let Some(pattern) = str_arg(items, 1) else {
        return wrong_arity("keys");
    };
    let keys = store.keys(pattern);
    Value::Array(keys.into_iter().map(|k| Value::Bulk(Some(k))).collect())
}

// ---- hashes ----

fn cmd_hset(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hset");
    };
    if items.len() < 4 || (items.len() - 2) % 2 != 0 {
        return wrong_arity("hset");
    }
    let mut added = 0;
    let mut i = 2;
    while i + 1 < items.len() {
        let (Some(f), Some(v)) = (str_arg(items, i), str_arg(items, i + 1)) else {
            return wrong_arity("hset");
        };
        match store.hset(key, f, v) {
            Ok(is_new) => added += is_new as i64,
            Err(e) => return store_err(e),
        }
        i += 2;
    }
    Value::Integer(added)
}

fn cmd_hmset(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hmset");
    };
    if items.len() < 4 || (items.len() - 2) % 2 != 0 {
        return wrong_arity("hmset");
    }
    let mut i = 2;
    while i + 1 < items.len() {
        let (Some(f), Some(v)) = (str_arg(items, i), str_arg(items, i + 1)) else {
            return wrong_arity("hmset");
        };
        if let Err(e) = store.hset(key, f, v) {
            return store_err(e);
        }
        i += 2;
    }
    Value::Simple("OK".to_string())
}

fn cmd_hsetnx(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(field), Some(value)) =
        (str_arg(items, 1), str_arg(items, 2), str_arg(items, 3))
    else {
        return wrong_arity("hsetnx");
    };
    match store.hget(key, field) {
        Ok(Some(_)) => Value::Integer(0),
        Ok(None) => match store.hset(key, field, value) {
            Ok(_) => Value::Integer(1),
            Err(e) => store_err(e),
        },
        Err(e) => store_err(e),
    }
}

fn cmd_hget(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(field)) = (str_arg(items, 1), str_arg(items, 2)) else {
        return wrong_arity("hget");
    };
    match store.hget(key, field) {
        Ok(Some(v)) => Value::Bulk(Some(v)),
        Ok(None) => Value::Bulk(None),
        Err(e) => store_err(e),
    }
}

fn cmd_hgetall(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hgetall");
    };
    match store.hgetall(key) {
        Ok(pairs) => Value::Array(
            pairs
                .into_iter()
                .flat_map(|(f, v)| [Value::Bulk(Some(f)), Value::Bulk(Some(v))])
                .collect(),
        ),
        Err(e) => store_err(e),
    }
}

fn cmd_hmget(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hmget");
    };
    if items.len() < 3 {
        return wrong_arity("hmget");
    }
    match store.hgetall(key) {
        Ok(pairs) => {
            let fields = all_str_args(items, 2);
            let map: std::collections::HashMap<&str, &str> =
                pairs.iter().map(|(f, v)| (f.as_str(), v.as_str())).collect();
            Value::Array(
                fields
                    .iter()
                    .map(|f| map.get(f).map(|v| Value::Bulk(Some(v.to_string()))).unwrap_or(Value::Bulk(None)))
                    .collect(),
            )
        }
        Err(e) => store_err(e),
    }
}

fn cmd_hdel(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hdel");
    };
    if items.len() < 3 {
        return wrong_arity("hdel");
    }
    let fields: Vec<String> = all_str_args(items, 2).into_iter().map(str::to_string).collect();
    match store.hdel(key, &fields) {
        Ok(n) => Value::Integer(n as i64),
        Err(e) => store_err(e),
    }
}

fn cmd_hexists(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(field)) = (str_arg(items, 1), str_arg(items, 2)) else {
        return wrong_arity("hexists");
    };
    match store.hexists(key, field) {
        Ok(b) => Value::Integer(b as i64),
        Err(e) => store_err(e),
    }
}

fn cmd_hlen(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hlen");
    };
    match store.hlen(key) {
        Ok(n) => Value::Integer(n as i64),
        Err(e) => store_err(e),
    }
}

fn cmd_hkeys(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hkeys");
    };
    match store.hkeys(key) {
        Ok(fields) => Value::Array(fields.into_iter().map(|f| Value::Bulk(Some(f))).collect()),
        Err(e) => store_err(e),
    }
}

fn cmd_hvals(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("hvals");
    };
    match store.hvals(key) {
        Ok(values) => Value::Array(values.into_iter().map(|v| Value::Bulk(Some(v))).collect()),
        Err(e) => store_err(e),
    }
}

fn cmd_hincrby(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(field), Some(delta)) =
        (str_arg(items, 1), str_arg(items, 2), int_arg(items, 3))
    else {
        return wrong_arity("hincrby");
    };
    match store.hincrby(key, field, delta) {
        Ok(n) => Value::Integer(n),
        Err(e) => store_err(e),
    }
}

// ---- lists ----

fn cmd_push(items: &[Value], store: &Store, back: bool) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity(if back { "rpush" } else { "lpush" });
    };
    if items.len() < 3 {
        return wrong_arity(if back { "rpush" } else { "lpush" });
    }
    let values: Vec<String> = all_str_args(items, 2)
        .into_iter()
        .map(str::to_string)
        .collect();
    let result = if back {
        store.rpush(key, &values)
    } else {
        store.lpush(key, &values)
    };
    match result {
        Ok(n) => Value::Integer(n as i64),
        Err(e) => store_err(e),
    }
}

fn cmd_pop(items: &[Value], store: &Store, back: bool) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity(if back { "rpop" } else { "lpop" });
    };
    let count = match items.get(2) {
        Some(_) => {
            let Some(n) = int_arg(items, 2) else {
                return wrong_arity(if back { "rpop" } else { "lpop" });
            };
            if n <= 0 {
                return Value::Array(Vec::new());
            }
            Some(n as usize)
        }
        None => None,
    };
    let result = if back {
        store.rpop(key, count)
    } else {
        store.lpop(key, count)
    };
    match result {
        Ok(values) => match count {
            Some(_) => Value::Array(values.into_iter().map(|v| Value::Bulk(Some(v))).collect()),
            None => Value::Bulk(values.into_iter().next()),
        },
        Err(e) => store_err(e),
    }
}

fn cmd_lrange(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(start), Some(stop)) =
        (str_arg(items, 1), int_arg(items, 2), int_arg(items, 3))
    else {
        return wrong_arity("lrange");
    };
    match store.lrange(key, start, stop) {
        Ok(values) => Value::Array(values.into_iter().map(|v| Value::Bulk(Some(v))).collect()),
        Err(e) => store_err(e),
    }
}

fn cmd_llen(items: &[Value], store: &Store) -> Value {
    let Some(key) = str_arg(items, 1) else {
        return wrong_arity("llen");
    };
    match store.llen(key) {
        Ok(n) => Value::Integer(n as i64),
        Err(e) => store_err(e),
    }
}

fn cmd_lindex(items: &[Value], store: &Store) -> Value {
    let (Some(key), Some(index)) = (str_arg(items, 1), int_arg(items, 2)) else {
        return wrong_arity("lindex");
    };
    match store.lindex(key, index) {
        Ok(Some(v)) => Value::Bulk(Some(v)),
        Ok(None) => Value::Bulk(None),
        Err(e) => store_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx(store: &Store) -> Context<'_> {
        static DB_PATH: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| PathBuf::from("/tmp/dump.rdb"));
        Context {
            store,
            db_path: &DB_PATH,
        }
    }

    fn cmd(args: &[&str]) -> Vec<Value> {
        args.iter()
            .map(|a| Value::Bulk(Some(a.to_string())))
            .collect()
    }

    fn run(store: &Store, args: &[&str]) -> Value {
        dispatch(&cmd(args), &ctx(store))
    }

    fn bulk(s: &str) -> Value {
        Value::Bulk(Some(s.to_string()))
    }

    #[test]
    fn set_get_del_flow() {
        let s = Store::new();
        assert_eq!(run(&s, &["SET", "foo", "bar"]), Value::Simple("OK".to_string()));
        assert_eq!(run(&s, &["GET", "foo"]), bulk("bar"));
        assert_eq!(run(&s, &["GET", "nope"]), Value::Bulk(None));
        assert_eq!(run(&s, &["DEL", "foo", "nope"]), Value::Integer(1));
        assert_eq!(run(&s, &["GET", "foo"]), Value::Bulk(None));
    }

    #[test]
    fn set_nx_xx_semantics() {
        let s = Store::new();
        assert_eq!(run(&s, &["SET", "k", "1", "NX"]), Value::Simple("OK".to_string()));
        assert_eq!(run(&s, &["SET", "k", "2", "NX"]), Value::Bulk(None));
        assert_eq!(run(&s, &["GET", "k"]), bulk("1"));
        assert_eq!(run(&s, &["SET", "k", "3", "XX"]), Value::Simple("OK".to_string()));
        assert_eq!(run(&s, &["GET", "k"]), bulk("3"));
        assert_eq!(run(&s, &["SET", "other", "x", "XX"]), Value::Bulk(None));
        assert_eq!(run(&s, &["SET", "k", "y", "NX", "XX"]), Value::Error("ERR syntax error".to_string()));
    }

    #[test]
    fn set_with_expiry_and_ttl() {
        let s = Store::new();
        assert_eq!(run(&s, &["SET", "k", "v", "EX", "10"]), Value::Simple("OK".to_string()));
        let ttl = run(&s, &["TTL", "k"]);
        assert!(matches!(ttl, Value::Integer(n) if (1..=10).contains(&n)));
        assert_eq!(run(&s, &["PTTL", "k"]), Value::Integer(10_000));
        assert_eq!(run(&s, &["TTL", "nope"]), Value::Integer(-2));
        assert_eq!(run(&s, &["EXPIRE", "nope", "5"]), Value::Integer(0));
        assert_eq!(run(&s, &["PERSIST", "k"]), Value::Integer(1));
        assert_eq!(run(&s, &["TTL", "k"]), Value::Integer(-1));
    }

    #[test]
    fn wrong_type_errors() {
        let s = Store::new();
        assert_eq!(run(&s, &["SET", "k", "str"]), Value::Simple("OK".to_string()));
        assert_eq!(run(&s, &["LPUSH", "k", "x"]), Value::Error(WRONGTYPE_ERR.to_string()));
        assert_eq!(run(&s, &["HGET", "k", "f"]), Value::Error(WRONGTYPE_ERR.to_string()));
        assert_eq!(run(&s, &["INCR", "k"]), Value::Error(NOT_INTEGER_ERR.to_string()));
    }

    #[test]
    fn counters_and_append() {
        let s = Store::new();
        assert_eq!(run(&s, &["INCR", "c"]), Value::Integer(1));
        assert_eq!(run(&s, &["INCRBY", "c", "4"]), Value::Integer(5));
        assert_eq!(run(&s, &["DECR", "c"]), Value::Integer(4));
        assert_eq!(run(&s, &["APPEND", "msg", "hello"]), Value::Integer(5));
        assert_eq!(run(&s, &["GET", "msg"]), bulk("hello"));
        assert_eq!(run(&s, &["STRLEN", "msg"]), Value::Integer(5));
        assert_eq!(run(&s, &["STRLEN", "nope"]), Value::Integer(0));
        assert_eq!(run(&s, &["APPEND", "c", "x"]), Value::Integer(2));
        assert_eq!(run(&s, &["INCR", "c"]), Value::Error(NOT_INTEGER_ERR.to_string()));
    }

    #[test]
    fn hash_commands() {
        let s = Store::new();
        assert_eq!(run(&s, &["HSET", "h", "a", "1", "b", "2"]), Value::Integer(2));
        assert_eq!(run(&s, &["HGET", "h", "a"]), bulk("1"));
        assert_eq!(run(&s, &["HEXISTS", "h", "b"]), Value::Integer(1));
        assert_eq!(run(&s, &["HLEN", "h"]), Value::Integer(2));
        assert_eq!(run(&s, &["HINCRBY", "h", "a", "10"]), Value::Integer(11));
        let mut all = run(&s, &["HGETALL", "h"]);
        if let Value::Array(items) = &mut all {
            items.sort_by(|a, b| {
                format!("{a:?}").cmp(&format!("{b:?}"))
            });
        }
        assert_eq!(
            all,
            Value::Array(vec![bulk("11"), bulk("2"), bulk("a"), bulk("b")])
        );
        assert_eq!(run(&s, &["HDEL", "h", "a"]), Value::Integer(1));
        assert_eq!(run(&s, &["HMGET", "h", "b", "zzz"]), Value::Array(vec![bulk("2"), Value::Bulk(None)]));
    }

    #[test]
    fn list_commands() {
        let s = Store::new();
        assert_eq!(run(&s, &["RPUSH", "l", "a", "b"]), Value::Integer(2));
        assert_eq!(run(&s, &["LPUSH", "l", "z"]), Value::Integer(3));
        assert_eq!(run(&s, &["LRANGE", "l", "0", "-1"]), Value::Array(vec![bulk("z"), bulk("a"), bulk("b")]));
        assert_eq!(run(&s, &["LINDEX", "l", "-1"]), bulk("b"));
        assert_eq!(run(&s, &["LPOP", "l"]), bulk("z"));
        assert_eq!(run(&s, &["LPOP", "l", "5"]), Value::Array(vec![bulk("a"), bulk("b")]));
        assert_eq!(run(&s, &["LLEN", "l"]), Value::Integer(0));
    }

    #[test]
    fn keyspace_commands() {
        let s = Store::new();
        run(&s, &["MSET", "user:1", "a", "user:2", "b"]);
        assert_eq!(run(&s, &["EXISTS", "user:1", "user:2", "nope"]), Value::Integer(2));
        assert_eq!(run(&s, &["TYPE", "user:1"]), Value::Simple("string".to_string()));
        let mut keys = run(&s, &["KEYS", "user:*"]);
        if let Value::Array(items) = &mut keys {
            items.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        }
        assert_eq!(keys, Value::Array(vec![bulk("user:1"), bulk("user:2")]));
        assert_eq!(run(&s, &["DBSIZE"]), Value::Integer(2));
        assert_eq!(run(&s, &["FLUSHALL"]), Value::Simple("OK".to_string()));
        assert_eq!(run(&s, &["DBSIZE"]), Value::Integer(0));
    }

    #[test]
    fn connection_commands() {
        let s = Store::new();
        assert_eq!(run(&s, &["PING"]), Value::Simple("PONG".to_string()));
        assert_eq!(run(&s, &["PING", "hey"]), bulk("hey"));
        assert_eq!(run(&s, &["ECHO", "hi"]), bulk("hi"));
        assert_eq!(run(&s, &["SELECT", "0"]), Value::Simple("OK".to_string()));
        assert_eq!(run(&s, &["SELECT", "1"]), Value::Error("ERR DB index is out of range".to_string()));
        assert_eq!(run(&s, &["BOGUS"]), Value::Error("ERR unknown command 'BOGUS'".to_string()));
    }

    #[test]
    fn arity_errors() {
        let s = Store::new();
        assert!(matches!(run(&s, &["GET"]), Value::Error(_)));
        assert!(matches!(run(&s, &["SET", "k"]), Value::Error(_)));
        assert!(matches!(run(&s, &["HSET", "h", "f"]), Value::Error(_)));
        assert!(matches!(run(&s, &["LRANGE", "l", "0"]), Value::Error(_)));
    }
}
