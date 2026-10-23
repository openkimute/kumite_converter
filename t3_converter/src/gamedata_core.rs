//! gamedata_core.py: shared helpers for export_gamedata: EXE/BNS access, compact JSON writer, input notation.
//!
//! All addresses are PS1 RAM addresses; the EXE loads at 0x80010000 (file offset = addr - 0x80010000 + 0x800).
//! Addresses and read values are i64 (Python ints); a read outside the data is an error (Python's struct.error).

use crate::pyjson::{self, Style};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub const EXE_BASE: i64 = 0x80010000 - 0x800;

/// Read-only view of SLUS_004.02 (or, with `base`, of an overlay record loaded at that address) by RAM address.
#[derive(Clone, Copy)]
pub struct Exe<'a> {
    pub d: &'a [u8],
    pub base: i64,
}

impl<'a> Exe<'a> {
    pub fn new(d: &'a [u8]) -> Exe<'a> {
        Exe { d, base: EXE_BASE }
    }
    pub fn with_base(d: &'a [u8], base: i64) -> Exe<'a> {
        Exe { d, base }
    }
    pub fn off(&self, a: i64) -> i64 {
        a - self.base
    }
    /// `d[o:o + n]` (clamped; empty before the start)
    pub fn bytes(&self, a: i64, n: usize) -> &'a [u8] {
        usize::try_from(a - self.base).map_or(&[], |o| crate::disc::py_slice(self.d, o, o.saturating_add(n)))
    }
    fn get<const N: usize>(&self, a: i64) -> Result<[u8; N]> {
        usize::try_from(a - self.base)
            .ok()
            .and_then(|o| self.d.get(o..o.checked_add(N)?))
            .and_then(|b| b.try_into().ok())
            .with_context(|| format!("address 0x{a:X} is outside the data"))
    }
    pub fn u8(&self, a: i64) -> Result<i64> {
        Ok(self.get::<1>(a)?[0] as i64)
    }
    pub fn s8(&self, a: i64) -> Result<i64> {
        Ok(self.get::<1>(a)?[0] as i8 as i64)
    }
    pub fn u16(&self, a: i64) -> Result<i64> {
        Ok(u16::from_le_bytes(self.get(a)?) as i64)
    }
    pub fn s16(&self, a: i64) -> Result<i64> {
        Ok(i16::from_le_bytes(self.get(a)?) as i64)
    }
    pub fn u32(&self, a: i64) -> Result<i64> {
        Ok(u32::from_le_bytes(self.get(a)?) as i64)
    }
    pub fn s32(&self, a: i64) -> Result<i64> {
        Ok(i32::from_le_bytes(self.get(a)?) as i64)
    }
    pub fn u16s(&self, a: i64, n: usize) -> Result<Vec<i64>> {
        (0..n as i64).map(|k| self.u16(a + 2 * k)).collect()
    }
    pub fn s16s(&self, a: i64, n: usize) -> Result<Vec<i64>> {
        (0..n as i64).map(|k| self.s16(a + 2 * k)).collect()
    }
    pub fn u32s(&self, a: i64, n: usize) -> Result<Vec<i64>> {
        (0..n as i64).map(|k| self.u32(a + 4 * k)).collect()
    }
    pub fn s32s(&self, a: i64, n: usize) -> Result<Vec<i64>> {
        (0..n as i64).map(|k| self.s32(a + 4 * k)).collect()
    }
    /// `cstr(a)`: up to 32 bytes, to the first NUL, as latin-1.
    pub fn cstr(&self, a: i64) -> String {
        self.cstr_n(a, 32)
    }
    pub fn cstr_n(&self, a: i64, n: usize) -> String {
        self.bytes(a, n).iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()
    }
    pub fn find(&self, text: &[u8]) -> Result<i64> {
        let i = self.d.windows(text.len().max(1)).position(|w| w == text);
        Ok(self.base + i.with_context(|| format!("{} is not in the data", String::from_utf8_lossy(text)))? as i64)
    }
}

/// `f'0x{v:0{w}X}'`
pub fn hx(v: i64, w: usize) -> String {
    let s = format!("{:X}", v.unsigned_abs());
    let sign = if v < 0 { "-" } else { "" };
    format!("0x{sign}{}{s}", "0".repeat(w.saturating_sub(s.len() + sign.len())))
}

// ---------------------------------------------------------------- compact pretty JSON
fn depth(o: &Value) -> usize {
    match o {
        Value::Object(m) => 1 + m.values().map(depth).max().unwrap_or(0),
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// `json.dumps(o, separators=(', ', ': '), ensure_ascii=False)`
const ONE: Style = Style { item: ", ", key: ": ", indent: None, sort_keys: false, ensure_ascii: false };

fn one(o: &Value) -> String {
    pyjson::to_string(o, ONE)
}

fn is_container(o: &Value) -> bool {
    matches!(o, Value::Array(_) | Value::Object(_))
}

fn enc(o: &Value, ind: usize, width: usize, flat_max: usize) -> String {
    let s = one(o);
    let len = s.chars().count(); // Python's len: code points
    let empty = match o {
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => true,
    };
    if empty || len + ind <= width || (len <= flat_max && depth(o) <= 2 && o.is_object()) {
        return s;
    }
    let pad = " ".repeat(ind + 2);
    let close = " ".repeat(ind);
    match o {
        Value::Array(a) if !a.iter().any(is_container) => {
            // lists of scalars: pack several per line
            let (mut lines, mut cur) = (Vec::new(), String::new());
            for x in a {
                let t = one(x);
                if !cur.is_empty() && cur.chars().count() + t.chars().count() + 2 + ind + 2 > width {
                    lines.push(std::mem::replace(&mut cur, t));
                } else if cur.is_empty() {
                    cur = t;
                } else {
                    cur = cur + ", " + &t;
                }
            }
            lines.push(cur);
            let body: Vec<String> = lines.iter().map(|l| format!("{pad}{l}")).collect();
            format!("[\n{}\n{close}]", body.join(",\n"))
        }
        Value::Array(a) => {
            let body: Vec<String> = a.iter().map(|x| format!("{pad}{}", enc(x, ind + 2, width, flat_max))).collect();
            format!("[\n{}\n{close}]", body.join(",\n"))
        }
        Value::Object(m) => {
            let body: Vec<String> = m
                .iter()
                .map(|(k, v)| {
                    let mut key = String::new();
                    pyjson::write_str(&mut key, k, true);
                    format!("{pad}{key}: {}", enc(v, ind + 2, width, flat_max))
                })
                .collect();
            format!("{{\n{}\n{close}}}", body.join(",\n"))
        }
        _ => s,
    }
}

/// Pretty JSON: a container stays on one line when it fits in 120 columns, or when it is a record of scalars /
/// scalar lists (depth <= 2) of at most 480 characters (one entry per line).
pub fn dumps(obj: &Value) -> String {
    enc(obj, 0, 120, 480) + "\n"
}

/// Writes `dumps(obj)`; returns its size in bytes.
pub fn write_json(out: &dyn conv_io::Vfs, path: &str, obj: &Value) -> Result<usize> {
    let s = dumps(obj);
    out.write(path, s.as_bytes())?;
    Ok(s.len())
}

// ---------------------------------------------------------------- input notation
pub const DIR: [&str; 10] = ["?", "d/b", "d", "d/f", "b", "n", "f", "u/b", "u", "u/f"];
pub const MOTIONS: [(&[&str], &str); 4] = [
    (&["b", "d/b", "d", "d/f", "f"], "hcf"),
    (&["f", "d/f", "d", "d/b", "b"], "hcb"),
    (&["d", "d/f", "f"], "qcf"),
    (&["d", "d/b", "b"], "qcb"),
];

pub fn buttons(nib: i64) -> Vec<String> {
    (0..4).filter(|i| nib >> i & 1 != 0).map(|i| (i + 1).to_string()).collect()
}

/// Numpad directions accepted by a single-frame command (bit 4+n = direction n).
pub fn cmd_dirs(c: i64) -> Vec<usize> {
    (1..10).filter(|&n| c >> (n + 4) & 1 != 0).collect()
}

fn button_text(b: &[String], anyof: bool) -> String {
    if anyof && b.len() > 1 { format!("({})", b.join("|")) } else { b.join("+") }
}

/// Single-frame command (cmd < 0xC000) -> Tekken notation.
/// Matcher 0x8002CC28: direction bits 5..13 (none = any direction); bits 0..3 = buttons 1..4;
/// bit 4 = 'any of' the buttons (or, with no buttons, 'direction held, buttons ignored').
/// Without bit 4 all listed buttons must be held with at least one newly pressed; with no buttons
/// and no bit 4 the frame must have no new button press.
pub fn single_text(c: i64) -> String {
    let ds = cmd_dirs(c);
    let d = if !ds.is_empty() && ds.len() < 9 {
        ds.iter().map(|&n| DIR[n]).collect::<Vec<_>>().join("|")
    } else {
        String::new()
    };
    let b = buttons(c & 0xF);
    let anyof = c & 0x10 != 0;
    let any_dir = if d.is_empty() { "*" } else { &d };
    if b.is_empty() {
        return if anyof { any_dir.to_string() } else { format!("{any_dir} (no button)") };
    }
    let bt = button_text(&b, anyof);
    if d.is_empty() { bt } else { format!("{d}+{bt}") }
}

pub fn step_text(v: i64) -> (String, String) {
    let d = DIR.get((v & 15) as usize).unwrap_or(&"?").to_string();
    let b = buttons((v >> 8) & 15);
    if b.is_empty() {
        return (d, String::new());
    }
    (d, button_text(&b, v >> 12 & 1 != 0))
}

/// Motion-input step words (dir | buttons<<8 | any<<12) -> readable notation, e.g. 'qcf+1+2'.
/// Leading neutral steps and neutral 'release' steps after a button step are dropped.
pub fn seq_text(steps: &[i64]) -> String {
    let mut out: Vec<(String, String)> = Vec::new();
    for (d, b) in steps.iter().map(|&v| step_text(v)) {
        if d == "n" && b.is_empty() && out.last().is_none_or(|l| !l.1.is_empty()) {
            continue;
        }
        out.push((d, b));
    }
    let (mut res, mut i) = (Vec::new(), 0);
    'outer: while i < out.len() {
        for (pat, name) in MOTIONS {
            let n = pat.len();
            let win = &out[i..(i + n).min(out.len())];
            if win.len() == n
                && win.iter().zip(pat).all(|(t, p)| t.0 == *p)
                && win[..n - 1].iter().all(|t| t.1.is_empty())
            {
                let b = &win[n - 1].1;
                res.push(if b.is_empty() { name.to_string() } else { format!("{name}+{b}") });
                i += n;
                continue 'outer;
            }
        }
        let (d, b) = &out[i];
        res.push(if b.is_empty() {
            d.clone()
        } else if d != "n" {
            format!("{d}+{b}")
        } else {
            b.clone()
        });
        i += 1;
    }
    res.join(",")
}

pub const SEQ_TABLES: [(i64, i64, i64); 2] = [(0x800954D0, 0xC00E, 0x3F), (0x800955CC, 0xC7FF, 0x29)];

pub struct Seq {
    pub window: i64,
    pub steps: Vec<i64>,
    pub address: i64,
}

/// Motion-input tables used by 0x8002CA84: cmd -> {window, steps[]}.
pub fn read_sequences(exe: &Exe) -> Result<BTreeMap<i64, Seq>> {
    let mut seqs = BTreeMap::new();
    for (base, c0, n) in SEQ_TABLES {
        for i in 0..n {
            let p = exe.u32(base + 4 * i)?;
            let window = exe.s16(p)?;
            let (mut steps, mut k) = (Vec::new(), 1);
            loop {
                let v = exe.u16(p + 2 * k)?;
                k += 1;
                if v == 0 {
                    break;
                }
                steps.push(v);
            }
            seqs.insert(c0 + i, Seq { window, steps, address: p });
        }
    }
    Ok(seqs)
}

/// -> (kind, human-readable input).
pub fn cmd_info(c: i64, seqs: &BTreeMap<i64, Seq>) -> (&'static str, String) {
    match c {
        _ if c < 0xC000 => ("command", single_text(c)),
        0xC000 => ("end", "END".into()),
        0xC001 => ("dash", "dash".into()),
        0xC002 => ("backdash", "backdash".into()),
        0xC00C => ("call", "CALL".into()),
        0xC00D => ("return", "RETURN".into()),
        _ => match seqs.get(&c) {
            Some(s) => ("sequence", seq_text(&s.steps)),
            None => ("special", hx(c, 4)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notation_as_python() {
        // values checked against gamedata_core.py
        assert_eq!(hx(0xC00E, 4), "0xC00E");
        assert_eq!(hx(-5, 4), "0x-005");
        assert_eq!(hx(0x800954D0, 0), "0x800954D0");
        assert_eq!(single_text(0x1 | 1 << 7 | 1 << 8), "d/f|b+1");
        assert_eq!(single_text(1 << 6 | 1 << 7 | 0x30), "d/b|d|d/f");
        assert_eq!(single_text(0x13), "(1|2)");
        assert_eq!(single_text(0x10), "*");
        assert_eq!(single_text(1 << 10), "f (no button)");
        assert_eq!(seq_text(&[5, 2, 3, 6 | 3 << 8]), "qcf+1+2");
        assert_eq!(seq_text(&[6, 5, 6, 0x105, 5]), "f,n,f,1");
        assert_eq!(seq_text(&[4, 1, 2, 3, 6 | 0x1100 | 0x200]), "hcf+(1|2)");
    }

    /// T3_GAMEDATA=<export_gamedata.py's gamedata/ dir>: every JSON file there, read back and written again, is
    /// byte-identical.
    #[test]
    fn dumps_as_python_files() {
        let Ok(dir) = std::env::var("T3_GAMEDATA") else {
            return;
        };
        let mut n = 0;
        for sub in ["", "movesets"] {
            for e in std::fs::read_dir(std::path::Path::new(&dir).join(sub)).into_iter().flatten().flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "json") {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    let v: Value = serde_json::from_str(&text).unwrap_or_default();
                    // shadows.json is written compact, theater.json with indent 1
                    let same = [dumps(&v), crate::pyjson::compact(&v), crate::pyjson::indent1(&v)].contains(&text);
                    assert!(same, "{} differs", p.display());
                    n += 1;
                }
            }
        }
        assert!(n > 20, "only {n} files in {dir}");
    }

    /// T3_BIN, T3_GAMEDATA: the motion-input sequences as shared.json has them.
    #[test]
    fn sequences_as_python() -> Result<()> {
        let (Ok(bin), Ok(dir)) = (std::env::var("T3_BIN"), std::env::var("T3_GAMEDATA")) else {
            return Ok(());
        };
        let game = crate::disc::Game::load(&conv_io::StdDisc(bin.into()))?;
        let seqs = read_sequences(&Exe::new(&game.exe))?;
        let j: Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/shared.json"))?)?;
        let got: Vec<Value> = seqs
            .iter()
            .map(|(&c, s)| {
                let steps: Vec<i64> = s.steps.clone();
                json!([c, hx(s.address, 0), s.window, seq_text(&steps), steps, cmd_info(c, &seqs).1])
            })
            .collect();
        let want: Vec<Value> = j["sequences"]["entries"]
            .as_array()
            .context("no entries")?
            .iter()
            .map(|e| {
                let steps: Vec<Value> = e["steps"].as_array().into_iter().flatten().map(|s| s["raw"].clone()).collect();
                json!([e["cmd"], e["address"], e["window"], e["input"], steps, e["input"]])
            })
            .collect();
        assert_eq!(got, want);
        Ok(())
    }

    #[test]
    fn dumps_layout() {
        let long: Vec<i64> = (0..60).collect();
        let v = json!({"a": long, "é": {"x": [1, {"y": 2.5}], "s": "ü"}});
        let s = dumps(&v);
        assert!(s.starts_with("{\n  \"a\": [\n    0, 1, 2,"));
        assert!(s.contains("\"\\u00e9\": {\"x\": [1, {\"y\": 2.5}], \"s\": \"ü\"}"));
    }
}
