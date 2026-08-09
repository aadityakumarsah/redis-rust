use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::snapshot;

pub const WRONGTYPE_ERR: &str = "WRONGTYPE Operation against a key holding the wrong kind of value";
pub const NOT_INTEGER_ERR: &str = "ERR value is not an integer or out of range";
pub const OVERFLOW_ERR: &str = "ERR increment or decrement would overflow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    String(String),
    Hash(HashMap<String, String>),
    List(VecDeque<String>),
}

impl Data {
    pub fn type_name(&self) -> &'static str {
        match self {
            Data::String(_) => "string",
            Data::Hash(_) => "hash",
            Data::List(_) => "list",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub data: Data,
    pub expires_at: Option<u128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    WrongType,
    NotAnInteger,
    Overflow,
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub struct Store {
    inner: Mutex<HashMap<String, Entry>>,
    dirty: AtomicBool,
    save_lock: Mutex<()>,
}

impl Store {
    pub fn new() -> Self {
        Store {
            inner: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            save_lock: Mutex::new(()),
        }
    }

    /// Load a snapshot from disk, falling back to an empty store.
    pub fn load_from(path: &Path) -> Self {
        let store = Self::new();
        match fs::read(path) {
            Ok(bytes) => match snapshot::deserialize(&bytes) {
                Ok(entries) => {
                    let n = entries.len();
                    *store.inner.lock().unwrap() = entries;
                    eprintln!("* Loaded {n} keys from {}", path.display());
                }
                Err(e) => eprintln!("* Warning: ignoring snapshot {}: {e}", path.display()),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("* Warning: cannot read {}: {e}", path.display()),
        }
        store
    }

    /// Write the full dataset to `path` atomically (tmp file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let _guard = self.save_lock.lock().unwrap();
        let entries = self.inner.lock().unwrap().clone();
        let bytes = snapshot::serialize(&entries);
        let tmp = path.with_extension("rdb.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)?;
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }

    pub fn save_if_dirty(&self, path: &Path) {
        if self.dirty.load(Ordering::Relaxed) {
            if let Err(e) = self.save(path) {
                eprintln!("* Autosave failed: {e}");
            }
        }
    }

    pub fn spawn_janitor(self: &Arc<Self>, every: Duration) {
        let store = Arc::clone(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(every);
            store.sweep_expired();
        });
    }

    pub fn spawn_autosave(self: &Arc<Self>, db_path: std::path::PathBuf, every: Duration) {
        let store = Arc::clone(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(every);
            store.save_if_dirty(&db_path);
        });
    }

    // ---- keyspace ----

    pub fn get(&self, key: &str) -> Option<Data> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        inner.get(key).map(|e| e.data.clone())
    }

    pub fn set(&self, key: &str, data: Data, ttl_ms: Option<u64>) {
        let mut inner = self.inner.lock().unwrap();
        inner.insert(
            key.to_string(),
            Entry {
                data,
                expires_at: ttl_ms.map(|ms| now_ms() + ms as u128),
            },
        );
        self.mark_dirty();
    }

