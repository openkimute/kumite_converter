//! Tekken 2 move sets in OpenKumite's format (movesets/*.json, the schema of kumite_sim::data::Moveset).
//!
//! A transplanted fighter keeps Tekken 3's system (movement, sidesteps, guard, jumps, throws, getting up, every hit
//! reaction: they come from a Tekken 3 donor's set, its motions copied onto the Tekken 2 skeleton by joint name) and
//! gets its own Tekken 2 attacks: every move reachable from the Tekken 2 stance through the command lists (own
//! entries, the EXE's shared lists 0x80047488 called with 0xFFF3), with its motion, damage, active frames, hit level,
//! string links, END frames and recovery, and a strike path baked from the striking bone.
//!
//! Tekken 2 data (docs/reference/tekken2/engine.md §3):
//!   move (0x2C): +0 motion, +4 u16 posture (base in bits 0-2, 0x08 guard), +6 u16 hit mask (low nibble 2 = blocked
//!   standing, 1 = blocked crouching; 0x20 hits standing, 0x10 hits crouching), +8 command list, +0xC s16 next slot,
//!   +0xE u16 damage (low 12 bits), +0x14 u8 frame count (0 = the motion's), +0x23 / +0x24 first / last active frame,
//!   +0x25 the striking bone. Common moves (slot | 0x40000000): EXE 0x80042864, their motions at 0x800D660C and
//!   their command lists at absolute EXE addresses.
//!   command entry (12 bytes, as Tekken 3's): cmd, req, cond, param, dest slot, transition, window, take-over.
//!   cmd: buttons in bits 0-3, 0x10 = press, directions in bits 5-13 (as Tekken 3); 0xFFFF END, 0xFFFE dash,
//!   0xFFFD backdash, 0xFFF3 call / 0xFFF2 return; req: 1..0x18 only character req-1, 0x19..0x30 not character
//!   req-0x19, 0x31.. opponent is character req-0x31.
//!
//! Not taken from Tekken 2 yet: entries with a condition (cond != 0; Tekken 2 numbers them differently), jumps and
//! jump attacks, throws (the donor's are kept), timed properties and sounds (the closest donor attack's are used,
//! with its hit reaction).

use crate::export_characters::{BLOCK0, Disc};
use crate::transplant::{T2_TO_T3, t3_parent};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};

const COMMON: u32 = 0x80042864;
pub const COMMON_MOTIONS: u32 = 0x800D660C;
const SHARED: u32 = 0x80047488;
const REACTIONS: u32 = 0x8003E5B4;
const THROW_ATTEMPTS: [(u32, i64); 2] = [(0x299, 0x5), (0x29A, 0xA)]; // Tekken 2 slots of the 1+3 / 2+4 throw attempts -> Tekken 3 command
// Tekken 2 property ops -> Tekken 3 (docs/reference/tekken3/move-properties.md §4): 3 effect at a bone, 1 effect at own bone,
// 0 (arg 4) the impact: the throw's damage is dealt there
fn prop_op(op: u8) -> Option<i64> {
    match op {
        3 => Some(8),
        1 => Some(6),
        _ => None,
    }
}
/// Tekken 2's lying slots -> Tekken 3's down aliases (face up, down)
fn down(slot: i16) -> i64 {
    if slot == 14 { 167 } else { 153 }
}
pub const EXE: u32 = 0x8000F800; // RAM address - file offset
const ALIAS_BASE: usize = 0x1000; // aliases of the transplanted moves (Tekken 3's table has 4023)
const NEUTRAL: i64 = 3; // Tekken 3's neutral alias

/// Tekken 2 hit mask -> Tekken 3 (level name, level, mask): 0x20 hits standing, 0x10 crouching; the low nibble is the
/// guard that blocks it (1 crouching, 2 standing). Hits standing only = high (ducked), blocked crouching = low, else
/// mid.
fn hit_level(mask: u16) -> (&'static str, i64, i64) {
    if mask & 0x30 == 0x20 {
        return ("high", 4, 0x412);
    }
    if mask & 0xF == 1 {
        return ("low", 1, 0x10F);
    }
    ("mid", 2, 0x217)
}
const DIRS: [&str; 9] = ["d/b", "d", "d/f", "b", "n", "f", "u/b", "u", "u/f"];
// Tekken 2 conditions (handlers 0x8004E230; a1 the fighter, a2 the opponent) -> Tekken 3's: 0x19 back turned
// (+0x28 > 0x4000) = 0x24; 0x2D (+0xBA == 0, the usual of a pair with 0x2E) = always. The others are left out
// (0x1E opponent attacking close, 0x27 / 0x2A opponent flag 0x40, 0x2E, 0x33, 0x35: no Tekken 3 match yet).
fn cond3(c: u8) -> Option<i64> {
    match c {
        0 | 0x2D => Some(0),
        0x19 => Some(0x24),
        _ => None,
    }
}
fn arm_bone(b: u8) -> bool {
    (3..11).contains(&b) // Tekken 2 bones of the arms (clavicle .. hand)
}

fn input_name(cmd: u16) -> String {
    if cmd & 0xC000 != 0 {
        return match cmd {
            0xFFFF => "END".into(),
            0xFFFE => "dash".into(),
            0xFFFD => "backdash".into(),
            _ => format!("special {cmd:04x}"),
        };
    }
    let b: Vec<String> = (0..4).filter(|i| cmd >> i & 1 != 0).map(|i| (i + 1).to_string()).collect();
    let ds: Vec<&str> = (5..14).filter(|k| cmd >> k & 1 != 0).map(|k| DIRS[k - 5]).collect();
    let d = if ds.is_empty() { "*".to_string() } else { ds.join("|") };
    if b.is_empty() { d } else { format!("{d}+{}", b.join("+")) }
}

/// A Tekken 2 move: its own (the record's) or the EXE's common table, and its index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Mv {
    pub common: bool,
    pub n: u32,
}

