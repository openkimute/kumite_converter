//! gamedata_moves: moveset (BNS record m+3 / 279..300) decoding for export_gamedata.
//!
//! Field meanings follow docs/reference/tekken3/engine.md §7 and docs/reference/tekken3/{move-properties,attack-paths,reactions}.md.
//! The attack-segment decoder is a port of reference/attackpath (0x8006A3BC / 0x80069714) that works
//! on in-memory buffers instead of reading the disc itself.

use crate::gamedata_core::{Exe, Seq, cmd_info};
use crate::{anim, camera_motion, disc::py_slice};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const MISSING: i64 = 0x3FFFFFFF;
pub const N_ALIASES: usize = 4023;
pub const HEADER_NAMES: [&str; 16] = [
    "count_word",
    "cancels",
    "moves",
    "aliases",
    "sound_lists",
    "property_lists",
    "borrow_bitmap",
    "motions",
    "camera_selectors",
    "camera_keyframes",
    "attack_descriptors",
    "end",
    "pad_30",
    "pad_34",
    "pad_38",
    "pad_3C",
];

// ---------------------------------------------------------------- name tables
type Names = &'static [(i64, &'static str)];
const POSTURE_BITS: Names = &[
    (0x1, "crouching"),
    (0x2, "standing"),
    (0x4, "lying"),
    (0x8, "crouch_guard"),
    (0x10, "stand_guard"),
    (0x20, "throwable_crouching"),
    (0x40, "throwable_standing"),
    (0x80, "throwable_grounded"),
    (0x100, "guard_needs_input"),
    (0x200, "grounded_subclass_facedown"),
    (0x400, "airborne"),
];
const STATE_CODES: Names = &[
    (1, "normal_move"),
    (2, "standing_guard"),
    (3, "neutral_stand"),
    (4, "crouching_move"),
    (5, "crouch_guard"),
    (6, "neutral_crouch"),
    (7, "down"),
    (9, "airborne_jump"),
    (0xA, "state_0A"),
    (0xC, "special_vs_low"),
];
const POSTURE_AI: Names = &[
    (0x10000, "ai_moves_forward"),
    (0x20000, "ai_moves_backward_non_attack"),
    (0x40000, "ai_bit18"),
    (0x80000, "ai_hop_low_profile"),
    (0x100000, "ai_bit20"),
    (0x200000, "ai_special_stance"),
    (0x400000, "ai_evasive_or_throw_type"),
    (0x800000, "ai_never_select"),
];
const LEVELS: Names = &[
    (0, "none"),
    (1, "low"),
    (2, "mid"),
    (3, "special_mid"),
    (4, "high"),
    (5, "level_5"),
    (6, "unblockable"),
    (7, "throw"),
    (8, "throw_link"),
];
const MASK_BITS: Names = &[
    (0x1, "hits_crouching"),
    (0x2, "hits_standing"),
    (0x4, "hits_grounded"),
    (0x8, "crouch_guard_blocks"),
    (0x10, "stand_guard_blocks"),
];
const ATTACK_AI: Names = &[
    (0x10000, "ai_hits_grounded_special"),
    (0x40000, "ai_range_close"),
    (0x80000, "ai_range_mid"),
    (0x100000, "ai_range_long"),
    (0x200000, "ai_evasive_special"),
    (0x400000, "ai_throw"),
    (0x800000, "ai_bit23"),
];
const FLAGS24: Names = &[
    (0x100, "state_181C_a"),
    (0x200, "state_181C_b"),
    (0x400, "unthrowable"),
    (0x800, "not_parryable"),
    (0x1000, "reaction_class"),
    (0x2000, "string_followup"),
    (0x4000, "guard_by_input"),
    (0x10000, "ki_charge"),
    (0x20000, "push_spheres_4_7_off"),
    (0x40000, "keep_hand_pose"),
    (0x80000, "chain_throw"),
    (0x10000000, "commit_root_position"),
    (0x20000000, "camera_tracks_root"),
    (0x40000000, "head_follows_opponent"),
    (0x80000000, "bit31"),
];
const DAMAGE_TYPES: [&str; 4] = ["normal", "throw", "type_2", "type_3"];
const PUSH_MODES: [&str; 8] = [
    "normal",
    "keep_0_and_3_others_1",
    "all_1",
    "all_halved",
    "all_0",
    "halve_0_and_3_others_1",
    "grow_0_shrink_4_5",
    "all_plus_100",
];
const SOUND_TYPES: [&str; 10] = [
    "skipped",
    "impact",
    "voice_cat0",
    "voice_cat1",
    "sfx",
    "sfx",
    "whoosh_unconsumed",
    "impact",
    "sound_script",
    "voice_control",
];
const PROP_OPS: Names = &[
    (1, "shake_small"),
    (2, "shake_medium"),
    (3, "shake_large"),
    (4, "ground_impact"),
    (5, "nop"),
    (6, "effect_at_bone_own"),
    (7, "effect_at_bone_other"),
    (8, "effect_at_bone_kind0"),
    (9, "effect_at_bone_kind1"),
    (0xA, "effect_at_bone_kind3"),
    (0xB, "damage_opponent"),
    (0xC, "damage_self"),
    (0xD, "heal_self"),
];
const THROW_POSTURE: [&str; 4] = ["standing", "crouching", "grounded_faceup", "grounded_facedown"];
const THROW_DIR: [&str; 4] = ["front", "side_2", "side_1", "back"];
const COND_NAMES: Names = &[
    (0, "always"),
    (1, "own_attack_contact"),
    (0x12, "throw_oblique_front"),
    (0x13, "throw_oblique_side"),
    (0x14, "throw_front_any"),
    (0x15, "throw_side_2_any"),
    (0x16, "throw_side_1_any"),
    (0x17, "throw_back_any"),
    (0x18, "opp_within_param"),
    (0x19, "opp_beyond_param"),
    (0x1A, "own_hit_landed"),
    (0x1B, "opp_blocking"),
    (0x1C, "own_attack_blocked"),
    (0x1D, "opp_not_in_throw_link"),
    (0x1E, "opp_attacking"),
    (0x1F, "opp_attacking_standing"),
    (0x20, "opp_attacking_crouching"),
    (0x21, "opp_not_attacking"),
    (0x22, "opp_standing"),
    (0x23, "opp_crouching"),
    (0x24, "back_turned"),
    (0x25, "back_turned_opp_side_plus"),
    (0x26, "back_turned_opp_side_minus"),
    (0x27, "self_quadrant_front"),
    (0x28, "self_quadrant_side_plus"),
    (0x29, "self_quadrant_back"),
    (0x2A, "self_quadrant_side_minus"),
    (0x2B, "opp_quadrant_front"),
    (0x2C, "opp_quadrant_side_1"),
    (0x2D, "opp_quadrant_back"),
    (0x2E, "opp_quadrant_side_2"),
    (0x2F, "never"),
    (0x30, "high_attack_incoming_close"),
    (0x31, "stun_pool_zero"),
    (0x32, "stun_pool_nonzero"),
    (0x33, "contact_flag_D5"),
    (0x34, "contact_flag_D5_opp_grounded"),
    (0x35, "opp_lying"),
    (0x36, "opp_not_lying"),
    (0x37, "opp_down_within_param"),
    (0x38, "opp_lying_self_back_turned"),
    (0x39, "opp_hit_or_blocked_not_airborne"),
    (0x3A, "escape_press_1"),
    (0x3B, "escape_press_2"),
    (0x3C, "escape_press_1_2"),
    (0x3D, "self_ko"),
    (0x3E, "side_flag_C6_clear"),
    (0x3F, "side_flag_C6_set"),
    (0x40, "side_flag_C6_clear_back_turned"),
    (0x41, "side_flag_C6_set_back_turned"),
    (0x42, "cond_42_alive"),
    (0x43, "ki_charge_active"),
    (0x44, "ki_charge_inactive"),
    (0x45, "reversal_struck"),
    (0x46, "reversal_high_parry"),
    (0x47, "reversal_mid_parry"),
    (0x48, "reversal_low_parry"),
    (0x49, "reversal_high_mid"),
    (0x4A, "reversal_vs_opp_alias"),
    (0x4B, "reversal_vs_motion_family"),
];
const TRANSITIONS: Names = &[
    (0, "hard_cut"),
    (2, "continue_frame"),
    (3, "continue_frame"),
    (4, "blend"),
    (5, "blend"),
    (6, "restart_keep_facing"),
    (0xB, "attack_start"),
    (0xC, "attack_start"),
    (0xD, "attack_start"),
    (0xE, "attack_start"),
    (0x11, "commit_only"),
    (0x1C, "locomotion_getup"),
    (0x1E, "hit_reaction"),
    (0x21, "throw_link"),
    (0x22, "throw_victim_link"),
    (0x23, "block_reaction"),
    (0x24, "hit_reaction_air"),
    (0x25, "hit_reaction_grounded"),
    (0x26, "hit_reaction_counter"),
    (0x27, "hit_reaction_side_a"),
    (0x28, "hit_reaction_side_b"),
    (0x2A, "hit_reaction_front"),
    (0x2B, "hit_reaction_back"),
    (0x2C, "use_end_entry"),
];