    pub fn set_if_absent(&self, key: &str, data: Data) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        if inner.contains_key(key) {
            return false;
        }
        inner.insert(
            key.to_string(),
            Entry {
                data,
                expires_at: None,
            },
        );
        self.mark_dirty();
        true
    }

    pub fn remove(&self, key: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        if inner.remove(key).is_some() {
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub fn remove_many(&self, keys: &[&str]) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        let mut removed = 0;
        for key in keys {
            self.expire_if_needed(&mut inner, key, now);
            if inner.remove(*key).is_some() {
                removed += 1;
            }
        }
        if removed > 0 {
            self.mark_dirty();
        }
        removed
    }

    pub fn exists(&self, key: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        inner.contains_key(key)
    }

    pub fn type_of(&self, key: &str) -> Option<&'static str> {
        self.get(key).map(|d| d.type_name())
    }

    pub fn ttl_ms(&self, key: &str) -> Option<i64> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        match inner.get(key) {
            None => None,
            Some(e) => match e.expires_at {
                Some(at) => Some(at as i64 - now as i64),
                None => Some(-1),
            },
        }
    }

    pub fn set_expiry_relative(&self, key: &str, ttl_ms: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        if let Some(e) = inner.get_mut(key) {
            e.expires_at = Some(now + ttl_ms as u128);
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub fn persist(&self, key: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        if let Some(e) = inner.get_mut(key) {
            e.expires_at = None;
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub fn keys(&self, pattern: &str) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        self.purge_expired(&mut inner);
        inner
            .keys()
            .filter(|k| glob_match(pattern, k))
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        self.purge_expired(&mut inner);
        inner.len()
    }

    pub fn flush(&self) {
        self.inner.lock().unwrap().clear();
        self.mark_dirty();
    }

    pub fn sweep_expired(&self) {
        let mut inner = self.inner.lock().unwrap();
        self.purge_expired(&mut inner);
    }

    // ---- strings ----

    pub fn incrby(&self, key: &str, delta: i64) -> Result<i64, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        let current: i64 = match inner.get(key) {
            None => 0,
            Some(e) => match &e.data {
                Data::String(s) => s.parse().map_err(|_| StoreError::NotAnInteger)?,
                _ => return Err(StoreError::WrongType),
            },
        };
        let new = current.checked_add(delta).ok_or(StoreError::Overflow)?;
        let expires_at = inner.get(key).and_then(|e| e.expires_at);
        inner.insert(
            key.to_string(),
            Entry {
                data: Data::String(new.to_string()),
                expires_at,
            },
        );
        self.mark_dirty();
        Ok(new)
    }

    pub fn append(&self, key: &str, suffix: &str) -> Result<usize, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        let entry = match inner.get(key) {
            None => Entry {
                data: Data::String(String::new()),
                expires_at: None,
            },
            Some(e) => match &e.data {
                Data::String(_) => e.clone(),
                _ => return Err(StoreError::WrongType),
            },
        };
        let mut value = match entry.data {
            Data::String(s) => s,
            _ => unreachable!(),
        };
        value.push_str(suffix);
        let len = value.len();
        inner.insert(
            key.to_string(),
            Entry {
                data: Data::String(value),
                expires_at: entry.expires_at,
            },
        );
        self.mark_dirty();
        Ok(len)
    }

    // ---- hashes ----

    pub fn hset(&self, key: &str, field: &str, value: &str) -> Result<bool, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        if inner.get(key).is_some_and(|e| !matches!(e.data, Data::Hash(_))) {
            return Err(StoreError::WrongType);
        }
        if !inner.contains_key(key) {
            inner.insert(
                key.to_string(),
                Entry {
                    data: Data::Hash(HashMap::new()),
                    expires_at: None,
                },
            );
        }
        let Entry { data, .. } = inner.get_mut(key).unwrap();
        let Data::Hash(h) = data else {
            return Err(StoreError::WrongType);
        };
        let is_new = h.insert(field.to_string(), value.to_string()).is_none();
        self.mark_dirty();
        Ok(is_new)
    }

    pub fn hget(&self, key: &str, field: &str) -> Result<Option<String>, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        match inner.get(key) {
            None => Ok(None),
            Some(Entry { data: Data::Hash(h), .. }) => Ok(h.get(field).cloned()),
            Some(_) => Err(StoreError::WrongType),
        }
    }

    pub fn hdel(&self, key: &str, fields: &[String]) -> Result<usize, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        match inner.get_mut(key) {
            None => Ok(0),
            Some(Entry { data: Data::Hash(h), .. }) => {
                let mut removed = 0;
                for f in fields {
                    if h.remove(f).is_some() {
                        removed += 1;
                    }
                }
                self.mark_dirty();
                Ok(removed)
            }
            Some(_) => Err(StoreError::WrongType),
        }
    }

    pub fn hexists(&self, key: &str, field: &str) -> Result<bool, StoreError> {
        self.hget(key, field).map(|v| v.is_some())
    }

    pub fn hgetall(&self, key: &str) -> Result<Vec<(String, String)>, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        match inner.get(key) {
            None => Ok(Vec::new()),
            Some(Entry { data: Data::Hash(h), .. }) => Ok(h
                .iter()
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect()),
            Some(_) => Err(StoreError::WrongType),
        }
    }

    pub fn hkeys(&self, key: &str) -> Result<Vec<String>, StoreError> {
        self.hgetall(key).map(|pairs| pairs.into_iter().map(|(f, _)| f).collect())
    }

    pub fn hvals(&self, key: &str) -> Result<Vec<String>, StoreError> {
        self.hgetall(key).map(|pairs| pairs.into_iter().map(|(_, v)| v).collect())
    }

    pub fn hlen(&self, key: &str) -> Result<usize, StoreError> {
        self.hgetall(key).map(|pairs| pairs.len())
    }

    pub fn hincrby(&self, key: &str, field: &str, delta: i64) -> Result<i64, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        let entry = match inner.get(key) {
            None => Entry {
                data: Data::Hash(HashMap::new()),
                expires_at: None,
            },
            Some(e) => match &e.data {
                Data::Hash(_) => e.clone(),
                _ => return Err(StoreError::WrongType),
            },
        };
        let Data::Hash(mut h) = entry.data else {
            unreachable!()
        };
        let current: i64 = match h.get(field) {
            Some(s) => s.parse().map_err(|_| StoreError::NotAnInteger)?,
            None => 0,
        };
        let new = current.checked_add(delta).ok_or(StoreError::Overflow)?;
        h.insert(field.to_string(), new.to_string());
        inner.insert(
            key.to_string(),
            Entry {
                data: Data::Hash(h),
                expires_at: entry.expires_at,
            },
        );
        self.mark_dirty();
        Ok(new)
    }

    // ---- lists ----

    fn push(&self, key: &str, values: &[String], front: bool) -> Result<usize, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        if inner.get(key).is_some_and(|e| !matches!(e.data, Data::List(_))) {
            return Err(StoreError::WrongType);
        }
        if !inner.contains_key(key) {
            inner.insert(
                key.to_string(),
                Entry {
                    data: Data::List(VecDeque::new()),
                    expires_at: None,
                },
            );
        }
        let Entry { data, .. } = inner.get_mut(key).unwrap();
        let Data::List(list) = data else {
            return Err(StoreError::WrongType);
        };
        for value in values {
            if front {
                list.push_front(value.clone());
            } else {
                list.push_back(value.clone());
            }
        }
        self.mark_dirty();
        Ok(list.len())
    }

    pub fn lpush(&self, key: &str, values: &[String]) -> Result<usize, StoreError> {
        self.push(key, values, true)
    }

    pub fn rpush(&self, key: &str, values: &[String]) -> Result<usize, StoreError> {
        self.push(key, values, false)
    }

    fn pop(&self, key: &str, front: bool, count: Option<usize>) -> Result<Vec<String>, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        let Some(entry) = inner.get_mut(key) else {
            return Ok(Vec::new());
        };
        let Data::List(list) = &mut entry.data else {
            return Err(StoreError::WrongType);
        };
        let take = count.unwrap_or(1).min(list.len());
        let mut out = Vec::with_capacity(take);
        for _ in 0..take {
            let value = if front {
                list.pop_front()
            } else {
                list.pop_back()
            };
            if let Some(v) = value {
                out.push(v);
            }
        }
        self.mark_dirty();
        Ok(out)
    }

    pub fn lpop(&self, key: &str, count: Option<usize>) -> Result<Vec<String>, StoreError> {
        self.pop(key, true, count)
    }

    pub fn rpop(&self, key: &str, count: Option<usize>) -> Result<Vec<String>, StoreError> {
        self.pop(key, false, count)
    }

    pub fn lrange(&self, key: &str, start: i64, stop: i64) -> Result<Vec<String>, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);

        match inner.get(key) {
            None => Ok(Vec::new()),
            Some(Entry { data: Data::List(list), .. }) => {
                let len = list.len() as i64;
                let start = if start < 0 { len + start } else { start }.max(0);
                let stop = if stop < 0 { len + stop } else { stop }.min(len - 1);
                if start > stop || start >= len {
                    return Ok(Vec::new());
                }
                Ok(list
                    .iter()
                    .skip(start as usize)
                    .take((stop - start + 1) as usize)
                    .cloned()
                    .collect())
            }
            Some(_) => Err(StoreError::WrongType),
        }
    }

    pub fn llen(&self, key: &str) -> Result<usize, StoreError> {
        self.lrange(key, 0, -1).map(|v| v.len())
    }

    pub fn lindex(&self, key: &str, index: i64) -> Result<Option<String>, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_ms();
        self.expire_if_needed(&mut inner, key, now);
        match inner.get(key) {
            None => Ok(None),
            Some(Entry { data: Data::List(list), .. }) => {
                let len = list.len() as i64;
                let idx = if index < 0 { len + index } else { index };
                if idx < 0 || idx >= len {
                    Ok(None)
                } else {
                    Ok(list.get(idx as usize).cloned())
                }
            }
            Some(_) => Err(StoreError::WrongType),
        }
    }

    // ---- internal ----

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn expire_if_needed(&self, inner: &mut HashMap<String, Entry>, key: &str, now: u128) {
        let expired = inner
            .get(key)
            .is_some_and(|e| e.expires_at.is_some_and(|t| t <= now));
        if expired {
            inner.remove(key);
            self.mark_dirty();
        }
    }

    fn purge_expired(&self, inner: &mut HashMap<String, Entry>) {
        let now = now_ms();
        let before = inner.len();
        inner.retain(|_, e| e.expires_at.map_or(true, |t| t > now));
        if inner.len() != before {
            self.mark_dirty();
        }
    }
}

