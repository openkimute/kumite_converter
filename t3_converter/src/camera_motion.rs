//! camera_motion.py: scripted camera motions (moveset camera_keyframes, header +0x24): a port of the decoder
//! 0x80038CE4.
//!
//! A camera selector entry whose value has bit 31 set points at moveset +0x24 + (value & 0x7FFFFFFF): a u16 prefix,
//! then a motion with a s16 header (frames = h & 0x3FF, channels = h >> 10) and per channel 1..16 segments
//! (word: segments - 1 << 12 | type << 10 | length). Types: 0 constant, 1 linear {v0, slope} (slope / 32 per frame,
//! / 1 under 10 frames), 2 keyframe curve (the character decoder's curve 0x800387C8 with k = 4), 3 raw per-frame
//! words. Odd channels are stored time-reversed. Ogre's scripted camera (id 0x4B, the True Ogre scene) is one.

use crate::anim::{self, Tables, s16, u16at};
use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

/// A segment: (type, length, data offset).
type Seg = (i64, i64, i64);

/// Header at p: (frames, channels, segments per channel, end offset).
fn walk(m: &[u8], mut p: i64) -> Result<(i64, i64, Vec<Vec<Seg>>, i64)> {
    let h = u16at(m, p)?;
    p += 2;
    let (n, c) = (h & 0x3FF, h >> 10);
    let mut chans = Vec::new();
    for _ in 0..c {
        let w = u16at(m, p)?;
        let mut segs = Vec::new();
        for _ in 0..((w >> 12) & 0xF) + 1 {
            let w = u16at(m, p)?;
            p += 2;
            let (typ, ln) = ((w >> 10) & 3, w & 0x3FF);
            segs.push((typ, ln, p));
            p += match typ {
                0 => 2,
                1 => 4,
                _ => 2 * ln,
            };
            if p > m.len() as i64 {
                bail!("past the end");
            }
        }
        chans.push(segs);
    }
    Ok((n, c, chans, p))
}

fn value(t_: &Tables, m: &[u8], n: i64, segs: &[Seg], t: i64) -> Result<Option<i64>> {
    let mut s1 = 0;
    for &(typ, ln, p) in segs {
        match typ {
            0 => {
                if t < s1 + ln {
                    return Ok(Some(s16(u16at(m, p)?)));
                }
                s1 += ln;
            }
            1 => {
                if t < s1 + ln {
                    let prod = s16(u16at(m, p + 2)?) * (t - s1);
                    // int(prod / d): exact in a double, truncated toward zero
                    let q = prod / if n < 10 { 1 } else { 32 };
                    return Ok(Some(s16(q + s16(u16at(m, p)?))));
                }
                s1 += ln;
            }
            2 => {
                // the curve's len key words follow the segment word
                let a3 = s1;
                for a in 1..ln {
                    s1 = s1 + 1 + (u16at(m, p + 2 * a)? & 0x1F);
                }
                if s1 >= t {
                    return Ok(Some(s16(anim::curve(t_, m, p, t - a3, s1 - a3 + 1, 4)?)));
                }
                s1 += 1;
            }
            _ => {
                if t < s1 + ln {
                    return Ok(Some(s16(u16at(m, p + 2 * (t - s1))?)));
                }
                s1 += ln;
            }
        }
    }
    Ok(None)
}

/// The motion at keyframe offset `off` (its u16 prefix): {offset, frames, channels, values[frame][channel]}.
pub fn decode(t_: &Tables, m: &[u8], off: usize) -> Result<Value> {
    let (n, c, chans, _) = walk(m, off as i64 + 2)?;
    if !(1 <= n && (1..=16).contains(&c)) {
        bail!("not a camera motion");
    }
    let mut values = Vec::new();
    for f in 0..n {
        let mut row = Vec::new();
        for (ch, segs) in chans.iter().enumerate() {
            let v = value(t_, m, n, segs, if ch & 1 != 0 { n - (f + 1) } else { f })?;
            row.push(v.unwrap_or(0));
        }
        values.push(row);
    }
    Ok(json!({"offset": off, "frames": n, "channels": c, "values": values}))
}

/// Every camera motion the selectors (dicts with a `value`) reference, keyed by selector index (as a string).
/// `exe`: the EXE bytes; `h`: the moveset's header words.
pub fn motions(exe: &[u8], data: &[u8], h: &[u32; 16], selectors: &[Value]) -> Result<Map<String, Value>> {
    let t_ = Tables::new(exe)?;
    let kf = crate::disc::py_slice(data, h[9] as usize, h[10] as usize);
    let mut out = Map::new();
    for (k, s) in selectors.iter().enumerate() {
        let v = s["value"].as_i64().unwrap_or(0) & 0xFFFFFFFF;
        if v & 0x80000000 == 0 {
            continue;
        }
        let off = (v & 0x7FFFFFFF) as usize;
        if off + 4 > kf.len() {
            continue;
        }
        // the Python skips a motion that fails to decode (ValueError, struct.error, IndexError)
        if let Ok(d) = decode(&t_, kf, off) {
            out.insert(k.to_string(), d);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    /// T3_BIN=<the disc image>, T3_GAMEDATA=<export_gamedata.py's gamedata/ dir>: each character moveset's
    /// camera_motions as the Python wrote them.
    #[test]
    fn motions_as_python() -> Result<()> {
        let (Ok(bin), Ok(dir)) = (std::env::var("T3_BIN"), std::env::var("T3_GAMEDATA")) else {
            return Ok(());
        };
        let game = crate::disc::Game::load(&conv_io::StdDisc(bin.into()))?;
        let mut n = 0;
        for id in 0..21 {
            let text = std::fs::read_to_string(format!("{dir}/movesets/moveset_{id:02}.json"))?;
            let j: Value = serde_json::from_str(&text)?;
            let r = j["source_records"]["character_records"][0].as_u64().context("no record")? as usize;
            let data = &game.rec[r];
            let h: [u32; 16] = std::array::from_fn(|k| crate::disc::u32le(data, 4 * k));
            let sel = j["camera_selectors"].as_array().context("no selectors")?;
            let got = motions(&game.exe, data, &h, sel)?;
            assert!(Value::Object(got) == j["camera_motions"], "moveset {id}");
            n += j["camera_motions"].as_object().map_or(0, |m| m.len());
        }
        assert!(n > 400, "{n} motions");
        Ok(())
    }
}