impl Mv {
    fn json(&self) -> Value {
        json!([if self.common { "common" } else { "own" }, self.n])
    }
}

/// A command entry: cmd, req, cond, param, dest, transition, window start, end, take-over.
type Ent = (u16, u8, u8, u16, u16, u8, u8, u8, u8);

/// struct.unpack_from's offset: negative counts from the end.
fn pyoff(len: usize, o: i64) -> Result<usize> {
    let o = if o < 0 { o + len as i64 } else { o };
    usize::try_from(o).map_err(|_| anyhow!("an offset before the data"))
}
fn rd<const N: usize>(b: &[u8], o: i64) -> Result<[u8; N]> {
    crate::ps1::bytes::<N>(b, pyoff(b.len(), o)?)
}
fn u16at(b: &[u8], o: i64) -> Result<u16> {
    Ok(u16::from_le_bytes(rd(b, o)?))
}
fn i16at(b: &[u8], o: i64) -> Result<i16> {
    Ok(i16::from_le_bytes(rd(b, o)?))
}
fn u32at(b: &[u8], o: i64) -> Result<u32> {
    Ok(u32::from_le_bytes(rd(b, o)?))
}
fn i32at(b: &[u8], o: i64) -> Result<i32> {
    Ok(i32::from_le_bytes(rd(b, o)?))
}
fn ent_at(b: &[u8], o: i64) -> Result<Ent> {
    let e: [u8; 12] = rd(b, o)?;
    Ok((
        u16::from_le_bytes([e[0], e[1]]),
        e[2],
        e[3],
        u16::from_le_bytes([e[4], e[5]]),
        u16::from_le_bytes([e[6], e[7]]),
        e[8],
        e[9],
        e[10],
        e[11],
    ))
}

/// A move's fields (the order the 'source' entries list them).
#[derive(Clone)]
pub struct Fields {
    pub posture: u16,
    pub mask: u16,
    pub next: i16,
    pub damage: u8,
    pub damage_flags: u8,
    pub frames: u8,
    pub jump: u16,
    pub air: bool,
    pub reaction: i16,
    pub active: (u8, u8),
    pub bone: u8,
    pub properties: i32,
}

impl Fields {
    fn source(&self, r: Mv) -> Value {
        json!({"t2": r.json(), "posture": self.posture, "mask": self.mask, "next": self.next, "damage": self.damage,
               "damage_flags": self.damage_flags, "frames": self.frames, "jump": self.jump, "air": self.air,
               "reaction": self.reaction, "active": [self.active.0, self.active.1], "bone": self.bone,
               "properties": self.properties})
    }
}

/// One Tekken 2 character's moves (block +7) with the EXE's common moves and shared lists.
pub struct T2Set<'a> {
    pub d: &'a Disc,
    pub ch: i64,
    pub mv: Vec<u8>,
    h: [i64; 10],
}