fn lookup(table: Names, v: i64) -> Option<&'static str> {
    table.iter().find(|e| e.0 == v).map(|e| e.1)
}

fn cond_name(c: i64) -> String {
    match c {
        2..=0x11 => format!("throw_{}_{}", THROW_POSTURE[((c - 2) / 4) as usize], THROW_DIR[((c - 2) % 4) as usize]),
        _ => lookup(COND_NAMES, c).map_or_else(|| format!("cond_{c:02X}"), str::to_string),
    }
}

fn transition_name(t: i64) -> String {
    match lookup(TRANSITIONS, t) {
        Some(n) => n.to_string(),
        None if (0x10..0x1E).contains(&t) => "locomotion".into(),
        None => format!("type_{t:02X}"),
    }
}

/// The names of the table's bits set in `v`, in bit order.
pub fn flag_names(v: i64, table: Names) -> Vec<&'static str> {
    table.iter().filter(|e| v & e.0 != 0).map(|e| e.1).collect()
}

pub fn req_decode(r: i64) -> Value {
    match r {
        0 => json!({"kind": "any"}),
        _ if r <= 0x17 => json!({"kind": "moveset_is", "value": r - 1}),
        _ if r <= 0x2E => json!({"kind": "moveset_is_not", "value": r - 0x18}),
        _ if r <= 0x45 => json!({"kind": "opponent_moveset_is", "value": r - 0x2F}),
        _ if r <= 0x5C => json!({"kind": "character_is", "value": r - 0x46}),
        _ => json!({"kind": "character_is_not", "value": r - 0x5D}),
    }
}

