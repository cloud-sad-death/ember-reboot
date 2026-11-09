//! Background keyword index: lowercase word → row ids, per session (`index.zst`).
use crate::store::SessionReader;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const MAX_WORDS: usize = 1_000_000;

#[derive(serde::Serialize, serde::Deserialize)]
struct Index {
    partial: bool,
    map: BTreeMap<String, Vec<u64>>,
}

fn tokens(b: &[u8], mut f: impl FnMut(&[u8])) {
    let mut s = None;
    for (i, &c) in b.iter().enumerate() {
        let w = c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
        match (w, s) {
            (true, None) => s = Some(i),
            (false, Some(a)) => {
                if i - a >= 3 && i - a <= 32 && b[a..i].iter().any(|c| !c.is_ascii_digit()) {
                    f(&b[a..i])
                }
                s = None
            }
            _ => {}
        }
    }
    if let Some(a) = s {
        if b.len() - a >= 3 && b.len() - a <= 32 && b[a..].iter().any(|c| !c.is_ascii_digit()) {
            f(&b[a..])
        }
    }
}

/// Builds the index for one session. `max_bytes_per_row` limits streaming rows only (0 = everything);
/// scanning search always covers 100% of saved output.
pub fn build(dir: &Path, max_bytes_per_row: u64) -> Result<usize> {
    let s = SessionReader::open(dir)?;
    let mut map: HashMap<String, Vec<u64>> = HashMap::new();
    let mut partial = false;
    for r in s.visible_rows() {
        let (a, b) = (r.text.0, s.row_end(r).1);
        let b = if max_bytes_per_row > 0 && r.mode == "streaming" { b.min(a + max_bytes_per_row) } else { b };
        let mut add = |t: &[u8]| {
            let k = String::from_utf8_lossy(t).to_lowercase();
            if map.len() >= MAX_WORDS && !map.contains_key(&k) {
                partial = true;
                return;
            }
            let v = map.entry(k).or_default();
            if v.last() != Some(&r.id) {
                v.push(r.id)
            }
        };
        tokens(r.header.as_bytes(), &mut add);
        let mut off = a;
        while off < b {
            let end = (off + (8 << 20)).min(b);
            let chunk = s.text.read(off, end)?;
            tokens(&chunk, &mut add);
            off = end;
        }
    }
    let n = map.len();
    let idx = Index { partial, map: map.into_iter().collect() };
    let json = serde_json::to_vec(&idx)?;
    std::fs::write(dir.join("index.zst"), zstd::bulk::compress(&json, 3)?)?;
    Ok(n)
}

/// Row ids containing every word of `q`, or None when the session has no index yet.
pub fn candidates(dir: &Path, q: &str) -> Option<Vec<u64>> {
    let comp = std::fs::read(dir.join("index.zst")).ok()?;
    let json = zstd::stream::decode_all(&comp[..]).ok()?;
    let idx: Index = serde_json::from_slice(&json).ok()?;
    let map = idx.map;
    let mut out: Option<Vec<u64>> = None;
    let mut words = vec![];
    tokens(q.as_bytes(), |t| words.push(String::from_utf8_lossy(t).to_lowercase()));
    for w in words {
        let ids = match map.get(&w) {
            Some(ids) => ids.clone(),
            // a capped index cannot prove absence: fall back to scanning this session
            None if idx.partial => return None,
            None => vec![],
        };
        out = Some(match out {
            None => ids,
            Some(prev) => prev.into_iter().filter(|x| ids.binary_search(x).is_ok()).collect(),
        });
    }
    out
}