impl<'a> T2Set<'a> {
    pub fn new(disc: &'a Disc, block: usize, char_id: i64) -> Result<T2Set<'a>> {
        let mut mv = disc.rec(BLOCK0 + 8 * block + 7)?;
        if mv.get(..4) == Some(b"arc ") {
            let (o, z) = (u32at(mv, 8)? as usize, u32at(mv, 12)? as usize);
            mv = crate::ps1::py_slice(mv, o, o + z);
        }
        let mut h = [
            mv.first().copied().unwrap_or(0) as i64,
            mv.get(1).copied().unwrap_or(0) as i64,
            i16at(mv, 2)? as i64,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        for (i, v) in h[3..].iter_mut().enumerate() {
            *v = u32at(mv, 4 + 4 * i as i64)? as i64;
        }
        Ok(T2Set { d: disc, ch: char_id, mv: mv.to_vec(), h })
    }

    pub fn slot(&self, k: i64) -> Result<Option<Mv>> {
        let s = u32at(&self.mv, self.h[3] + 4 * k)?;
        if s == 0xFFFFFFFF {
            return Ok(None);
        }
        Ok(Some(if s & 0x40000000 != 0 { Mv { common: true, n: s & 0xFFFF } } else { Mv { common: false, n: s } }))
    }

    pub fn raw(&self, r: Mv) -> &[u8] {
        let (buf, o) = if r.common {
            (&self.d.exe[..], (COMMON - EXE) as usize + 0x2C * r.n as usize)
        } else {
            (&self.mv[..], self.h[4] as usize + 0x2C * r.n as usize)
        };
        crate::ps1::py_slice(buf, o, o + 0x2C)
    }

    /// The move's motion offset (in the record, or the EXE's common motion area).
    pub fn motion(&self, r: Mv) -> Result<u32> {
        u32at(self.raw(r), 0)
    }

    /// The move's command list, the shared lists it calls expanded (in the order the walker 0x80067AE0 sees).
    pub fn entries(&self, r: Mv) -> Result<Vec<Ent>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Src {
            Own,
            Exe,
            Shared,
        }
        let w2 = u32at(self.raw(r), 8)? as i64;
        let (mut out, mut stack) = (Vec::new(), Vec::new());
        let (mut src, mut e) = (if r.common { Src::Exe } else { Src::Own }, w2);
        let step = |s: Src, e: i64| if s == Src::Exe { e + 12 } else { e + 1 };
        for _ in 0..1_000_000 {
            let ent = match src {
                Src::Own => ent_at(&self.mv, self.h[5] + 12 * e)?,
                Src::Exe => ent_at(&self.d.exe, e - EXE as i64)?,
                Src::Shared => ent_at(&self.d.exe, SHARED as i64 + 12 * e - EXE as i64)?,
            };
            let c = ent.0;
            if c == 0xFFF3 {
                stack.push((src, e));
                (src, e) = (Src::Shared, ent.4 as i64);
                continue;
            }
            if c == 0xFFF2 || (c == 0xFFFF && !stack.is_empty()) {
                (src, e) = stack.pop().context("a shared list returns with nothing to return to")?;
                e = step(src, e);
                continue;
            }
            out.push(ent);
            if c == 0xFFFF {
                return Ok(out);
            }
            e = step(src, e);
        }
        bail!("a command list without its END")
    }

    pub fn fields(&self, r: Mv) -> Result<Fields> {
        let b = self.raw(r);
        let u8at = |o: usize| b.get(o).copied().context("a move is short");
        Ok(Fields {
            posture: u16at(b, 4)?,
            mask: u16at(b, 6)?,
            next: i16at(b, 0xC)?,
            damage: u8at(0xE)?,
            damage_flags: u8at(0xF)?,
            frames: u8at(0x14)?,
            jump: u16at(b, 0x10)?,
            air: u8at(0x13)? == 9,
            reaction: i16at(b, 0x20)?,
            active: (u8at(0x23)?, u8at(0x24)?),
            bone: u8at(0x25)?,
            properties: i32at(b, 0x1C)?,
        })
    }

    /// The move's timed properties (block +7 table at header +0x18, 4-byte {u16 frame, u8 arg, u8 op}, ending with a
    /// zero word; move +0x1C = the first entry's index, -1 none).
    pub fn properties(&self, r: Mv) -> Result<Vec<(u16, u8, u8)>> {
        let i = self.fields(r)?.properties;
        if i < 0 || r.common {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        let mut o = pyoff(self.mv.len(), self.h[6] + 4 * i as i64)?;
        while o + 4 <= self.mv.len() {
            let (frame, arg, op) = (u16::from_le_bytes([self.mv[o], self.mv[o + 1]]), self.mv[o + 2], self.mv[o + 3]);
            if frame == 0 && arg == 0 && op == 0 {
                break;
            }
            out.push((frame, arg, op));
            o += 4;
        }
        Ok(out)
    }

    /// The move's sound list (move +0x18: own moves an index into the +7 table at header +0x1C, common moves an EXE
    /// address): u16 entries type << 9 | index up to 0xFFFF (player 0x80073174; types 4 / 5 voice the character's
    /// sound index on frame 1).
    pub fn sounds(&self, r: Mv) -> Result<Vec<(u16, u16)>> {
        let o = i32at(self.raw(r), 0x18)?;
        let (buf, mut a) = if r.common {
            let o = o as u32;
            if !(0x80010000..0x80200000).contains(&o) {
                return Ok(vec![]);
            }
            (&self.d.exe[..], (o - EXE) as i64)
        } else {
            if o < 0 {
                return Ok(vec![]);
            }
            (&self.mv[..], self.h[7] + 2 * o as i64)
        };
        let mut out = Vec::new();
        for _ in 0..8 {
            let v = u16at(buf, a)?;
            if v == 0xFFFF {
                break;
            }
            out.push((v >> 9, v & 0x1FF));
            a += 2;
        }
        Ok(out)
    }

    /// Reaction rid (EXE 0x8003E5B4, 18 s16): 7 parameters, then 11 victim slots (for a throw, the last is the
    /// victim's move in the thrower's slot table).
    pub fn reaction_entry(&self, rid: i16) -> Result<[i16; 18]> {
        let mut r = [0; 18];
        for (i, v) in r.iter_mut().enumerate() {
            *v = i16at(&self.d.exe, (REACTIONS - EXE) as i64 + 36 * rid as i64 + 2 * i as i64)?;
        }
        Ok(r)
    }

    pub fn gate(&self, req: u8) -> bool {
        let req = req as i64;
        if req == 0 {
            return true;
        }
        if req < 0x19 {
            return req - 1 == self.ch;
        }
        if req < 0x31 {
            return req - 0x19 != self.ch;
        }
        false // opponent-specific entries: dropped
    }
}

fn dirs_of(cmd: u16) -> Vec<u16> {
    (5..14).filter(|k| cmd >> k & 1 != 0).map(|k| k - 4).collect()
}

/// Tekken 2 entries the transplant follows: no condition, this character's, no jumps, no throws / intros.
fn keep_entry(t2: &T2Set, ent: &Ent) -> bool {
    let (cmd, req, cond) = (ent.0, ent.1, ent.2);
    if cond3(cond).is_none() || !t2.gate(req) {
        return false;
    }
    if cmd & 0xC000 != 0 {
        return matches!(cmd, 0xFFFD..=0xFFFF);
    }
    let ds = dirs_of(cmd);
    if !ds.is_empty() && ds.iter().all(|d| (7..=9).contains(d)) && cmd & 0xF == 0 {
        return false; // plain jumps: the donor's (jump attacks are Tekken 2's)
    }
    if matches!(cmd & 0xF, 0x5 | 0xA) {
        return false; // 1+3 / 2+4: throws (the donor's)
    }
    true
}

/// Tekken 2 moves reachable from `start` through kept entries, in breadth-first order.
fn reachable(t2: &T2Set, start: Mv) -> Result<Vec<Mv>> {
    let (mut seen, mut q) = (vec![start], std::collections::VecDeque::from([start]));
    while let Some(r) = q.pop_front() {
        for ent in t2.entries(r)? {
            if ent.0 == 0xFFFF || !keep_entry(t2, &ent) {
                continue;
            }
            let t = if ent.4 < 0x703 { t2.slot(ent.4 as i64)? } else { None };
            if let Some(t) = t
                && !seen.contains(&t)
            {
                seen.push(t);
                q.push_back(t);
            }
        }
    }
    Ok(seen)
}

/// (stance move, transplanted moves): the attacks and the system moves they pass through.
pub fn graph(t2: &T2Set) -> Result<(Mv, Vec<Mv>)> {
    let stance = t2.slot(3)?.context("the Tekken 2 set has no stance")?;
    let moves = reachable(t2, stance)?.into_iter().filter(|r| *r != stance).collect();
    Ok((stance, moves))
}

pub fn clip_name(t2: &T2Set, r: Mv, own_motions: &[usize]) -> Result<Option<String>> {
    let o = t2.motion(r)? as usize;
    if r.common {
        return Ok(Some(format!("t2c_{:03}", r.n)));
    }
    Ok(own_motions.iter().position(|m| *m == o).map(|i| format!("t2m_{i:03}")))
}

// ---- building the Tekken 3 set ----------------------------------------------------------------------------------
/// What the build takes from the port's data and the fighter's sound (transplant.rs sets it up).
pub struct Ctx<'a> {
    /// reactions.json entries, common.json moves, shared.json's shared cancels
    pub reactions_t3: Vec<Value>,
    pub common_t3: Vec<Value>,
    pub shared_cancels: Vec<Value>,
    /// Tekken 2 voice sound index -> its place in the fighter's category-0 voice row
    pub voice_index: BTreeMap<u16, i64>,
    /// sound.json sfx_table.t2_system.start: Tekken 2 bank-2 sound n is the entry start + n
    pub system_start: i64,
    /// Tekken 2 voice script id -> the port's sound.json script index
    pub script: Option<&'a mut dyn FnMut(u16) -> Result<Option<i64>>>,
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}
fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}
fn idx(v: &Value, i: i64) -> Result<&Value> {
    usize::try_from(i).ok().and_then(|i| v.get(i)).with_context(|| format!("no entry {i}"))
}
fn obj(v: &mut Value) -> Result<&mut Map<String, Value>> {
    v.as_object_mut().context("a move set entry is not an object")
}
fn list(v: &mut Value) -> Result<&mut Vec<Value>> {
    v.as_array_mut().context("a move set list is not a list")
}
fn zero_active(m: &Value) -> bool {
    m["active"] == json!([0, 0])
}