/// e = (cmd, req, cond, param, dest, transition, win_start, win_end, exec); `resolve` gives `dest_move`.
pub fn cancel_entry(
    index: i64,
    e: [i64; 9],
    seqs: &BTreeMap<i64, Seq>,
    resolve: Option<&dyn Fn(i64) -> Value>,
) -> Value {
    let [cmd, req, cond, param, dest, tr, ws, we, ex] = e;
    let (kind, text) = cmd_info(cmd, seqs);
    let mut o = json!({"index": index, "cmd": cmd, "kind": kind, "input": text, "req": req, "req_decoded": req_decode(req),
        "cond": cond, "cond_name": cond_name(cond), "param": param, "dest": dest,
        "transition": tr, "transition_type": tr & 0x3F, "transition_name": transition_name(tr & 0x3F),
        "turn_180": tr & 0x80 != 0, "transition_bit6": tr & 0x40 != 0,
        "win_start": ws, "win_end": we, "exec": ex});
    if kind == "call" {
        o["shared_list_start"] = json!(dest);
    } else if let Some(r) = resolve
        && kind != "return"
    {
        o["dest_move"] = r(dest);
    }
    o
}

/// A 12-byte cancel entry '<HBBHHBBBB' at `a`.
pub fn read_cancel(r: &Exe, a: i64) -> Result<[i64; 9]> {
    let (u8_, u16_) = (|o| r.u8(a + o), |o| r.u16(a + o));
    Ok([u16_(0)?, u8_(2)?, u8_(3)?, u16_(4)?, u16_(6)?, u8_(8)?, u8_(9)?, u8_(10)?, u8_(11)?])
}