/// Glob matching supporting `*` (any sequence) and `?` (any single char).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_pi, mut star_ti) = (usize::MAX, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            star_ti += 1;
            ti = star_ti;
            pi = star_pi + 1;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::new()
    }

    #[test]
    fn set_get_roundtrip() {
        let s = store();
        s.set("foo", Data::String("bar".to_string()), None);
        assert_eq!(s.get("foo"), Some(Data::String("bar".to_string())));
        assert_eq!(s.get("missing"), None);
        assert!(s.exists("foo"));
        assert!(!s.exists("missing"));
    }

    #[test]
    fn ttl_expires_key() {
        let s = store();
        s.set("k", Data::String("v".to_string()), Some(50));
        assert!(s.exists("k"));
        std::thread::sleep(Duration::from_millis(90));
        assert!(!s.exists("k"));
        assert_eq!(s.get("k"), None);
    }

    #[test]
    fn ttl_reports_remaining_and_negative_one() {
        let s = store();
        assert_eq!(s.ttl_ms("missing"), None);
        s.set("k", Data::String("v".to_string()), None);
        assert_eq!(s.ttl_ms("k"), Some(-1));
        s.set("k2", Data::String("v".to_string()), Some(10_000));
        assert!(s.ttl_ms("k2").unwrap() > 0);
    }

    #[test]
    fn incrby_creates_and_errors() {
        let s = store();
        assert_eq!(s.incrby("counter", 5), Ok(5));
        assert_eq!(s.incrby("counter", -2), Ok(3));
        s.set("bad", Data::String("abc".to_string()), None);
        assert_eq!(s.incrby("bad", 1), Err(StoreError::NotAnInteger));
        s.set("h", Data::Hash(HashMap::new()), None);
        assert_eq!(s.incrby("h", 1), Err(StoreError::WrongType));
    }

    #[test]
    fn append_creates_and_grows() {
        let s = store();
        assert_eq!(s.append("k", "hello"), Ok(5));
        assert_eq!(s.append("k", " world"), Ok(11));
        assert_eq!(s.get("k"), Some(Data::String("hello world".to_string())));
    }

    #[test]
    fn hash_operations() {
        let s = store();
        assert!(s.hset("h", "f1", "v1").unwrap());
        assert!(!s.hset("h", "f1", "v2").unwrap());
        assert_eq!(s.hget("h", "f1").unwrap(), Some("v2".to_string()));
        assert_eq!(s.hget("h", "nope").unwrap(), None);
        assert_eq!(s.hlen("h").unwrap(), 1);
        assert!(s.hexists("h", "f1").unwrap());
        assert_eq!(s.hdel("h", &["f1".to_string()]).unwrap(), 1);
        assert_eq!(s.hlen("h").unwrap(), 0);
        s.set("str", Data::String("x".to_string()), None);
        assert_eq!(s.hget("str", "f").unwrap_err(), StoreError::WrongType);
    }

    #[test]
    fn hincrby_works() {
        let s = store();
        assert_eq!(s.hincrby("h", "n", 10), Ok(10));
        assert_eq!(s.hincrby("h", "n", -3), Ok(7));
    }

    #[test]
    fn list_operations() {
        let s = store();
        assert_eq!(s.rpush("l", &["a".into(), "b".into(), "c".into()]).unwrap(), 3);
        assert_eq!(s.lpush("l", &["z".into()]).unwrap(), 4);
        assert_eq!(s.lrange("l", 0, -1).unwrap(), vec!["z", "a", "b", "c"]);
        assert_eq!(s.lrange("l", 1, 2).unwrap(), vec!["a", "b"]);
        assert_eq!(s.lrange("l", -2, -1).unwrap(), vec!["b", "c"]);
        assert_eq!(s.llen("l").unwrap(), 4);
        assert_eq!(s.lindex("l", 0).unwrap(), Some("z".to_string()));
        assert_eq!(s.lindex("l", -1).unwrap(), Some("c".to_string()));
        assert_eq!(s.lindex("l", 99).unwrap(), None);
        assert_eq!(s.lpop("l", None).unwrap(), vec!["z"]);
        assert_eq!(s.lpop("l", Some(2)).unwrap(), vec!["a", "b"]);
        assert_eq!(s.rpop("l", None).unwrap(), vec!["c"]);
        assert_eq!(s.lpop("l", None).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn glob_matching() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("foo?bar", "fooxbar"));
        assert!(!glob_match("foo?bar", "foobar"));
        assert!(!glob_match("foo?bar", "fooxxbar"));
        assert!(glob_match("", ""));
        assert!(glob_match("a*", "a"));
        assert!(!glob_match("a*b", "ac"));
        assert!(glob_match("h?llo*", "hello world"));
    }

    #[test]
    fn keys_matches_patterns() {
        let s = store();
        s.set("user:1", Data::String("a".to_string()), None);
        s.set("user:2", Data::String("b".to_string()), None);
        s.set("post:1", Data::String("c".to_string()), None);
        let mut matched = s.keys("user:*");
        matched.sort();
        assert_eq!(matched, vec!["user:1", "user:2"]);
        let mut all = s.keys("*");
        all.sort();
        assert_eq!(all, vec!["post:1", "user:1", "user:2"]);
    }
}