/// What a donor attack does to a standing victim hit from the front (reaction slot 0): 'down' (knocked down or
/// launched: a lying or airborne victim move) or 'hit'.
fn outcome_t3(ctx: &Ctx, donor: &Value, m: &Value) -> Result<bool> {
    if ctx.reactions_t3.is_empty() {
        return Ok(false);
    }
    let ri = int(&m["reaction"]["index"]);
    let vic = int(&ctx.reactions_t3.get(ri as usize).with_context(|| format!("no reaction {ri}"))?["victims"][0]);
    let aliases = arr(&donor["aliases"]);
    let none = json!({"bank": "none"});
    let r = if (vic as usize) < aliases.len() { &aliases[vic as usize] } else { &none };
    let v = match r["bank"].as_str() {
        Some("own") => Some(idx(&donor["moves"], int(&r["move"]))?),
        Some("common") if !ctx.common_t3.is_empty() => ctx.common_t3.get(int(&r["move"]) as usize),
        _ => None,
    };
    let Some(v) = v else { return Ok(false) };
    Ok(int(&v["posture"]["raw"]) & 0x400 != 0 || int(&v["posture"]["base"]) == 4)
}

/// The same for a Tekken 2 reaction (0x8003E5B4 slot 7, the front standing victim): posture lying = down.
fn outcome_t2(t2: &T2Set, rid: i16) -> Result<bool> {
    let v = t2.slot(t2.reaction_entry(rid)?[7] as i64)?;
    Ok(match v {
        Some(v) => t2.fields(v)?.posture & 7 == 4,
        None => false,
    })
}

/// A donor attack: (level name, arm, damage, move, knocks down).
type Template = (String, bool, i64, usize, bool);

fn donor_templates(ctx: &Ctx, donor: &Value) -> Result<Vec<Template>> {
    let mut out = Vec::new();
    for (i, m) in arr(&donor["moves"]).iter().enumerate() {
        let a = &m["attack"];
        let frames = arr(&a["frames"]);
        if int(&m["damage"]) > 0
            && !zero_active(m)
            && a.as_object().is_some_and(|o| !o.is_empty())
            && !frames.is_empty()
            && m["reaction"]["table"] == "reactions"
        {
            let arm = a["bones"].get("end0").map_or(0xC, int) < 0xC;
            let level = m["attack_mask"]["level_name"].as_str().unwrap_or("").to_string();
            out.push((level, arm, int(&m["damage"]), i, outcome_t3(ctx, donor, m)?));
        }
    }
    Ok(out)
}

/// The donor attack closest to a Tekken 2 one: same outcome (down / hit), level and limb first.
fn pick_template(templates: &[Template], level: &str, arm: bool, damage: i64, outcome: bool) -> Result<usize> {
    let keeps: [&dyn Fn(&Template) -> bool; 5] = [
        &|t| t.4 == outcome && t.0 == level && t.1 == arm,
        &|t| t.4 == outcome && t.0 == level,
        &|t| t.4 == outcome,
        &|t| t.0 == level,
        &|_| true,
    ];
    for keep in keeps {
        let mut best: Option<&Template> = None;
        for t in templates.iter().filter(|t| keep(t)) {
            if best.is_none_or(|b| (t.2 - damage).abs() < (b.2 - damage).abs()) {
                best = Some(t);
            }
        }
        if let Some(b) = best {
            return Ok(b.3);
        }
    }
    bail!("the donor has no attacks to take a template from")
}

/// world(clip, frame, joint) -> glTF position of a joint.
pub type World<'w> = dyn FnMut(&str, i64, &str) -> Result<[f64; 3]> + 'w;