// ---------------------------------------------------------------- attack paths (port of reference/attackpath)
/// layout code -> (bytes per frame, bytes before frame 0)
fn stride(code: i64) -> Option<(i64, i64)> {
    Some(match code {
        0 => (10, 0),
        1 => (5, 5),
        2 => (10, 10),
        3 | 4 => (15, 5),
        5 => (20, 0),
        6 | 7 | 9 => (10, 5),
        10 | 12 | 13 | 14 => (15, 0),
        _ => return None,
    })
}

type Pt = Option<[i64; 3]>;

pub fn decode_point(buf: &[u8], p: i64) -> [i64; 3] {
    // `buf[p:p + 5]` (clamped), little-endian
    let b = usize::try_from(p).map_or(&[][..], |p| py_slice(buf, p, p + 5));
    let v = b.iter().rev().fold(0i64, |v, &x| v << 8 | x as i64);
    [(v & 0x1FFF) - 0x1000, ((v >> 13) & 0x1FFF) - 0x1000, ((v >> 26) & 0x3FFF) - 0x2000]
}

fn at(buf: &[u8], o: i64) -> Result<i64> {
    usize::try_from(o)
        .ok()
        .and_then(|o| buf.get(o))
        .map(|&b| b as i64)
        .context("an attack descriptor runs past its data")
}

/// One call of 0x8006A3BC with the actor at the origin facing 0 (character space):
/// -> (segments, count, ends) or None.
#[allow(clippy::type_complexity)]
fn build(
    buf: &[u8],
    o: i64,
    f: i64,
    mode: i64,
    prev: Option<(Pt, Pt)>,
) -> Result<Option<([[Pt; 2]; 2], usize, (Pt, Pt))>> {
    if at(buf, o)? >= 0x18 {
        return Ok(None);
    }
    let fmt = at(buf, o + 4)? << 8 | at(buf, o + 5)?;
    let code = fmt & 15;
    let alt = if mode == 1 { (fmt & 0x7FFF) >> 4 } else { 0 };
    let Some((per, pre)) = stride(code) else { return Ok(None) };
    let s2 = std::cell::Cell::new(o + alt + 6 + per * f + pre);
    let nxt = || {
        s2.set(s2.get() + 5);
        Some(decode_point(buf, s2.get() - 5))
    };
    let (mut c0, mut c1): ([Pt; 2], [Pt; 2]) = ([None, None], [None, None]);
    let first = f == 0 || prev.is_none();
    if matches!(code, 1 | 4 | 6 | 7 | 2) {
        let back = if code == 2 { 10 } else { 5 };
        c0[0] = if first { Some(decode_point(buf, s2.get() - back)) } else { prev.and_then(|p| p.0) };
        c0[1] = nxt();
    } else {
        c0[0] = nxt();
        c0[1] = nxt();
    }
    match code {
        2 | 3 => {
            let back = if code == 2 { 10 } else { 15 };
            c1[0] = if first { Some(decode_point(buf, s2.get() - back)) } else { prev.and_then(|p| p.1) };
            c1[1] = nxt();
        }
        4 | 5 => c1 = [nxt(), nxt()],
        6 | 10 => c1 = [nxt(), c0[1]],
        7 | 11 => c1 = [c0[1], nxt()],
        12 => c1 = [nxt(), c0[0]],
        13 => c1 = [c0[0], nxt()],
        14 => {
            let p = nxt();
            c1 = [p, p];
        }
        9 => {
            c1[0] = if first { Some(decode_point(buf, s2.get() - 5)) } else { prev.and_then(|p| p.1) };
            c1[1] = c0[0];
        }
        _ => {}
    }
    let nseg = if at(buf, o + 2)? != 0 { 2 } else { 1 };
    Ok(Some(([c0, c1], nseg, (c0[1], c1[1]))))
}

fn segments(buf: &[u8], o: i64, a0: i64, a1: i64, mode: i64) -> Result<Vec<Value>> {
    let (mut out, mut prev) = (Vec::new(), None);
    for fr in a0..=a1 {
        let r = build(buf, o, fr - a0, mode, prev)?;
        prev = r.map(|r| r.2);
        let segs: Vec<Value> = r
            .map(|(s, n, _)| {
                s.iter().take(n).enumerate().filter(|(k, s)| *k == 0 || s[0].is_some()).map(|s| json!(s.1)).collect()
            })
            .unwrap_or_default();
        out.push(json!({"frame": fr, "segments": segs}));
    }
    Ok(out)
}

