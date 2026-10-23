//! embu.py: the attract embu's data (overlay record 0, loaded at 0x800B8D58; model archives records 1-3): shared by
//! export_gamedata (attract.json) and export_gltf (the embu-only models). Attract research §2.
//!
//! - Three cast variants N = 0..2. Script N: 10-byte entries {s16 frame, cmd, a, b, c} at 0x800B8E3C + 450 N, ended
//!   by cmd -1. Model swap (cmd 0 / 1, a = model index), play (cmd 2 / 3, a = alias, b..c = frames), fade (5).
//! - A slot's model index (the game's model index: record = 71 + 4 * index) maps through the byte table
//!   0x800B9384 + 45 N to a pair p of the archive record 1 + N: mesh = entry 2p, texture TIMs = entry 2p + 1.
//!   Most pairs are byte copies of the normal model; the others are embu-only versions.

use crate::disc::{py_slice, u32le};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};

pub const R0_BASE: usize = 0x800B8D58;
pub const SCRIPTS: usize = 0x800B8E3C;
pub const PAIRS: usize = 0x800B9384;
/// the variants' first cast (P1, P2) [0x800D3864: branches on the variant, written as constants]
pub const INITIAL: [(usize, usize); 3] = [(12, 0), (13, 1), (23, 30)];

pub fn archive(data: &[u8]) -> Result<Vec<&[u8]>> {
    let n = data.get(..4).map(|_| u32le(data, 0)).context("an archive record is too short")? as usize;
    (0..n)
        .map(|i| {
            let e = data.get(4 + 8 * i..12 + 8 * i).context("an archive's table runs past it")?;
            let (o, z) = (u32le(e, 0) as usize, u32le(e, 4) as usize);
            Ok(py_slice(data, o, o + z))
        })
        .collect()
}

/// Per variant: the script's entries {frame, cmd, a, b, c}, the ending cmd -1 entry included.
pub fn scripts(r0: &[u8]) -> Result<Vec<Vec<[i16; 5]>>> {
    let mut out = Vec::new();
    for n in 0..3 {
        let (mut a, mut s) = (SCRIPTS + 450 * n - R0_BASE, Vec::new());
        loop {
            let b = r0.get(a..a + 10).context("an embu script runs past record 0")?;
            let e: [i16; 5] = std::array::from_fn(|k| i16::from_le_bytes([b[2 * k], b[2 * k + 1]]));
            s.push(e);
            a += 10;
            if e[1] == -1 {
                break;
            }
        }
        out.push(s);
    }
    Ok(out)
}

/// Per variant: model index -> pair (255 = not in that variant's archive).
pub fn pair_tables(r0: &[u8]) -> Vec<BTreeMap<usize, usize>> {
    (0..3)
        .map(|n| {
            let a = PAIRS - R0_BASE + 45 * n;
            py_slice(r0, a, a + 45)
                .iter()
                .enumerate()
                .filter(|&(_, &p)| p != 255)
                .map(|(m, &p)| (m, p as usize))
                .collect()
        })
        .collect()
}

pub fn model_record(index: usize) -> usize {
    71 + 4 * index
}

/// The model indices variant n shows: its first cast and every swap.
pub fn used(r0: &[u8], n: usize) -> Result<Vec<i64>> {
    let mut s: BTreeSet<i64> = [INITIAL[n].0 as i64, INITIAL[n].1 as i64].into();
    s.extend(scripts(r0)?[n].iter().filter(|e| e[1] == 0 || e[1] == 1).map(|e| e[2] as i64));
    Ok(s.into_iter().collect())
}

/// Per variant: model index -> (pair, normal model record, true when the pair's mesh is a byte copy of the normal
/// model's; its texture entry holds that model's TIMs too, so the normal glb serves).
/// (pair, normal model record, same)
pub type Variant = BTreeMap<usize, (usize, usize, bool)>;

pub fn models(rec: &[Vec<u8>]) -> Result<Vec<Variant>> {
    let r0 = rec.first().context("no record 0")?;
    let mut out = Vec::new();
    for (n, table) in pair_tables(r0).iter().enumerate() {
        let arc = archive(rec.get(1 + n).context("no embu archive record")?)?;
        let mut v = BTreeMap::new();
        for m in used(r0, n)? {
            // a negative index never matches a pair
            let Some(&p) = usize::try_from(m).ok().and_then(|m| table.get(&m)) else {
                continue;
            };
            if 2 * p + 1 >= arc.len() {
                continue;
            }
            let (m, normal) = (m as usize, model_record(m as usize));
            v.insert(m, (p, normal, rec.get(normal).is_some_and(|r| arc[2 * p] == r.as_slice())));
        }
        out.push(v);
    }
    Ok(out)
}

pub fn glb_name(n: usize, p: usize) -> String {
    format!("embu_{n}_{p:02}.glb")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// T3_BIN=<the disc image>, T3_GAMEDATA=<export_gamedata.py's gamedata/ dir>: the variants' scripts and models
    /// as attract.json has them.
    #[test]
    fn variants_as_python() -> Result<()> {
        let (Ok(bin), Ok(dir)) = (std::env::var("T3_BIN"), std::env::var("T3_GAMEDATA")) else {
            return Ok(());
        };
        let game = crate::disc::Game::load(&conv_io::StdDisc(bin.into()))?;
        let j: Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/attract.json"))?)?;
        for (n, (script, models)) in scripts(&game.rec[0])?.into_iter().zip(models(&game.rec)?).enumerate() {
            let v = &j["embu"]["variants"][n];
            assert_eq!(v["script"], json!(script));
            let m: serde_json::Map<String, Value> = models
                .iter()
                .map(|(m, &(p, normal, same))| {
                    (m.to_string(), json!(if same { format!("model_{normal:03}.glb") } else { glb_name(n, p) }))
                })
                .collect();
            assert_eq!(v["models"], Value::Object(m));
        }
        Ok(())
    }
}