/// Strike segments per active frame in Tekken 3's attack space (frame f is pose f - 1; (-x, -y, z) x 1000): the
/// sweep from the end's previous place, and the limb (elbow / knee to wrist / ankle).
fn strike_path(world: &mut World, clip: &str, bone_end: &str, bone_mid: &str, a0: u8, a1: u8) -> Result<Vec<Value>> {
    let mut p = |f: i64, b: &str| -> Result<Value> {
        let p = world(clip, f, b)?;
        Ok(json!([
            (-p[0] * 1000.0).round_ties_even() as i64,
            (-p[1] * 1000.0).round_ties_even() as i64,
            (p[2] * 1000.0).round_ties_even() as i64
        ]))
    };
    let mut frames = Vec::new();
    for f in a0 as i64..=a1 as i64 {
        let (end, prev, mid) = (p(f - 1, bone_end)?, p((f - 2).max(0), bone_end)?, p(f - 1, bone_mid)?);
        frames.push(json!({"frame": f, "segments": [[prev, end], [mid, end]]}));
    }
    Ok(frames)
}

fn end_entry(dest: i64, ws: u8, we: u8, tk: u8, transition: i64) -> Value {
    json!({"cmd": 0xC000, "kind": "end", "input": "END", "req": 0, "cond": 0, "param": 0, "dest": dest,
           "win_start": ws, "win_end": we, "exec": tk, "transition": transition})
}