// ---------------------------------------------------------------- moveset
/// (buffer, offset, builtin key)
type Loc<'a> = (&'a [u8], i64, Option<i64>);

pub struct Moveset<'a> {
    pub d: &'a [u8],
    r: Exe<'a>,
    exe: Exe<'a>,
    seqs: &'a BTreeMap<i64, Seq>,
    common: Option<&'a Moveset<'a>>,
    is_common: bool,
    pub h: [i64; 16],
    pub count: i64,
    pub moveset_id: i64,
    prefix: String,
    pub motions: Vec<usize>,
    motion_index: HashMap<usize, usize>,
    has_aliases: bool,
    aliases: Option<Vec<i64>>,
}

impl<'a> Moveset<'a> {
    pub fn new(
        data: &'a [u8],
        exe: Exe<'a>,
        seqs: &'a BTreeMap<i64, Seq>,
        common: Option<&'a Moveset<'a>>,
        is_common: bool,
        prefix: Option<&str>,
    ) -> Result<Moveset<'a>> {
        let r = Exe::with_base(data, 0);
        let hv = r.u32s(0, 16).context("a moveset is shorter than its header")?;
        let h: [i64; 16] = std::array::from_fn(|k| hv[k]);
        let count = h[0] >> 16;
        let motions = if prefix.is_some() {
            // a set whose motions are packed without the u16 prefix the scan expects (the attract embu, record 0):
            // its motions are the ones its moves point at
            let mut s = BTreeSet::new();
            for i in 0..count {
                s.insert(((r.u32(h[2] + 56 * i)? & 0x3FFFFFFF) + h[7]) as usize);
            }
            s.into_iter().collect()
        } else {
            anim::find_motions(data, h[7] as usize, h[8] as usize)?
        };
        let has_aliases = h[3] != h[4];
        Ok(Moveset {
            d: data,
            r,
            exe,
            seqs,
            common,
            is_common,
            h,
            count,
            moveset_id: r.u8(1)?,
            prefix: prefix.unwrap_or(if is_common { "common" } else { "motion" }).to_string(),
            motion_index: motions.iter().enumerate().map(|(k, &o)| (o, k)).collect(),
            motions,
            has_aliases,
            aliases: if has_aliases { Some(r.u32s(h[3], N_ALIASES)?) } else { None },
        })
    }

    fn word(&self, i: i64) -> Result<Vec<i64>> {
        self.r.u32s(self.h[2] + 56 * i, 14)
    }

    fn common(&self) -> Result<&Moveset<'a>> {
        self.common.context("a moveset uses the common bank without one")
    }

    pub fn resolve(&self, a: i64) -> Value {
        let Some(v) = self.aliases.as_ref().and_then(|al| al.get(usize::try_from(a).ok()?)) else {
            return Value::Null;
        };
        match *v {
            MISSING => Value::Null,
            v if v & 0x40000000 != 0 => json!({"bank": "common", "move": v & 0xFFFF}),
            v => json!({"bank": "own", "move": v}),
        }
    }

    /// -> (name, index, bank, offset)
    fn anim_name(&self, w0: i64) -> Result<(Option<String>, Option<usize>, &'static str, i64)> {
        let off = w0 & 0x3FFFFFFF;
        let (idx, prefix, bank) = if w0 & 0x40000000 != 0 && !self.is_common {
            let c = self.common()?;
            (c.motion_index.get(&((off + c.h[7]) as usize)).copied(), "common", "common")
        } else {
            let bank = if self.is_common { "common" } else { "own" };
            (self.motion_index.get(&((off + self.h[7]) as usize)).copied(), self.prefix.as_str(), bank)
        };
        Ok((idx.map(|i| format!("{prefix}_{i:03}")), idx, bank, off))
    }

    /// The motion's family tag (the s16 before its header; reversal cond 0x4B, 0x8002D264).
    fn motion_tag(&self, w0: i64) -> Result<i64> {
        let off = w0 & 0x3FFFFFFF;
        let ms = if w0 & 0x40000000 != 0 && !self.is_common { self.common()? } else { self };
        let o = off + ms.h[7] - 2;
        Ok(if 0 <= o && o < ms.d.len() as i64 - 1 { ms.r.s16(o)? } else { 0 })
    }

    fn frames_of(&self, bank: &str, off: i64) -> Result<usize> {
        let ms = if bank == "common" && !self.is_common { self.common()? } else { self };
        anim::frames(ms.d, usize::try_from(off + ms.h[7]).context("a motion offset is negative")?)
    }

    fn sound_list(&self, idx: i64) -> Result<Value> {
        if idx == 0xFFFFFFFF {
            return Ok(Value::Null);
        }
        let (mut p, mut out) = (self.h[4] + 2 * idx, Vec::new());
        loop {
            let v = self.r.u16(p)?;
            p += 2;
            if v == 0xFFFF {
                break;
            }
            let kind = SOUND_TYPES.get((v >> 12) as usize).unwrap_or(&"unknown");
            out.push(json!({"raw": v, "type": v >> 12, "index": v & 0xFFF, "kind": kind}));
        }
        Ok(Value::Array(out))
    }

    fn prop_list(&self, idx: i64) -> Result<Option<Vec<Value>>> {
        if idx == 0xFFFFFFFF {
            return Ok(None);
        }
        let (mut p, mut out) = (self.h[5] + 4 * idx, Vec::new());
        loop {
            let (fr, w) = (self.r.u16(p)?, self.r.u16(p + 2)?);
            p += 4;
            if fr == 0 {
                break;
            }
            let (op, arg) = (w >> 8, w & 0xFF);
            let mut e =
                json!({"frame": fr, "op": op, "op_name": lookup(PROP_OPS, op).unwrap_or("unknown"), "arg": arg});
            if (0x0E..=0x3D).contains(&op) {
                let v = op - 0x0E;
                e["op_name"] = json!("hand_pose");
                e["hand"] = json!(["both", "A", "B"].get((v >> 4) as usize).unwrap_or(&"?"));
                e["kind"] = match op {
                    0x14 => json!(0),
                    0x15 => json!("model_default"),
                    _ => json!(v & 7),
                };
            }
            out.push(e);
        }
        Ok(Some(out))
    }

    /// record+0x28 as relocated by 0x8002CE14 -> (buffer, offset, builtin key or None).
    fn descriptor(&self, v: i64) -> Result<Option<Loc<'a>>> {
        if v == 0 || (0x7FF0..=0x7FFB).contains(&v) {
            let t = 0x80017C10;
            for k in 0..13 {
                let (key, ptr) = (self.exe.u16(t + 8 * k)?, self.exe.u32(t + 8 * k + 4)?);
                if key == v {
                    return Ok(Some((self.exe.d, self.exe.off(ptr), Some(v))));
                }
            }
            return Ok(None);
        }
        Ok(Some((self.d, self.h[10] + v - 4, None)))
    }

    fn desc0(&self, w: &[i64]) -> Result<Value> {
        Ok(match self.descriptor(w[10])? {
            None => Value::Null,
            Some((buf, o, _)) => json!(at(buf, o)?),
        })
    }

    fn attack(&self, w: &[i64], a0: i64, a1: i64) -> Result<Value> {
        let v = w[10];
        let Some((buf, o, builtin)) = self.descriptor(v)? else {
            return Ok(json!({"descriptor": v, "error": "unresolved"}));
        };
        let b: Vec<i64> = (0..6).map(|k| at(buf, o + k)).collect::<Result<_>>()?;
        let fmt = b[4] << 8 | b[5];
        let alt_offset = (fmt & 0x7FFF) >> 4;
        let mut res = json!({"descriptor": v, "builtin": builtin, "header_bytes": b,
            "bones": {"end0": b[0], "start0": b[1], "end1": b[2], "start1": b[3]},
            "fmt": fmt, "layout_code": fmt & 15, "alt_offset": alt_offset,
            "object_driven": b[0] >= 0x18, "segment_count": if b[2] != 0 { 2 } else { 1 }});
        if a0 > 0 && a1 >= a0 && v != 0 {
            res["frames"] = json!(segments(buf, o, a0, a1, 0)?);
            if alt_offset != 0 && builtin.is_none() {
                res["alt_frames"] = json!(segments(buf, o, a0, a1, 1)?);
            }
        } else {
            res["frames"] = json!([]);
        }
        Ok(res)
    }

    fn mov(&self, i: i64) -> Result<Value> {
        let w = self.word(i)?;
        let b = self.r.bytes(self.h[2] + 56 * i, 56);
        let b = |k: usize| b.get(k).map_or(0, |&x| x as i64);
        let (name, idx, bank, off) = self.anim_name(w[0])?;
        let (base, pbits) = (w[1] & 7, w[1]);
        let st = (pbits >> 11) & 0x1F;
        let mask = w[2] & 0xFFFF;
        let lvl = mask >> 8;
        let props = self.prop_list(w[8])?;
        let dmg = w[5] & 0x3FFF;
        let bonus: i64 = props.iter().flatten().filter(|p| p["op"] == 0x0B).filter_map(|p| p["arg"].as_i64()).sum();
        let (push, rx) = (w[12] & 0xFFFF, w[12] >> 16);
        let dt = (w[5] >> 14) & 3;
        let opt = |v: i64| if v == 0xFFFFFFFF { Value::Null } else { json!(v) };
        Ok(json!({
            "index": i, "anim": name, "motion": {"raw": w[0], "bank": bank, "offset": off, "index": idx},
            "motion_tag": self.motion_tag(w[0])?,
            "frames": self.frames_of(bank, off)?,
            "posture": {"raw": pbits, "base": base, "names": flag_names(pbits & 0x7FF, POSTURE_BITS), "state_code": st,
                        "state_name": lookup(STATE_CODES, st).map_or_else(|| format!("state_{st:02X}"), str::to_string),
                        "ai_hints": flag_names(pbits & 0xFFFF0000, POSTURE_AI)},
            "attack_mask": {"raw": w[2], "mask": mask, "level": lvl,
                            "level_name": lookup(LEVELS, lvl).map_or_else(|| format!("level_{lvl}"), str::to_string),
                            "bits": flag_names(mask & 0xFF, MASK_BITS), "ai_hints": flag_names(w[2] & 0xFFFF0000, ATTACK_AI)},
            "cancel_start": w[3],
            "next": {"alias": w[4] & 0xFFFF, "hi": w[4] >> 16, "move": self.resolve(w[4] & 0xFFFF)},
            "damage": dmg, "damage_runtime": dmg + bonus, "damage_type": dt,
            "damage_type_name": DAMAGE_TYPES[dt as usize], "damage_word_hi": w[5] >> 16,
            // record +0x16 (s16): the drift along the facing in the jump window, per frame /2048 (0x8003F3A8)
            "air_drift": if w[5] >> 31 != 0 { (w[5] >> 16) - 0x10000 } else { w[5] >> 16 },
            "bytes_18": {"frame_count_ram": b(0x18), "jump_start": b(0x19), "jump_end": b(0x1A), "hold_frame": b(0x1B)},
            "active": [b(0x2D), b(0x2E)], "hitstop": b(0x2C), "byte_2F": b(0x2F),
            "flags24": {"raw": w[9], "camera_script": w[9] & 0xFF, "names": flag_names(w[9] & 0xFFFFFF00, FLAGS24)},
            "push_control": {"raw": push, "mode": push & 15, "mode_name": PUSH_MODES.get((push & 15) as usize).unwrap_or(&"?"),
                             "disable_mask": (push >> 4) & 0xFF, "mask_only_near_active": push & 0x1000 != 0},
            "reaction": {"raw": rx, "table": if rx & 0xF000 != 0 { "throw_victims" } else { "reactions" },
                         "index": if rx & 0xF000 != 0 { rx & 0xFFF } else { rx }},
            "close_range": w[13],
            "sound_list": opt(w[7]), "sounds": self.sound_list(w[7])?,
            "property_list": opt(w[8]), "properties": props,
            "attack": if w[10] == 0 && b(0x2D) == 0 { Value::Null } else { self.attack(&w, b(0x2D), b(0x2E))? },
            // attack descriptor byte 0 (rec+0x28, builtins included) for every move: the AI tests the opponent's
            // (0x80059890: 0x18 Ogre, 0x1A Gon)
            "desc0": self.desc0(&w)?,
            "raw_hex": hex(self.r.bytes(self.h[2] + 56 * i, 56)),
        }))
    }

    fn cancels(&self) -> Result<Vec<Value>> {
        let n = (self.h[2] - self.h[1]).div_euclid(12);
        let res = |a| self.resolve(a);
        let resolve: Option<&dyn Fn(i64) -> Value> = if self.has_aliases { Some(&res) } else { None };
        let r = self.r;
        (0..n).map(|k| Ok(cancel_entry(k, read_cancel(&r, self.h[1] + 12 * k)?, self.seqs, resolve))).collect()
    }

    fn camera_selectors(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut p = self.h[8];
        while p < self.h[9] {
            out.push(json!({"type": self.r.u16(p)?, "cum_prob": self.r.u16(p + 2)?, "value": self.r.s32(p + 4)?}));
            p += 8;
        }
        Ok(out)
    }

    fn borrow_bitmap(&self) -> Value {
        let Some(aliases) = self.aliases.as_ref().filter(|_| self.h[6] != self.h[7]) else {
            return Value::Null;
        };
        let bm = slice(self.d, self.h[6], self.h[7]);
        let bits: Vec<usize> = (0..(bm.len() * 8).min(N_ALIASES)).filter(|&i| bm[i >> 3] >> (i & 7) & 1 != 0).collect();
        let miss: Vec<usize> = aliases.iter().enumerate().filter(|e| *e.1 == MISSING).map(|e| e.0).collect();
        json!({"hex": hex(bm), "bytes": bm.len(), "set_bits": bits.len(), "matches_missing_aliases": bits == miss})
    }

    pub fn export(&self, sources: Value) -> Result<Value> {
        let h = &self.h;
        let sections: Map<String, Value> = (1..12).map(|k| (HEADER_NAMES[k].to_string(), json!(h[k]))).collect();
        let moves: Vec<Value> = (0..self.count).map(|i| self.mov(i)).collect::<Result<_>>()?;
        let aliases = if self.has_aliases {
            json!(
                (0..N_ALIASES as i64)
                    .map(|a| match self.resolve(a) {
                        Value::Null => json!({"bank": "none", "move": null}),
                        v => v,
                    })
                    .collect::<Vec<_>>()
            )
        } else {
            Value::Null
        };
        let selectors = self.camera_selectors()?;
        let hu: [u32; 16] = std::array::from_fn(|k| h[k] as u32);
        let cams = camera_motion::motions(self.exe.d, self.d, &hu, &selectors)?;
        Ok(json!({
            "moveset_id": if self.is_common { Value::Null } else { json!(self.moveset_id) },
            "source_records": sources,
            "size": self.d.len(), "md5": md5_hex(self.d),
            "header": {"relocated_flag": self.r.u8(0)?, "id_byte": self.r.u8(1)?, "move_count": self.count,
                       "sections": sections},
            "motion_count": self.motions.len(),
            "anim_prefix": self.prefix,
            "moves": moves,
            "cancels": self.cancels()?,
            "aliases": aliases,
            "borrow_bitmap": self.borrow_bitmap(),
            "camera_selectors": selectors,
            "camera_keyframes_hex": hex(slice(self.d, h[9], h[10])),
            "camera_motions": cams,
        }))
    }
}

/// `d[a:b]` with Python's clamping (negative ends count from the end).
fn slice(d: &[u8], a: i64, b: i64) -> &[u8] {
    let n = d.len() as i64;
    let c = |x: i64| if x < 0 { (x + n).max(0) } else { x.min(n) } as usize;
    py_slice(d, c(a), c(b))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn md5_hex(b: &[u8]) -> String {
    hex(&Md5::digest(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_as_python() {
        assert_eq!(cond_name(2), "throw_standing_front");
        assert_eq!(cond_name(0x11), "throw_grounded_facedown_back");
        assert_eq!(cond_name(0x4C), "cond_4C");
        assert_eq!(transition_name(0x10), "locomotion");
        assert_eq!(transition_name(0x11), "commit_only");
        assert_eq!(transition_name(0x1D), "locomotion");
        assert_eq!(transition_name(0x1F), "type_1F");
        assert_eq!(decode_point(&[0xFF; 5], 0), [0xFFF, 0xFFF, 0x1FFF]);
        assert_eq!(decode_point(&[0, 0x10], 0), [0, -0x1000, -0x2000]);
    }
}