/// The Tekken 3 set: the donor's, its stance list re-made with the Tekken 2 commands first. world_of(clip, frame,
/// t3 bone name) -> glTF position of a joint (the transplanted glb). Also the transplanted moves.
pub fn build(
    ctx: &mut Ctx,
    t2: &T2Set,
    donor: &Value,
    motions_of_record: &[usize],
    world_of: &mut World,
    moveset_id: i64,
    throw_victims: &mut Vec<Value>,
) -> Result<(Value, Vec<Mv>)> {
    let mut out = donor.clone();
    let o = obj(&mut out)?;
    o.insert("moveset_id".into(), json!(moveset_id));
    o.insert("source_records".into(), json!([format!("Tekken 2 block +7 (character {})", t2.ch)]));
    for k in ["md5", "size", "raw_hex"] {
        o.shift_remove(k);
    }
    let (stance, moves) = graph(t2)?;
    let own_motions = motions_of_record;
    let alias_of: HashMap<Mv, i64> = moves.iter().enumerate().map(|(i, r)| (*r, (ALIAS_BASE + i) as i64)).collect();
    {
        let aliases = list(&mut out["aliases"])?;
        let need = (ALIAS_BASE + moves.len()).saturating_sub(aliases.len());
        aliases.extend((0..need).map(|_| json!({"bank": "none", "move": null})));
    }
    let templates = donor_templates(ctx, donor)?;
    let tpl_moves = arr(&donor["moves"]);
    let mut cancels: Vec<Value> = arr(&out["cancels"]).to_vec();

    let dest_alias = |slot: i64| -> Result<Option<i64>> {
        if slot == 1 || slot == 3 {
            return Ok(Some(NEUTRAL));
        }
        let r = if slot < 0x703 { t2.slot(slot)? } else { None };
        if r == Some(stance) {
            return Ok(Some(NEUTRAL));
        }
        Ok(r.and_then(|r| alias_of.get(&r).copied()))
    };

    // The move's Tekken 2 entries as Tekken 3 entries (kept ones whose destination exists), END last. In the
    // stance's list only the attacks, the dashes and f (the Mishima step chain) are taken: the donor's movement
    // (walking back guards in Tekken 3) stays.
    let convert_list = |r: Mv, stance_list: bool| -> Result<Vec<Value>> {
        let mut lst: Vec<Value> = Vec::new();
        for ent in t2.entries(r)? {
            let (cmd, _req, cond, param, dest, tr, ws, we, tk) = ent;
            if cmd == 0xFFFF {
                let a = dest_alias(dest as i64)?;
                lst.push(end_entry(a.unwrap_or(NEUTRAL), ws, we, tk, 6 | (tr & 0x80) as i64));
                break;
            }
            if !keep_entry(t2, &ent) {
                continue;
            }
            if stance_list && !(cmd & 0xF != 0 || cmd == 0xFFFE || cmd == 0xFFFD || cmd == 0x410) {
                continue;
            }
            let Some(a) = dest_alias(dest as i64)? else {
                continue;
            };
            let attack = match t2.slot(dest as i64)? {
                Some(to) => t2.fields(to)?.active.0 != 0,
                None => false,
            };
            let (kind, c3) = match cmd {
                0xFFFE => ("dash", 0xC001),
                0xFFFD => ("backdash", 0xC002),
                _ => ("command", cmd as i64),
            };
            lst.push(
                json!({"cmd": c3, "kind": kind, "input": input_name(cmd), "req": 0, "cond": cond3(cond).unwrap_or(0),
                            "param": param, "dest": a, "win_start": ws, "win_end": we, "exec": tk,
                            "transition": (if attack { 12 } else { 6 }) | (tr & 0x80) as i64}),
            );
        }
        if lst.last().is_none_or(|e| e["kind"] != "end") {
            lst.push(end_entry(NEUTRAL, 0, 0, 0, 6));
        }
        Ok(lst)
    };

    // the transplanted moves
    for &r in &moves {
        let f = t2.fields(r)?;
        let clip = clip_name(t2, r, own_motions)?;
        let nframes = motion_frames(t2, r)?; // +0x14 (the homing / landing frame) is kept in 'source'
        let (a0, a1) = f.active;
        let level = if a0 != 0 { hit_level(f.mask) } else { ("none", 0, 0) };
        let arm = arm_bone(f.bone);
        let n_moves = arr(&out["moves"]).len();
        let next = dest_alias(f.next as i64)?.filter(|a| *a != 0).unwrap_or(NEUTRAL);
        let mut mv = json!({"index": n_moves, "anim": clip, "frames": nframes, "motion_tag": n_moves,
                            "next": {"alias": next, "hi": 0}, "damage": if a0 != 0 { f.damage } else { 0 },
                            "damage_type": 0, "air_drift": 0, "active": if a0 != 0 { [a0, a1] } else { [0, 0] }, "hitstop": 0,
                            "reaction": {"index": 0, "table": "reactions"}, "close_range": 0,
                            "flags24": {"raw": 0x70000000, "camera_script": 0}, "bytes_18": {}, "source": f.source(r)});
        let base = f.posture & 7;
        mv["posture"] = if f.air {
            json!({"raw": 0x4C02, "base": 2}) // airborne (a jump attack): lows pass under
        } else if f.posture & 0x08 != 0 {
            if base == 1 { json!({"raw": 0x3129, "base": 1}) } else { json!({"raw": 0x1052, "base": 2}) }
        } else if base == 1 {
            json!({"raw": 0x2021, "base": 1})
        } else {
            json!({"raw": 0x842, "base": 2})
        };
        mv["attack_mask"] = json!({"raw": level.2, "mask": level.2, "level": level.1, "level_name": level.0});
        if a0 != 0 {
            let ti = pick_template(&templates, level.0, arm, f.damage as i64, outcome_t2(t2, f.reaction)?)?;
            let t = &tpl_moves[ti];
            for k in ["reaction", "close_range", "hitstop", "push_control", "flags24", "sounds", "sound_list", "desc0"]
            {
                if let Some(x) = t.get(k) {
                    mv[k] = x.clone();
                }
            }
            let bone = T2_TO_T3.get(f.bone as usize).copied().unwrap_or("hand_B");
            let mid = t3_parent(bone).with_context(|| format!("{bone} strikes but has no parent"))?;
            let ta = t.get("attack").filter(|a| a.as_object().is_some_and(|o| !o.is_empty()));
            let clip = clip.clone().context("an attack without its clip")?;
            mv["attack"] = json!({"header_bytes": ta.and_then(|a| a.get("header_bytes")).cloned().unwrap_or(json!([])),
                                  "bones": ta.and_then(|a| a.get("bones")).cloned().unwrap_or(Value::Null),
                                  "frames": strike_path(world_of, &clip, bone, mid, a0, a1)?});
            mv["template"] = t["index"].clone();
        }
        // the sounds: the Tekken 2 move's own swing, hit and voices instead of the donor's
        let mut sounds: Vec<Value> =
            arr(&mv["sounds"]).iter().filter(|e| ![1, 2, 6, 7, 8, 9, 10].contains(&int(&e["type"]))).cloned().collect();
        sounds.extend(hit_entries(ctx, t2, r)?);
        sounds.extend(voice_entries(ctx, t2, r, f.damage as i64)?);
        mv["sounds"] = json!(sounds);
        let mi = mv["index"].clone();
        list(&mut out["moves"])?.push(mv);
        out["aliases"][alias_of[&r] as usize] = json!({"bank": "own", "move": mi});
    }
    // command lists of the transplanted moves
    for &r in &moves {
        let m = int(&out["aliases"][alias_of[&r] as usize]["move"]) as usize;
        out["moves"][m]["cancel_start"] = json!(cancels.len());
        cancels.extend(convert_list(r, false)?);
    }
    // throws: the donor's 1+3 / 2+4 attempts (their motion and timing), connecting to the Tekken 2 throws
    add_throws(ctx, t2, donor, &mut out, &mut cancels, own_motions, throw_victims)?;
    // the stance: the Tekken 2 commands first, then the donor's list without the donor's own attacks (its shared
    // lists inlined so they can be filtered); throws and everything that is not an attack stay
    let neutral_move = int(&donor["aliases"][NEUTRAL as usize]["move"]) as usize;
    let st = idx(&donor["moves"], neutral_move as i64)?;
    let t2_first: Vec<Value> = convert_list(stance, true)?.into_iter().filter(|e| e["kind"] != "end").collect();
    let mut kept: Vec<&Value> = Vec::new();
    let mut end = None;
    for e in arr(&donor["cancels"]).iter().skip(int(&st["cancel_start"]) as usize) {
        if e["kind"] == "call" {
            let sub = ctx
                .shared_cancels
                .iter()
                .skip(int(&e["dest"]) as usize)
                .take_while(|s| s["kind"] != "return" && s["kind"] != "end");
            kept.extend(sub.filter(|x| !donor_attack(donor, x)));
            continue;
        }
        if e["kind"] == "end" {
            end = Some(e);
            break;
        }
        if !donor_attack(donor, e) {
            kept.push(e);
        }
    }
    let end = end.context("the donor's stance list has no END")?;
    out["moves"][neutral_move]["cancel_start"] = json!(cancels.len());
    // throws come first, as in Tekken 2's list (else 1+3 would match the 1 entries)
    let is_throw = |x: &&Value| x["kind"] == "command" && matches!(int(&x["cmd"]) & 0xF, 0x5 | 0xA);
    cancels.extend(kept.iter().filter(|x| is_throw(x)).map(|x| (*x).clone()));
    cancels.extend(t2_first);
    cancels.extend(kept.iter().filter(|x| !is_throw(x)).map(|x| (*x).clone()));
    cancels.push(end.clone());
    out["moves"][neutral_move]["anim"] = json!(clip_name(t2, stance, own_motions)?);
    out["moves"][neutral_move]["frames"] = json!(motion_frames(t2, stance)?);
    for (i, c) in cancels.iter_mut().enumerate() {
        obj(c)?.insert("index".into(), json!(i));
    }
    out["cancels"] = json!(cancels);
    Ok((out, moves))
}

/// A Tekken 2 move's voices (its sound list, 0x80073174) as port sound entries: type 4 / 5 a voice on frame 1 (type
/// 2), 6 a timed script (the throws' voices and slams: type 8); a move with damage (+0xE, the +0x48 the random voice
/// tests) that neither voices nor locks (type 7 arg 2) gets the random attack voice (the port's type 10: Tekken 3's
/// type 9 means other things, and the fighters keep Tekken 3 common moves).
fn voice_entries(ctx: &mut Ctx, t2: &T2Set, r: Mv, damage: i64) -> Result<Vec<Value>> {
    let (mut out, mut random) = (Vec::new(), damage > 0);
    for (typ, i) in t2.sounds(r)? {
        if typ == 4 || typ == 5 {
            random &= typ == 5;
            if let Some(&v) = ctx.voice_index.get(&i) {
                out.push(json!({"type": 2, "index": v, "raw": 2 << 12 | v}));
            }
        } else if typ == 6 && ctx.script.is_some() {
            if let Some(script) = ctx.script.as_mut()
                && let Some(k) = script(i)?
            {
                out.push(json!({"type": 8, "index": k, "raw": 8 << 12 | k}));
            }
        } else if typ == 7 && i == 2 {
            random = false;
        }
    }
    if random {
        out.push(json!({"type": 10, "index": 0, "raw": 10 << 12}));
    }
    Ok(out)
}

/// A Tekken 2 move's swing and hit sounds (its sound list, 0x80073174, through the table 0x8004EC98 = 0x200 | index
/// − 3: bank-2 sound index − 3, in the fighter's own bank at sfx entry SYSTEM_START + n): type 2 (11..14) the swing on
/// the first active frame (0x80073390: the port's type 6), type 3 (3..10) the hit sound as it lands (0x800733B0: the
/// port's impact, type 1). Blocks and K.O.s play the sound player's own (sound.json `system_impacts`).
fn hit_entries(ctx: &Ctx, t2: &T2Set, r: Mv) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (typ, i) in t2.sounds(r)? {
        let k = ctx.system_start + i as i64 - 3;
        if typ == 2 && (11..=14).contains(&i) {
            out.push(json!({"type": 6, "index": k, "raw": 6 << 12 | k}));
        } else if typ == 3 && (3..=10).contains(&i) {
            out.push(json!({"type": 1, "index": k, "raw": 1 << 12 | k}));
        }
    }
    Ok(out)
}

/// Tekken 2 timed properties as Tekken 3's; the throw's damage at its impact (op 0, the first one).
fn t3_properties(t2: &T2Set, r: Mv, damage: i64) -> Result<Vec<Value>> {
    let (mut out, mut dealt) = (Vec::new(), false);
    for (frame, arg, op) in t2.properties(r)? {
        if let Some(p) = prop_op(op) {
            out.push(json!({"frame": frame, "op": p, "arg": arg}));
        } else if op == 0 && !dealt && damage != 0 {
            out.push(json!({"frame": frame, "op": 0x0B, "arg": damage}));
            dealt = true;
        }
    }
    if damage != 0 && !dealt {
        out.push(json!({"frame": (motion_frames(t2, r)? as i64 * 2 / 3).max(1), "op": 0x0B, "arg": damage}));
    }
    Ok(out)
}

fn new_alias(out: &mut Value) -> Result<usize> {
    let a = list(&mut out["aliases"])?;
    a.push(json!({"bank": "none", "move": null}));
    Ok(a.len() - 1)
}

fn add_move(out: &mut Value, mut mv: Value) -> Result<usize> {
    let moves = list(&mut out["moves"])?;
    let i = moves.len();
    obj(&mut mv)?.insert("index".into(), json!(i));
    moves.push(mv);
    Ok(i)
}

/// Each Tekken 2 throw (attempt slot -> its connect move -> cond 0x02 front / 0x03 side) as a Tekken 3 throw: the
/// thrower's move (Tekken 2 motion, properties, damage at the impact; reaction = a new throw_victims entry naming the
/// victim's move, which goes into this set: victims play the thrower's), linked with transition 0x21; the donor's
/// attempt (1+3 / 2+4: motion, active frames, distance) re-pointed at them, the donor's other connections (back
/// throw) kept.
fn add_throws(
    ctx: &mut Ctx,
    t2: &T2Set,
    donor: &Value,
    out: &mut Value,
    cancels: &mut Vec<Value>,
    own_motions: &[usize],
    throw_victims: &mut Vec<Value>,
) -> Result<()> {
    let d_aliases = arr(&donor["aliases"]);
    let d_moves = arr(&donor["moves"]);
    // a victim's move of the donor to start from (posture lying, its break / getting-up list): the first throw
    // victim the donor has as its own
    let donor_victim = throw_victims.iter().map(int).find_map(|a| {
        let r = d_aliases.get(usize::try_from(a).ok()?)?;
        if r["bank"] != "own" {
            return None;
        }
        let m = d_moves.get(int(&r["move"]) as usize)?;
        (int(&m["posture"]["base"]) == 4).then_some(m)
    });
    let Some(donor_victim) = donor_victim else {
        return Ok(());
    };
    let mut made: HashMap<Mv, i64> = HashMap::new();

    for (slot, button) in THROW_ATTEMPTS {
        let attempt = t2.slot(slot as i64)?.context("no throw attempt move")?;
        let mut connect = None;
        for e in t2.entries(attempt)? {
            if e.2 == 0x2C {
                connect = t2.slot(e.4 as i64)?;
                break;
            }
        }
        let Some(connect) = connect else { continue };
        let mut links: BTreeMap<u8, i64> = BTreeMap::new();
        for e in t2.entries(connect)? {
            if e.0 != 0xFFFF
                && (e.2 == 2 || e.2 == 3)
                && !links.contains_key(&e.2)
                && let Some(r) = t2.slot(e.4 as i64)?
            {
                let ta = match made.get(&r) {
                    Some(&ta) => ta,
                    None => {
                        let ta = throw_move(ctx, t2, out, cancels, own_motions, throw_victims, donor_victim, r)?;
                        made.insert(r, ta);
                        ta
                    }
                };
                links.insert(e.2, ta);
            }
        }
        // the donor's attempts for this button (every alias its lists use for a 1+3 / 2+4 attempt)
        for (a, r) in d_aliases.iter().enumerate() {
            if r["bank"] != "own" {
                continue;
            }
            let mi = int(&r["move"]);
            let m = idx(&donor["moves"], mi)?;
            if m["reaction"]["table"] != "throw_victims"
                || zero_active(m)
                || int(&m["attack_mask"]["raw"]) & 0x400000 == 0
            {
                continue;
            }
            let lst: Vec<&Value> = arr(&donor["cancels"]).iter().skip(int(&m["cancel_start"]) as usize).collect();
            if !lst.iter().take(8).any(|c| int(&c["cond"]) == 2) {
                continue;
            }
            if !iter_donor_entries(ctx, donor)?
                .iter()
                .any(|c| int(&c["cmd"]) & 0xF == button && c.get("dest") == Some(&json!(a)))
            {
                continue;
            }
            let mut new = idx(&out["moves"], mi)?.clone();
            obj(&mut new)?.insert("cancel_start".into(), json!(cancels.len()));
            for c in lst {
                let mut c = c.clone();
                if c["kind"] == "command" && matches!(int(&c["cond"]), 2..=4) {
                    let k = if int(&c["cond"]) == 2 { 2 } else { 3 };
                    if let Some(l) = links.get(&k) {
                        c["dest"] = json!(l);
                    }
                }
                let is_end = c["kind"] == "end";
                cancels.push(c);
                if is_end {
                    break;
                }
            }
            let i = add_move(out, new)?;
            out["aliases"][a] = json!({"bank": "own", "move": i});
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn throw_move(
    ctx: &mut Ctx,
    t2: &T2Set,
    out: &mut Value,
    cancels: &mut Vec<Value>,
    own_motions: &[usize],
    throw_victims: &mut Vec<Value>,
    donor_victim: &Value,
    r: Mv,
) -> Result<i64> {
    let f = t2.fields(r)?;
    let clip = clip_name(t2, r, own_motions)?;
    let vslot = t2.reaction_entry(f.reaction)?[17];
    let v = t2.slot(vslot as i64)?.context("a throw without its victim's move")?;
    // the victim's move: the thrower's Tekken 2 motion for it, lying at the end (face up / down)
    let vf = t2.fields(v)?;
    let mut vmv = donor_victim.clone();
    let n_moves = arr(&out["moves"]).len();
    let o = obj(&mut vmv)?;
    o.insert("anim".into(), json!(clip_name(t2, v, own_motions)?));
    o.insert("frames".into(), json!(motion_frames(t2, v)?));
    o.insert("next".into(), json!({"alias": down(vf.next), "hi": 0}));
    o.insert("properties".into(), json!(t3_properties(t2, v, 0)?));
    o.insert("motion_tag".into(), json!(n_moves));
    o.insert("source".into(), vf.source(v));
    let va = new_alias(out)?;
    let vi = add_move(out, vmv)?;
    out["aliases"][va] = json!({"bank": "own", "move": vi});
    throw_victims.push(json!(va));
    // the thrower's move
    let n_moves = arr(&out["moves"]).len();
    let props = t3_properties(t2, r, f.damage as i64)?;
    let sounds = voice_entries(ctx, t2, r, f.damage as i64)?;
    let tmv = json!({"anim": clip, "frames": motion_frames(t2, r)?, "motion_tag": n_moves,
                     "posture": {"raw": 0x842, "base": 2}, "attack_mask": {"raw": 0x800, "mask": 0x800, "level": 8, "level_name": "throw_link"},
                     "next": {"alias": NEUTRAL, "hi": 0}, "damage": 0, "damage_type": 0, "air_drift": 0, "active": [0, 0],
                     "hitstop": 0, "reaction": {"index": throw_victims.len() - 1, "table": "throw_victims"}, "close_range": 0,
                     "flags24": {"raw": 0x30000000, "camera_script": 0}, "bytes_18": {},
                     "properties": props, "sounds": sounds, "source": f.source(r)});
    let i = add_move(out, tmv)?;
    let ta = new_alias(out)?;
    out["aliases"][ta] = json!({"bank": "own", "move": i});
    out["moves"][i]["cancel_start"] = json!(cancels.len());
    cancels.push(end_entry(NEUTRAL, 0, 0, 0, 6));
    Ok(ta as i64)
}

fn iter_donor_entries<'v>(ctx: &'v Ctx, donor: &'v Value) -> Result<Vec<&'v Value>> {
    let st = idx(&donor["moves"], int(&donor["aliases"][NEUTRAL as usize]["move"]))?;
    let mut out = Vec::new();
    for e in arr(&donor["cancels"]).iter().skip(int(&st["cancel_start"]) as usize) {
        if e["kind"] == "call" {
            out.extend(
                ctx.shared_cancels
                    .iter()
                    .skip(int(&e["dest"]) as usize)
                    .take_while(|s| s["kind"] != "return" && s["kind"] != "end"),
            );
        } else if e["kind"] == "end" {
            break;
        } else {
            out.push(e);
        }
    }
    Ok(out)
}

/// A donor entry leading to one of the donor's own attacks (throws excepted).
fn donor_attack(donor: &Value, e: &Value) -> bool {
    if e["kind"] != "command" && e["kind"] != "sequence" {
        return false;
    }
    let Some(a) = arr(&donor["aliases"]).get(int(&e["dest"]) as usize) else {
        return false;
    };
    if a["bank"] != "own" {
        return false;
    }
    let m = &donor["moves"][int(&a["move"]) as usize];
    !zero_active(m) && m["reaction"]["table"] != "throw_victims"
}

pub fn motion_frames(t2: &T2Set, r: Mv) -> Result<u8> {
    let o = t2.motion(r)? as usize;
    let (buf, at) = if r.common { (&t2.d.exe[..], (COMMON_MOTIONS - EXE) as usize + o) } else { (&t2.mv[..], o) };
    buf.get(at).copied().context("a motion past its record")
}
