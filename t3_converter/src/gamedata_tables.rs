//! gamedata_tables: EXE table decoders for export_gamedata (shared, reactions, rules, roster, stages, music, AI,
//! camera).
//!
//! Sources: docs/reference/tekken3/engine.md §5-13 and docs/reference/tekken3/*.md. Every table carries its RAM address so values can
//! be traced back to the EXE. Values described only in code (formulas, thresholds) are emitted as data with
//! the routine address that implements them.

use crate::embu;
use crate::gamedata_core::{DIR, Exe, Seq, hx, seq_text, step_text};
use crate::gamedata_moves::{cancel_entry, hex, read_cancel};
use crate::mips::{Args, Run, calls, trace, trace_text};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

const SLOT_NAMES: [&str; 14] = [
    "front_standing",
    "front_crouching",
    "counter_standing",
    "counter_crouching",
    "side_a_standing",
    "side_a_crouching",
    "side_b_standing",
    "side_b_crouching",
    "back_standing",
    "back_crouching",
    "block_standing",
    "block_crouching",
    "grounded_faceup",
    "grounded_facedown",
];

fn slot_order() -> Value {
    json!([
        {"case": "blocked", "slot_base": 10, "add_st": true, "entry_mode": 0x23},
        {"case": "airborne (+0xDB or posture & 0x400)", "slot_base": null, "add_st": false, "use": "air_reactions", "entry_mode": 0x24},
        {"case": "posture & 0x200 (lying face-down)", "slot_base": 13, "add_st": false, "entry_mode": 0x25},
        {"case": "posture & 0x004 (lying)", "slot_base": 12, "add_st": false, "entry_mode": 0x25},
        {"case": "counter hit", "slot_base": 2, "add_st": true, "entry_mode": 0x26},
        {"case": "direction 0 (front)", "slot_base": 0, "add_st": true, "entry_mode": 0x2A},
        {"case": "direction 1 (side A)", "slot_base": 4, "add_st": true, "entry_mode": 0x27},
        {"case": "direction 3 (side B)", "slot_base": 6, "add_st": true, "entry_mode": 0x28},
        {"case": "direction 2 (back)", "slot_base": 8, "add_st": true, "entry_mode": 0x2B}])
}
const PARAM_NAMES: [&str; 7] = [
    "pushback_angle",
    "pushback_curve_hit",
    "unused_angle",
    "pushback_curve_counter",
    "pushback_angle_grounded",
    "pushback_curve_grounded",
    "pushback_curve_block",
];

/// Python's `list[i]` (a negative index counts from the end).
fn pyget<T>(v: &[T], i: i64) -> Result<&T> {
    let k = if i < 0 { i + v.len() as i64 } else { i };
    usize::try_from(k).ok().and_then(|k| v.get(k)).with_context(|| format!("index {i} is outside a table"))
}

/// Character `c`'s name: its first selection descriptor's (0x80097D40, +0 the name), in title case.
pub fn char_name(exe: &Exe, c: i64) -> Result<String> {
    let name = exe.cstr(exe.u32(exe.u32(0x80097D40 + 16 * c)?)?);
    let mut prev = false;
    Ok(name
        .chars()
        .map(|ch| {
            let r = if prev { ch.to_ascii_lowercase() } else { ch.to_ascii_uppercase() };
            prev = ch.is_alphabetic();
            r
        })
        .collect())
}

/// Arena `k`'s label: its number and who lives there (the selection descriptors' home stage, +10).
pub fn arena_name(exe: &Exe, k: i64) -> Result<String> {
    let mut who: Vec<String> = Vec::new();
    for s in 0..92 {
        if exe.u8(0x800958C4 + s)? != 255 && exe.u8(exe.u32(0x80097D40 + 4 * s)? + 10)? == k {
            let n = char_name(exe, s / 4)?;
            if !n.is_empty() && !who.contains(&n) {
                who.push(n);
            }
        }
    }
    Ok(format!("Stage {} ({})", k + 1, who.join(", ")))
}

fn bytes_list(b: &[u8]) -> Value {
    json!(b.iter().map(|&x| x as i64).collect::<Vec<_>>())
}

/// `re.sub('%[<set>]', '', s)`
fn strip_fmt(s: &str, set: &str) -> String {
    let (mut out, mut it) = (String::new(), s.chars().peekable());
    while let Some(c) = it.next() {
        if c == '%' && it.peek().is_some_and(|n| set.contains(*n)) {
            it.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// A printf string's pieces between its colour changes (%c), without the font and place codes.
fn printf_parts(s: &str) -> Vec<String> {
    strip_fmt(s, "fHV").split("%c").filter(|p| !p.is_empty()).map(String::from).collect()
}

pub(crate) fn rgb(w: i64) -> Value {
    json!([w & 0xFF, (w >> 8) & 0xFF, (w >> 16) & 0xFF])
}

pub fn shared(exe: &Exe, seqs: &BTreeMap<i64, Seq>) -> Result<Value> {
    let (mut ents, mut starts) = (Vec::new(), vec![0]);
    for k in 0..672 {
        let e = read_cancel(exe, 0x80017C78 + 12 * k)?;
        ents.push(cancel_entry(k, e, seqs, None));
        if (e[0] == 0xC00D || e[0] == 0xC000) && k + 1 < 672 {
            starts.push(k + 1);
        }
    }
    let mut sq = Vec::new();
    for (&c, s) in seqs {
        let steps: Vec<Value> = s
            .steps
            .iter()
            .map(|&v| {
                let b: Vec<i64> = (0..4).filter(|i| (v >> 8) >> i & 1 != 0).map(|i| i + 1).collect();
                json!({"raw": v, "dir": v & 15, "dir_name": DIR.get((v & 15) as usize).unwrap_or(&"?"), "buttons": b,
                       "any_of": v >> 12 & 1 != 0})
            })
            .collect();
        sq.push(
            json!({"cmd": c, "cmd_hex": hx(c, 4), "table": hx(if c < 0xC7FF { 0x800954D0 } else { 0x800955CC }, 0),
                       "address": hx(s.address, 0), "window": s.window, "input": seq_text(&s.steps), "steps": steps}),
        );
    }
    let mut bi = Vec::new();
    for k in 0..13 {
        let (key, ptr) = (exe.u16(0x80017C10 + 8 * k)?, exe.u32(0x80017C10 + 8 * k + 4)?);
        let b: Vec<i64> = exe.bytes(ptr, 6).iter().map(|&x| x as i64).collect();
        if b.len() < 6 {
            bail!("a built-in attack descriptor is outside the EXE");
        }
        let fmt = b[4] << 8 | b[5];
        let meaning = if key == 0 {
            "no attack"
        } else if b[0] >= 0x18 {
            "object-driven hit (byte 0 >= 0x18)"
        } else {
            "sentinel: points decode far behind the actor and never connect"
        };
        bi.push(json!({"key": key, "key_hex": hx(key, 4), "address": hx(ptr, 0), "header_bytes": b,
                       "bones": {"end0": b[0], "start0": b[1], "end1": b[2], "start1": b[3]}, "layout_code": fmt & 15,
                       "object_driven": b[0] >= 0x18, "meaning": meaning}));
    }
    let dt = exe.u16s(0x80010738, 18)?;
    // reversal families (cond 0x4B, 0x8002D264): 52 lists of motion tags (s16, ended by a value <= 0)
    let mut fam = Vec::new();
    for k in 0..52 {
        let (mut a, mut tags) = (exe.u32(0x80095694 + 4 * k)?, Vec::new());
        loop {
            let t = exe.s16(a)?;
            if t <= 0 {
                break;
            }
            tags.push(t);
            a += 2;
        }
        fam.push(tags);
    }
    let side = |o: usize| -> Map<String, Value> { (0..9).map(|n| ((n + 1).to_string(), json!(dt[o + n]))).collect() };
    Ok(json!({
        "reversal_families": {"table": "0x80095694", "lists": "0x800178C4..0x80017BD4", "families": fam,
                              "note": "cond 0x4B: the attacker motion tag (moves[].motion_tag) is in families[param]"},
        "shared_cancels": {"address": "0x80017C78", "count": 672, "list_starts": starts, "entries": ents,
                           "note": "Reached through cancel entries with cmd 0xC00C (dest = start index); 0xC00D returns."},
        "sequences": {"tables": ["0x800954D0 (cmds 0xC00E..0xC04C)", "0x800955CC (cmds 0xC7FF..0xC827)"],
                      "step_format": "u16: bits 0-3 numpad direction, bits 8-11 buttons 1..4, bit 12 any-of",
                      "matching": "0x8002CA84: steps matched backwards through the 60-frame input history within `window` frames; frames with no button press may sit between steps",
                      "entries": sq},
        "attack_builtins": {"table": "0x80017C10", "entries": bi},
        "direction_table": {"address": "0x80010738",
                            "description": "numpad direction (1..9) -> PS1 pad bits, row 0 = facing right (side 0), row 1 = mirrored",
                            "side0": side(0), "side1": side(9)},
        "buttons": {"1": "square (LP)", "2": "triangle (RP)", "3": "cross (LK)", "4": "circle (RK)"},
        "history": {"length": 60, "format": "buttons << 4 | numpad direction (relative to facing), 0x8002C580"},
    }))
}

pub fn reactions(exe: &Exe) -> Result<Value> {
    let mut ents = Vec::new();
    for i in 0..633 {
        let a = 0x80010CE8 + 42 * i;
        ents.push(json!({"index": i, "victims": exe.u16s(a, 14)?, "params": exe.s16s(a + 28, 7)?}));
    }
    let mut curves = Vec::new();
    for k in 0..71 {
        let v = exe.s16s(0x8001075C + 20 * k, 10)?;
        curves.push(json!({"index": 10 * k, "constant_frames": v[0], "velocity": v[1], "impulses": v[2..]}));
    }
    let air_when = [
        "normal juggle hit (front/back)",
        "spike: strike vector Y >= 0x6E or special moves",
        "already in late fall near the ground",
        "juggle hit from side 3",
        "juggle hit from side 1",
        "entry mode 0x1E (cancel-driven)",
    ];
    let mut air = Vec::new();
    for (k, when) in air_when.iter().enumerate() {
        let k = k as i64;
        air.push(
            json!({"index": k, "target_height": exe.u16(0x8001A630 + 4 * k)?, "alias": exe.u16(0x8001A632 + 4 * k)?,
                        "when": when}),
        );
    }
    let mut close = Vec::new();
    for k in 0..58 {
        close.push(json!({"index": k, "reaction_id": exe.u16(0x800177DC + 4 * k)?, "max_distance": exe.s16(0x800177DE + 4 * k)?}));
    }
    let fixed = 0x8001DEC0;
    Ok(json!({
        "reaction_table": "0x80010CE8 (633 x 42 bytes: u16 victims[14], s16 params[7])",
        "slot_names": SLOT_NAMES, "param_names": PARAM_NAMES,
        "slot_selection": {
            "routine": "0x8002EBA4", "st": "0 if victim posture & 2 (standing) else 1",
            "order": slot_order(),
            "direction": "0x80043A94: q = (victim.facing - angle_to_attacker + 0x2000) & 0xFFFF; q>>14 -> 0 front, 1 side A, 2 back, 3 side B",
            "ko": "HP 0 on a clean hit whose reaction returns to neutral (alias 3 or 0x28): re-select with the victim airborne (air table), or alias 0xD66 collapse when global 0x800AFA93 is set. Chip-damage KO becomes a clean hit.",
            "damage_types_with_reaction": "damage type 0, or 2 when the victim is neither airborne nor grounded",
            "reaction_id_lookup": "close range (+0xD4): close_range[record+0x34].reaction_id; id & 0xF000: throw_victims[id & 0xFFF] gives the victim alias directly; mode-8 body hit: body_hit_entry"},
        "entries": ents,
        "pushback_curves": {"address": "0x8001075C", "count": 71,
                            "format": "referenced by u16 index (multiple of 10, in s16 units): n frames of constant velocity, then 8 per-frame impulses",
                            "per_frame": "step = (n-- > 0 ? velocity : 0) + (frame <= 8 ? impulses[frame-1] : 0) + extra(+0x124), along the pushback angle",
                            "entries": curves},
        "air_reactions": {"address": "0x8001A630", "entries": air},
        "air_pushback_curve": {"address": "0x80019D2C", "samples": exe.s16s(0x80019D2C, 8)?},
        "throw_victims": {"address": "0x800174C4", "count": 397,
                          "note": "u16 victim aliases; the last entry overlaps close_range entry 0",
                          "entries": exe.u16s(0x800174C4, 397)?},
        "close_range": {"address": "0x800177DC", "format": "{u16 reaction_id, s16 max_distance}; used when record+0x34 != 0 and distance < max_distance",
                        "entries": close},
        "body_hit_entry": {"address": hx(fixed, 0), "victims": exe.u16s(fixed, 14)?, "params": exe.s16s(fixed + 28, 7)?,
                           "note": "mode-8 (Ball/Force) body collision reaction"},
        "juggle": {"pop_velocity": "vy = clamp(+-100, 4*dmg + 40 - 7*n - (victimVelY + strikeVecY)/8)",
                   "gravity": "y(t) = vy*t - 3*t^2", "spike": {"vy": 400, "horizontal_per_frame": 0x28},
                   "float_factor": "+0x98 = clamp(0..100, dmg + 7*(n+1) + 10)", "routine": "0x8002F074"},
        "stun_pool": {"normal_hit": "min(0x42, |dmg| + old/2)", "throw": "clamp(0x22..0x42, dmg*32/45 + 0x22)",
                      "drain": "after +0x11A < 31: -1/frame, -3 more if button 0x20 pressed, -1 more for any button"},
    }))
}

const SLOT_NAMES_18: [&str; 18] = [
    "root",
    "chest",
    "head",
    "clavicle_A",
    "upperarm_A",
    "forearm_A",
    "hand_A",
    "clavicle_B",
    "upperarm_B",
    "forearm_B",
    "hand_B",
    "pelvis",
    "thigh_A",
    "shin_A",
    "foot_A",
    "thigh_B",
    "shin_B",
    "foot_B",
];
/// inverse of skel.MESH_SLOT (head_b row 20 omitted)
const SLOT_ROW: [i64; 18] = [0, 1, 19, 11, 12, 13, 14, 15, 16, 17, 18, 3, 5, 6, 7, 8, 9, 10];

/// 0x800423F8 / 0x80042410
fn hurt_y_offset(i: i64) -> i64 {
    match i {
        8 => -0x78,
        11 => -0x3C,
        _ => 0,
    }
}

/// Per-frame centre writer 0x80041F6C (tail 0x800423A4..0x800424A0): centre = translation (+0x14) of the bone
/// world matrix actor+0x8F4+0x44*slot, slot = s32 table[i]; hurt 8/11 then get centre.y += offset.
fn sphere_layout(
    exe: &Exe,
    table: i64,
    n: i64,
    base: i64,
    stride: i64,
    hurt: bool,
    zones: Option<&[i64]>,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for i in 0..n {
        let slot = exe.s32(table + 4 * i)?;
        let dy = if hurt { hurt_y_offset(i) } else { 0 };
        let name = pyget(&SLOT_NAMES_18, slot)?;
        let rule = format!("centre = joint origin of slot {slot} ({name})")
            + &if dy != 0 {
                format!(", then centre.y -= {} (PS1 y is down, so {} units UP)", -dy, -dy)
            } else {
                String::new()
            };
        let mut e = json!({"index": i, "record": hx(base + stride * i, 0), "bone_slot": slot, "slot_name": name,
                       "mdl_row": pyget(&SLOT_ROW, slot)?, "gltf_joint": name,
                       "offset_game": [0, dy, 0], "offset_gltf_m": [0.0, (-dy) as f64 * 0.001, 0.0],
                       "offset_frame": "world/actor axes (added after the bone transform, not rotated by the bone)",
                       "rule": rule});
        if let Some(z) = zones {
            e["damage_percent"] = json!(pyget(z, i)?);
        }
        out.push(e);
    }
    Ok(out)
}

pub fn rules(exe: &Exe) -> Result<Value> {
    let zones = exe.u16s(0x8001DEEC, 14)?;
    let hurt_layout = sphere_layout(exe, 0x80096FB8, 14, 0x20C, 0x14, true, Some(&zones))?;
    let push_layout = sphere_layout(exe, 0x80096F40, 8, 0x324, 0x10, false, None)?;
    let (mut hurt, mut push) = (Vec::new(), Vec::new());
    for c in 0..22 {
        hurt.push(
            json!({"character": c, "name": char_name(exe, c)?, "radii": exe.s16s(exe.u32(0x80096FF0 + 4 * c)?, 14)?}),
        );
        push.push(
            json!({"character": c, "name": char_name(exe, c)?, "radii": exe.s16s(exe.u32(0x80096F60 + 4 * c)?, 8)?}),
        );
    }
    let life = exe.u32s(0x800225EC, 8)?;
    let dflt = exe.u32(0x8002260C)?;
    // (the queued aliases a range: addiu -first, sltiu count)
    let queued = -imm(exe, 0x8002EF50)?;
    Ok(json!({
        "fps": 60,
        "bone_slots": {"note": "bone slot numbers used by the move data (timed effect ops 6..0xA, attack paths, sphere layouts) -> glTF joint names",
                       "joints": SLOT_NAMES_18},
        "health": {"default_max": dflt >> 16, "default_raw": dflt, "address": "0x8002260C",
                   "representation": "16.16 fixed point at player +0x3F4 (health) / +0x3F8 (max)",
                   "life_options": life.iter().map(|v| v >> 16).collect::<Vec<_>>(), "life_options_raw": life, "life_table": "0x800225EC"},
        "round_time": {"formula_frames": "(option + 2) * 600", "options_seconds": [20, 30, 40, 50, 60],
                       "default_option": 2, "default_seconds": 40, "timeout_winner": "more health-bar units", "ring_out": false},
        "win_lose_motions": [0xD67, 0xD68, 0xD69, 0xD6A],
        "damage_zones": {"address": "0x8001DEEC", "percent": zones,
                         "formula": "clean hit: base * zone% / 100 (zone = struck hurt-sphere index)"},
        "hurt_spheres": {"table": "0x80096FF0 (22 x pointer -> 14 x s16 radius)", "per_character": hurt,
                         "test": "segment vs vertical cylinder: box reject, clip to [cy-r, cy+r], XZ distance <= r (0x8004770C)",
                         "record": "actor +0x20C + 0x14*i: centre xyz s32, radius s16 +0xC, r^2 s32 +0x10",
                         "centre_writer": "0x80041F6C (loop 0x800423A4-0x8004243C), every frame, slot table 0x80096FB8 (14 x s32)",
                         "centre_source": "bone world matrix actor +0x8F4 + 0x44*slot, translation at +0x14",
                         "zone": "zone index == hurt-sphere index i; damage_percent from 0x8001DEEC",
                         "layout": hurt_layout,
                         "extra": "actor +0x3A4 = copy of hurt sphere 0 centre (0x800424A0)"},
        "push_spheres": {"table": "0x80096F60 (22 x pointer -> 8 x s16 radius)", "per_character": push,
                         "record": "actor +0x324 + 0x10*i: centre xyz s32, radius s16 at +0xC (= +0x330 + 0x10*i)",
                         "centre_writer": "0x80041F6C (loop 0x80042440-0x8004249C), every frame, slot table 0x80096F40 (8 x s32)",
                         "centre_source": "bone world matrix actor +0x8F4 + 0x44*slot, translation at +0x14 (no offset)",
                         "radius_writer": "0x8003EC64 (move +0x30 low nibble selects radius set / disable)",
                         "layout": push_layout},
        "hit_masks": {"0x412": "high", "0x217": "mid", "0x10F": "low", "0x31F": "special_mid", "0x607": "unblockable",
                      "0x706": "throw", "0x800": "throw_link",
                      "bits": {"0x01": "hits crouching", "0x02": "hits standing", "0x04": "hits grounded",
                               "0x08": "blocked by crouch guard", "0x10": "blocked by stand guard"},
                      "test": "hit if mask & victim.posture & 7; blocked if guard_mask & mask and victim faces attacker (+0x30 < 0x4000)",
                      "low_vs_jump": "mask 0x10F whiffs against jump state (+0x69 == 0xC) during [rec+0x19, rec+0x1A-5] and airborne victims"},
        "counter_hit": {"multiplier": [12, 10], "routine": "0x80044B50",
                        "condition": "victim move damage != 0 and victim frame < its first active frame, or either fighter's ki timer (+0x122) running",
                        "bonus_normal": "(0.3*0.3*|victim move damage|) * f / max(startup, f)",
                        "bonus_ki": "0.3 * |victim move damage|", "reaction_slot": "2+st", "pushback_param": 3},
        "close_range": {"multiplier": [3, 2], "zone_scaling": false},
        "juggle": {"first_air_hit_percent": 80, "later_air_hits_percent": 50, "counter": "player +0x94",
                   "pop_reduction_per_hit": 7},
        "chip_damage": {"formula": "base/10 + 2", "enabled_when": "GUARD DAMAGE option (0x800AFA58) or attacker ki timer"},
        "ki_charge": {"frames": 120, "move_flag": "0x24 bit 0x10000"},
        "trades": "both hits apply at full damage, neither is a counter hit",
        "throws": {"damage_type": 1, "connect_range": 1536, "connect_frames_example": [12, 14],
                   "escape": {"1+3 throws": "press 1", "2+4 throws": "press 2", "1+2": "cond 0x3C"},
                   "escape_windows": {"front": [1, 15], "side": [1, 5], "back": null},
                   "first_press_decides": true, "break_frames": 30, "simultaneous": "random pick",
                   "damage": "timed property op 0x0B on the thrower move (no zone/counter/juggle scaling)"},
        // the rules the code keeps for some characters (character ids, +0x18; move sets, +0x1C)
        "characters": {"unthrowable": {"routine": "0x800404E8", "characters": [imm(exe, 0x800404EC)?],
                                       "note": "but by the same character"},
                       "unreversible": {"routine": "0x8002D8CC", "characters": [imm(exe, 0x8002D8D0)?]},
                       "throw_bulk": {"routine": "0x800452CC", "characters": [imm(exe, 0x800452D4)?, imm(exe, 0x800452E0)?],
                                      "note": "the throw distance is the pair's less 40 for each of these"},
                       "sidestep_lean": {"characters": [7],
                                         "note": "past 135 degrees off the opponent: 0x71C, not 0x11C7"},
                       "hitstop_flight": {"routine": "0x8003F248", "not_against_movesets": [imm(exe, 0x8003F254)?],
                                          "note": "a juggle flight goes on through the hit-stop"},
                       // (the routine 0x80039074 zeroes the head's yaw, sp+0x8C, for this character: 0x800392B4)
                       "head_without_yaw": {"routine": "0x80039074", "characters": [imm(exe, 0x800392B8)?]}},
        // the rules the code keeps for some moves (aliases, +0xA0; move sets, +0x16)
        "moves": {"long_hitstop": {"routine": "0x80044310", "alias": imm(exe, 0x80044314)?, "frames": imm(exe, 0x8004431C)?,
                                   "note": "a hit by a move entered by this alias holds the victim these frames, not the move's hit-stop"},
                  // (the second: unless a cancel into one of `unless_queued` is queued, 0x8002EF48, which keeps the
                  // normal air reaction without the strike-down test)
                  "air_spikes": {"routine": "0x8002EE94", "note": "the attacker in this alias of this move set spikes the airborne victim",
                                 "moves": [{"moveset": imm(exe, 0x8002EEF8)?, "alias": imm(exe, 0x8002EF14)?},
                                           {"moveset": imm(exe, 0x8002EF1C)?, "alias": imm(exe, 0x8002EF2C)?,
                                            "unless_queued": (0..imm(exe, 0x8002EF54)?).map(|k| k + queued).collect::<Vec<_>>()}]}},
        "hitstop": "victim frame counter frozen for attacker record +0x2C frames (alias 0x898 sets 30)",
        "frame_advantage": "W_victim - W_attacker + A (W = END entry win_start, A = first active frame)",
        "input_buffer_frames": 5,
        "reaction_slot_selection": {"routine": "0x8002EBA4", "st": "0 standing (posture & 2), 1 otherwise",
                                    "order": slot_order(), "slot_names": SLOT_NAMES},
    }))
}

pub fn roster(exe: &Exe, rec: &[Vec<u8>], model_moveset_file: &BTreeMap<i64, String>) -> Result<Value> {
    let mfile = |m: i64| model_moveset_file.get(&m).map_or(Value::Null, |f| json!(f));
    let mut sels = Vec::new();
    for s in 0..92 {
        let p = exe.u32(0x80097D40 + 4 * s)?;
        let b: Vec<i64> = exe.bytes(p, 12).iter().map(|&x| x as i64).collect();
        if b.len() < 12 {
            bail!("a selection descriptor is outside the EXE");
        }
        let nm = exe.cstr(exe.u32(p)?);
        let mi = exe.u8(0x800958C4 + s)?;
        let m = (mi != 255).then_some(71 + 4 * mi);
        sels.push(json!({"selection": s, "character": s / 4, "costume": s & 3, "name": nm,
                         "model_index": (mi != 255).then_some(mi), "model_record": m,
                         "glb": m.map(|m| format!("model_{m:03}.glb")),
                         "moveset_file": m.map_or(Value::Null, mfile),
                         "moveset_id": b[9], "home_stage": b[10], "bgm_id": b[11],
                         "descriptor": {"address": hx(p, 0), "raw": b, "char_or_announcer_id": b[4], "byte5": b[5],
                                        "byte6": b[6], "byte7": b[7], "player_1A": b[8]}}));
    }
    let mut grid = Vec::new();
    for k in 0..21 {
        let a = 0x80022768 + 6 * k;
        let sel = exe.s16(a + 4)?;
        grid.push(json!({"slot": k, "row": k / 7, "col": k % 7, "x": exe.s16(a)?, "y": exe.s16(a + 2)?,
                         "selection": sel, "character": sel.div_euclid(4), "name": char_name(exe, sel.div_euclid(4))?}));
    }
    let (mut unl, mut a) = (Vec::new(), 0x80098214);
    loop {
        let (t, v) = (exe.u8(a)?, exe.u8(a + 1)?);
        a += 2;
        if t == 0 {
            break;
        }
        let meaning = match t {
            1 => "unlock character",
            2 => "unlock costume bit",
            3 => "unlock mode (+0x37, Theater, likely)",
            4 => "unlock mode (+0x36, Tekken Ball, likely)",
            5 => "unlock everything",
            _ => "?",
        };
        let mut e = json!({"type": t, "arg": v, "meaning": meaning});
        if t == 1 {
            e["name"] = json!(char_name(exe, v)?);
        }
        if t == 2 {
            e["name"] = json!(if v == 8 { "Tiger (Eddy costume)".to_string() } else { format!("costume bit {v}") });
        }
        unl.push(e);
    }
    let mut models = Vec::new();
    for m in (71..276).step_by(4) {
        let size = *rec.get(m as usize).and_then(|r| r.get(4)).context("a model record is too short")?;
        models.push(json!({"model_record": m, "glb": format!("model_{m:03}.glb"), "size_percent": size,
                           "moveset_record": m + 3, "moveset_file": mfile(m)}));
    }
    let mut chars = Vec::new();
    for c in 0..23 {
        let p = exe.u32(0x80097D40 + 16 * c)?;
        let mut selections = Vec::new();
        for s in 4 * c..4 * c + 4 {
            if exe.u8(0x800958C4 + s)? != 255 {
                selections.push(s);
            }
        }
        chars.push(
            json!({"id": c, "name": char_name(exe, c)?, "exe_name": exe.cstr(exe.u32(p)?), "selections": selections}),
        );
    }
    Ok(json!({
        "characters": chars,
        "selections": {"formula": "character*4 + costume", "descriptor_table": "0x80097D40", "model_index_table": "0x800958C4",
                       "note": "Mokujin descriptor says moveset 9 but its own m+3 data is moveset 15 (moveset_file follows the data)",
                       "entries": sels},
        "select_grid": {"address": "0x80022768", "format": "{s16 x, s16 y, s16 selection}", "columns": 7, "rows": 3, "entries": grid},
        "unlocks": {"initial_roster_mask": exe.u32(0x80097EF0)?, "roster_mask_address": "0x80097EF0",
                    "costume_mask_default": exe.u32(0x80097EF4)?, "costume_mask_address": "0x80097EF4",
                    "costume_mask_allowed_bits": 0x50382,
                    "sequence_address": "0x80098214", "sequence": unl},
        "models": models,
    }))
}

pub fn stages(exe: &Exe) -> Result<Value> {
    let mut homes: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for s in 0..92 {
        let p = exe.u32(0x80097D40 + 4 * s)?;
        if exe.u8(0x800958C4 + s)? != 255 {
            homes.entry(exe.u8(p + 10)?).or_default().insert(s / 4);
        }
    }
    let sin = |i: i64| exe.s16(0x8001E8C4 + 2 * (i & 0xFFF));
    let cos = |i: i64| exe.s16(0x8001F0C4 + 2 * (i & 0xFFF));
    let tz = |v: i64| v / 4096; // shift toward zero
    let mut out = Vec::new();
    for k in 0..20i64 {
        let hs: Vec<i64> = homes.get(&k).into_iter().flatten().copied().collect();
        let hn: Vec<String> = hs.iter().map(|&c| char_name(exe, c)).collect::<Result<_>>()?;
        let (name, tex, geo, tile, cols, rows) = if k < 15 {
            (arena_name(exe, k)?, 36 + k, Some(56 + k), 1800, 10, 10)
        } else if k < 19 {
            (format!("Tekken Force stage {}", k - 14), 51 + k - 15, None, 2500, 10, 3)
        } else {
            ("Tekken Ball beach".to_string(), 55, None, 1800, 10, 10)
        };
        let a = 0x80096C78 + 28 * k;
        let b = exe.bytes(a, 28);
        if b.len() < 28 {
            bail!("a stage light is outside the EXE");
        }
        let (c, e) = (exe.u16(a + 12)?, exe.u16(a + 14)?);
        let kd = [tz(cos(e - 0x800)? * cos(c)?), sin(c)?, tz(sin(e - 0x800)? * cos(c)?)];
        out.push(json!({"id": k, "name": name, "glb": format!("stage_{k:02}.glb"), "texture_record": tex, "backdrop_record": geo,
                        "home_characters": hs, "home_names": hn,
                        "floor": {"tile_size": tile, "map_columns": cols, "map_rows": rows},
                        "light": {"address": hx(a, 0), "raw": bytes_list(b),
                                  "ambient": exe.u32(a)?, "ambient_note": "each channel SetBackColor(v >> 4)",
                                  "material_rgbc": bytes_list(&b[4..8]), "key_color_rgb": [b[10], b[9], b[8]],
                                  "key_angles": [c, e], "key_direction_raw": kd,
                                  "key_direction_note": "x = cos(E-0x800)*cos(C), y = sin(C), z = sin(E-0x800)*cos(C) (4096 = 1); normalised and negated into light matrix row 1",
                                  "fill_light": "L3 = (0,-1,0) at half the key colour (0 for stage 17)",
                                  "unknown_10": bytes_list(&b[16..20]), "shadow_angles": [exe.u16(a + 20)?, exe.u16(a + 22)?],
                                  "unknown_18": bytes_list(&b[24..28])}}));
    }
    Ok(json!({"light_table": "0x80096C78 (28 bytes per stage; stage 8 with flag 0x800B6C10 uses entry 12)",
              "floor_rule": "world tile (i, j) shows map row (-j) mod rows, column (i + cols/2) mod cols",
              "stages": out}))
}

pub fn music(exe: &Exe) -> Result<Value> {
    let mut xa = Vec::new();
    for k in 0..50 {
        let v = exe.u32s(0x80024D78 + 12 * k, 3)?;
        xa.push(json!({"index": k, "start_sector": v[0], "end_sector": v[1], "channel": v[2], "file": format!("{k:02}.flac")}));
    }
    let mut users: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for s in 0..92 {
        if exe.u8(0x800958C4 + s)? != 255 {
            users.entry(exe.u8(exe.u32(0x80097D40 + 4 * s)? + 11)?).or_default().insert(char_name(exe, s / 4)?);
        }
    }
    let mut bgm = Vec::new();
    for i in 0..32 {
        let mut cols = Vec::new();
        for c in 0..2 {
            let (a, v) = (exe.u8(0x80024FD0 + 4 * i + 2 * c)?, exe.u8(0x80024FD0 + 4 * i + 2 * c + 1)?);
            let cd = a & 0x80 != 0;
            cols.push(json!({"raw": [a, v], "cdda": cd, "xa_index": (!cd).then_some(a & 0x7F),
                             "cdda_track": cd.then_some(a & 0x7F), "volume": v & 0x7F, "loop": v & 0x80 != 0,
                             "file": if cd { format!("cdda_track{}.flac", a & 0x7F) } else { format!("{:02}.flac", a & 0x7F) }}));
        }
        bgm.push(
            json!({"id": i, "columns": cols, "characters": users.get(&i).into_iter().flatten().collect::<Vec<_>>()}),
        );
    }
    Ok(json!({"bgm_table": "0x80024FD0 (32 ids x 2 columns {xa index | 0x80 = CD-DA track, volume | 0x80 = loop})",
              "column_select": "BGM SELECT option byte 0x80097F05: column 0 = arranged/REMIXED (default, likely), column 1 = arcade; SILENT = none",
              "ending_bgm_id": 4, "files_from": "export_media music/ (NN.flac = XA table index, cdda_track2.flac)",
              "bgm": bgm, "xa_table": {"address": "0x80024D78", "entries": xa}}))
}

const PROFILE_NAMES: &[(i64, &str)] = &[
    (0x04, "attack_cooldown_base"),
    (0x06, "attack_cooldown_0"),
    (0x08, "attack_cooldown_1"),
    (0x0A, "attack_cooldown_2"),
    (0x0C, "attack_cooldown_3"),
    (0x0E, "max_string_length"),
    (0x10, "throw_attempt_prob"),
    (0x12, "throw_escape_prob"),
    (0x14, "string_continue_prob"),
    (0x16, "throw_select_prob"),
    (0x18, "guard_aware_attack_prob"),
    (0x1A, "fast_poke_prob"),
    (0x1C, "guard_prob"),
    (0x1E, "guard_prob_cap"),
    (0x20, "counter_attack_prob"),
    (0x22, "counter_attack_cap"),
    (0x24, "sidestep_prob"),
    (0x26, "in_string_interrupt_prob"),
    (0x28, "in_string_interrupt_cap"),
    (0x2A, "in_string_guard_prob"),
    (0x2C, "in_string_guard_cap"),
    (0x2E, "dead_followup_count_a"),
    (0x30, "dead_followup_prob_a"),
    (0x32, "consecutive_attack_limit_a"),
    (0x34, "dead_followup_prob_b"),
    (0x36, "consecutive_attack_limit_b"),
    (0x38, "dead_followup_prob_c"),
    (0x3A, "evade_and_handler_prob"),
    (0x3C, "pressure_on_stun_prob"),
    (0x46, "gate_always_zero"),
    (0x48, "anti_air_prob"),
    (0x4A, "unused_4A"),
    (0x4C, "keep_walk_back_prob"),
    (0x4E, "unused_4E"),
    (0x50, "unused_50"),
    (0x5A, "parry_prob"),
    (0x5C, "unused_5C"),
    (0x5E, "counter_throw_startup_prob"),
    (0x62, "stun_recovery_prob"),
    (0x64, "unused_64"),
];

/// Canned AI input (player 0x8006183C): one u16 word per frame (dir | buttons<<8, 0 = no input);
/// the first word is always played, then playback continues while the next word is non-zero.
/// Words become pad bits through 0x8002301C (numpad, side-0 frame: 4 = back 0x8000, 6 = forward 0x2000).
fn ai_seq(exe: &Exe, p: i64) -> Result<Value> {
    let limit = 200;
    let mut words = vec![exe.u16(p)?];
    while words.len() < limit {
        let w = exe.u16(p + 2 * words.len() as i64)?;
        if w == 0 {
            break;
        }
        words.push(w);
    }
    let toks: Vec<String> = words
        .iter()
        .map(|&v| {
            if v == 0 {
                return "-".to_string();
            }
            let (mut d, b) = step_text(v);
            if v & 15 == 0 {
                d = "n".into();
            }
            if b.is_empty() {
                d
            } else if d != "n" {
                format!("{d}+{b}")
            } else {
                b
            }
        })
        .collect();
    Ok(json!({"address": hx(p, 0), "words": words, "text": toks.join(",")}))
}

pub fn ai(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    // the arcade ladder (record 5's 0x800B1FCC): the boss (and the player who meets another, 0x800B2010), four
    // draws from the first pool (andi mask), up to eight from the roster less the mask's cleared bits (lui / ori),
    // then the final boss
    let r5 = overlay(rec, 5, 0x800B0548)?;
    let bits = |m: i64| (0..21).filter(|k| m >> k & 1 != 0).collect::<Vec<i64>>();
    let (boss, first) = (imm(&r5, 0x800B1FE8)?, imm(&r5, 0x800B2128)?);
    let lui = |a: i64| -> Result<i64> {
        let w = r5.u32(a)?;
        if w >> 26 != 0xF {
            bail!("not a lui at 0x{a:X}");
        }
        Ok((w & 0xFFFF) << 16)
    };
    let except = bits(!(lui(0x800B2060)? | imm(&r5, 0x800B2064)?) & 0x1F_FFFF);
    let mut profs = Vec::new();
    for d in 0..3 {
        for l in 0..10 {
            let a = 0x800231AC + d * 1100 + l * 110;
            let mut f = Map::new();
            f.insert("wake_up_intervals".into(), bytes_list(exe.bytes(a, 4)));
            for o in (4..110).step_by(2) {
                let name =
                    PROFILE_NAMES.iter().find(|e| e.0 == o).map_or_else(|| format!("off_{o:02X}"), |e| e.1.to_string());
                f.insert(name, json!(exe.s16(a + o)?));
            }
            profs
                .push(json!({"difficulty": d, "difficulty_name": (["EASY", "MEDIUM", "HARD"][d as usize]), "level": l,
                              "address": hx(a, 0), "fields": f, "raw_hex": hex(exe.bytes(a, 110))}));
        }
    }
    let mut spacing = Vec::new();
    for c in 0..22 {
        spacing.push(json!({"character": c, "name": char_name(exe, c)?, "values": exe.u16s(0x80098260 + 12 * c, 6)?}));
    }
    let mut per = Vec::new();
    for m in 0..22 {
        let a = 0x80022D14 + 16 * m;
        let (cp, p4, p8, pc) = (exe.s16(a)?, exe.u32(a + 4)?, exe.u32(a + 8)?, exe.u32(a + 12)?);
        let mut combos = Vec::new();
        if pc != 0 {
            let mut k = pc;
            while exe.s16(k)? >= 0 {
                let sp = exe.u32(k + 4)?;
                combos.push(json!({"prob": exe.s16(k)?, "alias": exe.u16(k + 2)?, "sequence": ai_seq(exe, sp)?}));
                k += 8;
            }
        }
        let opt = |p: i64| if p != 0 { ai_seq(exe, p) } else { Ok(Value::Null) };
        per.push(json!({"moveset": m, "can_parry": cp, "high_mid_reversal": opt(p4)?, "low_parry": opt(p8)?,
                        "combo_preludes": combos}));
    }
    let hdesc = |ms: i64| match ms {
        0 => "vs Paul: his alias 0x2C7 in zone < 3 -> walk back if his frame > 15 else attack now",
        3 => "King self: canned throws (0x800230DE standing, 0x800230A6 crouching, 0x800230C2 downed)",
        4 => "vs Yoshimitsu: react to his special aliases (sidestep / dash / back / filter lows)",
        6 => "Hwoarang self: in stance alias 0xDE3 -> forward 15/16",
        10 | 13 | 16 => "stub",
        14 => "vs Ogre: vs alias 0xDE4 / 0x16E sidestep, back off, punish d/f+2 / d/f+1 / d/f+4 (0x8002309C)",
        19 => "vs Gon: vs alias 0x137 attack now if his zone < 2, back off if >= 4",
        _ => "",
    };
    let mut hand = Vec::new();
    for k in 0..10 {
        let a = 0x80023134 + 12 * k;
        let ms = exe.u16(a)?;
        let h = |p: i64| if p != 0 { json!(hx(p, 0)) } else { Value::Null };
        hand.push(json!({"moveset": ms, "vs_handler": h(exe.u32(a + 4)?), "self_handler": h(exe.u32(a + 8)?),
                         "description": hdesc(ms)}));
    }
    // what each handler does (`kind`, the port's name for the routine) and the aliases its code compares
    let im = |a: i64| imm(exe, a);
    let spin = -im(0x80061BF8)?;
    let routines = json!({
        "0x800628C4": {"kind": "back_off", "aliases": {"attack": [im(0x800628E4)?]},
                       "note": "his alias: walk back past its frame 15 in his zones 1..2, before that attack now (0x800628DC's alias returns as not handled)"},
        "0x8006251C": {"kind": "canned_throws", "aliases": {"reach": [im(0x8006262C)?]},
                       "note": "standing, crouching and downed throws (canned_sequences throws_*) when the reach alias can be input"},
        "0x80061B88": {"kind": "special_moves", "aliases": {"rush": [im(0x80061BA0)?], "stance": [im(0x80061BF0)?],
                                                            "spin": (0..im(0x80061C1C)?).map(|k| spin + k).collect::<Vec<_>>(),
                                                            "ignore": [im(0x80061C24)?], "sweep": [im(0x80061D44)?, im(0x80061D4C)?],
                                                            "flight": [im(0x80061D54)?, im(0x80061E44)?, im(0x80061E4C)?, im(0x80061E54)?]}},
        "0x8006299C": {"kind": "stance_dash", "aliases": {"stance": [im(0x800629B0)?]}},
        "0x80061EF0": {"kind": "fire_dodge", "aliases": {"breath": [im(0x80061F0C)?], "low": [im(0x80061F2C)?]}},
        "0x80062940": {"kind": "close_in", "aliases": {"attack": [im(0x80062950)?]}},
    });
    // (a value the code compares in more than one place: the same in each)
    let same = |at: &[i64]| -> Result<i64> {
        let v: Vec<i64> = at.iter().map(|&a| im(a)).collect::<Result<_>>()?;
        match v.split_first() {
            Some((&first, rest)) if rest.iter().all(|&x| x == first) => Ok(first),
            _ => bail!("the AI compares different values at {at:X?}: {v:?}"),
        }
    };
    // the routine's tests of move sets (+0x16) and aliases (+0xA0); the move set 0 is the zero register (beq zero),
    // written here as 0
    let rules = json!({
        "routine": "0x80059890",
        "fire": {"address": "0x80059A54", "moveset": im(0x80059A58)?, "descriptor": im(0x80059A70)?,
                 "note": "his move (descriptor byte 0) of his move set: +0x200, no stance escape, walks go on"},
        "low_fire": {"address": "0x80059A88", "moveset": im(0x80059A94)?, "descriptor": im(0x80059AAC)?,
                     "note": "+0x201: his attack word gains 0x10000 (a low), the wake-up backs off"},
        "stance": {"address": "0x8005A464", "moveset": im(0x8005A468)?, "alias": im(0x8005A47C)?,
                   "note": "+0x88: no reactions, no okizeme, his string count reset"},
        "chain_throws": {"address": "0x8005A8E0", "format": "a chain throw's next part to these aliases: against these move sets (all when absent) never, only on the coin (lcg & 0x70), or only without it",
                         "entries": [{"aliases": [im(0x8005A8E4)?], "allow": "never"},
                                     {"aliases": [im(0x8005A8EC)?], "against": [im(0x8005A904)?], "allow": "coin"},
                                     {"aliases": [im(0x8005A8F4)?, im(0x8005A91C)?], "against": [0, im(0x8005A93C)?], "allow": "no_coin"},
                                     {"aliases": [im(0x8005A924)?], "against": [0], "allow": "no_coin"},
                                     {"aliases": [im(0x8005A954)?, im(0x8005A984)?], "against": [0, im(0x8005A9A4)?], "allow": "never"}]},
        "escaped_throws": {"address": "0x8005AB28", "note": "his throws the victim always tries to escape (and the evade plans react to); of these move sets (all when absent)",
                           "entries": [{"aliases": [im(0x8005AB2C)?, im(0x8005AB5C)?, im(0x8005AB64)?, im(0x8005AB6C)?]},
                                       {"aliases": [same(&[0x8005AB7C, 0x8005AB90])?], "movesets": [im(0x8005AB74)?, 0]},
                                       {"aliases": [im(0x8005ABA0)?], "movesets": [im(0x8005AB98)?]}]},
        "wake_forward": {"address": "0x8005B418", "movesets": [im(0x8005B41C)?], "note": "my wake-up category 4 is forward, not back"},
        "turn_by_facing": {"addresses": ["0x8005E0B4", "0x8005E180"], "movesets": [same(&[0x8005E0B4, 0x8005E180])?], "note": "turning round: back when turned past 90 degrees, else forward (not by his zone)"},
        "parries": {"addresses": ["0x8005D368", "0x8005FF94"], "aliases": [same(&[0x8005D368, 0x8005FF94])?, same(&[0x8005D36C, 0x8005FF98])?],
                    "note": "reaction 5 inputs them; the rare neutral pick leaves them out"},
        "late_parry": {"address": "0x8005C504", "moveset": im(0x8005C508)?, "until": [im(0x8005C524)?, im(0x8005C51C)? - 1],
                       "alias": im(0x8005D41C)?, "note": "reaction 6: my move set's parry while his attack is these frames away"},
        "counter_alias": {"routine": "0x80061A44", "alias": im(0x80061A4C)?, "note": "his alias: P20 takes 2 off the cooldown"},
        "unthrown": {"addresses": ["0x8005E824", "0x8005F4F8", "0x80062574"], "moveset": same(&[0x8005E824, 0x8005F4F8, 0x80062574])?,
                     "note": "no throws against this move set"},
        "evade_closer": {"address": "0x8005CF7C", "moveset": im(0x8005CF80)?, "distance": im(0x8005CF8C)?,
                         "note": "the evade plans take his distance as this much nearer"},
    });
    let mut force = Vec::new();
    for d in 0..3 {
        force.push((0..5).map(|l| exe.s16s(0x80022FA4 + d * 30 + l * 6, 3)).collect::<Result<Vec<_>>>()?);
    }
    // the canned inputs the AI plays (0x8006183C); `id` names them for the port
    let mut canned = Vec::new();
    for (i, name, a) in [
        ("sidestep_0", "sidestep (n, d x4, n x5), sidestep side 0", 0x8002306E),
        ("sidestep_1", "sidestep (n, u x4, n x5), sidestep side 1", 0x80023086),
        ("backdash", "backdash (n, b, n, b, n x3): evade plans 4 / 6, vs Yoshimitsu", 0x80022C46),
        ("unused_22C3E", "never referenced", 0x80022C3E),
        ("okizeme", "okizeme u/f, u/f+2", 0x80022C2A),
        // King's handler (0x8006251C) picks one of each group: + rand index * 0xE
        ("throws_stand_0", "King throw vs standing: f,d/f+1+3", 0x800230DE),
        ("throws_stand_1", "King throw vs standing: f,d/f+2+4", 0x800230EC),
        ("throws_stand_2", "King throw vs standing: f,d/f+1+2", 0x800230FA),
        ("throws_crouch_0", "King throw vs crouching: d/b+1+3", 0x800230A6),
        ("throws_crouch_1", "King throw vs crouching: d/b+2+4", 0x800230B4),
        ("throws_down_0", "King throw vs downed: d+2+4", 0x800230C2),
        ("throws_down_1", "King throw vs downed: d+2+4", 0x800230D0),
    ] {
        let mut s = ai_seq(exe, a)?;
        s["id"] = json!(i);
        s["doc_label"] = json!(name);
        canned.push(s);
    }
    // motion records of the dash / backdash commands (0x8002CD7C: cmd 0xC001 -> 0x8001006C, 0xC002 -> 0x80010078),
    // laid out like the motion inputs: s16 window, step words until 0; the AI plays the steps for these entries
    let mut motion_records = Vec::new();
    for (cmd, a) in [(0xC001, 0x8001006C), (0xC002, 0x80010078)] {
        let mut steps = Vec::new();
        loop {
            let v = exe.u16(a + 2 + 2 * steps.len() as i64)?;
            if v == 0 {
                break;
            }
            steps.push(v);
        }
        motion_records.push(json!({"cmd": cmd, "address": hx(a, 0), "window": exe.s16(a)?, "steps": steps}));
    }
    Ok(json!({
        "profiles": {"address": "0x800231AC", "layout": "difficulty*1100 + level*110", "probability_unit": 4096,
                     "test": "probabilities pass when (aiLCG & 0xFFF) < P", "entries": profs},
        "spacing": {"address": "0x80098260", "format": "6 x u16 per character: [0..3] reach boundaries, [4] unused, [5] body offset",
                    "entries": spacing},
        "zone_masks": {"my_zone": exe.u32s(0x80098230, 6)?, "his_zone": exe.u32s(0x80098248, 6)?,
                       "bits": {"0x40000": "close", "0x80000": "mid", "0x100000": "long"}},
        "per_moveset": {"address": "0x80022D14", "entries": per},
        "handlers": {"address": "0x80023134", "entries": hand, "routines": routines},
        "rules": rules,
        "force_ai": {"address": "0x80022FA4", "format": "[difficulty][min(level,3)] = {attack interval, cooldown, prob}",
                     "entries": force},
        "canned_sequences": {"word_to_pad": {"table": "0x8002301C", "numpad": exe.u16s(0x8002301C, 10)?,
                                             "buttons": {"0x100": 0x80, "0x200": 0x10, "0x400": 0x40, "0x800": 0x20}},
                             "entries": canned},
        "motion_records": {"routine": "0x8002CD7C", "note": "step words (dir | buttons << 8), one per frame",
                           "entries": motion_records},
        "boss_punish_pads": {"address": "0x8002309C", "pad_words": exe.u16s(0x8002309C, 4)?,
                             "text": ["d/f+2", "d/f+1", "d/f+4", "d/f+4"],
                             "note": "PS1 pad words picked by rand() & 3 in 0x80061EF0"},
        "evade_choices": exe.u16s(0x80022E80, 7)?, "wakeup_categories": exe.u16s(0x80022E74, 6)?,
        "rng": {"ai_lcg": "x = x*5 + 3 (0x800ADCA0); tests use the old value",
                "rand": "x = x*0x41C64E6D + 0x3039; return (x>>16) & 0x7FFF (0x8007A318)",
                "randint": "(n * ((rand() + aiLCG) & 0x7FFF)) >> 15, then the AI LCG steps",
                "selection_rng": "x = x*5 + 1 (0x8009F2A8, 0x8004CE54)"},
        "learning": {"on_hit": "P1C += 10 (<= P1E), P20 += 10 (<= P22); in his string also P2A += 40 (<= P2C), P26 += 20 (<= P28)",
                     "rage": "3rd hit by the same move -> level 9 profile for 600 frames", "low_hp": "opponent HP <= 10%: P18, P1A, P20, P22 = 0",
                     "practice": "cooldowns halved, P1C quartered, no learning, level 4"},
        "arcade": {"stages_1_4": "random distinct from characters 0-9", "stages_5_8": "random from unlocked except Heihachi/Ogre/True Ogre",
                   "stage_9": "Heihachi (Jin if the player is Heihachi)", "stage_10": "Ogre", "level": "stage index 0..9",
                   "costume": "0 for Ogre/True Ogre, else (player costume != 0)",
                   // the same as the port draws it: `draw` distinct characters from `from` (or from everyone but
                   // `except`), then each boss (`instead` for a player who is `player`); the port's TAG ARCADE ends
                   // with `boss_pairs`
                   "ladder": [{"draw": first, "from": bits(imm(&r5, 0x800B2088)?)}, {"draw": imm(&r5, 0x800B2174)? - first, "except": except},
                              {"boss": boss, "player": boss, "instead": imm(&r5, 0x800B2010)?}, {"boss": imm(&r5, 0x800B21CC)?}],
                   "boss_pairs": [[boss, imm(&r5, 0x800B21CC)?]]},
        "team": {"level": "difficulty + 3 + (0x800AE218 & 3), +1 second-to-last CPU member, +3 last, clamp 9",
                 "pool": "unlocked & 0x1F7FFF minus already chosen; least-picked characters"},
        "survival": {"pool": "wins < 7: 0x3FF, < 17: 0xFBFFF, else 0x1FFFFF; & 0x157FFF (never Mokujin/Gon/Dr.B); avoid last 4",
                     "difficulty": "MEDIUM, sometimes HARD",
                     "levels": [{"wins_below": 5, "level": [1, 3]}, {"wins_below": 10, "level": [2, 4]},
                                {"wins_below": 20, "level": [3, 7]}, {"wins_below": 50, "level": [3, 7]},
                                {"wins_from": 50, "rule": "w%10==0: 0; w%5==3: 6..9; else 3..9"}]},
        "dead_code_note": "planned follow-up executor (profile 0x2E..0x38) never fires in retail",
    }))
}

pub fn camera(exe: &Exe) -> Result<Value> {
    let intro = exe.s16s(0x80098508, 30)?;
    let mut presets = Vec::new();
    for k in 0..30 {
        let v = exe.s16s(0x80023F18 + 20 * k, 10)?;
        presets
            .push(json!({"type": k + 1, "in_frames": v[0], "field_1": v[1], "pitch": v[2..6], "yaw_offset": v[6..10]}));
    }
    let mut rows = Vec::new();
    for k in 0..37 {
        let (t, cp, val) = (exe.u16(0x80098370 + 8 * k)?, exe.u16(0x80098372 + 8 * k)?, exe.s32(0x80098374 + 8 * k)?);
        rows.push(json!({"row": k, "type": t, "cum_prob": cp, "value": val}));
    }
    let mut shakes = Vec::new();
    for n in 0..3 {
        let p = exe.u32(0x80097AA4 + 4 * n)?;
        let mut s = Vec::new();
        loop {
            let v = exe.s8(p + s.len() as i64)?;
            if v == -128 {
                break;
            }
            s.push(v);
        }
        let n_frames = s.len();
        shakes.push(json!({"id": n, "address": hx(p, 0), "pitch_offsets": s, "frames": n_frames}));
    }
    let r = |a: i64, n: usize| exe.s16s(a, n);
    Ok(json!({
        "angle_units": "4096 per turn (camera module keeps pitch/yaw << 6)",
        "projection_h": 500, "aspect_y_scale": [7122, 4096],
        "tables": {"sin": {"address": "0x8001E8C4", "note": "rsin(i) = sin[i & 0xFFF], rcos(i) = sin[(i & 0xFFF) + 1024]; 4096 = 1.0",
                           "values": r(0x8001E8C4, 5120)?},
                   "atan": {"address": "0x800210C8", "routine": "0x8004B39C",
                            "note": "ratan2 octant helper: angle = atan[q] + q/2 for q = 1024*minor/major",
                            "values": bytes_list(exe.bytes(0x800210C8, 1025))}},
        "fight": {"distance": 0x158D, "rest_eye_height": -1112, "sample_y_offset": -290, "margin": 150, "fit_px": 280,
                  "dead_zone_px": 48, "lateral_easing": [1, 8], "pullback_out": [1, 8], "pullback_in": [1, 32],
                  "vertical_keep_px": 112, "centre_point_world_y": -450, "centre_drop_px": "100 - 450*H/dist",
                  "vertical_out": {"rate": [1, 8], "max_step": 800}, "vertical_in": {"rate": [1, 16], "max_step": 1200},
                  "yaw": {"start_threshold": 0xE38, "brake_zone": 0x38E4, "freeze_above": 0x1AAAA, "acc": 0x600,
                          "acc_brake": 0x300, "damping": [14, 128], "max_velocity": 0xDDD0, "settle_frames": 32,
                          "freeze_first_frames": 200,
                          "move_caps": {"0x27": 0x200, "0x28": 0x180, "0x29": 0x80, "0x2A": 0x40}},
                  "pitch_lag": {"threshold": 0x38E3, "max_step": 0x100, "easing": [1, 16]},
                  "initial_eye": [0, -1450, -8000], "routines": {"yaw": "0x80064810", "framing": "0x80064E10", "director": "0x800630D8"}},
        "intro": {"address": "0x80098508", "frames_first_round": 120, "frames_other": 53, "blend_frames": 16,
                  "offsets": intro.chunks(3).collect::<Vec<_>>(), "ball_offset": r(0x80098544, 3)?,
                  "formula": "eye = base.eye + off[v]*(1 + sin(0xC00 + t*1024/T)), t counts T..0; v = round counter % 10"},
        "situation_presets": {"address": "0x80023F18", "entries": presets, "slot1_distance": 5517 * 114 / 128},
        "script_selectors": {"address": "0x80098370", "note": "rows for camera index < 0x2B; type 1 = preset, 2 = keyframe script",
                             "rows": rows},
        "shake": {"routine": "0x8004AD34", "pointer_table": "0x80097AA4", "scripts": shakes,
                  "note": "signed byte added to RVIEW.pitch per frame; 0x80 ends"},
        "ko": {"cam_a": {"radius": 3953, "eye_y": -850, "min_distance": 2513, "approach": 8, "easing": [1, 32]},
               "cam_b": {"radius": r(0x8001DF54, 3)?, "side_offset": r(0x8001DF5C, 3)?,
                         "eye_height": r(0x8001DF64, 3)?, "drift": 8, "min_distance": 2308}},
        "win": {"radius": 3978, "radius_large_moveset_11": 6596, "eye_y": -1500, "target_y": -1425, "target_easing": [6, 128],
                "approach_beyond": 3664, "loser_overhead_height": -7853},
        "replay": {"start_distance": 0x2B1A, "target_distance": 0x113D, "easing": [6, 256], "sample_every": 1,
                   "note": "init 0x8006953C, per frame 0x80069414; samples y = min(chest, root.y); yaw and offsets kept in s16"},
        "gsqrt": {"address": "0x8004AF40", "routine": "0x8004AEDC", "values": r(0x8004AF40, 192)?,
                  "note": "GTE SquareRoot0: t = clz(a) & ~1, m = a scaled to 64..255, (tbl[m-64] << ((31-t)>>1)) >> 12"},
        "end": {
            "note": "round end (docs: round-end research): 90 KO frames with the fight camera, the half-speed replay (none on time up), then the winner camera A/B (alternating, B first) or, when a CPU wins the match, the loser camera; every change is a cut; H stays 500",
            "ko_frames": 90, "draw_frames": 75, "loser_frames": 60, "pose_cap": 300,
            "replay": {"pre_roll": 30, "min_frames": 150, "ring": 300, "blink": 16,
                       "variant_a": {"yaw_from": -0x71C7, "yaw_range": 0x1E38E, "offset_degrees": [25, 30]},
                       "variant_b": {"yaw_from": 0x5556, "yaw_range": 0x15555},
                       "offset_easing_shift": 7},
            "aim": {"routine": "0x80046270", "min_distance": 0x16A8, "chest_clamp": 4004, "floor_clamp": -6917, "chest_offset": -290},
            "cam_a": {"init": "0x80046460", "frame": "0x800465BC", "arc_from": 0x2AB, "arc_range": 683, "radius": 3953,
                      "eye_y": -850, "stop_distance": 2513, "approach": 8, "pitch_shift": 5, "yaw_shift": 5},
            "cam_b": {"init": "0x800467A0", "frame": "0x80046A20", "radius": r(0x8001DF54, 3)?,
                      "side": r(0x8001DF5C, 3)?, "height": r(0x8001DF64, 3)?, "drift_shift": 9,
                      "push_distance": 2308, "push_shift": 4, "pitch_shift": 5, "yaw_shift": 4, "height_reach": -7500,
                      "height_shift": 4},
            "overhead": {"frame": "0x8006895C", "height": -7853, "pitch": 0x400, "yaw_from": 0x800, "jitter": [-64, 63]},
            "orbit": {"arc_from": 0x200, "arc_range": 1025, "radius": [3978, 6596], "large_moveset_key": 11,
                      "eye_y": -1500, "target_y": -1425, "stop_distance": [3664, 6282], "approach": 2, "target_easing": [6, 7]},
            "motions": {"win": [0xD67, 0xD68, 0xD69, 0xD6A], "lose_ko": 0xD65, "lose_time_up": 0xD6C},
        },
        "attract": {"program_frames": 0x2D0, "program_lengths": r(0x800242BC, 4)?, "orbit_distance": 0x17F2},
    }))
}

/// Sound codes (docs/reference/tekken3/move-properties.md §5, docs/reference/tekken3/engine.md §9.1).
pub fn sound(exe: &Exe) -> Result<Value> {
    let codes = exe.u16s(0x80097050, 0xD4)?;
    let im = |a: i64| imm(exe, a);
    // (characters 5, 7, 10 and 18 have their own arm table, 0x8001A6FC: the same sounds)
    if exe.u16s(0x8001A6FC, 2)? != exe.u16s(0x8001A710, 2)?
        || [0x8001A704, 0x8001A708, 0x8001A70C].iter().map(|&a| exe.u16(a)).collect::<Result<Vec<_>>>()?
            != [0x8001A718, 0x8001A71C, 0x8001A720].iter().map(|&a| exe.u16(a)).collect::<Result<Vec<_>>>()?
    {
        bail!("the two arm impact tables differ");
    }
    let copy_character = imm(exe, 0x80075454)?;
    // timed sound scripts (move sound entry type 8; interpreter 0x800413E0): u32 {u16 cmd << 12 | frame, s16 code},
    // a frame of 0 ends the script; the pointer table runs until the first null
    let mut scripts = Vec::new();
    loop {
        let p = exe.u32(0x800971F8 + 4 * scripts.len() as i64)?;
        if !(0x80010000..0x800B0000).contains(&p) || scripts.len() >= 1024 {
            break;
        }
        let mut entries = Vec::new();
        for k in 0..64 {
            let (w, code) = (exe.u16(p + 4 * k)?, exe.u16(p + 4 * k + 2)?);
            if w & 0xFFF == 0 {
                break;
            }
            entries.push([w >> 12, w & 0xFFF, code]);
        }
        scripts.push(entries);
    }
    let mut rows = Vec::new();
    for r in 0..23 {
        let row_a = 0x8001AD54 + 24 * r;
        let p = exe.u32(row_a)?;
        let counts = exe.s16s(p, 4)?;
        let n = if counts.iter().all(|c| (0..64).contains(c)) { counts.iter().sum::<i64>() } else { 0 };
        let mut row = json!({"character": r, "address": hx(p, 0), "counts": if n != 0 { counts.clone() } else { vec![0; 4] },
                             "codes": exe.s16s(p + 8, n as usize)?.iter().map(|c| c & 0xFFFF).collect::<Vec<_>>()});
        // the attack voice (0x80041734): row+4 -> [count, sound-list entries...]; the voice lock it sets, row+0xC
        let a = exe.u32(row_a + 4)?;
        if (0x80010000..0x800A0000).contains(&a) {
            let k = exe.s16(a)?;
            if (0..64).contains(&k) {
                let e: Vec<i64> = exe.s16s(a + 2, k as usize)?.iter().map(|c| c & 0xFFFF).collect();
                row["attack"] = json!({"entries": e, "lock": exe.u16(row_a + 0xC)?});
            }
        }
        // the hurt voice (0x80041600): row+8 -> [light count, heavy count, light entries..., heavy entries...]
        let h = exe.u32(row_a + 8)?;
        if (0x80010000..0x800A0000).contains(&h) {
            let (lo, hi) = (exe.s16(h)?, exe.s16(h + 2)?);
            if (0..64).contains(&lo) && (0..64).contains(&hi) {
                let e: Vec<i64> = exe.s16s(h + 4, (lo + hi) as usize)?.iter().map(|c| c & 0xFFFF).collect();
                row["hurt"] = json!({"light": e[..lo as usize], "heavy": e[lo as usize..]});
            }
        }
        rows.push(row);
    }
    let mut remap = Vec::new();
    // Mokujin's sound remap table 0x8001AF7C: {s16 sfx_table index, u16 code} pairs up to -1
    let mut a = 0x8001AF7C;
    while exe.s16(a)? != -1 {
        remap.push([exe.s16(a)?, exe.u16(a + 2)?]);
        a += 4;
    }
    let by_round: Vec<i64> = exe.u16s(0x8001A3BA, 10)?.iter().map(|v| v & 0xFFF).collect();
    Ok(json!({
        "code_format": {"bank": "bits 15-14: 0 silent, 1 system SFX (audio/sfx/sys), 2 announcer (ann), 3 the player's character bank (m<model>)",
                        "voice": "bits 13-10", "volume": "bits 9-6 (index into 0x800278FC)", "program": "bits 5-4",
                        "tone": "bits 3-0; the sample is the tone of that program whose key range holds tone + 60"},
        "scripts": {"address": "0x800971F8", "routine": "0x800413E0",
                    "format": "[cmd, frame, code]: fires when the move reaches frame; cmd 0 = sound-list table index (0x80040C00), cmd 1/2 = a sound-list entry (type << 12 | index: 1/6/7 table sounds, 2-5 voice categories 0-3)",
                    "entries": scripts},
        "volumes": {"address": "0x800278FC", "routine": "0x800754D4 (SePlay)", "note": "volume index -> SPU volume (of 127)",
                    "table": bytes_list(exe.bytes(0x800278FC, 16))},
        "sfx_table": {"address": "0x80097050", "note": "move sound-list index (type 1/4/5/7) -> SePlay code; 0 = none",
                      "codes": codes},
        // (the codes the sound player's own code plays: arm first, leg second where there are two)
        "impact_rules": {"routine": "0x80040EA0", "format": "sound-list entries (type << 12 | table index); an arm: descriptor byte 0 below 0xC (a K.O.: below 0xB)",
                         "ko": [im(0x80040FDC)?, im(0x80040FF8)?], "blocked": [im(0x8004128C)?, im(0x8004126C)?],
                         "wooden_block": [im(0x80041238)?, im(0x80041218)?],
                         "auto": [im(0x8004130C)?, im(0x80041314)?, im(0x8004131C)?, im(0x80041324)?],
                         // (0x80041CB4: by the attacker's damage below 13, 23, 33, more; an arm's lightest by rand
                         // bit 7)
                         "legs": [im(0x80041D04)?, im(0x80041D24)?, im(0x80041D50)?, im(0x80041D6C)?],
                         "arms": {"light": exe.u16s(0x8001A710, 2)?, "tiers": [exe.u16(0x8001A718)?, exe.u16(0x8001A71C)?, exe.u16(0x8001A720)?]},
                         "metal": {"routine": "0x80041B70", "light": im(0x80041B94)?, "ring": [im(0x80041BF4)?, im(0x80041C10)?]},
                         // (rand * 5 >> 8: a shift and an add at 0x80041C60)
                         "wood": {"routine": "0x80041C40", "address": "0x8001A6F0", "entries": exe.u16s(0x8001A6F0, 5)?},
                         // (struck, the character checked at 0x800412AC (+0x18) rings as metal, 0x80041B70; the
                         // port keys it by that character's voice row: its first selection descriptor's byte 8, +0x1A)
                         "metal_row": exe.u8(exe.u32(0x80097D40 + 16 * imm(exe, 0x800412B0)?)? + 8)?},
        // (a jump's take-off 0x80041830, its landings 0x80040DF4, the attack's swing 0x800418F8 -> 0x80041B30)
        "move_sounds": {"take_off": im(0x80041858)?, "landing": im(0x800418E8)?, "air_landing": im(0x800418A0)?,
                        "swing": im(0x80041B44)?, "format": "sound-list entries (type << 12 | table index)"},
        "announcer": {
            "note": "move sound-list table indices (play with 0x80040C00(index)); traced in the round code",
            "round": 0x3B, "final_round": 0x3A, "fight": 0x3C,
            "round_numbers": {"address": "0x8001A3BA", "by_round": by_round},
            "call_frames": {"routine": "0x8003DAC4", "first_round": {"round": 61, "number": 100, "fight": 150},
                            "other_rounds": {"round": 1, "number": 40, "fight": 90},
                            "final_round_when": "round number (0x80095484) >= round limit (0x800ADF3C): no number"},
            "result": {"routine": "0x8003DEC0", "flags_routine": "0x8003E474",
                       "flags": {"1": "time up", "2": "K.O.", "4": "double K.O.", "8": "perfect (winner at full health)",
                                 "16": "winner below 0xCD/0x1000 of the bar", "32": "draw"},
                       "order": ["time_up", "perfect", "double_ko", "great", "ko"],
                       "time_up": 0x13, "perfect": 0x19, "double_ko": 0x15, "great": 0x18, "ko": 0x1A},
            "match": {"routine": "0x8003E060", "you_win": 0x11, "you_lose": 0x12, "draw": 0x14,
                      "rule": "equal wins: draw; else you_win if the winner is a human player (0x800ADF10 bits), else you_lose"},
            "continue": {"routine": "0x80050100", "start": 0x16, "count": im(0x80050140)? & 0xFFF, "zero": im(0x80050134)? & 0xFFF,
                         "count_rule": "index count + seconds left (8..1)",
                         "labels": "start = \"continue?\", zero = \"game over\" [G: by where they play]"},
        },
        // (the character compared at 0x80075450 (+0x18); its voice row: its first selection descriptor's byte 8)
        "copy_voice_remap": {"routine": "0x800753BC -> 0x80075548", "character": copy_character,
                          "row": exe.u8(exe.u32(0x80097D40 + 16 * copy_character)? + 8)?,
                          "note": "a sound owned by Mokujin (+0x18 = 15; his voice row +0x1A: row) whose code equals sfx_table[from] plays `to` (a code)",
                          "pairs": remap},
        "type9_sounds": {"table": "0x8001AD4C", "codes": exe.u16s(0x8001AD4C, 3)?,
                         "note": "type-9 entry 3: one of these (k = (rand & 0xFF) * 3 >> 8), unused by the data"},
        "voice_rows": {"address": "0x8001AD54", "stride": 24, "by": "actor +0x1A: byte 8 of the selection descriptor (roster.json player_1A), not the character id (they differ from Panda on)",
                       "note": "type-2 entries: category 0 voice codes[sum(counts[:cat]) + index], frame 1, with a voice lock. attack (0x80041734): as a damaging move starts (+0xDA), the lock clear and nothing voiced yet, 1 in 4 (rand & 3 == 0): entries[(rand & 0xFF) * count >> 8], then the lock (0x800971E8) = lock frames. hurt (0x80041600): a clean hit (+0xD0) on a living fighter, its cooldown (0x800971DC) clear, on odd frames (0x800AFA4C & 1): light (attacker +0x5E < 20) or heavy entries likewise, then the cooldown = 40. K.O. (0x80040F0C, +0xDE): a category-2 voice (entry 0x4000 | (rand & 0xFF) * counts[2] >> 8). The lock and the cooldown count down a frame each (0x80041920). Entries are sound-list codes: type 2-5 voice categories 0-3.",
                       "rows": rows},
    }))
}

/// PS1 CLUT word -> VRAM (x, y).
fn clut_xy(word: i64) -> Value {
    json!([16 * (word & 0x3F), word >> 6])
}

/// PS1 tpage word -> VRAM page (x, y) in 16-bit words and the depth in bits.
fn tpage_xy(word: i64) -> (Value, i64) {
    (json!([64 * (word & 0xF), 256 * ((word >> 4) & 1)]), [4, 8, 16, 16][((word >> 7) & 3) as usize])
}

fn overlay(rec: &[Vec<u8>], r: usize, base: i64) -> Result<Exe<'_>> {
    Ok(Exe::with_base(rec.get(r).with_context(|| format!("no record {r}"))?, base))
}

/// The sprites that the straight code from `pc` draws with 0x8004DB9C (ot, prim, x, y, w, h, VRAM x, VRAM y, CLUT
/// word, mode), `n` of them.
fn sprites(e: &Exe, pc: i64, n: usize) -> Result<Vec<Value>> {
    Ok(calls(e, pc, n, 0x8004DB9C)?
        .into_iter()
        .map(|[_, _, x, y, w, h, vx, vy, clut, mode, ..]| {
            let bpp = [4, 8, 16, 16][((mode >> 7) & 3) as usize];
            json!({"x": x, "y": y, "w": w, "h": h, "vram": [vx, vy], "bpp": bpp, "clut": clut_xy(clut)})
        })
        .collect())
}

/// The first text that the code from `pc` prints (see `printf_text`).
pub(crate) fn printf_call(e: &Exe, pc: i64) -> Result<Value> {
    printfs_to(e, pc, &Run::default(), 1, PRINTF)?.into_iter().next().context("no printf call")
}

/// The texts that the code from `pc` prints in `run` until it returns (see `printf_text`).
pub(crate) fn printfs(e: &Exe, pc: i64, run: &Run) -> Result<Vec<Value>> {
    printfs_to(e, pc, run, usize::MAX, PRINTF)
}

/// The `k`-th text that the code from `pc` prints in `run` (the others' formats may be built by calls it steps
/// over).
fn printf_nth(e: &Exe, pc: i64, run: &Run, k: usize) -> Result<Value> {
    let t = trace_text(e, pc, run, k + 1, PRINTF)?;
    let (a, fmt) = t.get(k).context("a text the code does not print")?;
    printf_text(fmt.as_deref(), a)
}

/// The first `n` texts that the code from `pc` prints in `run` with the printf-like `target`.
fn printfs_to(e: &Exe, pc: i64, run: &Run, n: usize, target: i64) -> Result<Vec<Value>> {
    trace_text(e, pc, run, n, target)?.iter().map(|(a, fmt)| printf_text(fmt.as_deref(), a)).collect()
}

/// The constant an instruction puts in its register: the immediate of addi / addiu / slti / sltiu (signed) or
/// andi / ori / xori (unsigned), 0 for a move from zero (addu / or rd, zero, zero).
pub(crate) fn imm(e: &Exe, a: i64) -> Result<i64> {
    let w = e.u32(a)?;
    Ok(match w >> 26 {
        8..=0xB => e.s16(a)?,
        0xC..=0xE => w & 0xFFFF,
        0 if matches!(w & 63, 0x21 | 0x25) && (w >> 16) & 0x3FF == 0 => 0,
        _ => bail!("not a constant at 0x{a:X}"),
    })
}

const PRINTF: i64 = 0x8004CE74;

/// A text printed with 0x8004CE74 as the pieces it draws: each a run of the format between its colour, font and
/// place codes (%c, %f, %H, %V take an argument each), with its font, colour and place; the other conversions
/// (%d, %02d, %x, %s...) stay in the text, their arguments for the reader to give.
fn printf_pieces(fmt: Option<&str>, a: &Args) -> Result<Vec<Value>> {
    let fmt = fmt.with_context(|| format!("a printf format outside the image at 0x{:X}", a[0] & 0xFFFF_FFFF))?;
    let (mut args, mut state, mut text, mut out) = (a[1..].iter(), [0i64; 4], String::new(), Vec::new());
    let flush = |text: &mut String, state: &[i64; 4], out: &mut Vec<Value>| {
        if !text.is_empty() {
            out.push(json!({"text": std::mem::take(text), "font": state[1], "colour": state[0], "x": state[2], "y": state[3]}));
        }
    };
    let mut it = fmt.chars().peekable();
    while let Some(ch) = it.next() {
        if ch != '%' {
            text.push(ch);
            continue;
        }
        // a '0' (zero-padded), then one digit 1-8 (the width; '9' goes to 0x8004D604): each later digit would replace it
        // (0x8004D080)
        let mut spec = String::new();
        if it.next_if_eq(&'0').is_some() {
            spec.push('0');
        }
        if let Some(d) = it.next_if(|d| ('1'..='8').contains(d)) {
            spec.push(d);
        }
        let code = it.next().with_context(|| format!("a printf format ending in a '%': {fmt:?}"))?;
        // the conversions (the jump table at 0x80021DB8): %D, %d, %x and %s stay in the text; %h, %v, %p, %C, %b
        // and the others (0x8004D604) are not modelled
        match code {
            'c' | 'f' | 'H' | 'V' if spec.is_empty() => {
                flush(&mut text, &state, &mut out);
                state["cfHV".find(code).unwrap_or(0)] =
                    *args.next().context("a printf call with more codes than arguments")?;
            }
            '%' if spec.is_empty() => text.push('%'),
            'D' | 'd' | 'x' | 's' => {
                args.next();
                text.push_str(&format!("%{spec}{code}"));
            }
            _ => bail!("a printf conversion %{spec}{code} not modelled: {fmt:?}"),
        }
    }
    flush(&mut text, &state, &mut out);
    Ok(out)
}

/// The pieces (see `printf_pieces`) of the `k`-th text the code from `pc` prints in `run`.
fn printed(e: &Exe, pc: i64, run: &Run, k: usize) -> Result<Vec<Value>> {
    let t = trace_text(e, pc, run, k + 1, PRINTF)?;
    let (a, fmt) = t.get(k).context("a text the code does not print")?;
    printf_pieces(fmt.as_deref(), a)
}

/// A text printed with 0x8004CE74 (its arguments: a format, `fmt` as the call reads it, then an argument per
/// code): the text without the colour, font and place codes, the font, the first colour and the place; a text that
/// changes colour also has its pieces and their colours.
fn printf_text(fmt: Option<&str>, a: &Args) -> Result<Value> {
    let fmt = fmt.with_context(|| format!("a printf format outside the image at 0x{:X}", a[0] & 0xFFFF_FFFF))?;
    let (mut args, mut colours, mut place) = (a[1..].iter(), Vec::new(), [0; 3]);
    for code in fmt.split('%').skip(1).filter_map(|s| s.trim_start_matches(|c: char| c.is_ascii_digit()).chars().next())
    {
        let v = *args.next().context("a printf call with more codes than arguments")?;
        match code {
            'c' => colours.push(v),
            'f' | 'H' | 'V' => place["fHV".find(code).unwrap_or(0)] = v,
            _ => {}
        }
    }
    let mut out = json!({"text": strip_fmt(fmt, "cfHV"), "font": place[0], "colour": colours.first().copied().unwrap_or(0),
                         "x": place[1], "y": place[2]});
    if colours.len() > 1 {
        out["parts"] = json!(printf_parts(fmt));
        out["colours"] = json!(colours);
    }
    Ok(out)
}

/// Title, main menu and character select layouts (docs: screens research; record 9 = select, record 10 =
/// title / menus, both loaded at 0x800B8D58). Pictures are named by VRAM place: a sprite is drawn from the
/// picture that holds VRAM (page + u, v) with the CLUT at the given VRAM (x, y); see pictures/index.json
/// ('vram') and pictures/cluts.json.
pub fn screens(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let (r9, r10) = (overlay(rec, 9, 0x800B8D58)?, overlay(rec, 10, 0x800B8D58)?);

    let mut fonts = Vec::new();
    for f in 0..4 {
        // font 3 = font 2's glyphs with advance 21 (the VS screen's names)
        let a = 0x80021D64 + 16 * f;
        let ptr = exe.u32(a)?;
        let (page, bpp) = tpage_xy(exe.u16(a + 12)?);
        let mut glyphs = Map::new();
        for c in 0x21..0x7F {
            let uv = exe.u16(ptr + 2 * c)?;
            glyphs.insert((c as u8 as char).to_string(), json!([uv & 0xFF, uv >> 8]));
        }
        fonts.push(json!({"font": f, "address": hx(a, 0), "glyph_table": hx(ptr, 0), "advance": exe.s16(a + 4)?,
                          "line_height": exe.s16(a + 6)?, "w": exe.s16(a + 8)?, "h": exe.s16(a + 10)?, "page": page, "bpp": bpp,
                          "clut_base": exe.u8(a + 14)?,
                          "clut_rule": "colour c: CLUT at (256 + 16*((c + clut_base) & 15), 504 + ((c + clut_base) >> 4))",
                          "glyphs": glyphs}));
    }

    let mut push = Vec::new();
    for k in 0..4 {
        let (p, half) = (exe.u32(0x800220BC + 8 * k)?, exe.u32(0x800220BC + 8 * k + 4)?);
        push.push(json!({"text": exe.cstr(p), "half_width": half}));
    }

    let mut items = Vec::new();
    for k in 0..10 {
        let a = 0x800B8E44 + 8 * k;
        items
            .push(json!({"name": r10.cstr(r10.u32(a)?), "mode": r10.u8(a + 4)?, "needs_two_pads": r10.u8(a + 5)? != 0,
                          "colour": r10.u8(a + 6)?, "unlock": r10.u8(a + 7)?}));
    }

    let mut layouts = Vec::new();
    for l in 0..2 {
        let v = r9.s32s(0x801102DC + 48 * l, 12)?;
        let (mut deco, mut a) = (Vec::new(), [0x800B8DF8, 0x800B8F08][l as usize]);
        while r9.s16(a)? != -1 {
            let e = r9.s16s(a, 8)?;
            deco.push(json!({"x": e[1], "y": e[2], "w": e[3], "h": e[4], "vram": [e[5], e[6]], "bpp": 4, "clut": clut_xy(e[7] & 0xFFFF)}));
            a += 16;
        }
        layouts.push(json!({"rows": l + 1, "portrait_y": v[1], "name_y": v[2], "split_y": v[3], "grid_y": v[4],
                            "row_dy": v[5], "cursor_h": v[6], "cursor_dy": v[7], "tag_dy": v[8], "timer_y": v[9],
                            "timer_clut": clut_xy(v[10]), "push_colour": v[11], "decorations": deco}));
    }
    let mut players = Vec::new();
    for p in 0..2 {
        let w = r9.u32s(0x800B8D58 + 40 * p, 10)?;
        // (the cursor's uv in the one-row and the two-row layout and its CLUT, the 1P / 2P tag's uv and the CPU's:
        // words 4..7)
        players.push(json!({"portrait_x": w[0] & 0xFFFF, "name_x": w[0] >> 16, "side": w[3],
                            "default_slot": w[9] & 0xFFFF, "tag_dx": w[9] >> 16,
                            "cursor_uv": [[w[4] & 0xFF, (w[4] >> 8) & 0xFF], [w[5] & 0xFF, (w[5] >> 8) & 0xFF]],
                            "cursor_clut": clut_xy(w[4] >> 16), "tag_uv": [w[6] & 0xFF, (w[6] >> 8) & 0xFF],
                            "cpu_tag_uv": [w[7] & 0xFF, (w[7] >> 8) & 0xFF]}));
    }
    let (wall_page, _) = tpage_xy(imm(&r9, 0x8010FE08)?);
    let i9 = |a: i64| imm(&r9, a);
    // the select screen's pieces (record 9; their routines):
    // - the faces of a row 35 apart (0x8010D318) from x 11 (the slot builder's script, 0x8010D1F0: not traced);
    // - the cursors (0x8010F880): two halves (w 0x8010FAA8, u and x stepping as wide) from the face's x
    //   + 0x8010F978, in the page 0x8010FACC, the CPU's in the CLUT of the lui at 0x8010FA44; the tags (w, h: the ori
    //   / lui at 0x8010F9B8 / 0x8010F9B4) in the page 0x8010FA04;
    // - the glow behind a portrait (0x8010F5DC): 0x8010F65C wide, from portrait_y + 0x8010F670 to + 0x8010F680 in
    //   the character's colour, then to grey (0x8010F6A8) at + 0x8010F6C0;
    // - the lower panel (0x8010FB34): from split_y + 0x8010FC04 to 0x8010FC08, 0x8010FC2C wide, black to the red at
    //   0x8010FB8C in the two-row layout (black in the other);
    // - the names (0x8010E7DC): PUSH START at y 0x8010E870; a name, h the lui at 0x8010E8A0, from the names sheet
    //   (page 0x8010E8E0, CLUT the lui at 0x8010E8C8) at u 128 * (cell >> 4 & 1), v 16 * (cell & 15) (shifts at
    //   0x8010E8B4); the mode's label (font 0x8010E9A4, colour 0x8010E9A0) at name_y + 0x8010E9AC, its x kept within
    //   0x8010E980..0x8010E994;
    // - the timer at x 0x8011020C (0x8004E128's digits);
    // - the edge arrows (0x8010E67C): w, h the ori / lui at 0x8010E734 / 0x8010E730, uv and CLUT 0x8010E73C / its lui
    //   and 0x8010E774, at x 0x8010E750 and 0x8010E784 (bobbing), y split_y + 0x8010E6CC, page 0x8010E7A8;
    // - the light streaks (0x8010EB94 / 0x8010EA00): one every 4 frames (andi 3, 0x8010EBCC), at most 0x8010EC30,
    //   starting at 0x8010EA4C bright, dimming 0x8010EA68 + rand & 3 a frame, 0x8010EAA8 long, 0x8010EAFC wide,
    //   rising 16 px a frame (a shift at 0x8010EA94); the original spreads their x over the portrait's width by
    //   (quarter << 10 | rand) * 126 >> 12 (0x8010EC50), the port by `bands` of `band_w`;
    // - the wall (0x8010FC80): the first column 0x8010FD68 wide from u + 0x8010FD64, the others 0x8010FDD4; it and
    //   the streaks are clipped to the draw area from its top (y 20, RAM 0x800AE232 at run time: not read);
    // - a portrait (0x8010F140): 0x8010F1BC wide, fading in over 0x8010F1E4 steps
    let (cursor_page, _) = tpage_xy(i9(0x8010FACC)?);
    let (tag_page, _) = tpage_xy(i9(0x8010FA04)?);
    let (names_page, _) = tpage_xy(i9(0x8010E8E0)?);
    let (arrow_page, _) = tpage_xy(i9(0x8010E7A8)?);
    let pieces = json!({
        "faces": {"x": 11, "dx": i9(0x8010D318)?},
        "cursor": {"routine": "0x8010F880", "page": cursor_page, "dx": i9(0x8010F978)?, "half_w": i9(0x8010FAA8)?,
                   "cpu_clut": clut_xy(r9.u16(0x8010FA44)?), "tag_page": tag_page, "tag_w": i9(0x8010F9B8)?,
                   "tag_h": r9.u16(0x8010F9B4)?},
        "glow": {"routine": "0x8010F5DC", "w": i9(0x8010F65C)?, "top": i9(0x8010F670)?, "mid": i9(0x8010F680)?,
                 "bottom": i9(0x8010F6C0)?, "grey": rgb((r9.u16(0x8010F6A4)? << 16) | i9(0x8010F6A8)?)},
        "panel": {"routine": "0x8010FB34", "dy": i9(0x8010FC04)?, "bottom": i9(0x8010FC08)?, "w": i9(0x8010FC2C)?,
                  "red": rgb(i9(0x8010FB8C)?)},
        "names": {"routine": "0x8010E7DC", "push_y": i9(0x8010E870)?, "page": names_page,
                  "clut": clut_xy(r9.u16(0x8010E8C8)?), "h": r9.u16(0x8010E8A0)?, "cell": [128, 16],
                  "label": {"font": i9(0x8010E9A4)?, "colour": i9(0x8010E9A0)?, "dy": i9(0x8010E9AC)?,
                            "x": [i9(0x8010E980)?, i9(0x8010E994)?]}},
        "timer_x": i9(0x8011020C)?,
        "arrows": {"routine": "0x8010E67C", "page": arrow_page, "w": i9(0x8010E734)?, "h": r9.u16(0x8010E730)?,
                   "uv": [[i9(0x8010E73C)? & 0xFF, i9(0x8010E73C)? >> 8], [i9(0x8010E774)? & 0xFF, i9(0x8010E774)? >> 8]],
                   "clut": clut_xy(r9.u16(0x8010E738)?), "x": [i9(0x8010E750)?, i9(0x8010E784)?], "dy": i9(0x8010E6CC)?},
        "streaks": {"routine": "0x8010EB94 / 0x8010EA00", "every": 4, "max": i9(0x8010EC30)?, "start": i9(0x8010EA4C)?,
                    "dim": i9(0x8010EA68)?, "dim_rand": 4, "len": i9(0x8010EAA8)?, "w": i9(0x8010EAFC)?, "rise": 16,
                    "bands": 4, "band_w": 32},
        "wall": {"first_w": i9(0x8010FD68)?, "first_du": i9(0x8010FD64)?, "w": i9(0x8010FDD4)?, "top": 20},
        "portrait": {"routine": "0x8010F140", "w": i9(0x8010F1BC)?, "fade": i9(0x8010F1E4)?},
    });
    let (per_row, hidden) = (imm(&r9, 0x8010D714)?, imm(&r9, 0x8010D684)?);
    // the wall scrolls by frame << 7 >> 4 (0x8010FD14)
    let scroll = ((r9.u32(0x8010FD14)? >> 6) & 31) - ((r9.u32(0x8010FD1C)? >> 6) & 31);
    let scroll = u32::try_from(scroll).ok().and_then(|s| 1i64.checked_shl(s)).context("the wall's scroll shifts")?;
    // the faces (0x8004B928, selection at sp+16): character 20's slices, then those above 21 (the question mark)
    let face = |sel: i64| trace(exe, 0x8004B928, &Run { args: &[0, 0, 0, 0, sel], ..Run::default() }, 3, 0x8004D9BC);
    let slices = |v: &[Args]| -> Vec<Value> {
        let (u0, y0) = v.first().map_or((0, 0), |c| (c[5] & 0xFF, c[3] >> 16));
        v.iter().map(|c| json!([(c[5] & 0xFF) - u0, (c[3] >> 16) - y0, c[4] >> 16])).collect()
    };
    let im = |a: i64| imm(exe, a);
    let question = im(0x80054504)?;
    let (face_20, face_q) = (face(80)?, face(question)?);
    // the red slash over a face (flag 0x40 at sp+20, 0x8004BA6C): two sprites (0x8004D9BC: y in a3's high half, w and
    // h at sp+16, uv and CLUT at sp+20) in the page 0x8004DAA8 sets after them; y from the face's (moved down at
    // 0x8004BA5C)
    let face_dy = imm(exe, 0x8004BA5C)?;
    let slashed = Run { args: &[0, 0, 0, 0, 0, 0x40], ..Run::default() };
    let slash = trace(exe, 0x8004B928, &slashed, 2, 0x8004D9BC)?;
    let (slash_page, slash_bpp) =
        tpage_xy(trace(exe, 0x8004B928, &slashed, 1, 0x8004DAA8)?.first().map_or(0, |c| c[2]));
    let slash = json!({"page": slash_page, "bpp": slash_bpp,
                       "clut": clut_xy(slash.first().map_or(0, |c| (c[5] >> 16) & 0xFFFF)),
                       "sprites": slash.iter().map(|c| json!({"u": c[5] & 0xFF, "v": (c[5] >> 8) & 0xFF, "w": c[4] & 0xFFFF,
                                                               "h": c[4] >> 16, "dy": (c[3] >> 16) - face_dy})).collect::<Vec<_>>()});
    let mut one_row = Vec::new();
    for c in 0..10 {
        let (w, page) = (r9.u32(0x800B8DA8 + 8 * c)?, r9.u16(0x800B8DA8 + 8 * c + 4)?);
        let (px, py, bpp) = (64 * (page & 0xF), 256 * ((page >> 4) & 1), [4, 8, 16, 16][((page >> 7) & 3) as usize]);
        one_row.push(
            json!({"vram": [px + (w & 0xFF) * bpp / 16, py + ((w >> 8) & 0xFF)], "bpp": bpp, "clut": clut_xy(w >> 16)}),
        );
    }
    let (menu_rows, menu_from) = (imm(&r10, 0x800DBD08)?, imm(&r10, 0x800DBC40)?);
    // (a colour from a lui and an ori; a rect's x0, y0, x1, y1 from ori / lui / ori / lui)
    let word10 = |hi: i64, lo: i64| -> Result<Value> { Ok(rgb((r10.u16(hi)? << 16) | imm(&r10, lo)?)) };
    let quad10 = |x0: i64, y0: i64, x1: i64, y1: i64| -> Result<Value> {
        Ok(json!([imm(&r10, x0)?, r10.u16(y0)?, imm(&r10, x1)?, r10.u16(y1)?]))
    };
    let tiles: Vec<Value> = r9
        .u32s(0x801185FC, 10)?
        .iter()
        .map(|&t| json!({"u": t & 0xFF, "v": (t >> 8) & 0xFF, "clut": clut_xy(t >> 16)}))
        .collect();
    let mut portraits = Vec::new();
    for s in 0..88 {
        let word = exe.u32(exe.u32(0x80097AB8 + 4 * s)?)?;
        portraits.push(
            json!({"selection": s, "portrait": word & 0x1F, "flipped": word & 0x20 != 0, "fade_type": (word >> 6) & 3,
                              "glow": [(word >> 8) & 0xFF, (word >> 16) & 0xFF, word >> 24]}),
        );
    }
    let fades = r9.s32s(0x8011857C, 12)?;
    let mut faces = Vec::new();
    for c in 0..24 {
        let (w, t) = (exe.u32(0x8002152C + 8 * c)?, exe.u16(0x8002152C + 8 * c + 4)?);
        let (page, bpp) = tpage_xy(t);
        faces.push(json!({"index": c, "page": page, "bpp": bpp, "u": w & 0xFF, "v": (w >> 8) & 0xFF, "clut": clut_xy(w >> 16)}));
    }
    let mut subtitles = Map::new();
    for (k, a) in
        [("time_attack", 0x800B90A4), ("survival", 0x800B90B4), ("practice", 0x800B90C0), ("force", 0x800B90CC)]
    {
        subtitles.insert(k.into(), json!(r9.cstr(a)));
    }
    let mut hud = fight_hud(exe, rec)?;
    hud["practice"] = practice(exe, rec)?;
    let vs = versus(exe, rec)?;
    let wall_rows: Vec<Value> = (0..8).map(|r| bytes_list(r9.bytes(0x801185BC + 8 * r, 8))).collect();
    let mut places = Vec::new();
    for k in 0..8 {
        let (a, p) = (0x800EAF0C + 24 * k, r10.s16s(0x800EAF0C + 24 * k + 4, 6)?);
        places.push(json!({"button_bit": r10.u16(a)?, "boxed": [p[0], p[1]], "list": [p[2], p[3]],
                           "lead": [p[4], p[5]], "colour": rgb(r10.u32(a + 20)?)}));
    }
    let tips = |a: i64| -> Result<Vec<Vec<i64>>> { (0..8).map(|k| r10.s16s(a + 4 * k, 2)).collect() };
    // KEY CONFIGURATION's pieces (record 10): the title (0x800DF2EC), the pad's picture (0x800DEC24: x + 0x800DEC2C,
    // y + 0x800DEC30, w, h 0x800DEC68 / 0x800DEC70; VRAM x 0x800DEC40, y the DualShock's 0x800DEC44 or the digital's 0;
    // CLUTs 0x800DEC4C / 0x800DEC54; 8 bpp, 0x800DEC78), the rows (x + 0x800DECB0, y + 0x800DECBC.., their highlight's
    // widths 0x800DECD8..; colours idle / on 0x800DEC90 / 0x800DEC9C; the vibration's no / yes 0x800DEDC4 / 0x800DEDC8,
    // its x the port's), the highlight (0x800DC21C: x - 0x800DC2AC, y - 0x800DC2B0, w + 0x800DC298, h the lui at
    // 0x800DC2A0, colour the lui / ori at 0x800DC270), a label box and its list (0x800DE42C: 0x800DE7B0 wide,
    // 0x800DE780 high, its inside 0x800DE778 wide; open, a row of 0x800DE5A4 for each of 0x800DE5C0 commands, the box
    // 0x800DE724 higher); a command's label (0x800DE23C: the two sidesteps' arrows, commands 10 and 11, uv 0x800DE2D0 /
    // 0x800DE2C8, w, h the ori / lui at 0x800DE2D8 / 0x800DE2D4, at x + 0x800DE2E4, in the page 0x800DE31C and the
    // label's colour's font CLUT)
    let i10 = |a: i64| imm(&r10, a);
    let draw = json!({
        "title": printf_call(&r10, 0x800DF2EC)?,
        "pad": {"x": i10(0x800DEC2C)?, "y": i10(0x800DEC30)?, "w": i10(0x800DEC68)?, "h": i10(0x800DEC70)?, "bpp": 8,
                "vram": [[i10(0x800DEC40)?, i10(0x800DEC44)?], [i10(0x800DEC40)?, 0]],
                "cluts": [clut_xy(i10(0x800DEC4C)?), clut_xy(i10(0x800DEC54)?)]},
        "rows": {"x": i10(0x800DECB0)?, "y": [i10(0x800DECBC)?, i10(0x800DEDE4)?, i10(0x800DEF1C)?, i10(0x800DEFD0)?],
                 "w": [i10(0x800DECD8)?, i10(0x800DEE1C)?, i10(0x800DEF34)?, i10(0x800DEFE8)?],
                 "colours": [i10(0x800DEC90)?, i10(0x800DEC9C)?], "values": [i10(0x800DEDC4)?, i10(0x800DEDC8)?], "value_dx": 99},
        "bar": {"dx": i10(0x800DC2AC)?, "dy": i10(0x800DC2B0)?, "dw": i10(0x800DC298)?, "h": r10.u16(0x800DC2A0)?,
                "rgb": rgb((r10.u16(0x800DC270)? << 16) | i10(0x800DC274)?)},
        "box": {"w": i10(0x800DE7B0)? & 0xFF, "h": i10(0x800DE780)?, "inner": i10(0x800DE778)? & 0xFF, "row": i10(0x800DE5A4)?,
                "rows": i10(0x800DE5C0)?, "open": i10(0x800DE724)?},
        "arrows": {"uv": [[i10(0x800DE2D0)? & 0xFF, 0], [i10(0x800DE2C8)? & 0xFF, 0]], "w": i10(0x800DE2D8)?,
                   "h": r10.u16(0x800DE2D4)?, "dx": i10(0x800DE2E4)?, "page": tpage_xy(i10(0x800DE31C)?).0}});
    let s10 = |a: i64| strip_fmt(&r10.cstr(a), "cfHV");
    // the commands' names in KEY CONFIGURATION's order (0x800EAED0), then NO and YES
    let label = |a: i64| -> Result<String> { Ok(r10.cstr(r10.u32(a)?)) };
    Ok(json!({
        "vs": vs,
        "modes": battle_modes(exe, rec)?,
        "records": records_screen(exe, rec)?,
        "grid": grid_select(exe, rec)?,
        "note": "Coordinates are in the 368x480 screen. VRAM places are in 16-bit words (x) and lines (y).",
        // (not the game's: who OpenKumite's debug previews, `--screen`, show, by character id; the boss and team
        // picks as [character, costume] and the usage as [character, plays])
        "previews": {"player": 7, "versus": [4, 1], "practice": [7, 0], "force_bosses": [[0, 0], [5, 0], [13, 0]],
                     "usage": [[7, 9], [0, 6], [2, 5], [13, 3], [4, 2], [17, 1]], "survival": 9,
                     "teams": [[7, 9, 1, 2], [3, 5, 6]],
                     // (the Theater's: a few endings seen; everyone's, 0..20; the alternates)
                     "theater": {"some": [7, 9, 11, 16], "all": (0..21).collect::<Vec<_>>(), "alternates": [8, 11, 16]}},
        "hud": hud,
        // (a colour's CLUT (0x8004CF44): x 16 * (c & 15 | the ori at 0x8004CF5C), y (c >> 4) + the addiu / ori at
        // 0x8004CF4C / 0x8004CF50)
        "fonts": {"address": "0x80021D64", "printf": "0x8004CE74", "fonts": fonts,
                  "clut_origin": [16 * imm(exe, 0x8004CF5C)?, imm(exe, 0x8004CF4C)? | imm(exe, 0x8004CF50)?]},
        "title": {"draw": "0x800DA4BC (record 10) / 0x800D2614 (record 0)",
                  "strips": sprites(&r10, 0x800DA5C4, 6)?, "strips_options": sprites(&r10, 0x800DA700, 6)?,
                  "presents": printf_call(exe, 0x8004F804)?,
                  "logo": sprites(&r10, 0x800DA548, 1)?.pop(),
                  "copyright": [printf_call(&r10, 0x800DA594)?, printf_call(&r10, 0x800DA5AC)?],
                  // (no %f: font 0; 0x8004EE14)
                  "push_start": {"address": "0x800220BC", "strings": push, "y": imm(exe, 0x8004EEC8)?, "font": 0,
                                 "colour": imm(exe, 0x8004EEB8)?,
                                 "blink": format!("visible while (frame & 0x{:X}) != 0", imm(exe, 0x8004EE44)?)}},
        "menu": {"address": "0x800B8E44", "handler": "0x800DB5B4", "items": items,
                 "font": imm(&r10, 0x800DBCF4)?, "row_height": menu_rows,
                 // (the selected item is the fourth row: from item - 3)
                 "selected_y": imm(&r10, 0x800DBC2C)? - menu_from * menu_rows,
                 "clip": [imm(&r10, 0x800DBD20)?, imm(&r10, 0x800DBD28)?, imm(&r10, 0x800DBD30)?, imm(&r10, 0x800DBD38)?],
                 "centre_x": imm(&r10, 0x800DBCA8)?,
                 // (the rows from the first's y (0x800DBC2C), the selected `from` below it, as many after; the
                 // highlight (0x800DBD60): two gradients from x 0x800DBED8 to the centre 0x800DBD84 and on to 0x800DBF20,
                 // y from the lui at 0x800DBD80 to the one at 0x800DBD98, colour the lui / ori at 0x800DBD70, with lines
                 // from y 0x800DBD80 / 0x800DBD90 to 0x800DBD88 / 0x800DBD98 across the screen (0x800DBE00) in the colour at
                 // 0x800DBDA8; the list's fades (0x800DBB04, 0x800DBB68: [x0, y0, x1, y1] from ori / lui pairs, their
                 // colours the lui / ori at 0x800DBB30 / 0x800DBB60))
                 "first_y": imm(&r10, 0x800DBC2C)?, "from": menu_from.abs(),
                 "bar": {"x": [imm(&r10, 0x800DBED8)?, imm(&r10, 0x800DBD84)?, imm(&r10, 0x800DBF20)?],
                         "y": [r10.u16(0x800DBD80)?, r10.u16(0x800DBD98)?], "rgb": word10(0x800DBD70, 0x800DBD7C)?,
                         "lines": [[r10.u16(0x800DBD80)?, r10.u16(0x800DBD88)?], [r10.u16(0x800DBD90)?, r10.u16(0x800DBD98)?]],
                         "line_rgb": word10(0x800DBDA8, 0x800DBDAC)?, "right": imm(&r10, 0x800DBE00)?},
                 "fades": [{"rect": quad10(0x800DBB08, 0x800DBB04, 0x800DBB10, 0x800DBB14)?, "rgb": word10(0x800DBB30, 0x800DBB38)?},
                           {"rect": quad10(0x800DBB6C, 0x800DBB68, 0x800DBB74, 0x800DBB78)?, "rgb": word10(0x800DBB60, 0x800DBB64)?}],
                 // (Tekken Ball's name, record 8, over the port's list of its modes)
                 "ball": strip_fmt(&overlay(rec, 8, 0x800B0548)?.cstr(0x800B05E8), "cfHV"),
                 // (Tekken Force's name: its item's, battle type 8, without " MODE", for the port's mode lists)
                 "force": items.iter().find(|i| i["mode"] == 8).and_then(|i| i["name"].as_str())
                     .map(|n| n.trim_end_matches(" MODE"))},
        // the menus' system sounds (SePlay 0x800753BC, a1): the title's START, the main menu's cursor and confirm, the
        // options' back (record 10), the select screen's cursor, pick and a player joining (record 9), the key
        // configuration's list
        "sounds": {"start": imm(&r10, 0x800DB53C)?, "cursor": imm(&r10, 0x800DB958)?, "confirm": imm(&r10, 0x800DB96C)?,
                   "back": imm(&r10, 0x800DCA58)?, "select_cursor": imm(&r9, 0x8010DF84)?, "pick": imm(&r9, 0x8010DBEC)?,
                   "join": imm(&r9, 0x8010E238)?, "list_open": imm(&r10, 0x800DEA48)?},
        "key_config": {"routine": "0x800DF014 (record 10)", "places": {"address": "0x800EAF0C", "entries": places}, "draw": draw,
                       "tips": {"address": "0x800B9564", "dualshock": tips(0x800B9564)?, "digital": tips(0x800B9584)?},
                       // (the values padded to one width in the original: the port places them)
                       "texts": {"title": s10(0x800B95F8), "setting": s10(0x800B95A4),
                                 "vibration": printf_parts(&r10.cstr(0x800B95BC)).first().map_or("", |p| p.trim_end()),
                                 "yes": label(0x800EAF08)?.trim(), "no": label(0x800EAF04)?.trim(),
                                 "commands": (0..13).map(|k| label(0x800EAED0 + 4 * k)).collect::<Result<Vec<_>>>()?,
                                 "default": s10(0x800B95D8), "exit": s10(0x800B95E8)}},
        "select": {"handler": "0x8010FF4C", "title": sprites(&r9, 0x8010FE34, 3)?,
                   // (a row's length 0x8010D714; 0x8010D65C-84 takes a slot's column as k % 11, by a multiply, and
                   // hides those from 0x8010D684 on)
                   "slots": {"address": "0x800B9018", "per_row": per_row, "hidden": [hidden, hidden + per_row],
                                                       "characters": bytes_list(r9.bytes(0x800B9018, 22))},
                   "layouts": layouts, "players": players, "pieces": pieces,
                   "wall": {"page": wall_page, "bpp": 4, "tile_h": imm(&r9, 0x8010FD08)?, "tiles": tiles,
                            "rows": wall_rows,
                            "scroll": format!("up {scroll} px per frame"), "speed": scroll},
                   "portrait_fades": {"heights": fades.chunks(3).collect::<Vec<_>>(),
                                      "keys": r9.s32s(0x801185AC, 4)?},
                   "subtitles": subtitles, "portraits": portraits,
                   // (one row: record 9's own faces, 0x8010F7F0: a sprite (lui / ori: h, w) from {uv | CLUT << 16,
                   // tpage} by character id; the first ten, the table's other words are other data)
                   "one_row_faces": {"address": "0x800B8DA8", "drawer": "0x8010F7F0", "w": imm(&r9, 0x8010F7F4)?,
                                     "h": r9.u16(0x8010F7F0)?, "faces": one_row},
                   "faces": {"address": "0x8002152C", "drawer": "0x8004B928",
                             // (the "?" of a member not picked yet, 0x80054504: a selection, the face a quarter; the
                             // other one, 0x80054500, the next face)
                             "question": [question / 4, question / 4 + im(0x80054500)? - question],
                             "w": face_20.first().map_or(0, |c| c[4] & 0xFFFF),
                             "h": face_20.iter().map(|c| c[4] >> 16).sum::<i64>(),
                             "index": "character id (selection 0x59, Tiger, uses the next entry)",
                             "slices": {"20": slices(&face_20), "other_above_21": slices(&face_q),
                                        "format": "[u offset, y offset, height], each 32 wide"},
                             // (flag 0x20: greyed, the CLUT of the lui at 0x8004BB34; flag 0x40: the slash)
                             "grey_clut": clut_xy(exe.u16(0x8004BB34)?), "slash": slash,
                             "faces": faces}}}))
}

/// The fight's 2D HUD (driver 0x8003D6B0): bars 0x8004E274, names 0x8004E8B4, timer 0x8004E128, round
/// markers 0x8004E594, announcements 0x8003DCC8 / 0x8003DF68 / 0x8003E170, pause 0x800780BC. Positions and
/// colours that are immediates in the code are given with the routine that holds them.
fn fight_hud(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r5 = overlay(rec, 5, 0x800B0548)?;
    let s = |a: i64| exe.cstr(a);
    let i = |a: i64| imm(exe, a);
    let word = clut_xy;
    let mut bars = Vec::new();
    for p in 0..2 {
        let a = 0x80097D14 + 16 * p;
        bars.push(json!({"x": exe.s16(a + 0xC)?, "y": exe.s16(a + 0xE)?, "side": exe.s16(a + 0xA)?}));
    }
    let gem = exe.u32s(0x80021F24, 2)?;
    let anchors = [exe.s16s(0x80021F34, 2)?, exe.s16s(0x80021F34 + 8, 2)?];
    // the announcements: runs of their routines in the state that picks each (the frame 0x80095460, the round
    // limit 0x800ADF3C, the battle type 0x800AFA88, the K.O. flags 0x800ADE78, the humans 0x800ADF10, player 1's
    // health 0x800A926E)
    let say = |pc: i64, state: &[(i64, i64, i64)], ret: i64| printfs(exe, pc, &Run { state, ret, ..Run::default() });
    let intro = |state: &[(i64, i64, i64)]| say(0x8003DCC8, &[&[(0x80095460, 4, 0xFF)], state].concat(), 0);
    let (round, final_round) = (intro(&[(0x800ADF3C, 2, 1)])?, intro(&[])?);
    let ko = |flags: i64| pick(say(0x8003DF68, &[(0x800ADE78, 2, flags)], 0)?, 0);
    let result = |humans: i64, ret: i64| pick(say(0x8003E170, &[(0x800ADF10, 2, humans), (0x800A926E, 2, 1)], ret)?, 0);
    let mut wins = result(3, 0)?;
    // the winner's name centred: with strlen 0 and 1 (x = centre - (per_char * strlen(name) + offset))
    let (x0, x1, centre) = (wins["x"].as_i64().unwrap_or(0), result(3, 1)?["x"].as_i64().unwrap_or(0), i(0x8003E274)?);
    if let Some(w) = wins.as_object_mut() {
        w.remove("x");
    }
    wins["x_rule"] = json!({"centre": centre, "per_char": x0 - x1, "offset": centre - x0});
    let mut replay = pick(say(0x80032348, &[(0x800AFA4C, 4, 0x10)], 0)?, 0)?;
    replay["routine"] = json!("0x80032348");
    replay["blink"] = json!(format!("frame & 0x{:X}", i(0x80032354)?));
    let uv: Vec<Value> = exe.u32s(0x80021EE8, 12)?.iter().map(|w| json!([w & 0xFF, w >> 8])).collect();
    let cluts: Vec<Value> = exe.u16s(0x80021F44, 4)?.into_iter().map(word).collect();
    let plate_cluts: Vec<Value> = exe.u16s(0x80021F2C, 4)?.into_iter().map(word).collect();
    // the pause items (0x80098A44): 3 (0x8007812C), in battle type 7 4 (0x80078178, the panel 0x80078174 high
    // against 0x80078114)
    let pause_items =
        |a: i64| -> Result<Vec<String>> { (0..i(a)?).map(|k| Ok(s(exe.u32(0x80098A44 + 4 * k)?))).collect() };
    // the pause (0x800780BC, a0 the player; item 0 selected, 0x80098A40): its items, the header (shown while frame
    // 0x800AE218 & 0x20) and the boxes; player 2's header ends at the screen's right (0x800AE234: the draw area's
    // width, 0x80048320)
    let paused = |p: i64| -> Result<[(i64, i64, i64); 3]> {
        Ok([(0x800AE218, 4, 0x20), (0x80098A40, 1, 0), (0x800AE234, 2, if p == 2 { i(0x80048320)? } else { 0 })])
    };
    let header = |p: i64| printf_nth(exe, 0x800780BC, &Run { args: &[p], state: &paused(p)?, ..Run::default() }, 3);
    let (header, header_2) = (header(1)?, header(2)?);
    let run_1 = Run { args: &[1], state: &paused(1)?, ..Run::default() };
    // the items' format is built at run time ("%c%f%H%V" 0x80097D08, "%p" 0x80027BEC, the item): colour, font, x, y
    let items = trace(exe, 0x800780BC, &run_1, 2, PRINTF)?;
    let item = |k: usize, a: usize| items.get(k).map_or(0, |c| c[a]);
    let boxes: Vec<Value> = trace(exe, 0x800780BC, &run_1, 2, 0x8004DA10)?
        .iter()
        .map(|a| json!({"rgb": rgb(a[2]), "rect": [a[3] & 0xFFFF, a[3] >> 16, a[4] & 0xFFFF, a[4] >> 16]}))
        .collect();
    let routine = |r: &str, t: Value| merge(json!({"routine": r}), t);
    // (game over: states 0x80050B9C / 0x80050BDC of 0x80050428, shown while the counter from 0 is below 121)
    let game_over = merge(routine("0x80050BDC", printf_call(exe, 0x80050BDC)?), json!({"frames": i(0x80050C18)?}));
    let (page, bpp) = tpage_xy(i(0x8004E6C4)?);
    // (the bars' tpage: the high half of their uv word, lui 0x8004E4C4)
    let (bar_page, bar_bpp) = tpage_xy(exe.u16(0x8004E4C4)?);
    let (timer_page, timer_bpp) = tpage_xy(i(0x8004E1FC)?);
    let (plate_page, plate_bpp) = tpage_xy(exe.u16(0x8004E980)?);
    let uv_at = |a: i64| -> Result<[i64; 2]> { Ok([exe.u16(a)? & 0xFF, exe.u16(a)? >> 8]) };
    let ([cap_u, cap_v], [cap2_u, cap2_v]) = (uv_at(0x8004E3AC)?, uv_at(0x8004E404)?);
    let ([strip_u, strip_v], [plate_u, plate_v]) = (uv_at(0x8004E500)?, uv_at(0x8004EA0C)?);
    let (plate_dy, timer_x, infinity, time_x) = (i(0x8004E9A0)?, i(0x8003D9A0)?, i(0x8004E178)?, imm(&r5, 0x800B1AB4)?);
    let marks: Vec<String> = strip_fmt(&r5.cstr(0x800B0574), "cfHV").chars().map(String::from).collect();
    let push = exe.u32s(0x8002213C, 4)?;
    let blink = |e: &Exe, a: i64| -> Result<String> { Ok(format!("frame & 0x{:X}", imm(e, a)?)) };
    // Team Battle's members (record 5 0x800B1764): per side a person per member from x (0x800B17C8 / 0x800B17C4)
    // stepping (0x800B17CC / 0x800B17E8), a quad u..u + w (0x800B17D4, 0x800B17D8) from the y of the lui at
    // 0x800B18E8 to the one at 0x800B1900, its CLUT fighting or down (0x800B18AC / 0x800B18A4), in the page
    // 0x800B1970 sets; a member down has the slash sprite over it (w, h: 0x800B185C / the lui at 0x800B1848; uv
    // 0x800B1864; CLUT and y: the luis at 0x800B1860 / 0x800B1874)
    let (team_page, team_bpp) = tpage_xy(imm(&r5, 0x800B1970)?);
    let (team_top, slash_uv) = (r5.u16(0x800B18E8)?, imm(&r5, 0x800B1864)?);
    let team = json!({"routine": "0x800B1764 (record 5)", "x": [imm(&r5, 0x800B17C8)?, imm(&r5, 0x800B17C4)?],
                      "step": [imm(&r5, 0x800B17CC)?, imm(&r5, 0x800B17E8)?], "y": team_top,
                      "h": r5.u16(0x800B1900)? - team_top, "u": imm(&r5, 0x800B17D4)?, "w": imm(&r5, 0x800B17D8)?,
                      "v": imm(&r5, 0x800B1910)? >> 8, "page": team_page, "bpp": team_bpp,
                      "cluts": {"fighting": word(imm(&r5, 0x800B18AC)?), "down": word(imm(&r5, 0x800B18A4)?)},
                      "slash": {"u": slash_uv & 0xFF, "v": slash_uv >> 8, "w": imm(&r5, 0x800B185C)?,
                                "h": r5.u16(0x800B1848)?, "y": r5.u16(0x800B1874)?, "clut": word(r5.u16(0x800B1860)?)}});
    Ok(json!({
        "list": "bars, timer, markers and names go behind the fighters (2D list B, main OT 1006); announcements in front",
        "team": team,
        "bars": {"routine": "0x8004E274", "players": bars, "w": i(0x8004E360)?, "h": exe.u16(0x8004E294)?, "page": bar_page,
                 "bpp": bar_bpp, "left_cap": {"u": cap_u, "v": cap_v, "w": exe.u16(0x8004E2AC)?, "dx": 0},
                 "right_cap": {"u": cap2_u, "v": cap2_v, "w": exe.u16(0x8004E2AC)?, "dx": i(0x8004E3EC)?},
                 "strip": {"u": strip_u, "v": strip_v},
                 // (where side 0 fills from leftwards, side 1 rightwards: x + these)
                 "fill_dx": [i(0x8004E450)?, i(0x8004E444)?],
                 "cluts": {"fill": word(i(0x8004E4B0)?), "trail": word(i(0x8004E4C0)?), "empty": word(i(0x8004E4BC)?)},
                 "refill_px_per_frame": i(0x8004E388)?, "trail_divisor": 1 << ((exe.u32(0x8004E370)? >> 6) & 31)},
        "timer": {"routine": "0x8004E128", "digits_x": [timer_x + i(0x8004E1EC)?, timer_x], "y": i(0x8003D9A4)?,
                  "page": timer_page, "bpp": timer_bpp, "uv": uv, "clut": word(i(0x8003D9AC)?),
                  // (a digit's w, h: the ori / lui at 0x8004E19C / 0x8004E198; the next one's x, 0x8004E1EC)
                  "w": i(0x8004E19C)?, "h": exe.u16(0x8004E198)?, "step": i(0x8004E1EC)?,
                  "infinity": [infinity >> 4, infinity & 15]},
        "markers": {"routine": "0x8004E594", "y": exe.u16(0x8004E688)?, "w": exe.u16(0x8004E664)?,
                    "h": exe.u16(0x8004E660)?, "x": [i(0x8004E5EC)?, i(0x8004E5E4)?],
                    "step": [i(0x8004E5F4)?, i(0x8004E5FC)?], "page": page, "bpp": bpp,
                    "u": gem[0] & 0xFF, "v": (gem[0] >> 8) & 0xFF,
                    "won_clut": word(gem[0] >> 16), "not_won_clut": word(gem[1] >> 16), "blink": blink(exe, 0x8004E650)?},
        "names": {"routine": "0x8004E8B4", "y": anchors[0][1], "anchor_x": [anchors[0][0], anchors[1][0]],
                  "align": ["left", "right"], "cluts": cluts,
                  "plate_cluts": plate_cluts,
                  "plate": {"page": plate_page, "bpp": plate_bpp, "u": plate_u, "v": plate_v,
                            "h": i(0x8004E9E4)? - plate_dy, "dy": plate_dy, "pad": -i(0x8004E99C)?},
                  "pictures": "pictures/names/model_NNN_index.png (texture record TIM at 80,0)"},
        "intro": {"routine": "0x8003DCC8", "round": pick(round.clone(), 0)?,
                  "final_round": pick(final_round, 0)?, "ready": pick(round, 1)?,
                  "practice": pick(intro(&[(0x800AFA88, 4, 5)])?, 0)?,
                  // Team and Survival show their own text for the whole intro instead of the round
                  "team": pick(intro(&[(0x800AFA88, 4, 2)])?, 0)?,
                  "survival": pick(intro(&[(0x800AFA88, 4, 4)])?, 0)?,
                  // Tekken Force: its stage (number: the stage + 1), or the final stage
                  "force": {"stage": printf_call(exe, 0x8003DE00)?, "final": printf_call(exe, 0x8003DE2C)?},
                  // (later rounds: blez, from frame 1)
                  "round_from": {"first": i(0x8003DD30)?, "other": 1},
                  "ready_from": {"first": i(0x8003DE6C)?, "other": i(0x8003DE88)?},
                  "ends": {"first": i(0x8003CC40)?, "other": i(0x8003CC3C)?}},
        "ko": {"routine": "0x8003DF68", "frames": i(0x8003CFA4)?, "time_up": ko(1)?,
               "perfect": ko(8)?, "double_ko": ko(4)?, "ko": ko(0)?},
        "result": {"routine": "0x8003E170", "draw": pick(say(0x8003E170, &[], 0)?, 0)?,
                   "wins": wins,
                   "you_win": result(1, 0)?, "you_lose": result(2, 0)?},
        "demo": printf_call(exe, 0x8003D8A8)?,
        "replay": replay,
        // (Force: SCORE and HI-SCORE at x[0] / x[1], their colours 0x8004ECB0 / 0x8004ECCC)
        "under_bar": {"routine": "0x8004EB54", "y": i(0x8004EB8C)?, "x": exe.s16s(0x80021FBC, 2)?,
                      "score_colours": [i(0x8004ECB0)?, i(0x8004ECCC)?],
                      "centre_x": exe.s16s(0x80021FBC + 4, 2)?,
                      "push": {"texts": [s(push[0]), s(push[2])],
                               "half_width": [push[1], push[3]], "colour": i(0x8004EBCC)?, "blink": blink(exe, 0x8004EF14)?},
                      "stage": {"routine": "0x800B19B0 (record 5)", "format": strip_fmt(&r5.cstr(0x800B0550), "cfHV"),
                                "final": strip_fmt(&r5.cstr(0x800B0564), "cfHV"), "colour": imm(&r5, 0x800B1A84)?,
                                // (from final_from: STAGE while stage_while, else FINAL)
                                "final_from": imm(&r5, 0x800B1A54)?, "stage_while": blink(&r5, 0x800B1A70)?},
                      "time": {"colour": imm(&r5, 0x800B1AE8)?,
                               "dx": [time_x, time_x + imm(&r5, 0x800B1AF4)?, time_x + imm(&r5, 0x800B1AFC)?],
                               "marks": marks,
                               "marks_dx": [time_x + imm(&r5, 0x800B1AC8)?, time_x + imm(&r5, 0x800B1AD4)?],
                               "marks_dy": imm(&r5, 0x800B1ACC)?,
                               "format": "mm'ss\"cc (hundredths), at most 99'59\"99"}},
        "pause": {"routine": "0x800780BC", "items": pause_items(0x8007812C)?,
                  "ball_items": pause_items(0x80078178)?, "ball_battle_type": i(0x80078168)?,
                  "panel_h": [i(0x80078114)?, i(0x80078174)?],
                  "item_font": item(0, 2), "item_y": item(0, 4), "item_dy": item(1, 4) - item(0, 4),
                  "selected_colour": item(0, 1), "colour": item(1, 1),
                  "header": strip_fmt(&exe.cstr(0x80027BF0), "pcfHV"), "header_font": header["font"],
                  "header_colour": header["colour"],
                  "header_y": header["y"], "header_x": [&header["x"], &header_2["x"]],
                  "border": boxes.get(1), "panel": boxes.first(), "blink": blink(exe, 0x8007836C)?},
        "continue": merge(routine("0x8004FFF0 (printf 0x80050168)", printf_call(exe, 0x8005014C)?),
                          json!({"counter": i(0x8004FFE4)?, "step": i(0x80050094)?, "game_over": game_over})),
    }))
}

/// The `k`-th of the texts a run prints.
fn pick(texts: Vec<Value>, k: usize) -> Result<Value> {
    texts.into_iter().nth(k).context("a text the code does not print")
}

/// `a` with `b`'s fields after its own.
fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(m), Value::Object(e)) = (a.as_object_mut(), b) {
        m.extend(e);
    }
    a
}

/// Team Battle, Time Attack and Survival (docs: modes research; per-type code in record 5, result screens in
/// record 302, both loaded where the code runs): rules per battle type, health carry-over tables, the under-bar
/// texts, the result screens' strings and layouts (code immediates, with their routines), and the records'
/// defaults (the initial save: per-character best times 0x80097F4C, survival top ten 0x80097FFC).
fn battle_modes(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let (r5, r302) = (overlay(rec, 5, 0x800B0548)?, overlay(rec, 302, 0x800B8D58)?);
    let s302 = |a: i64| strip_fmt(&r302.cstr_n(a, 40), "cfHV");
    let mut ta = Vec::new();
    // a row per character: the usage counters' rows, 0x800513B4 (id % n: a multiply by lui / ori's 2^(32 + shift) / n,
    // the shift at 0x800513D0); the table runs up to the survival one, as many rows
    let magic = exe.u16(0x800513B4)? << 16 | imm(exe, 0x800513B8)?;
    let shift = (exe.u32(0x800513D0)? >> 6) & 31;
    let rows = ((1i64 << (32 + shift)) + magic / 2) / magic;
    let table = (0x80097FFC_i64 - 0x80097F4C) / 8;
    if rows != table {
        bail!("{rows} usage rows but a time-attack table of {table}");
    }
    for i in 0..rows {
        ta.push(json!({"character": i, "frames": exe.u32(0x80097F4C + 8 * i)?, "name": exe.cstr_n(0x80097F4C + 4 + 8 * i, 4)}));
    }
    let mut surv = Vec::new();
    for i in 0..10 {
        let a = 0x80097FFC + 8 * i;
        surv.push(json!({"character": exe.u16(a)?, "wins": exe.u16(a + 2)?, "name": exe.cstr_n(a + 4, 4)}));
    }
    let ranks: Vec<String> = (0..11).map(|i| Ok(r5.cstr_n(r5.u32(0x800B49B0 + 4 * i)?, 8))).collect::<Result<_>>()?;
    let wins: Vec<String> = (0..5).map(|i| Ok(r5.cstr_n(r5.u32(0x800B499C + 4 * i)?, 12))).collect::<Result<_>>()?;
    // the team banners: {text, colour} (0x800B8E08)
    let banner = |k: i64| -> Result<Value> {
        Ok(json!([r302.cstr_n(r302.u32(0x800B8E08 + 8 * k)?, 16), r302.u32(0x800B8E0C + 8 * k)?]))
    };
    // texts whose format the code builds in a buffer ("%c%f%H%V" 0x80097D08 and the text): colour, font, x, y
    let i302 = |a: i64| imm(&r302, a);
    let built = |pc: i64, run: &Run, k: usize| -> Result<Args> {
        trace(&r302, pc, run, k + 1, PRINTF)?.get(k).copied().context("a text the code does not print")
    };
    // the time attack's texts (0x800EF6F4): shown (0x800FA7D0), a new record (0x800FA7C0) blinking on frame & 3
    let ta_texts = |frame: i64| {
        printfs(
            &r302,
            0x800EF6F4,
            &Run { state: &[(0x800FA7D0, 4, 1), (0x800FA7C0, 4, 1), (0x800AE218, 4, frame)], ..Run::default() },
        )
    };
    let (ta_on, ta_off) = (ta_texts(1)?, ta_texts(0)?);
    // a row (0x800EF47C: row, frames, x, y): the label, the time's marks and digits (from the row's x, y); its box
    // (an additive gradient from x + 0x800EF658 to + 0x800EF674, y + 0x800EF660 to + 0x800EF684, from the colour of
    // the lui / ori at 0x800EF63C) and the opponent's half-size face (0x800F0CFC at x + 0x800EF61C, y + 0x800EF62C);
    // the total's marks and digits (0x800EF6F4) and the player's face (0x8004B928 at x 0x800EF734 + 0x800EF878, y
    // 0x800EF894)
    let row = |k: usize| printed(&r302, 0x800EF47C, &Run::default(), k);
    let ta_total = Run { state: &[(0x800FA7D0, 4, 1)], ..Run::default() };
    let ta_row = json!({"label": row(0)?.first().context("no time attack row label")?, "marks": row(1)?, "digits": row(2)?,
                        "box": {"x": [i302(0x800EF658)?, i302(0x800EF674)?], "y": [i302(0x800EF660)?, i302(0x800EF684)?],
                                "rgb": rgb((r302.u16(0x800EF63C)? << 16) | i302(0x800EF640)?)},
                        "face": [i302(0x800EF61C)?, i302(0x800EF62C)?]});
    let ta_total = json!({"marks": printed(&r302, 0x800EF6F4, &ta_total, 1)?, "digits": printed(&r302, 0x800EF6F4, &ta_total, 2)?,
                          "face": [i302(0x800EF734)? + i302(0x800EF878)?, i302(0x800EF894)?]});
    let new_record = ta_on.last().context("no new record text")?;
    let new_record = json!({"text": new_record["text"], "font": new_record["font"],
                            "colours": [&new_record["colour"], &ta_off.last().context("no new record text")?["colour"]],
                            "x": new_record["x"], "y": new_record["y"]});
    // survival (0x800F0458): the wins (0x800EFEA4: count, x, y), the rank (0x800F006C: rank, _, y; rank 0 the
    // practice text blinking on frame & 0x10)
    let win = printf_nth(
        &r302,
        0x800EFEA4,
        &Run { args: &[1, imm(&r302, 0x800F0988)?, imm(&r302, 0x800F0998)?], ..Run::default() },
        1,
    )?;
    let rank_y = imm(&r302, 0x800F09DC)?;
    let greatest = built(0x800F006C, &Run { args: &[1, 0, rank_y], ..Run::default() }, 1)?;
    let practice = |frame: i64| {
        built(0x800F006C, &Run { args: &[0, 0, rank_y], state: &[(0x800AE218, 4, frame)], ..Run::default() }, 0)
    };
    let (practice, practice_on) = (practice(0)?, practice(0x10)?);
    // the faces beaten (0x800F0238 from the table 0x800B8FEC: x, y each; their counts in font 0x800F03D8, the port's
    // right-aligned at x + 32, y + 60 where the original prints at x + 20, y + 68: 0x800F029C / 0x800F02E8); the total
    // (0x800EFEA4: x + 0x800EFED4, its width 0x800EFECC, font / colour 0x800EFFE0 / 0x800EFFDC); the rank's line
    // (0x800F006C: x 0x800F0170, font / colour 0x800F016C / 0x800F0168, the rank blinking 0x800F015C / 0x800F0160)
    let survival_faces: Vec<Value> =
        (0..21).map(|k| Ok(json!(r302.u16s(0x800B8FEC + 4 * k, 2)?))).collect::<Result<_>>()?;
    // (each face through 0x800F0238 with the flags of 0x800F09FC, the face itself at x, y + those of 0x800F03F4 /
    // 0x800F0400)
    let survival_layout = json!({
        "faces": survival_faces, "count": {"font": i302(0x800F03D8)?, "dx": 32, "dy": 60},
        "face": {"flags": i302(0x800F09FC)?, "dx": i302(0x800F03F4)?, "dy": i302(0x800F0400)?},
        "total": {"x": i302(0x800F0988)? + i302(0x800EFED4)?, "y": i302(0x800F0998)?, "width": i302(0x800EFECC)?,
                  "font": i302(0x800EFFE0)?, "colour": i302(0x800EFFDC)?},
        "rank": {"x": i302(0x800F0170)?, "y": rank_y, "font": i302(0x800F016C)?, "colour": i302(0x800F0168)?,
                 "blink": [i302(0x800F015C)?, i302(0x800F0160)?]}});
    // Team Battle's (0x800EF340): the members (0x800EE074: from x 0x800EF3C0 every 0x800EE210; P1's at y 0x800EF3C8, P2's
    // at 0x800EF430 + 30 (shifts at 0x800EF420) a row of the log less than 0x800EF410); the log (0x800EE250: n.  VS from
    // x 0x800EE500 at y 0x800EE52C, row by row in three columns 96 wide and rows 30 high (a divide by 3 and shifts), the
    // two half-size faces (0x800F0CFC) at x 0x800EE438 / 0x800EE498 and y 0x800EE464); the result banner (from y
    // 0x800EDF84). The headers' places are the port's.
    let (log_x, log_y) = (i302(0x800EE500)?, i302(0x800EE52C)?);
    let team_layout = json!({
        "members": {"x": i302(0x800EF3C0)?, "dx": i302(0x800EE210)?, "y": [i302(0x800EF3C8)?, i302(0x800EF430)?],
                    "rows": i302(0x800EF410)?, "row_dy": 30},
        "log": {"x": log_x, "dx": 96, "columns": 3, "y": i302(0x800EE464)?, "dy": 30, "text_dy": log_y - i302(0x800EE464)?,
                "faces_dx": [i302(0x800EE438)? - log_x, i302(0x800EE498)? - log_x], "faces_dy": i302(0x800EE464)? - log_y,
                // the two faces' flags by the match's result (0x800EE2C4: P1's win, P2's, a draw's stores of $zero
                // at 0x800EE33C / 0x800EE340)
                "face_flags": {"p1_won": [i302(0x800EE31C)?, i302(0x800EE324)?], "p2_won": [i302(0x800EE300)?, i302(0x800EE308)?],
                               "draw": [0, 0]}},
        "headers": {"x": [20, 244, 318], "y": [76, 260]},
        "banner": {"y": i302(0x800EDF84)?, "dy": 15}});
    // the ten best times' places (0x800B8ED4: x, y)
    let rows = r302.u16s(0x800B8ED4, 20)?;
    // the Force results' texts and the bosses' faces (0x800F18E8, traced from 0x800F1920 in its state 6 (0x800AE224:
    // straight to the drawing) with its tally at 400 frames, 0x800FA844; its flags 0x800AFAC8: the three keys, the
    // final character saved (8), the bosses shown (0x10); a new high score, 0x800AFAC7; the blinking texts by
    // frame & 2, 0x800FA840; the bosses' count done, 0x800FA84C)
    let force_texts = |flags: i64, frame: i64, target: i64| -> Result<Vec<(Args, Option<String>)>> {
        let st = [
            (0x800FA844, 4, 400),
            (0x800FA848, 4, 400),
            (0x800FA84C, 4, 2),
            (0x800AFAC8, 1, flags),
            (0x800AFAC7, 1, 1),
            (0x800AE224, 2, 6),
            (0x800FA840, 4, frame),
        ];
        trace_text(&r302, 0x800F1920, &Run { state: &st, ..Run::default() }, usize::MAX, target)
    };
    let pieces_of = |t: Vec<(Args, Option<String>)>| -> Result<Vec<Value>> {
        Ok(t.iter().map(|(a, f)| printf_pieces(f.as_deref(), a)).collect::<Result<Vec<_>>>()?.concat())
    };
    // (in the order they print: the title, YOUR SCORE: and the score, HIGH SCORE: and the high score, NEW RECORD!!; then
    // KEYS: and the metals, TO BE CONTINUED...; or the saved line's two pieces; or BOSS:, FORCE: and the count)
    let (keys, keys_blink) = (pieces_of(force_texts(7, 0, PRINTF)?)?, pieces_of(force_texts(7, 2, PRINTF)?)?);
    let (saved, saved_blink) = (pieces_of(force_texts(8, 0, PRINTF)?)?, pieces_of(force_texts(8, 2, PRINTF)?)?);
    let bosses = pieces_of(force_texts(0x10, 0, PRINTF)?)?;
    // the k-th piece, its text the string `want` (a number: a conversion)
    let saved_parts = printf_parts(&r302.cstr_n(0x800B92C8, 40));
    let at = |v: &[Value], k: usize, want: Option<&str>| -> Result<Value> {
        let t = v.get(k).context("a Force result text the code does not print")?;
        let text = t["text"].as_str().unwrap_or_default();
        if want.map_or(!text.starts_with('%'), |w| text != w) {
            bail!("the Force results' piece {k} is {text:?}, not {}", want.unwrap_or("a number"));
        }
        Ok(t.clone())
    };
    let named = |v: &[Value], k: usize, a: i64| at(v, k, Some(&s302(a)));
    let blink = |a: &[Value], b: &[Value], k: usize, want: &str| -> Result<Value> {
        let mut t = at(a, k, Some(want))?;
        t["colours"] = json!([t["colour"], at(b, k, Some(want))?["colour"]]);
        Ok(t)
    };
    let saved_part = |k: usize| saved_parts.get(k).map(String::as_str).context("the saved line's pieces");
    let force_layout = json!({
        "your_score": named(&keys, 1, 0x800B927C)?, "score": at(&keys, 2, None)?,
        "high_score": named(&keys, 3, 0x800B929C)?, "hiscore": at(&keys, 4, None)?,
        "new_record": blink(&keys, &keys_blink, 5, &s302(0x800B92B0))?, "keys": named(&keys, 6, 0x800B92F0)?,
        "metals": [named(&keys, 7, 0x800B9300)?, named(&keys, 8, 0x800B9310)?, named(&keys, 9, 0x800B9320)?],
        "to_be_continued": named(&keys, 10, 0x800B9330)?,
        "saved": [at(&saved, 6, Some(saved_part(0)?))?, blink(&saved, &saved_blink, 7, saved_part(1)?)?],
        "boss": named(&bosses, 6, 0x800B934C)?, "force": named(&bosses, 7, 0x800B935C)?, "count": at(&bosses, 8, None)?,
        "boss_faces": force_texts(0x10, 0, 0x8004B928)?.iter().map(|(a, _)| json!([a[2], a[3]])).collect::<Vec<_>>()});
    // max health (0x800516CC): the life table 0x800225EC's entry (16.16); arcade's 8, or 3 when byte
    // 0x800AFAA4 (0x800AFA88 + 28) is 2
    let life = |a: i64| -> Result<i64> { Ok(exe.u32(a)? >> 16) };
    let arcade = |a: i64| life(0x800225EC + 4 * imm(exe, a)?);
    // the battle setup (rec10 0x800DA910) copies the options (0x80097EF0: + 0x17 the rounds, + 0x18 the round time,
    // each from 0) and its case per battle type (the jump table 0x800B8DC8) overrides some: the store `at` (to `off`
    // of its base) of zero or of the addiu at `from`. Rounds to win: the option + 1 (0x8002A4A4); a round: (option
    // + 2) * 600 frames.
    let r10 = overlay(rec, 10, 0x800B8D58)?;
    let set = |at: i64, off: i64, from: Option<i64>| -> Result<i64> {
        let w = r10.u32(at)?;
        let rt = (w >> 16) & 31;
        if !matches!(w >> 26, 0x28 | 0x29) || r10.s16(at)? != off {
            bail!("no store to {off} at 0x{at:X}");
        }
        match from {
            None if rt == 0 => Ok(0),
            Some(f) if rt != 0 && (r10.u32(f)? >> 16) & 31 == rt => imm(&r10, f),
            _ => bail!("the store at 0x{at:X} is not of the value"),
        }
    };
    let (rounds, frames) = (|o: i64| o + 1, |o: i64| (o + 2) * 600);
    Ok(json!({
        "rules": {
            "note": "per battle type: max health (0x800516CC, both fighters), rounds to win (rec10 0x800DA910), round time in frames (null = the ROUND TIME option), difficulty forced (null = the option); rounds to win null = the ROUND option's, default_rounds_to_win at first (0x80097F07)",
            "default_rounds_to_win": rounds(exe.u8(0x80097F07)?),
            "arcade": {"max_health": [arcade(0x80051714)?, arcade(0x80051710)?], "rounds_to_win": null, "round_frames": null,
                       "difficulty": null},
            "team": {"max_health": life(0x80022600)?, "rounds_to_win": rounds(set(0x800DABD8, 0x20, None)?),
                     "round_frames": null, "difficulty": null, "carry_over": true},
            "time_attack": {"max_health": life(0x8002260C)?, "rounds_to_win": rounds(set(0x800DAC38, 0x20, Some(0x800DAC08))?),
                            "round_frames": frames(set(0x800DAC40, -0x2108, Some(0x800DAC3C))?), "difficulty": 1},
            "survival": {"max_health": life(0x800225F8)?, "rounds_to_win": rounds(set(0x800DAC9C, 0x20, None)?),
                         "round_frames": frames(set(0x800DACA0, -0x2108, Some(0x800DAC98))?), "difficulty": 1,
                         "carry_over": true},
        },
        "carry_over": {
            "routine": "0x800517EC (at the K.O. frame; per type 0x8002263C)",
            "team": {"routine": "0x80051850", "who": "the winner, health > 0", "index": "min(7, health * 8 / max)",
                     "gain_256ths": exe.u32s(0x800225AC, 8)?},
            "survival": {"routine": "0x80051898", "who": "the human winner, health > 0",
                         "index": "fight frames of the match / 300, at most 7", "gain_256ths": exe.u32s(0x800225CC, 8)?},
            "formula": "start = min(max, health + (max >> 8) * gain[index]) (16.16)"},
        "under_bar": {
            "labels": {"team": exe.cstr(0x80021FE4), "time_attack": exe.cstr(0x80021FFC), "survival": exe.cstr(0x80022014),
                       "ball": exe.cstr(0x8002202C), "colour": imm(exe, 0x8004EC88)?},
            "game_over": {"text": exe.cstr(0x80021FD0), "colour": imm(exe, 0x8004EBE4)?, "dx": imm(exe, 0x8004EBF0)?},
            "survival": {"routine": "0x800B1C14 / 0x800B1B3C (record 5)",
                         "ranks": ranks,
                         "rank_colour": imm(&r5, 0x800B1C78)?, "wins": wins,
                         "wins_colour": imm(&r5, 0x800B1BD8)?, "wins_dx": imm(exe, 0x8004EC64)?,
                         "wins_max": imm(&r5, 0x800B1B70)?}},
        "survival": {
            "routine": "0x800B26D0 (opponent) / 0x800B0B84 (level)",
            "pools": [{"below_wins": 7, "characters": (0..10).collect::<Vec<_>>()},
                      {"below_wins": 17, "characters": (0..14).chain([16, 18]).collect::<Vec<_>>()},
                      {"below_wins": null, "characters": (0..15).chain([16, 18, 20]).collect::<Vec<_>>()}],
            "recent": 4, "spread": 0, "spread_from_total_20": 4},
        "team": {"routine": "0x800B09B0", "size": {"min": 1, "max": 8, "default": 4},
                 "cpu_level": "difficulty + 3 + (frame & 3); +1 for the second-to-last CPU member, +3 for the last; at most 9"},
        "results": {
            "music": i302(0x800EF9F4)?, "tick_sfx": i302(0x800EFACC)?, "banner_sfx": i302(0x800EF238)?,
            // the half-size face (0x800F0CFC): flag 0x40 slashes it with the face slash's two sprites as quads
            // `size` across (x + 0x800F0D64), at y + dy (the second at 0x800F0DC0); 0x20 greys it, 0x80 leaves it out
            "half_face": {"routine": "0x800F0CFC (record 302)",
                          "slash": {"size": i302(0x800F0D64)?, "dy": [0, i302(0x800F0DC0)?]}},
            "win_voice": r302.u16(0x800EFBC0)?, "frames": i302(0x800EFC3C)?,
            "time_attack": {"routine": "0x800EF92C (record 302)", "title": printf_call(&r302, 0x800EFCF4)?,
                            "stage": [s302(0x800B8EFC), s302(0x800B8F10)],
                            "rows_x": rows.iter().step_by(2).collect::<Vec<_>>(),
                            "rows_y": rows.iter().skip(1).step_by(2).collect::<Vec<_>>(),
                            "your_time": ta_on.first(), "new_record": new_record, "row": ta_row, "total": ta_total},
            "survival": {"routine": "0x800F0458 (record 302)", "title": printf_call(&r302, 0x800F094C)?,
                         "total": printf_call(&r302, 0x800F099C)?,
                         "win": [s302(0x800B906C), s302(0x800B9078)],
                         "win_at": [&win["x"], &win["y"]],
                         "count_colours": bytes_list(r302.bytes(0x800B90DC, 4)),
                         "greatest": {"text": r302.cstr_n(0x800B90B0, 24), "font": greatest[2], "colour": greatest[1],
                                      "x": greatest[3], "y": greatest[4]},
                         "practice": {"text": r302.cstr_n(0x800B90C4, 28), "font": practice[2],
                                      "colours": [practice[1], practice_on[1]], "x": practice[3], "y": practice[4]},
                         "layout": survival_layout,
                         "rank": {"routine": "0x800F006C (record 302)", "text": printf_parts(&r302.cstr(0x800B90A0)).first(),
                                  "suffixes": (1..5).map(|k| Ok(r302.cstr(r302.u32(0x800F2398 + 4 * k)?))).collect::<Result<Vec<_>>>()?,
                                  "suffix": "suffixes[min(rank, 4) - 1] (table 0x800F2398 from rank 1)"}},
            "force": {"routine": "0x800F1D78 / 0x800F1F3C (record 302)", "saved": printf_parts(&r302.cstr_n(0x800B92C8, 40)),
                      "tick_sfx": i302(0x800F1C58)?, "saved_sfx": i302(0x800F1DAC)?,
                      "to_be_continued": s302(0x800B9330), "title": printf_call(&r302, 0x800F1BC0)?,
                      "your_score": s302(0x800B927C), "high_score": s302(0x800B929C), "new_record": s302(0x800B92B0),
                      "keys": s302(0x800B92F0), "metals": [s302(0x800B9300), s302(0x800B9310), s302(0x800B9320)],
                      "boss": s302(0x800B934C), "force": s302(0x800B935C), "layout": force_layout},
            "team": {"routine": "0x800EEE8C (record 302)", "title": printf_call(&r302, 0x800EF36C)?, "layout": team_layout,
                     "players": [s302(0x800B8DFC), s302(0x800B8DF0)],
                     "banners": {"win": banner(0)?, "lose": banner(1)?, "draw": banner(2)?,
                                 "player": [banner(3)?, banner(4)?]}},
        },
        // (the name entry's letters and banned names: records.entry)
        "records": {"time_attack": ta, "survival": surv},
    }))
}

/// The records screens (mode 17, record 11 at 0x800B8D58; docs: records research): the page geometry and
/// strings, the colour sets, the usage bars' colours, the name entry's letters and banned names, the stages the
/// orbiting background skips, and the timings (code immediates, with their routines).
fn records_screen(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r11 = overlay(rec, 11, 0x800B8D58)?;
    let i = |a: i64| imm(&r11, a);
    let cstr = |a: i64| r11.cstr_n(a, 8);
    let sets = |a: i64, n: usize, stride: i64| -> Vec<Value> {
        (0..2).map(|k| bytes_list(r11.bytes(a + stride * k, n))).collect()
    };
    let suffixes: Vec<String> = (0..4).map(|k| Ok(cstr(r11.u32(0x800CB828 + 4 * k)?))).collect::<Result<_>>()?;
    let mut bar_colours = Vec::new();
    for c in 0..21 {
        bar_colours.push(r11.u32s(0x800B8D58 + 20 * c, 5)?.into_iter().map(rgb).collect::<Vec<_>>());
    }
    let banned: Vec<String> = (0..5).map(|k| Ok(cstr(r11.u32(0x800CB854 + 4 * k)?))).collect::<Result<_>>()?;
    let letters: String = r11.bytes(0x800B9054, 40).iter().map(|&b| b as char).collect();
    let word = |a: i64| strip_fmt(&r11.cstr_n(a, 16), "pcfHV");
    let run = |pc: i64, args: &[i64], target: i64| trace(&r11, pc, &Run { args, ..Run::default() }, usize::MAX, target);
    let nth = |v: &[Args], k: usize| v.get(k).copied().context("a call the code does not make");
    // the banner (0x800C1B18): sprites (x, y, w, h, VRAM x, y; u, v their place in the texture page) and the
    // pieces that fade in, each drawn twice (quads: shade at a2 / a3, corners at sp+24.., the first uv at sp+40)
    let opaque: Vec<Value> = run(0x800C1B18, &[], 0x8004DB9C)?
        .iter()
        .map(|a| json!([a[2], a[4], (a[6] & 63) << [2, 1, 0, 0][((a[9] >> 7) & 3) as usize], a[7] & 255]))
        .collect();
    let sprite = nth(&run(0x800C1B18, &[], 0x8004DB9C)?, 0)?;
    let blend: Vec<Value> = run(0x800C1B18, &[], 0x8004D924)?
        .iter()
        .step_by(2)
        .map(|a| {
            json!({"x": a[6] & 0xFFFF, "w": (a[7] & 0xFFFF) - (a[6] & 0xFFFF), "u": a[10] & 0xFF, "v": (a[10] >> 8) & 0xFF,
                   "shade": [a[2] & 0xFF, a[3] & 0xFF]})
        })
        .collect();
    // a row of each page at (0, 0): 0x800C0D1C (time attack: rank, character, record, x; y), 0x800C10B4 (survival:
    // rank, record, x, y), 0x800C13C4 (usage: rank, character, per-mille share, top share; the grow 0..64, x, y);
    // the formats are "%p%c%f" then the place: a[3] the font, then H / V in the format's order
    let ta = run(0x800C0D1C, &[0, 0, 0, 0], PRINTF)?;
    let (rank, name, marks, digits, player) = (nth(&ta, 0)?, nth(&ta, 1)?, nth(&ta, 2)?, nth(&ta, 3)?, nth(&ta, 4)?);
    let face = nth(&run(0x800C0D1C, &[0, 0, 0, 0], 0x8004B928)?, 0)?;
    let lines: Vec<Value> = run(0x800C0D1C, &[0, 0, 0, 0], 0x8004DA10)?
        .iter()
        .map(|a| json!([a[3] & 0xFFFF, a[3] >> 16, a[4] & 0xFFFF, a[4] >> 16]))
        .collect();
    let line_colour = nth(&run(0x800C0D1C, &[0, 0, 0, 0], 0x8004DA10)?, 0)?[2];
    // survival: one win, then two (a record at the scratchpad: +2 its wins)
    let wins = |n: i64| {
        trace(
            &r11,
            0x800C10B4,
            &Run { args: &[0, 0x1F80_0000, 0, 0], state: &[(0x1F80_0002, 2, n)], ..Run::default() },
            usize::MAX,
            PRINTF,
        )
    };
    let (one, many) = (wins(1)?, wins(2)?);
    // usage: a share 1 against a top 2 grown 1000 (shown 15 per mille: "%02d.%1x%%"), then 0 ("  0%"), and its
    // bar (0x800C0B34: character, per mille, right, y)
    let usage = run(0x800C13C4, &[0, 0, 1, 2, 1000], PRINTF)?;
    let (share, none) = (nth(&usage, 2)?, nth(&run(0x800C13C4, &[0, 0, 0, 0], PRINTF)?, 2)?);
    let bar = nth(&run(0x800C13C4, &[0, 0, 0, 0], 0x800C0B34)?, 0)?;
    let full = nth(&run(0x800C0B34, &[0, 1000, bar[2], 0], 0x8004D8B0)?, 0)?;
    let advance_0 = exe.s16(0x80021D64 + 4)?;
    // the name entry: its band, countdown and cursor (row 0, letter 1: 0x800D310C)
    let (band, countdown) = (nth(&run(0x800C2CB8, &[], 0x8004DA10)?, 0)?, printf_call(&r11, 0x800C2D3C)?);
    let cursor =
        |k: i64| trace(&r11, 0x800C20D8, &Run { state: &[(0x800D310C, 2, k)], ..Run::default() }, 1, 0x8004DA10);
    let (c0, c1) = (nth(&cursor(0)?, 0)?, nth(&cursor(1)?, 0)?);
    // the lists: who always shows (mask) and the last character (the mask of all)
    let bits = |m: i64| (0..32).filter(|b| m >> b & 1 != 0).collect::<Vec<i64>>();
    let last = |hi: i64, lo: i64| -> Result<i64> {
        Ok(63 - ((r11.u16(hi)? << 16 | r11.u16(lo)?) as u64).leading_zeros() as i64)
    };
    Ok(json!({
        "routine": "0x800C2434 (mode 17), init 0x800C2DBC, drawers 0x800C1DE8 / 0x800C22A8",
        "banner": {"routine": "0x800C1B18", "pictures": ["r011/000.png", "r011/001.png", "r011/002.png"], "y": sprite[3],
                   "h": sprite[5], "opaque": opaque, "blend": blend,
                   "format": "opaque [x, w, u, v]; blend pieces fade into the background: back - white*shade + picture*shade"},
        "pages": ["time_attack", "survival", "usage"],
        "rows": {"pitch": i(0x800C1ED4)?, "rest_y": i(0x800C281C)?, "per_page": i(0x800C27F4)?,
                 "clip_attract": [0, i(0x800C1FE8)?, i(0x800C1FF0)?, i(0x800C1FF8)?],
                 "clip_entry": [0, i(0x800C23F0)?, i(0x800C23F8)?, i(0x800C23F8)?],
                 "rank": {"font": rank[3], "x": rank[4], "dy": rank[5], "suffixes": suffixes},
                                  // (flag 8, sp+20: no frame, nor its +2 / +3 at 0x8004BA58; 0x8004B964)
                 "face": {"x": face[2], "dy": face[3]},
                 "name": {"font": name[3], "x": name[4], "dy": name[5]},
                 "underlines": lines, "underline_colour": rgb(line_colour),
                 "player": {"font": player[3], "x": player[4], "dy": player[5]}},
        "time_attack": {"routine": "0x800C0D1C / 0x800C172C", "colours": sets(0x800CB838, 4, 5),
                        "colour_order": "rank, name, time, player; set 1 = the new row flickering",
                        "shown_at_most": r11.u16(0x800C1750)? << 16 | r11.u16(0x800C1754)?,
                        "always": bits(i(0x800C1748)?), "max_character": last(0x800C179C, 0x800C17A0)?,
                        // "%p%c%f%V%H'%H\"", "%p%c%f%V%H%02d%H%02d%H%02d"
                        "marks": {"font": marks[3], "dy": marks[4], "x": [marks[5], marks[6]]},
                        "digits": {"font": digits[3], "dy": digits[4], "x": [digits[5], digits[7], digits[9]]}},
        "survival": {"routine": "0x800C10B4 / 0x800C189C", "colours": sets(0x800CB844, 4, 4), "max_wins": i(0x800C1104)?,
                     "one": {"x": nth(&one, 2)?[4], "word": word(0x800B8FCC), "word_x": nth(&one, 3)?[4]},
                     "many": {"x": nth(&many, 2)?[4], "word": word(0x800B8FDC), "word_x": nth(&many, 3)?[4]},
                     "dy": nth(&one, 2)?[5]},
        "usage": {"routine": "0x800C13C4 / 0x800C19B8 / bar 0x800C0B34", "colours": [bytes_list(r11.bytes(0x800CB84C, 3))],
                  "count": "usage counters 0 + 2 + 3 (1P sessions, 2P losses, 2P draws)", "always": bits(i(0x800C19C8)?),
                  "max_character": last(0x800C1A1C, 0x800C1A20)?,
                  "bar": {"right": bar[2], "dy": bar[3], "h": i(0x800C0BEC)?, "max_w": bar[2] - full[6],
                          "side": [i(0x800C0C4C)?, i(0x800C0C68)?], "top_right_dx": i(0x800C0CB0)?, "grow_frames": i(0x800C2898)?,
                          "colours": bar_colours,
                          "colour_order": "side, front left, front right, top left, top right"},
                  // "%p%c%f%H%V%02d%f%H%V.%H%1x%%" (the sign follows the tenth), "%p%c%f%H%V  0%f%H%V%%"
                  "percent": {"whole": {"font": share[3], "x": share[4], "x_full": none[4], "dy": share[5]},
                              "dot": {"font": share[7], "x": share[8], "dy": share[9]},
                              "tenth": {"font": share[7], "x": share[10]},
                              "sign": {"font": share[7], "x": share[10] + advance_0, "x_full": none[7]}}},
        "timing": {"fade": i(0x800C2784)?, "slide": i(0x800C2844)?, "hold": i(0x800C2654)?, "end_wait": i(0x800C2918)?,
                   "entry_end_wait": i(0x800C2B9C)?, "bgm_fade": i(0x800C2BB0)?,
                   "eased": format!("step = clamp((target - scroll + {}) >> {}, {}, {})", i(0x800C2A44)?,
                                    (r11.u32(0x800C2A48)? >> 6) & 31, i(0x800C2A58)?, i(0x800C2A68)?),
                   "entry_frames": i(0x800C2A94)?, "record_bgm": i(0x800C25BC)?,
                   "start_sfx": imm(exe, 0x8004B600)?, "letter_sfx": i(0x800C2B4C)?, "confirm_sfx": i(0x800C2B30)?},
        "entry": {"routine": "0x800C2E90", "letters": letters,
                  "banned": banned, "unlock_name": cstr(0x800B9090),
                  // (the pad's repeat, 0x80029F90: after 20 - 1 frames, then every 1 - (-3))
                  "unlock_character": i(0x800C3148)?, "repeat": [imm(exe, 0x80029FCC)? - 1, 1 - imm(exe, 0x80029FE4)?],
                  "band": {"colour": rgb(band[2]), "y": band[3] >> 16, "h": band[4] >> 16},
                  "countdown": {"font": countdown["font"], "colour": countdown["colour"], "x": countdown["x"],
                                "y": countdown["y"]},
                  "cursor": {"colour": rgb(c0[2]), "x": c0[3] & 0xFFFF, "dx": (c1[3] & 0xFFFF) - (c0[3] & 0xFFFF),
                             "y": c0[3] >> 16, "w": c0[4] & 0xFFFF, "h": c0[4] >> 16}},
        // (the orbiting background: the stages mod 13 (0x800C3330, 0x800C3370-7C), the radius * 5000
        // (0x800C33E4-FC), multiplications in the code)
        "background": {"routine": "0x800C331C / 0x800C34BC", "stages": 13,
                       "skip": r11.u32s(0x800CB868, 4)?,
                       "radius": 5000, "height": -i(0x800C3484)?, "turn": i(0x800C34DC)?, "start_angle": i(0x800C33DC)?,
                       "fighters_x": i(0x800C3460)?},
    }))
}

/// Face texture animation (docs: face research; 0x80034100 blink, 0x80033DC8 Gon's eye strips, 0x80033ECC his
/// pupils): per model id ((model record - 71) / 4) the blink's destination and closed-eye source rectangles, and
/// Gon's two eye strips, in atlas pixels (atlas x = (VRAM x - 384) * 4, y as loaded for player 1). Tables:
/// positions 0x8009579C, sizes 0x8009586C (words, lines), blink 0x800959CC, Gon 0x800958BC.
pub fn faces(exe: &Exe) -> Result<Value> {
    let pos: Vec<Vec<i64>> = (0..52).map(|k| exe.s16s(0x8009579C + 4 * k, 2)).collect::<Result<_>>()?;
    let size: Vec<Vec<i64>> = (0..6).map(|k| exe.s16s(0x8009586C + 4 * k, 2)).collect::<Result<_>>()?;
    let rect = |p: &[i64], z: &[i64]| [(p[0] - 384) * 4, p[1], z[0] * 4, z[1]];
    let signed = |a: i64, n: i64| (0..n).map(|k| exe.s8(a + k)).collect::<Result<Vec<_>>>();
    let mut models = Map::new();
    for m in 0..52 {
        let e = signed(0x800959CC + 4 * m, 4)?;
        if e[3] >= 0 && e[0] >= 0 {
            let z = pyget(&size, e[1])?;
            let d = rect(pyget(&pos, e[0])?, z);
            let closed = rect(pyget(&pos, e[3])?, z);
            let o = models.entry(m.to_string()).or_insert_with(|| json!({}));
            o["blink"] = json!({"dest": d, "closed": closed[..2]});
        }
    }
    for m in [42, 43] {
        let t = signed(0x800958BC + 6 * m, 6)?;
        let z = pyget(&size, t[1])?;
        let mut eyes = Vec::new();
        for (k, (src_dy, dst_dy)) in [(8, 0x11), (21, 0xD)].into_iter().enumerate() {
            let p = pyget(&pos, t[2 + k])?;
            let (x, y) = ((p[0] - 384) * 4, p[1]);
            eyes.push(json!({"x": x, "w": z[0] * 4, "h": z[1], "dest_y": y + dst_dy, "source_y": y + src_dy,
                             "offset": "eye 0: max(e, 0); eye 1: min(e, 0)"}));
        }
        let o = models.entry(m.to_string()).or_insert_with(|| json!({}));
        o["pupils"] = json!(eyes);
    }
    Ok(json!({"routine": "0x80034100 (blink), 0x800397FC / 0x80033DC8 / 0x80033ECC (Gon)",
              "blink": {"closed_frames": 4, "open_frames": "(rand() & 0xFE) + 1", "starts": "closed on the first frame",
                        "forced": "move flags24 bit 0x40000, or posture bit 0x4 without flags24 bit 0x40000000"},
              "pupils": {"character": 17, "angle": "a = atan2(L.y, L.x) in 4096ths, L = head world rotation^T (opponent head - own head)",
                         "e": "a < 0x800: max(-(a / 60), -17); else min(-((a - 0xF00) / 60), 17)", "limit": 17, "divisor": 60},
              "models": models}))
}

/// Practice mode (record 6, overlay slot A at 0x800B0548): the menus (0x800B5318 / 0x800B545C / 0x800B5918 /
/// 0x800B5D18), the in-fight attack data (0x800B6248) and the hit markers (0x800B6554). Positions, fonts and
/// colours are immediates in that code.
fn practice(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r6 = overlay(rec, 6, 0x800B0548)?;
    let s = |a: i64| strip_fmt(&r6.cstr_n(a, 48), "cfHVhv");
    let lst =
        |a: i64, n: usize| -> Result<Vec<String>> { Ok(r6.u32s(a, n)?.into_iter().map(|p| r6.cstr(p)).collect()) };
    let item = |a: i64, y: i64, value: Option<&[String]>| {
        let mut o = json!({"text": s(a), "y": y});
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            o["values"] = json!(v);
        }
        o
    };
    // runs of the menus and the HUD with the practice state (pointer 0x800B8A2C) at the scratchpad: P + offset
    const P: i64 = 0x1F80_0000;
    let run = |pc: i64, args: &[i64], state: &[(i64, i64, i64)], target: i64| {
        trace(
            &r6,
            pc,
            &Run { args, state: &[&[(0x800B8A2C, 4, P)], state].concat(), ..Run::default() },
            usize::MAX,
            target,
        )
    };
    let nth = |v: &[Args], k: usize| v.get(k).copied().context("a call the code does not make");
    // a menu's texts: "%c%f%H%V" (colour, font, x, y), the item's own format or "%s" with the item
    let (mode, free, vs, combo) = (
        run(0x800B5318, &[], &[], PRINTF)?,
        run(0x800B545C, &[], &[], PRINTF)?,
        run(0x800B5918, &[], &[], PRINTF)?,
        run(0x800B5D18, &[], &[], PRINTF)?,
    );
    let at = |m: &[Args], a: i64| m.iter().position(|c| c[0] & 0xFFFF_FFFF == a || c[5] & 0xFFFF_FFFF == a);
    let y =
        |m: &[Args], a: i64| -> Result<i64> { Ok(nth(m, at(m, a).context("an item the menu does not print")?)?[4]) };
    let next =
        |m: &[Args], a: i64| -> Result<Args> { nth(m, at(m, a).context("an item the menu does not print")? + 1) };
    let ok = |m: &[Args]| -> Result<Value> {
        Ok(json!({"text": "", "y": nth(m, 1)?[4], "values": lst(0x800B8A38, 2)?, "ok_cancel": true}))
    };
    let (onoff, replay) = (lst(0x800B8A30, 2)?, lst(0x800B8A64, 5)?);
    let mode_items: Vec<Value> = [0x800B0760, 0x800B0778, 0x800B0788, 0x800B07A0, 0x800B07B8]
        .iter()
        .map(|&a| Ok(item(a, y(&mode, a)?, None)))
        .collect::<Result<_>>()?;
    let (title, first, other) = (nth(&mode, 0)?, nth(&mode, 1)?, nth(&mode, 2)?);
    // the boxes (0x800B6764): mode select (P+117 set), else a mode's menu; panel, then border
    let boxes = |on: i64| run(0x800B6764, &[], &[(P + 117, 1, on)], 0x8004DA10);
    let (ms, sub) = (boxes(1)?, boxes(0)?);
    let rect = |a: Args| json!([a[3] & 0xFFFF, a[3] >> 16, a[4] & 0xFFFF, a[4] >> 16]);
    // the HUD (0x800B6248; "%c%h%v": colour, column, row in font 0's cells): the replay hint (P+129 = 4), the
    // damage of both sides, the last hit (P+20 the record: +12 against +14), counter and clean hits (+26, +27)
    let (advance, line) = (exe.s16(0x80021D64 + 4)?, exe.s16(0x80021D64 + 6)?);
    // (0x800954BC: the HUD draws once it is 0, after practice's startup frames, 0x800B2D7C)
    let hud = |state: &[(i64, i64, i64)]| {
        run(
            0x800B6248,
            &[],
            &[&[(0x800954BC, 4, 0), (P + 129, 1, 4), (P + 157, 1, 1), (P + 46, 1, 1), (P + 47, 1, 1)], state].concat(),
            PRINTF,
        )
    };
    let h = hud(&[])?;
    let colour_vs = |state: &[(i64, i64, i64)]| -> Result<i64> { Ok(nth(&hud(state)?, 3)?[1]) };
    let cell = |c: Args| (c[2] * advance, c[3] * line);
    let text = |a: i64, c: Args| {
        let (x, y) = cell(c);
        json!({"text": s(a), "font": 0, "colour": c[1], "x": x, "y": y})
    };
    let (total, total_2, last) = (nth(&h, 2)?, nth(&h, 1)?, nth(&h, 3)?);
    // the combo (0x800B691C: hits, damage), its label and damage by their digits
    let combo_at = |hits: i64, damage: i64| -> Result<Vec<Value>> {
        printfs(&r6, 0x800B691C, &Run { args: &[hits, damage], ..Run::default() })
    };
    let (c1, c10, c100, cneg) = (combo_at(1, 1)?, combo_at(10, 10)?, combo_at(0, 100)?, combo_at(0, -1)?);
    let x_of = |v: &[Value], k: usize| v.get(k).map_or(0, |t| t["x"].as_i64().unwrap_or(0));
    let field = |v: &[Value], k: usize, f: &str| v.get(k).map_or(Value::Null, |t| t[f].clone());
    let label_x = x_of(&c1, 0);
    let damage_label = |v: &[Value]| x_of(v, v.len().saturating_sub(1));
    // the markers (0x800B6F70: x, y, kind): a label (kinds 0-2 from 0x800B8B34, 3 its own) and the burst; the
    // kinds of the hit masks the code tells apart (0x800B6594...)
    let marker = |k: i64| run(0x800B6F70, &[0, 0, k], &[], 0x8004D9BC);
    let sprite = |c: Args| {
        json!({"u": c[5] & 0xFF, "v": (c[5] >> 8) & 0xFF, "clut": clut_xy((c[5] >> 16) & 0xFFFF), "w": c[4] & 0xFFFF,
               "h": c[4] >> 16})
    };
    let labels: Vec<Value> = (0..4).map(|k| Ok(sprite(nth(&marker(k)?, 0)?))).collect::<Result<_>>()?;
    let (burst, special) = (nth(&marker(0)?, 1)?, nth(&marker(3)?, 1)?);
    let kind = |mask: i64| -> Result<Option<i64>> {
        Ok(run(0x800B6554, &[], &[(P + 86, 2, 1), (P + 84, 2, mask)], 0x800B6F70)?.first().map(|c| c[2]))
    };
    let (mut kinds, mut none) = (Vec::new(), Vec::new());
    for a in [0x800B6594, 0x800B65B4, 0x800B65C4, 0x800B65D8, 0x800B65EC, 0x800B65FC, 0x800B6610, 0x800B6618] {
        let mask = imm(&r6, a)?;
        match kind(mask)? {
            Some(k) => kinds.push((k, mask)),
            None => none.push(mask),
        }
    }
    kinds.sort();
    let kinds: Map<String, Value> = kinds.into_iter().map(|(k, m)| (format!("0x{m:X}"), json!(k))).collect();
    let (page, bpp) = tpage_xy(imm(&r6, 0x800B7098)?);
    Ok(json!({
        "overlay": "record 6 at 0x800B0548",
        "box": {"border_rgb": rgb(nth(&ms, 1)?[2]), "panel_rgb": rgb(nth(&ms, 0)?[2]),
                "mode_select": {"border": rect(nth(&ms, 1)?), "panel": rect(nth(&ms, 0)?)},
                "sub": {"border": rect(nth(&sub, 1)?), "panel": rect(nth(&sub, 0)?)}},
        "text": {"title_font": title[2], "title_colour": title[1], "title_y": title[4], "item_font": first[2],
                 "item_colour": other[1], "selected_colour": first[1],
                 "value_right": next(&free, 0x800B07F8)?[3] - nth(&free, 0)?[3],
                 "x": {"mode_select": title[3], "sub": nth(&free, 0)?[3]},
                 // (the printer 0x800B7134 draws 'u' / 'd' (0x800B716C / 0x800B7174) as arrows: uv 0x800B7160 /
                 // 0x800B7164, w, h the ori / lui at 0x800B717C / 0x800B7178, the page 0x800B71F0, the text's colour's
                 // CLUT)
                 "arrows": {"chars": [char::from(imm(&r6, 0x800B716C)? as u8).to_string(),
                                      char::from(imm(&r6, 0x800B7174)? as u8).to_string()],
                            "uv": [[imm(&r6, 0x800B7160)? & 0xFF, imm(&r6, 0x800B7160)? >> 8],
                                   [imm(&r6, 0x800B7164)? & 0xFF, imm(&r6, 0x800B7164)? >> 8]],
                            "w": imm(&r6, 0x800B717C)?, "h": r6.u16(0x800B7178)?, "page": tpage_xy(imm(&r6, 0x800B71F0)?).0}},
        "mode_select": {"title": s(0x800B074C), "items": mode_items},
        "menus": [
            {"mode": "freestyle", "title": s(0x800B0760), "items": [
                ok(&free)?, item(0x800B07D4, y(&free, 0x800B07D4)?, None),
                merge(item(0x800B07E4, y(&free, 0x800B07E4)?, None),
                      json!({"value_y": next(&free, 0x800B07E4)?[4], "values": lst(0x800B8A40, 9)?,
                             "value_names": {"6": s(0x800B0A40), "7": s(0x800B0A30)}})),
                item(0x800B07F8, y(&free, 0x800B07F8)?, Some(&onoff)), item(0x800B0808, y(&free, 0x800B0808)?, Some(&onoff)),
                item(0x800B0814, y(&free, 0x800B0814)?, Some(&onoff)), item(0x800B0824, y(&free, 0x800B0824)?, Some(&replay)),
                item(0x800B0834, y(&free, 0x800B0834)?, None)]},
            {"mode": "vs_cpu", "title": s(0x800B084C), "items": [
                ok(&vs)?, item(0x800B07D4, y(&vs, 0x800B07D4)?, None),
                item(0x800B085C, y(&vs, 0x800B085C)?, Some(&lst(0x800B8A84, 3)?)),
                item(0x800B086C, y(&vs, 0x800B086C)?, Some(&lst(0x800B8A90, 10)?)),
                item(0x800B0808, y(&vs, 0x800B0808)?, Some(&onoff)), item(0x800B0814, y(&vs, 0x800B0814)?, Some(&onoff)),
                item(0x800B0824, y(&vs, 0x800B0824)?, Some(&replay)), item(0x800B0834, y(&vs, 0x800B0834)?, None)]},
            {"mode": "combo_training", "title": s(0x800B0788), "items": [
                ok(&combo)?, item(0x800B07D4, y(&combo, 0x800B07D4)?, None),
                merge(item(0x800B0878, y(&combo, 0x800B0878)?, None), json!({"name_x": next(&combo, 0x800B0878)?[3]})),
                merge(item(0x800B0894, y(&combo, 0x800B0894)?, None),
                      json!({"none": s(0x800B08A0), "none_x": next(&combo, 0x800B0894)?[3]})),
                item(0x800B0808, y(&combo, 0x800B0808)?, Some(&onoff)), item(0x800B0814, y(&combo, 0x800B0814)?, Some(&onoff)),
                item(0x800B0824, y(&combo, 0x800B0824)?, Some(&replay[..replay.len().min(4)])),
                item(0x800B0834, y(&combo, 0x800B0834)?, None)]}],
        // (the options' first values, set when practice starts, 0x800B28C0-0x800B29A4: attack data and the CPU's
        // difficulty 0x800B28CC, its level 0x800B2964, the others sb zero)
        "defaults": {"attack_data": imm(&r6, 0x800B28CC)?, "counter_attacks": 0, "hit_analysis": 0, "combo_replay": 0,
                     "dummy": 0, "cpu_difficulty": imm(&r6, 0x800B28CC)?, "cpu_level": imm(&r6, 0x800B2964)?},
        // (the dummy's actions, 0x800B81A8, return at once for this move set: it never holds up to get up)
        "dummy": {"routine": "0x800B81A8", "inactive_moveset": imm(&r6, 0x800B81C8)?},
        "hud": {"routine": "0x800B6248", "startup_frames": imm(&r6, 0x800B28E0)?,
                "total": {"text": s(0x800B0930), "font": 0, "colour": total[1], "x": [cell(total).0, cell(total_2).0],
                          "y": cell(total).1},
                "last_hit": {"text": s(0x800B0944), "font": 0, "x": cell(last).0, "y": cell(last).1, "colour_equal": last[1],
                             "colour_less": colour_vs(&[(P + 32, 2, 1)])?, "colour_more": colour_vs(&[(P + 34, 2, 1)])?},
                "counter": text(0x800B095C, nth(&h, 4)?),
                "clean": text(0x800B096C, nth(&h, 5)?),
                "combo": {"frames": imm(&r6, 0x800B3A98)?,
                          "hits": {"font": field(&c1, 0, "font"), "colour": field(&c1, 0, "colour"), "x": label_x,
                                   "y": field(&c1, 0, "y")},
                          "label": {"text": s(0x800B09B0), "font": field(&c1, 1, "font"), "colour": field(&c1, 1, "colour"),
                                    "x": [x_of(&c1, 1), x_of(&c10, 1)], "y": field(&c1, 1, "y")},
                          "damage": {"font": field(&c1, 2, "font"), "colour": field(&c1, 2, "colour"), "x": x_of(&c1, 2),
                                     "y": field(&c1, 2, "y")},
                          "damage_label": {"text": s(0x800B09C0), "font": field(&c1, 3, "font"),
                                           "colour": field(&c1, 3, "colour"), "x": label_x,
                                           "dx": [damage_label(&c1) - label_x, damage_label(&c10) - label_x,
                                                  damage_label(&c100) - label_x],
                                           "negative_dx": damage_label(&cneg) - damage_label(&c1), "y": field(&c1, 3, "y")}},
                "replay_hint": text(0x800B0918, nth(&h, 0)?),
                "markers": {"frames": imm(&r6, 0x800B40B0)?, "ring": imm(&r6, 0x800B6668)?, "page": page, "bpp": bpp,
                            "kinds": kinds,
                            // (other masks: a2 = 0 at 0x800B65A8)
                            "default_kind": kind(0)?.context("no marker for an unlisted mask")?, "no_marker": none,
                            "labels": labels,
                            "burst": {"u": burst[5] & 0xFF, "v": (burst[5] >> 8) & 0xFF, "w": burst[4] & 0xFFFF,
                                      "h": burst[4] >> 16, "clut": clut_xy((burst[5] >> 16) & 0xFFFF),
                                      "special_clut": clut_xy((special[5] >> 16) & 0xFFFF)}}},
    }))
}

/// The pre-fight VS screen (mode 11, 0x80052520; per battle type 0x80022708; objects 0x80051CDC /
/// 0x80052114 / 0x80052368 / 0x800522F4): lightning panel grid, mode colour bar, portraits in frames with
/// glows, names in font 3, VS and the mode text. Arcade / VS / demo handlers in record 5, Practice in record 6
/// (both at overlay slot A 0x800B0548).
fn versus(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let (r5, r6, r7, r8) = (
        overlay(rec, 5, 0x800B0548)?,
        overlay(rec, 6, 0x800B0548)?,
        overlay(rec, 7, 0x800B0548)?,
        overlay(rec, 8, 0x800B0548)?,
    );
    let s = |o: &Exe, a: i64| strip_fmt(&o.cstr_n(a, 40), "cfHV");
    let slide = |o: &Exe, a: i64| -> Result<Vec<Vec<i64>>> { (0..2).map(|p| o.s16s(a + 8 * p, 4)).collect() };
    let frame = exe.s16s(0x800226C0, 16)?;
    let rows: Vec<i64> = exe.u32s(0x80022668, 8)?.iter().map(|v| v >> 16).collect();
    let colours: Vec<Value> = exe.u32s(0x80022694, 7)?.into_iter().map(rgb).collect();
    let (fp, fnm) = (r7.s16s(0x800B6540, 4)?, r7.s16s(0x800B6548, 4)?);
    let i = |a: i64| imm(exe, a);
    let word = |hi: i64, lo: i64| -> Result<i64> { Ok(exe.u16(hi)? << 16 | exe.u16(lo)?) };
    // the texts (0x800522F4: format, colour, font, x, y): VS and the mode (arcade, record 5), Force's second line
    let texts = |o: &Exe, pc: i64, n: usize| -> Result<Vec<Value>> {
        let t = printfs_to(o, pc, &Run::default(), n, 0x800522F4)?;
        if t.len() < n {
            bail!("no {n} calls to 0x800522F4 in the code at 0x{pc:X}");
        }
        Ok(t)
    };
    let (vs, force) = (texts(&r5, 0x800B3824, 2)?, texts(&r7, 0x800B6164, 2)?);
    let place = |t: &Value| json!({"font": t["font"], "colour": t["colour"], "x": t["x"], "y": t["y"]});
    let (inner_h, source_y) = (i(0x80051E48)?, i(0x80051E4C)?);
    // Team Battle's extras (record 5): per side the strip of members {x, y, step, x with eight} (0x800B06D4), its grey box
    // (0x800B33E8: y - 2 (0x800B3534), 0x800B3524 high, the colour of the lui / ori at 0x800B3408), the two gradients
    // (0x800B3240: [x0, y0, x1, y1] from ori / lui pairs, the bright colour 0x800B32A8 and the dim one 0x800B3278)
    let r5i = |a: i64| imm(&r5, a);
    let xy5 = |lo: i64, hi: i64| -> Result<[i64; 2]> { Ok([r5i(lo)?, r5.u16(hi)?]) };
    let word5 = |hi: i64| -> Result<Value> { Ok(rgb((r5.u16(hi)? << 16) | r5i(hi + 4)?)) };
    let (a0, a1, b0, b1) = (
        xy5(0x800B3284, 0x800B3280)?,
        xy5(0x800B329C, 0x800B3298)?,
        xy5(0x800B32DC, 0x800B32D8)?,
        xy5(0x800B32E4, 0x800B32E0)?,
    );
    let team = json!({"routine": "0x800B3460", "strips": [r5.s16s(0x800B06D4, 4)?, r5.s16s(0x800B06DC, 4)?],
                      "box": {"dy": r5i(0x800B3534)?, "h": r5i(0x800B3524)?, "rgb": word5(0x800B3408)?},
                      "gradients": [[a0[0], a0[1], a1[0], a1[1]], [0, b0[1], b0[0], b1[1]]],
                      "bright": word5(0x800B32A8)?, "dim": word5(0x800B3278)?});
    Ok(json!({
        // (hold_frames: the port's own stand-in for the load; the original holds VS while it loads the fight)
        "routine": "0x80052520", "slide_frames": i(0x800527A4)?, "slide_shift": (exe.u32(0x800520E8)? >> 6) & 31,
        "hold_frames": 60,
        "lightning": {"records": [10 + 2, 11 + 2], "pictures": ["r012/000_full.png", "r013/000_full.png"],
                      "columns": exe.s16s(0x80022658, 7)?, "rows": rows, "tile": i(0x80051A1C)?},
        "bar": {"routine": "0x80051B8C", "y": i(0x80051D38)?, "h": exe.u16(0x80051BE0)?, "fade": -i(0x80051C04)?,
                "colours": colours, "widths": exe.s16s(0x800226B0, 7)?,
                "kinds": ["arcade (1 human)", "vs (2 humans)", "team", "survival", "practice", "ball", "force"]},
        "team": team,
        "portrait": {"routine": "0x80051DB8", "w": frame[2], "h": frame[5] + frame[7],
                     "inner": [i(0x80051DC4)?, i(0x80051DDC)?, i(0x80051E38)?, inner_h],
                     "source_rows": [source_y, source_y + inner_h],
                     "frame_rgb": rgb(word(0x80051F48, 0x80051F4C)?), "frame": frame.chunks(4).collect::<Vec<_>>(),
                     "glow_split": i(0x80051FF8)?, "glow_bottom": rgb(word(0x80052014, 0x80052018)?)},
        "names": {"routine": "0x80052368", "font": i(0x80052400)?, "colour": i(0x800523B0)?,
                  "clamp": [i(0x80052454)?, i(0x8005245C)?]},
        "vs_text": vs.first(),
        "mode_text": vs.get(1).map(place),
        // per mode, the lightning and bar (0x80051CDC: a0, a1; "humans - 1" at run time) from its handler
        "modes": {
            "arcade": {"text": s(&r5, 0x800B06F0), "lightning": "humans - 1", "bar": "humans - 1",
                       "portraits": slide(&r5, 0x800B4A08)?, "names": slide(&r5, 0x800B4A18)?},
            "vs": {"text": s(&r5, 0x800B0704), "lightning": imm(&r5, 0x800B396C)?, "bar": imm(&r5, 0x800B396C)?,
                   "portraits": slide(&r5, 0x800B4A08)?, "names": slide(&r5, 0x800B4A18)?},
            "demo": {"text": s(&r5, 0x800B0718), "lightning": imm(&r5, 0x800B3C74)?, "bar": imm(&r5, 0x800B3C74)?,
                     "portraits": slide(&r5, 0x800B4A08)?, "names": slide(&r5, 0x800B4A18)?},
            "practice": {"text": s(&r6, 0x800B09DC), "lightning": imm(&r6, 0x800B6BB4)?, "bar": imm(&r6, 0x800B6BBC)?,
                         "portraits": slide(&r6, 0x800B8B14)?, "names": slide(&r6, 0x800B8B24)?},
            // Team (r5 0x800B399C): its own slides, FIGHT n (match number), the team bar; Time Attack uses the
            // Arcade handler; Survival (0x800B3AD0): FIGHT n (total wins + 1), red lightning, the survival bar
            "team": {"text": s(&r5, 0x800B0704), "lightning": "humans - 1", "bar": imm(&r5, 0x800B3AA0)?,
                     "portraits": slide(&r5, 0x800B4A28)?, "names": slide(&r5, 0x800B4A38)?},
            "time_attack": {"text": s(&r5, 0x800B06F0), "lightning": "humans - 1", "bar": "humans - 1",
                            "portraits": slide(&r5, 0x800B4A08)?, "names": slide(&r5, 0x800B4A18)?},
            "survival": {"text": s(&r5, 0x800B0704), "lightning": imm(&r5, 0x800B3B94)?, "bar": imm(&r5, 0x800B3B9C)?,
                         "portraits": slide(&r5, 0x800B4A08)?, "names": slide(&r5, 0x800B4A18)?},
            // Tekken Force's stage card (record 7 0x800B6164): one portrait, no VS; STAGE n / FINAL STAGE on a
            // second line, the stage's name in font 3 (right-aligned, a second line below)
            "force": {"text": s(&r7, 0x800B0ABC), "lightning": imm(&r7, 0x800B6348)?, "bar": imm(&r7, 0x800B6350)?,
                      "one_player": true,
                      "portraits": [&fp, &fp], "names": [&fnm, &fnm],
                      "line2": merge(json!({"stage": s(&r7, 0x800B0AD4), "final": s(&r7, 0x800B0AE8)}),
                                     force.get(1).map(place).unwrap_or_default()),
                      // the stage's name in font 3 at 0x800B6550 (x, y), a second line further down
                      "stage_name": {"font": imm(&r7, 0x800B62C4)?, "x": r7.s16(0x800B6550)?,
                                     "advance": exe.s16(0x80021D64 + 16 * 3 + 4)?, "y": r7.s16(0x800B6552)?,
                                     "dy": imm(&r7, 0x800B6334)?}},
            // Tekken Ball (record 8 0x800B50F0): TEKKEN BALL, the ball's bar
            "ball": {"text": s(&r8, 0x800B05E8), "lightning": "humans - 1", "bar": imm(&r8, 0x800B51B0)?,
                     "portraits": slide(&r8, 0x800B6444)?, "names": slide(&r8, 0x800B6454)?},
        },
    }))
}

/// Select screen B (mode 10, 0x80055590): the 7x3 grid select VS MODE, Team Battle and Tekken Ball use (and
/// QUICK SELECT). Background and glow quads (-1 = the mode's colour A / B), the scrolling TEKKEN3 letters and
/// banner, the cell frame lines and colour sets, per-player constants, titles, the VS handicap gauge and win
/// counter (record 5 at slot A 0x800B0548).
fn grid_select(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r5 = overlay(rec, 5, 0x800B0548)?;
    let rgbn = |w: i64| if w == 0xFFFFFFFF { Value::Null } else { rgb(w) };
    let quads = |a: i64, n: i64| -> Result<Vec<Value>> {
        (0..n)
            .map(|k| {
                let colours: Vec<Value> = exe.u32s(a + 32 * k + 16, 4)?.into_iter().map(rgbn).collect();
                Ok(json!({"xy": exe.s16s(a + 32 * k, 8)?, "colours": colours}))
            })
            .collect()
    };
    let clut = clut_xy;
    const PLAYER: [&str; 26] = [
        "frame_flags",
        "_1",
        "tag_uv",
        "tag_clut",
        "cpu_tag_uv",
        "cpu_tag_clut",
        "default_sel",
        "_7",
        "tag_dx",
        "cpu_tag_dx",
        "name_x",
        "name_y",
        "strip_x",
        "strip_y",
        "vs_strip_x",
        "vs_strip_y",
        "side",
        "gauge_x",
        "gauge_hidden_x",
        "gauge_y",
        "life_dx",
        "life_y",
        "percent_dx",
        "percent_y",
        "team_strip_x",
        "team_strip_y",
    ];
    let mut pl = Vec::new();
    for p in 0..2 {
        let v = exe.s16s(0x80098160 + 0x34 * p, 26)?;
        let m: Map<String, Value> = PLAYER
            .iter()
            .zip(v)
            .filter(|(k, _)| !matches!(**k, "_1" | "_7"))
            .map(|(&k, v)| (k.to_string(), if k.ends_with("tag_clut") { clut(v & 0xFFFF) } else { json!(v) }))
            .collect();
        pl.push(m);
    }
    let letters: Vec<Value> = exe
        .u32s(0x800227E8, 7)?
        .iter()
        .map(|&w| json!({"u": w & 0xFF, "v": (w >> 8) & 0xFF, "clut": clut(w >> 16)}))
        .collect();
    let s = |o: &Exe, a: i64| strip_fmt(&o.cstr_n(a, 60), "cfHV");
    let mut cells = Vec::new();
    for k in 0..21 {
        let a = 0x80022768 + 6 * k;
        cells.push(json!({"x": exe.u16(a)?, "y": exe.u16(a + 2)?, "character": exe.u8(a + 4)? >> 2}));
    }
    let mut mode_colours = Vec::new();
    for k in 0..9 {
        let ab = exe.u32s(0x80022B4C + 8 * k, 2)?;
        mode_colours.push([rgbn(ab[0]), rgbn(ab[1])]);
    }
    let mut frame_lines = Vec::new();
    for k in 0..10 {
        frame_lines.push(json!({"colour": exe.s32(0x800215EC + 12 * k)?, "xy": exe.s16s(0x800215EC + 12 * k + 4, 4)?}));
    }
    let frame_colours: Vec<Vec<Value>> =
        (0..4).map(|k| Ok(exe.u32s(0x80021664 + 40 * k, 3)?.into_iter().map(rgb).collect())).collect::<Result<_>>()?;
    let titles: Vec<String> = exe.u32s(0x800981C8, 9)?.into_iter().map(|p| exe.cstr_n(p, 60)).collect();
    let mut pieces = Vec::new();
    for k in 0..8 {
        let a = 0x800B065C + 6 * k;
        pieces.push(json!({"w": r5.u8(a)?, "lit": clut(r5.u16(a + 2)?), "unlit": clut(r5.u16(a + 4)?)}));
    }
    let levels: Vec<String> = r5.u32s(0x800B49E8, 8)?.into_iter().map(|p| r5.cstr(p).replace("%%", "%")).collect();
    // (each level's full health in percent: its text's number)
    let percents: Vec<i64> = levels
        .iter()
        .map(|l| l.trim().trim_end_matches('%').parse().context("a LIFE level's percent"))
        .collect::<Result<_>>()?;
    let (no_entry, selected) = (exe.cstr(0x8002292C), exe.cstr(0x80022938));
    let team_size = json!({"routine": "0x800547F8", "format": strip_fmt(&exe.cstr(0x80022954), "cfHV"),
                           "players": [exe.cstr(0x80022944), exe.cstr(0x8002294C)]});
    let i = |a: i64| imm(exe, a);
    // the cells (0x80022768): a row's length, the step across; a cell's height (0x80054380)
    let cell = |k: usize, f: &str| cells.get(k).map_or(0, |c| c[f].as_i64().unwrap_or(0));
    let columns = cells.iter().take_while(|c| c["y"] == cells[0]["y"]).count();
    // the scrolling letters (0x80055080: textured quads in page 0x80055288, u + 24 / v + 40 drawn 48 wide, at y
    // 0x800551E4) and banner (printf 0x80055394), both stepping on modulo their period
    let (page, bpp) = tpage_xy(exe.u16(0x80055288)?);
    let (letter_w, period, banner_period) = (i(0x80055244)?, i(0x800552E8)?, -i(0x8005535C)?);
    // the question-mark face (0x8004B928 with selection 88, or 89 the blue one: face + 1): three slices, [u from
    // their block, y, h, [VRAM x, y of the block]] in its face's page (0x8002152C); the third is in the second's block
    let q = i(0x80054504)? >> 2;
    let (face_page, _) = tpage_xy(exe.u16(0x8002152C + 8 * q + 4)?);
    let page_xy = [face_page[0].as_i64().unwrap_or(0), face_page[1].as_i64().unwrap_or(0)];
    let question =
        |sel: i64| trace(exe, 0x8004B928, &Run { args: &[0, 0, 0, 0, sel], ..Run::default() }, 3, 0x8004D9BC);
    let (gold, blue) = (question(i(0x80054504)?)?, question(i(0x80054500)?)?);
    let (face_dy, at) = (i(0x8004BA5C)?, |k: usize| gold.get(k).copied().unwrap_or_default());
    let slice = |k: usize, block: usize| {
        let (c, b) = (at(k), at(block));
        json!([
            (c[5] & 0xFF) - (b[5] & 0xFF),
            (c[3] >> 16) - face_dy,
            c[4] >> 16,
            [page_xy[0] + (b[5] & 0xFF) / 4, page_xy[1] + ((b[5] >> 8) & 0xFF)]
        ])
    };
    let clut_of = |v: &[Args]| clut_xy(v.first().map_or(0, |c| (c[5] >> 16) & 0xFFFF));
    // VS mode's panel (record 5 0x800B2FE0: a box with soft sides, then a tab under it) and wins (0x800B287C)
    let drawn = |t: i64| -> Result<Vec<Args>> { trace(&r5, 0x800B2FE0, &Run::default(), usize::MAX, t) };
    let (boxes, soft) = (drawn(0x8004DA10)?, drawn(0x8004D8B0)?);
    let corner = |k: usize, a: usize| soft.get(k).map_or([0, 0], |c| [c[a] & 0xFFFF, c[a] >> 16]);
    let wins = printfs(&r5, 0x800B287C, &Run { args: &[0x1F80_0000], ..Run::default() })?;
    let place = |k: usize| wins.get(k).map_or([Value::Null, Value::Null], |t| [t["x"].clone(), t["y"].clone()]);
    let font = |k: usize| wins.get(k).map_or(Value::Null, |t| t["font"].clone());
    let (w0, w1, draws, dash, draw) = (place(0), place(1), place(2), place(3), place(4));
    let colour = |k: usize| wins.get(k).map_or(Value::Null, |t| t["colour"].clone());
    // the LIFE gauge (record 5 0x800B2A18): the frame's caps (w, h: the ori / lui at 0x800B2C28 / 0x800B2C24; uv
    // 0x800B2C30 and, x + 0x800B2C7C, 0x800B2C6C) and its middle (from x + 0x800B2C94, 0x800B2CC4 wide, u
    // 0x800B2C9C to 0x800B2CA4), the gauge from x - 128 (0x800B2BEC) on the left side; the dark box (x + 0x800B2C04,
    // y + 0x800B2C14, w, h: the ori / lui at 0x800B2BF4 / 0x800B2BF0); the pieces (from x - 1 / x + 2, 0x800B2AE8 /
    // 0x800B2AEC; y + 0x800B2AF8 to + 0x800B2B00; uv 0x800B2BB4); the arrow (x - 95 / x + 87, 0x800B2AA4 / 0x800B2AA0;
    // w, h the ori / lui at 0x800B2AAC / 0x800B2AA8; uv 0x800B2AB4); the page: the lui at 0x800B2CA0
    let r5i = |a: i64| imm(&r5, a);
    let uv5 = |a: i64| -> Result<Value> { Ok(json!([r5i(a)? & 0xFF, (r5i(a)? >> 8) & 0xFF])) };
    let (gauge_page, _) = tpage_xy(r5.u16(0x800B2CA0)?);
    let gauge = json!({"page": gauge_page, "left": r5i(0x800B2BEC)?,
                       "caps": {"w": r5i(0x800B2C28)?, "h": r5.u16(0x800B2C24)?, "uv": [uv5(0x800B2C30)?, uv5(0x800B2C6C)?],
                                "right_dx": r5i(0x800B2C7C)?},
                       "middle": {"dx": r5i(0x800B2C94)?, "w": r5i(0x800B2CC4)?, "uv": uv5(0x800B2C9C)?,
                                  "texels": (r5i(0x800B2CA4)? & 0xFF) - (r5i(0x800B2C9C)? & 0xFF)},
                       "box": [r5i(0x800B2C04)?, r5i(0x800B2C14)?, r5i(0x800B2BF4)?, r5.u16(0x800B2BF0)?],
                       "pieces_x": [r5i(0x800B2AE8)?, r5i(0x800B2AEC)?], "pieces_y": [r5i(0x800B2AF8)?, r5i(0x800B2B00)?],
                       "piece_uv": uv5(0x800B2BB4)?,
                       "arrow": {"dx": [r5i(0x800B2AA4)?, r5i(0x800B2AA0)?], "w": r5i(0x800B2AAC)?, "h": r5.u16(0x800B2AA8)?,
                                 "uv": uv5(0x800B2AB4)?}});
    let (uv_u, uv_v) = (exe.u16(0x8004E770)? & 0xFF, exe.u16(0x8004E770)? >> 8);
    let mut out = json!({
        // (the battle types: the game's numbers for its modes)
        "routine": "0x80055590", "modes": {"vs": 1, "team": 2, "ball": 7},
        "cells": cells, "columns": columns, "cell_w": cell(1, "x") - cell(0, "x"), "cell_h": i(0x800545DC)?,
        "mode_colours": mode_colours,
        "background": quads(0x80022A8C, 4)?, "glows": quads(0x80022B0C, 2)?,
        "letters": {"cells": letters, "page": page, "bpp": bpp, "w": letter_w, "h": i(0x80055298)?,
                    "scale": i(0x80055268)? / letter_w, "y": exe.u16(0x800551E4)?, "step": i(0x800552F8)?,
                    "period": period, "speed": i(0x800551D4)? - period},
        "banner": {"text": exe.cstr_n(0x800981EC, 60), "font": i(0x80055388)?, "colour": i(0x80055384)?,
                   "y": i(0x80055328)?, "period": banner_period, "speed": i(0x80055320)? - banner_period},
        "frame_lines": frame_lines,
        "frame_colours": frame_colours,
        "face": {"dx": i(0x8004BA58)?, "dy": face_dy, "question": q, "question_blue": (i(0x80054500)? >> 2) + i(0x8004BB08)?,
                 "question_slices": [slice(0, 0), slice(1, 1), slice(2, 1)],
                 "question_cluts": {"gold": clut_of(&gold), "blue": clut_of(&blue)}},
        "players": pl,
        "titles": titles,
        // (the ticker: titles[6], 0x80054C88)
        "ticker": {"index": 6, "font": i(0x80054CA0)?, "y": i(0x80054CB8)?,
                   "colours": [i(0x80054C9C)?, i(0x80054CC0)?, i(0x80054C9C)?, i(0x80054CC0)?, i(0x80054C9C)?, i(0x80054CC0)?],
                   "note": "each %c switches to the next colour"},
        "title": {"font": i(0x80054C5C)?, "colour": i(0x80054C58)?, "y": i(0x80054C78)?, "idle_frames": i(0x80054BA4)?},
        "names": {"font": i(0x800549A8)?, "colour": i(0x80054948)?, "y": pl.first().map_or(Value::Null, |p| p["name_y"].clone()),
                  "no_entry": no_entry, "selected": selected, "no_entry_colour": i(0x80054964)?,
                  "selected_colour": i(0x80054970)?, "bar_dy": i(0x800549BC)?, "bar_h": i(0x8004E72C)?, "bar_u": uv_u,
                  "bar_v": uv_v,
                  // (the bar from x + 0x800549B8, the name's width + 0x800549D0 wide; PUSH START at y + 0x800549F8)
                  "bar_dx": i(0x800549B8)?, "bar_dw": i(0x800549D0)?, "push_dy": i(0x800549F8)?},
        // (a cursor's tag, 0x80054520: y - 4 (0x8005456C), w and h 0x80054570 / 0x80054574, in the page 0x800545C4)
        "tag": {"dy": i(0x8005456C)?, "w": i(0x80054570)?, "h": i(0x80054574)?, "page": tpage_xy(i(0x800545C4)?).0},
        // (Team Battle and Ball: VS between the sides, 0x8005581C)
        "vs_text": printf_call(exe, 0x80055804)?,
        "vs": {"panel": {"box": [corner(0, 6)[0], corner(0, 6)[1], corner(1, 7)[0], corner(1, 9)[1]],
                         "soft": corner(0, 7)[0] - corner(0, 6)[0],
                         "tab": [corner(2, 6)[0], corner(2, 6)[1], corner(3, 7)[0], corner(2, 8)[1]],
                         // (the tab's slanted sides: its bottom edge's two ends)
                         "tab_bottom": [corner(2, 8)[0], corner(3, 9)[0]],
                         "darken": boxes.first().map_or(0, |c| c[2] & 0xFF)},
               "wins_format": s(&r5, 0x800B0630), "win_x": [&w0[0], &w1[0]], "win_y": w0[1], "win_font": font(0),
               "win_colours": bytes_list(r5.bytes(0x800B49E0, 4)), "dash": dash,
               "draw_text": [s(&r5, 0x800B064C), &draw[0], &draw[1]],
               "draws": draws, "draw_font": font(2), "draw_colours": bytes_list(r5.bytes(0x800B49E4, 4)),
               "dash_font": font(3), "dash_colour": colour(3), "draw_text_font": font(4), "draw_text_colour": colour(4),
               // (the handicap's first level: 0x800DAB78, record 10)
               "gauge": {"pieces": pieces,
                         "levels": levels, "percents": percents, "default": imm(&overlay(rec, 10, 0x800B8D58)?, 0x800DAB78)?,
                         "life": r5.cstr(0x800B06CC), "slide": imm(&r5, 0x800B2DC8)?,
                         "frame_clut": clut_xy(r5.u16(0x800B2C2C)?), "arrow_clut": clut_xy(r5.u16(0x800B2AB0)?),
                         "draw": gauge}},
        // (exit: the back sound 0x8004B5F4)
        "sounds": {"cursor": i(0x80053930)?, "pick": i(0x80052F9C)?, "exit": i(0x8004B600)?}, "music": i(0x800556C8)?,
    });
    out["team_size"] = team_size;
    Ok(out)
}

/// In-engine cut-scenes. The True Ogre scene (arcade overlay record 5 at 0x800B0548: init 0x800B3CA0, per
/// frame 0x800B4060): after the human wins a round against the stage-10 opponent (Ogre), the human's fighter is
/// shown as the stage-9 boss (Heihachi, or Jin for a Heihachi player) lying under Ogre, both at the origin facing
/// +x, playing Ogre's moves 2227 / 2228 (the body borrows Ogre's animation), on Ogre's scripted camera; BGM 5,
/// then an additive white fade. Afterwards the opponent becomes True Ogre and the match goes on (no VS screen).
/// Values are read from the code and tables at the addresses given.
pub fn scenes(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r5 = overlay(rec, 5, 0x800B0548)?;

    let script = |mut a: i64| -> Result<Vec<Vec<i64>>> {
        let mut out = Vec::new();
        loop {
            let e = r5.s16s(a, 5)?;
            a += 10;
            let end = e[1] == -1;
            out.push(e);
            if end {
                return Ok(out);
            }
        }
    };
    let cam_id = imm(exe, 0x80067160)?;
    Ok(json!({"boss_change": {
        "routine": {"init": "0x800B3CA0", "frame": "0x800B4060", "trigger": "0x80051204", "swap": "0x800B2870"},
        "trigger": {"stage_index": 9, "note": "arcade / time attack, one human, once a run; when a round ends with the human at >= 1 win and the CPU short of the match (time attack: no scene, straight to the swap)"},
        // [frame, cmd, a, b, c]: cmd 2 / 3 = P1 / P2 starts alias a from move frame b (end c); 5 = white fade at a
        // per frame; 6 = BGM; -1 = end. Table by the human's side.
        "scripts": {"p1_human": script(0x800B0728)?, "p2_human": script(0x800B075C)?},
        "bgm": imm(&r5, 0x800B4320)?,
        // scripted camera: the id in Ogre's moveset (moveset key 0xE, 0x80067158), selector entry id - 0x2B + 1
        "camera_moveset": imm(exe, 0x80067158)?, "camera_id": cam_id, "camera_selector": cam_id - 0x2B + 1,
        // mode-1 camera (0x800665D8): eye = channels 0..2 / 2, target = 3..5 / 2, then the dolly eye = target +
        // (eye - target) * 500 / 450 (+0x6A); anchor (0, 0, 0), yaw 0, H 500
        "camera_halve": 2, "camera_dolly": [500, 450],
        // the body: the stage-9 boss's model index (0x800958C4 index) by the boss costume (0 / other); a Heihachi
        // player (character 13) gets Jin
        "body": {"alt_character": imm(&r5, 0x800B3DA0)?,
                 "model_index": [imm(&r5, 0x800B3DC4)?, imm(&r5, 0x800B3DC0)?],
                 "model_index_alt": [imm(&r5, 0x800B3DB8)?, imm(&r5, 0x800B3DB0)?]},
        "facing": 0x4000,
        // fighters show move frame (scene frame - 9): none before the scripted start; the stage stops drawing once the
        // fade level reaches 0xD0 (0x800B4500..); then 4 frames of full white (state 2)
        "stage_off_level": 0xD0, "white_frames": 4,
        "becomes": imm(&r5, 0x800B2870)?,
    }}))
}

/// The attract loop (attract research): a 4-step cycle on counter 0x80097F3E, the NAMCO PRESENTS screen
/// (0x8004FA60), the title timings (record 10 0x800DB1B8 / record 0 0x800D3228), the embu (record 0 0x800D3748:
/// scripts, casts, models) and the demo fight (battle type 6). Tables are read from overlay record 0; timings and
/// switches are code constants with their addresses.
pub fn attract(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r0 = rec.first().context("no record 0")?;
    let b = embu::R0_BASE;
    let mut variants = Vec::new();
    for (n, (script, models)) in embu::scripts(r0)?.into_iter().zip(embu::models(rec)?).enumerate() {
        // model index -> the glb that shows it (an embu-only mesh, or the normal costume)
        let m: Map<String, Value> = models
            .iter()
            .map(|(m, &(p, normal, same))| {
                (m.to_string(), json!(if same { format!("model_{normal:03}.glb") } else { embu::glb_name(n, p) }))
            })
            .collect();
        variants.push(json!({"initial_models": [embu::INITIAL[n].0, embu::INITIAL[n].1], "archive_record": 1 + n,
                             "script": script, "models": m}));
    }
    let p1 = 0x800D55BC - b;
    let namco = printf_call(exe, 0x8004F804)?;
    let parts: Vec<Value> = namco["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(namco["colours"].as_array().into_iter().flatten())
        .map(|(t, c)| json!([t, c]))
        .collect();
    // the fade: a box over the whole screen (0x8004DA10: a3 = y << 16 | x, sp+16 = h << 16 | w)
    let [_, _, _, at, wh, ..] = calls(exe, 0x8004F82C, 1, 0x8004DA10)?.into_iter().next().context("no fade box")?;
    let fade_box = [at & 0xFFFF, at >> 16, wh & 0xFFFF, wh >> 16];
    // the attract demo's versus screen: stage 1
    let stage_1 = strip_fmt(&exe.cstr(0x8001A464), "cfHV").replace("%1x", "1");
    Ok(json!({
        "cycle": {"counter": "0x80097F3E", "steps": [
            {"step": 0, "presents": true, "movie": "video/20.avi", "title": "after_movie", "records_after_demo": true},
            {"step": 1, "presents": true, "embu": true, "title": "after_embu", "records_after_demo": true},
            {"step": 2, "presents": true, "movie": "video/21.avi", "title": "after_movie", "records_after_demo": false},
            {"step": 3, "presents": false, "title": "white_in", "records_after_demo": true}],
            "note": "START on a title / embu / movie advances the step too; the main menu idles 480 frames back to the cycle"},
        "presents": {"routine": "0x8004FA60", "parts": parts, "font": namco["font"], "x": namco["x"],
                     "y": namco["y"], "fade_box": fade_box, "fade_in": 32, "fade_step": 8, "hold": 91,
                     "fade_out": 32, "black": 2, "start_sfx": 0x4CC0},
        "title": {"white_in": [26, 10], "white_hold": 11, "white_out": [86, 3], "hold": 300, "black_out": [32, 8],
                  "after_embu_black": 1, "menu_idle": 480},
        "embu": {
            "routine": "0x800D3748", "stage": 4, "bgm": 0, "moveset": "movesets/moveset_embu.json",
            "alias_base": 3438, "end_frame": 3542, "swap_frames": 7, "wait_frames": 4,
            // the camera: a chain of the set's camera motions, shot k = id 0x2B + 2k (selector 2k + 1); script frame
            // c shows key c + 1 of the chained shots (mode 1 math as the True Ogre scene: halve, dolly 500/450)
            "camera": {"first_selector": 1, "selector_step": 2, "shots": 22, "key_offset": 1, "halve": 2,
                       "dolly": [500, 450]},
            "actors": {"position": [0, 0, 0], "facing": 0x4000},
            "unlock": {"variant_2_character": 19, "variant_1_unlocked_at_least": 15},
            "variants": variants},
        "demo_fight": {
            "routine": "0x800D2E3C", "p1_list": bytes_list(crate::disc::py_slice(r0, p1, p1 + 14)), "health": 125,
            "round_time_option": 2, "ai_level": 4, "end_frames_after_ko": 90, "stage_from_p1": true,
            "vs_text": stage_1, "game_over": printf_call(exe, 0x8003D8A8)?,
            "note": "P1 rotates through p1_list (unlocked); P2 = list[((frame & 0xFFF) % (n - 1) + i1 + 1) % n]; one round only, no replay or win pose; START (either pad) -> main menu, no other button"},
    }))
}

/// Tekken Ball (battle type 7; docs: ball research). Record 8 is its overlay (slot A, 0x800B0548): the ball
/// types (0x800B5F9C), the ball mesh (UV sphere, radius 384: vertices 0x800B5FE4, normals 0x800B6134, triangles
/// 0x800B6284, quads 0x800B62C4, face colours 0x800B6364 + 64 * type), the body-test offsets (0x800B6344) and the
/// select texts (0x800B6464). The rules that are code immediates are given with their routines.
pub fn ball(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r8 = overlay(rec, 8, 0x800B0548)?;
    let names: Vec<String> = (0..6).map(|k| Ok(r8.cstr_n(r8.u32(0x800B6464 + 4 * k)?, 16))).collect::<Result<_>>()?;
    let mut types = Vec::new();
    for t in 0..3 {
        let a = 0x800B5F9C + 24 * t;
        let colours: Vec<Value> = r8.u32s(0x800B6364 + 64 * t, 16)?.into_iter().map(rgb).collect();
        types.push(json!({"speed_unit": r8.u32(a)?, "lob_unit": r8.u32(a + 4)?, "gravity": r8.u32(a + 8)?,
                          "flag": r8.u8(a + 12)?, "ambient": [r8.u8(a + 13)?, r8.u8(a + 14)?, r8.u8(a + 15)?],
                          "damage_percent": r8.u32(a + 16)?, "zone": r8.u32(a + 20)?,
                          "level": names[2 * t as usize], "name": names[2 * t as usize + 1],
                          "colours": colours}));
    }
    let rows =
        |a: i64, n: i64, stride: i64| -> Result<Vec<Vec<i64>>> { (0..n).map(|k| r8.s16s(a + stride * k, 3)).collect() };
    let i8 = |a: i64| imm(&r8, a);
    // a gauge's fill: the CLUT of colour c (0x800B4BF4: row c >> 4 | the ori there, column c & 15 | the ori at
    // 0x800B4C54)
    let (row, col) = (r8.u16(0x800B4BF8)?, r8.u16(0x800B4C54)?);
    let fill = |c: i64| clut_xy((((c >> 4) | row) << 6) | (c & 15) | col);
    // the select's damage (0x800B5604: its label, then the value and its sign; the buffer's format is built)
    let shown = calls(&r8, 0x800B5604, 2, PRINTF)?;
    let (label, damage) = (shown.first().copied().unwrap_or_default(), shown.get(1).copied().unwrap_or_default());
    let tris: Vec<Value> = (0..16).map(|k| bytes_list(r8.bytes(0x800B6284 + 4 * k, 3))).collect();
    // the moves the rules push back (0x800B068C): a switch on alias - first (0x800B06C4) over `n` cases (0x800B06D0,
    // jump table 0x800B0548); each case block's stores: push (+2), frames (+3, +4), the move's end frame (+5)
    let (first, n) = (-i8(0x800B06C4)?, i8(0x800B06D0)?);
    let mut push_back = Vec::new();
    for (case, push, frames, cancel) in [
        (0x800B06F8, Some(0x800B0718), Some([0x800B0720, 0x800B0728]), None),
        (0x800B0734, Some(0x800B0754), Some([0x800B075C, 0x800B0764]), Some(0x800B076C)),
        (0x800B0778, None, None, Some(0x800B0790)),
    ] {
        let mut aliases = Vec::new();
        for k in 0..n {
            if r8.u32(0x800B0548 + 4 * k)? == case {
                aliases.push(first + k);
            }
        }
        let or0 = |a: Option<i64>| a.map_or(Ok(0), i8);
        push_back.push(json!({"aliases": aliases, "push": or0(push)?,
                              "frames": [or0(frames.map(|f| f[0]))?, or0(frames.map(|f| f[1]))?], "cancel": or0(cancel)?}));
    }
    let moves = json!({"routine": "0x800B068C, EXE 0x8003F554 / 0x8002DB9C", "push_back": push_back,
                       "format": "pushed back `push` units a frame in frames..=frames (none: 0), the move ending at frame `cancel` (0: its own end)",
                       "self_charge": {"routine": "0x800B16F4", "moveset": i8(0x800B16F8)?, "except_alias": i8(0x800B1764)?,
                                       "note": "this move set's unblockable or throw-type hit on the ball (an alias but this) takes the other side's charge from its own health"}});
    // the system sounds the overlay plays (SePlay a1): the serve, the drop after a point, a bounce (twice), the "!"
    // pop-up, the smash, the Attack Ball hitting a fighter (50+ power / below; guarded), a bump
    let bounce = i8(0x800B0A30)?;
    if i8(0x800B1388)? != bounce {
        bail!("the ball's two bounce sounds differ");
    }
    let sounds = json!({"serve": i8(0x800B2738)?, "drop": i8(0x800B29D8)?, "bounce": bounce, "pop": i8(0x800B18B4)?,
                        "smash": i8(0x800B1C14)?, "hit": [i8(0x800B2204)?, i8(0x800B2208)?], "guarded": i8(0x800B221C)?,
                        "bump": i8(0x800B229C)?});
    let quads: Vec<Value> = (0..32).map(|k| bytes_list(r8.bytes(0x800B62C4 + 4 * k, 4))).collect();
    // (the caps: w, h the ori / lui at 0x800B49E8 / 0x800B49C4, uv and CLUT 0x800B4B60 / its lui and 0x800B4B94,
    // the right one at x + 0x800B4B14; the strip: uv 0x800B4CBC, h 0x800B4CA4, from x + 0x800B4BD4 rightwards
    // or x + 0x800B4BE0 leftwards)
    // the select's arrows beside the ball while its type is chosen (0x800B53B8: two flat triangles, 0x8004D71C, each
    // [x, y] from an ori / lui pair; shown while frame & 0x20, 0x800B53B0, in the colour of 0x800B53CC)
    let at = |lo: i64, hi: i64| -> Result<[i64; 2]> { Ok([i8(lo)?, r8.u16(hi)?]) };
    let ball_arrows = json!({"routine": "0x800B53B8",
        "triangles": [[at(0x800B53E4, 0x800B53D0)?, at(0x800B53BC, 0x800B53B8)?, at(0x800B53C4, 0x800B53C0)?],
                      [at(0x800B5418, 0x800B5410)?, at(0x800B53F8, 0x800B53F4)?, at(0x800B5400, 0x800B53FC)?]],
        "colour": rgb(i8(0x800B53CC)?), "blink": i8(0x800B53B0)?});
    let gauges = json!({"routine": "0x800B49A8", "w": i8(0x800B4AE8)?, "x": [r8.s16(0x800B6430)?, r8.s16(0x800B6440)?],
                       "y": r8.s16(0x800B6432)?, "full": i8(0x800B0AC0)?, "grow": i8(0x800B4AB0)?,
                       "caps": {"u": [i8(0x800B4B60)? & 0xFF, i8(0x800B4B94)? & 0xFF], "v": i8(0x800B4B60)? >> 8,
                                "w": i8(0x800B49E8)?, "h": r8.u16(0x800B49C4)?, "right_dx": i8(0x800B4B14)?,
                                "clut": clut_xy(r8.u16(0x800B4B5C)?)},
                       "strip": {"u": i8(0x800B4CBC)? & 0xFF, "v": i8(0x800B4CBC)? >> 8, "h": i8(0x800B4CA4)?,
                                 "from_dx": [i8(0x800B4BD4)?, i8(0x800B4BE0)?]},
                       "cluts": {"p1": fill(i8(0x800B4A2C)?), "p2": fill(i8(0x800B4A20)?), "trail": clut_xy(i8(0x800B4C64)?),
                                 "empty": clut_xy(i8(0x800B4C68)?)}});
    // the floating texts (0x800B3F98: text, place, colour, font, frames; drawn rising a line a frame,
    // 0x800B3E8C): the hits' font and frames (0x800B1A98, 0x800B18A0), font 1's lift (0x800B40F4) and its
    // half advance (a shift and a subtract at 0x800B40DC); a ring of 8 (0x800B3FE0)
    let texts = json!({"routine": "0x800B3F98", "font": i8(0x800B1A98)?, "frames": i8(0x800B18A0)?, "dy": i8(0x800B40F4)?,
                      "dx_per_char": 7, "max": i8(0x800B3FE0)?});
    let mut out = json!({
        "note": "game units, y down; velocities in 1/256 unit per frame; angles 4096 per turn",
        "types": types,
        "mesh": {"vertices": rows(0x800B5FE4, 42, 8)?, "normals": rows(0x800B6134, 42, 8)?, "triangles": tris, "quads": quads,
                 "colour_index": "triangles use colour [face & 7], quads [8 + (face & 7)]"},
        "body_offsets": rows(0x800B6344, 4, 8)?,
        "constants": {
            "routine": "0x800B11DC (state machine), 0x800B0B14 (reset), 0x800B07C8 (round setup)",
            "attack_radius": 0x260, "rest_y": -0x180, "serve": {"x": 0xD00, "y": -0x500, "first_round_wait": 100,
                                                                  "wait": 40, "appear": 32, "drop_after": 600},
            "respawn_y": -0xE40, "smash_apex": -0x1400, "ceiling": -0x2000, "out_x": 0x2800, "point_damage": 30,
            "rest_limit": 600, "drag": 0x20, "cooldown": 5, "z_band": 0x80, "fighter_x": 0x1400,
            "court": {"x": 0x1400, "z": 0x100, "height": -0x1200}, "max_health": 130,
            "replay_passes": [2, 6], "replay_ring": 30, "loser_shot_frames": 240,
            "moves": moves,
            "sounds": sounds,
            "cpu": {"routine": "0x800B4DC8", "pool": (0..14).chain([16, 18]).collect::<Vec<_>>(), "spread": 4, "first_ever": 17,
                    "level": "difficulty * 4 + 1"},
            // (its stage, the beach)
            "stage": imm(exe, 0x8004F71C)?,
            "court_lines": {"routine": "0x800B38DC", "centre_half_width": 100, "z0": -8000, "dz": 1536,
                            "brightness": [0, 48, 96, 144, 192, 144, 96, 48, 0]},
            "select": {"routine": "0x800B5478", "level": {"font": i8(0x800B5554)?, "colour": i8(0x800B5550)?, "y": i8(0x800B5568)?},
                       "name": {"font": i8(0x800B55C0)?, "colour": i8(0x800B55BC)?, "y": i8(0x800B55DC)?},
                       "damage_label": {"text": r8.cstr(0x800B0644), "font": label[2], "colour": label[1], "x": label[3],
                                        "y": label[4]},
                       // "%c%f%H%V%3d%f%H%V%%": the damage, then the sign
                       "damage": {"font": damage[2], "colour": damage[1], "x": damage[3], "y": damage[4]},
                       "percent": {"font": damage[6], "x": damage[7], "y": damage[8]}},
            "vs_text": r8.cstr_n(0x800B05E8 + 8, 16)},
    });
    out["constants"]["gauges"] = gauges;
    out["constants"]["texts"] = texts;
    out["constants"]["select"]["arrows"] = ball_arrows;
    // (the ball itself is a 3D model the select's camera renders (0x800B2CD0); the port draws it flat at about this
    // centre and radius, measured)
    out["constants"]["select"]["ball"] = json!({"x": 178, "y": 352, "radius": 39});
    Ok(out)
}

/// Tekken Force (battle type 8; docs: force research). Record 7 is its overlay (slot A, 0x800B0548); records
/// 51-54 its stages, whose part 3 is the event script (128 x 20 bytes, read in order). Tables from the overlay;
/// rules that are code immediates are given with their routines.
pub fn force(exe: &Exe, rec: &[Vec<u8>]) -> Result<Value> {
    let r7 = overlay(rec, 7, 0x800B0548)?;
    // The final stage's character: 0x800B1EA0, after FINAL STAGE CHALLENGE, into slot 1. Its music is derived, not
    // read: record 7 sets no BGM for the final stage, so it gets what the boss stages play (the boss's selection
    // descriptor's byte 11, 0x800B2848): this character's first descriptor's byte 11.
    let final_character = imm(&r7, 0x800B1EA0)?;
    let final_bgm = exe.u8(exe.u32(0x80097D40 + 16 * final_character)? + 11)?;
    let s7 = |a: i64| strip_fmt(&r7.cstr_n(a, 40), "cfHV");
    let mut names = Vec::new();
    for i in 0..5 {
        let (a, b) = (r7.u32(0x800B6558 + 8 * i)?, r7.u32(0x800B6558 + 8 * i + 4)?);
        names.push([r7.cstr_n(a, 24), r7.cstr_n(b, 24)]);
    }
    let script = |r: usize| -> Result<Vec<Value>> {
        let t = overlay(rec, r, 0)?;
        if t.u32(0)? < 4 {
            return Ok(Vec::new());
        }
        let (o, size) = (t.u32(4 + 8 * 3)?, t.u32(4 + 8 * 3 + 4)?);
        let mut out = Vec::new();
        for k in 0..size / 20 {
            let a = o + 20 * k; // '<ihhhhhhhh': x, type, dx, _, z, _, p0, p1, _
            let typ = t.s16(a + 4)?;
            out.push(
                json!({"x": t.s32(a)?, "type": typ, "dx": t.s16(a + 6)?, "z": t.s16(a + 10)?, "p0": t.s16(a + 14)?,
                            "p1": t.s16(a + 16)?}),
            );
            t.s16(a + 18)?; // the struct needs all 20 bytes
            if typ == 7 {
                break;
            }
        }
        Ok(out)
    };
    let r7raw = overlay(rec, 7, 0)?;
    let stream: Vec<[i64; 2]> =
        (0..64).map(|k| Ok([r7raw.s16(6 * k)?, r7raw.s16(6 * k + 2)?])).collect::<Result<_>>()?;
    let mut routes = Vec::new();
    for sel in 0..92 {
        let p = r7.u32(0x800B6378 + 4 * sel)?;
        routes.push((0..4).map(|k| r7.s16s(p + 4 * k, 2)).collect::<Result<Vec<_>>>()?);
    }
    let mut stages = Vec::new();
    for i in 0..5i64 {
        stages.push(json!({"name": names[i as usize], "record": if i < 4 { 51 + i.min(3) } else { 51 },
                           "stage": if i < 4 { 15 + i } else { 15 }, "name_colour": r7.u8(0x800B6580 + i)?,
                           "script": if i < 4 { script(51 + i as usize)? } else { Vec::new() }}));
    }
    let health: Vec<Vec<i64>> = (0..5)
        .map(|i| (0..5).map(|c| Ok(r7.u32(0x800B0684 + 20 * i + 4 * c)? >> 16)).collect())
        .collect::<Result<_>>()?;
    let classes: Vec<Value> = (0..5)
        .map(|c| Ok(json!({"bar_w": r7.s16(0x800B0670 + 4 * c)?, "seconds": r7.s16(0x800B0670 + 4 * c + 2)?})))
        .collect::<Result<_>>()?;
    let costumes: Vec<Value> = (0..5).map(|s| bytes_list(r7.bytes(0x800B06E8 + 2 * s, 2))).collect();
    // the countdown voice (0x800B2590): at each frame left (an immediate) the sound-list code the lhu after it reads
    // (lui 0x8009 + its offset)
    let mut countdown = Vec::new();
    for (frames, code) in [
        (0x800B259C, 0x800B25B4),
        (0x800B25AC, 0x800B25CC),
        (0x800B25C4, 0x800B25E4),
        (0x800B25DC, 0x800B25FC),
        (0x800B25F4, 0x800B2614),
    ] {
        countdown.push([imm(&r7, frames)?, exe.u16(0x8009_0000 + r7.s16(code)?)?]);
    }
    // the final character's health: the health table's, its class (+0xE) set at 0x800B1E7C, on the final stage's row
    let final_health = pyget(pyget(&health, 4)?, imm(&r7, 0x800B1E7C)?)?;
    // the CPU's level per stage (0x800B5C2C): a switch over the stages below 5 (0x800B5C38, jump table 0x800B0A48)
    // whose cases store a level (the instruction after their lui); the default 0x800B5C90 stores 0
    let mut ai_level = Vec::new();
    for i in 0..imm(&r7, 0x800B5C38)? {
        let case = r7.u32(0x800B0A48 + 4 * i)?;
        ai_level.push(if case == 0x800B5C90 { 0 } else { imm(&r7, case + 4)? });
    }
    let i7 = |a: i64| imm(&r7, a);
    let uv7 = |a: i64| -> Result<Value> { Ok(json!([i7(a)? & 0xFF, (i7(a)? >> 8) & 0xFF])) };
    // the HUD's pieces (record 7): the bars (0x800B2C34: right edge x + 0x800B2CE8; caps w, h the ori / lui at
    // 0x800B2C6C / 0x800B2C48, uv and CLUT 0x800B2E28 / its lui and 0x800B2E60; the strip's uv 0x800B2F44, its CLUTs
    // shown / trail / empty 0x800B2EF4 / 0x800B2F04 / 0x800B2F00, the page the lui at 0x800B2F08); the progress map
    // (0x800B34E0: boxes from x 0x800B3584 every 0x800B3624, y 0x800B355C, 0x800B358C high, the final box
    // 0x800B36CC wide, the lines from x 0x800B3718, y 0x800B373C to 0x800B3744; the stages' colours 0x800B6518); the
    // keys (0x800B42CC: w, h 0x800B42E0 / 0x800B42E8, VRAM and CLUT x per key 0x800B4344.. / 0x800B4350.., CLUT y
    // 0x800B4370, x every 0x800B43C0, y 0x800B43CC); the tally's lines (0x800B19D8: x 0x800B1A64, y from 0x800B19E0
    // every 0x800B1A84, font and colour 0x800B1A3C.. per line; the key's and the final challenge's x 0x800B1DCC /
    // 0x800B1E40, blinking to the colour of 0x800B1DB8); NOW LOADING... (0x800B166C) and STAGE CLEAR!! (EXE 0x8003D468); the name plates (0x800B32AC: the
    // player's, then the soldiers' right-aligned, x and y every 8 bytes from 0x800B0A00); the boss's bonus time
    // (0x800B17FC: x - 0x800B1834, colour 0x800B1800); +N SEC over a soldier (0x800B2394: from x - 0x800B2350, "+" (font
    // 0x800B2398, colour 0x800B239C) y + 0x800B23C8, the number in font 0x800B23DC, then SEC (font 0x800B23F4, colour
    // 0x800B23F8) y + 0x800B23FC) and +LIFE! (x - 0x800B4150, colours 0x800B417C / 0x800B4180)
    let (bar_page, _) = tpage_xy(r7.u16(0x800B2F08)?);
    let line = |font: i64, colour: i64| -> Result<Value> { Ok(json!([i7(font)?, i7(colour)?])) };
    let draw = json!({
        "bars": {"right": i7(0x800B2CE8)?, "page": bar_page,
                 "caps": {"w": i7(0x800B2C6C)?, "h": r7.u16(0x800B2C48)?, "uv": [uv7(0x800B2E28)?, uv7(0x800B2E60)?],
                          "clut": clut_xy(r7.u16(0x800B2E24)?)},
                 "strip": {"uv": uv7(0x800B2F44)?,
                           "cluts": [clut_xy(i7(0x800B2EF4)?), clut_xy(i7(0x800B2F04)?), clut_xy(i7(0x800B2F00)?)]}},
        "map": {"routine": "0x800B34E0", "x": i7(0x800B3584)?, "dx": i7(0x800B3624)?, "y": i7(0x800B355C)?,
                "h": i7(0x800B358C)?, "final_w": i7(0x800B36CC)?, "lines_x": i7(0x800B3718)?,
                "lines_y": [i7(0x800B373C)?, i7(0x800B3744)?],
                "colours": (0..4).map(|k| Ok(json!(bytes_list(r7.bytes(0x800B6518 + 4 * k, 3))))).collect::<Result<Vec<_>>>()?},
        "keys": {"routine": "0x800B42CC", "w": i7(0x800B42E0)?, "h": i7(0x800B42E8)?,
                 "vram": [[i7(0x800B4344)?, i7(0x800B4348)?], [i7(0x800B4354)?, i7(0x800B4358)?], [i7(0x800B4364)?, i7(0x800B4368)?]],
                 "clut_x": [i7(0x800B4350)?, i7(0x800B4360)?, i7(0x800B436C)?], "clut_y": i7(0x800B4370)?,
                 "dx": i7(0x800B43C0)?, "y": i7(0x800B43CC)?},
        "tally": {"routine": "0x800B19D8", "x": i7(0x800B1A64)?, "y": i7(0x800B19E0)?, "dy": i7(0x800B1A84)?,
                  "lines": [line(0x800B1A3C, 0x800B1A40)?, line(0x800B1ABC, 0x800B1ABC)?, line(0x800B1B58, 0x800B1B5C)?,
                            line(0x800B1D40, 0x800B1D44)?],
                  "key_x": i7(0x800B1DCC)?, "final_x": i7(0x800B1E40)?, "blink": i7(0x800B1DB8)?},
        "now_loading": {"font": i7(0x800B1674)?, "colour": i7(0x800B1678)?, "x": i7(0x800B167C)?, "y": i7(0x800B1680)?},
        "stage_clear": printf_call(exe, 0x8003D468)?,
        "plates": (0..3).map(|k| r7.s16s(0x800B0A00 + 8 * k, 2)).collect::<Result<Vec<_>>>()?,
        "bonus_time": {"dx": i7(0x800B1834)?, "colour": i7(0x800B1800)?},
        "popup": {"dx": i7(0x800B2350)?, "plus": [i7(0x800B2398)?, i7(0x800B239C)?], "plus_dy": i7(0x800B23C8)?,
                  "number_font": i7(0x800B23DC)?, "sec": [i7(0x800B23F4)?, i7(0x800B23F8)?], "sec_dy": i7(0x800B23FC)?},
        "life": {"dx": i7(0x800B4150)?, "colours": [i7(0x800B417C)?, i7(0x800B4180)?]},
        // (a roast chicken lying at y 0x800B3DE8, drawn (0x800B3E90) w = the lui / ori at 0x800B3FA4 over its depth
        // wide and the one at 0x800B3FB4 over it high)
        "chicken": {"y": i7(0x800B3DE8)?, "w": (r7.u16(0x800B3FA4)? << 16) | i7(0x800B3FA8)?,
                    "h": (r7.u16(0x800B3FB4)? << 16) | i7(0x800B3FB8)?}});
    Ok(json!({
        "note": "game units; battle type 8; stage index i = 0..3 (records 51..54, stage ids 15..18), 4 = the final stage",
        "draw": draw,
        "stages": stages,
        "stream": {"routine": "0x800B1FC4", "entries": stream, "end": 0x63, "spawn_dx": 5000,
                   "note": "p0 of a stream event indexes this table by entries (kind, class); it ends at kind 0x63"},
        "soldier_costumes": costumes,
        "soldier_selection_base": 84,
        "health": health,
        "classes": classes,
        "boss_target_frames": r7.u32s(0x800B06F4, 5)?,
        "stage_bonus": r7.u32s(0x800B0708, 5)?,
        // (a selection past the table, the port's fighters brought over: the port's own choice, the arcade's
        // stage-9 boss (record 5's 0x800B1FE8) in its first costume)
        "boss_routes": {"routine": "0x800B2658", "by_selection": routes, "default": [imm(&overlay(rec, 5, 0x800B0548)?, 0x800B1FE8)?, 0],
                        "note": "[character, costume] per stage; costume -1: (player character == boss) ? (sel & 3) == 0 : (sel & 3) != 0"},
        "target_priority": r7.u32s(0x800B0A38, 4)?,
        "hud": {"routine": "0x800B2C34 (bars: x, y at 0x800B64F4, 16 apart)",
                "bars": (0..3).map(|k| r7.s16s(0x800B64F4 + 16 * k, 2)).collect::<Result<Vec<_>>>()?,
                "stage_clear": strip_fmt(&exe.cstr(0x8001A33C), "cfHV"), "key": s7(0x800B0938),
                "final_challenge": s7(0x800B0950), "texts": "at 0x8003D46C, 0x800B1DC4, 0x800B1E38",
                "now_loading": s7(0x800B088C), "stage_bonus": s7(0x800B08B8), "clear_bonus": s7(0x800B08D4),
                "clear_time": s7(0x800B08F0), "bonus_time": s7(0x800B0914), "sec": s7(0x800B0984),
                // (" +LIFE!": its place is the port's)
                "life": s7(0x800B0A28).trim_start(),
                "score": strip_fmt(&exe.cstr(0x8002203C), "cfHV"), "hi_score": strip_fmt(&exe.cstr(0x80022050), "cfHV")},
        "constants": {
            "ai_level": {"routine": "0x800B5C2C", "by_stage": ai_level},
            "clock": 3600, "clock_cap": 5940, "max_health": 130,
            "final": {"character": final_character, "health": final_health, "bgm": final_bgm},
            "chicken": {"routine": "0x800B3E44 / 0x800B3E90", "heal": 50, "reach": 500, "y": 1056, "despawn_behind": 5000,
                        "max": 2, "sfx": imm(&r7, 0x800B40C8)?, "voice": imm(&r7, 0x800B41E0)?},
            "countdown": {"routine": "0x800B2590", "frames": countdown.iter().map(|c| c[0]).collect::<Vec<_>>(),
                          "codes": countdown.iter().map(|c| c[1]).collect::<Vec<_>>()},
            "camera": {"routine": "0x800B4F78 / 0x800B5020", "eye_y": -3500, "eye_z": -7000, "pitch_min": 0x2AAA,
                       "pitch_max": 0x10000, "h": 500, "roll_stage3": 0x82, "arena": 5000, "margins": [-100, 60]},
            // (the arena compared at 0x8004328C has its front slanted, 0x800B2BB8; the players' x by the frustum: a
            // line in z per edge, [left slope, left, right slope, right], the slanted arena's second)
            "bounds": {"routine": "0x80043240", "back_z": -imm(exe, 0x80043278)?, "front_z": -imm(exe, 0x80043294)?,
                       "slanted_stage": imm(exe, 0x8004328C)?, "front_slanted": [-0.259, -2285],
                       "human_x": [[-323, -2553, 329, 1971], [-375, -2445, 333, 2175]], "cpu_x": imm(exe, 0x8004331C)?},
            "target": {"routine": "0x800B54A8", "far": 2500, "close": 400, "cooldown": 120},
            "score": {"per_damage": 10, "per_throw_damage": 20, "cap": 99999990, "clear_per_health": 100},
            "tally": {"stage_bonus_at": 30, "clear_bonus_at": 60, "time_at": 90, "done_at": 120, "sfx": imm(&r7, 0x800B4550)?,
                      "key_sfx": imm(&r7, 0x800B4558)?},
        },
    }))
}

/// DualShock vibration (0x80028EC0 request, 0x800291D8 60 Hz tick, 0x8002A128 output): 28 patterns
/// (0x80095334: small motor script, large motor script), each a priority slot 0..3 (0x80010050). A script is
/// u16 steps: low byte frames (0 = 1), bits 8..11 level L, high nibble 1 = another step follows; a script whose
/// first word has a zero high nibble is not installed (the slot's running motor is left alone: null here).
/// Per pad and motor, a request replaces its slot's channel; every frame each channel steps on and the highest
/// active slot gives the motor: large = L * 16 + 15, small = bit (tick & 7) of small_masks[L] (0x80095414).
pub fn rumble(exe: &Exe) -> Result<Value> {
    let script = |mut p: i64| -> Result<Value> {
        if exe.u16(p)? & 0xF000 == 0 {
            return Ok(Value::Null);
        }
        let mut steps = Vec::new();
        loop {
            let w = exe.u16(p)?;
            steps.push([(w & 0xFF).max(1), (w >> 8) & 15]);
            if w & 0xF000 != 0x1000 {
                return Ok(json!(steps));
            }
            p += 2;
        }
    };
    let mut patterns = Vec::new();
    for i in 0..28 {
        let sl = exe.u32s(0x80095334 + 8 * i, 2)?;
        patterns
            .push(json!({"id": i, "slot": exe.u8(0x80010050 + i)?, "small": script(sl[0])?, "large": script(sl[1])?}));
    }
    // the patterns the fight's events request (0x800759D4: a1 at each request): on the fighter's pad (blocked, a clean
    // hit below 21 damage (+0xE8) or heavy, close, counter, thrown, the floor, the K.O., a ki charge), on the other's
    // (his ki, his block, hits, close, counter, throw, the K.O.), and the moves' effects (0x80075E44's cases: a fire
    // hit on both, a bone effect on both, a ground impact, the breath, the fire of the key configuration's test)
    let im = |a: i64| imm(exe, a);
    // the fire moves' descriptor bytes (0x8006F120: a switch on byte 0 - first (0x8006F128) over its cases
    // (0x8006F12C, jump table 0x800251AC)): the breath's case calls 0x8006E99C (kind 3), the other fire's 0x8006EB28
    // (kind 4); the AI's fire rules compare the same bytes
    let first = -im(0x8006F128)?;
    let mut by_case = BTreeMap::new();
    for k in 0..im(0x8006F12C)? {
        by_case.insert(exe.u32(0x800251AC + 4 * k)?, first + k);
    }
    let byte_of = |case: i64| by_case.get(&case).copied().with_context(|| format!("no fire case 0x{case:X}"));
    let (breath, other) = (byte_of(0x8006F180)?, byte_of(0x8006F198)?);
    if (breath, other) != (im(0x80059A70)?, im(0x80059AAC)?) {
        bail!("the rumble's fire descriptors ({breath}, {other}) are not the AI's");
    }
    let fire = json!({"routine": "0x8006F120", "breath": breath, "test": other});
    let requests = json!({
        "blocked": im(0x80075A84)?, "hit": im(0x80075ACC)?, "heavy": im(0x80075AEC)?, "close": im(0x80075B1C)?,
        "counter": im(0x80075B4C)?, "thrown": im(0x80075B7C)?, "floor": im(0x80075BB4)?, "ko": im(0x80075BCC)?,
        "ki": im(0x80075C1C)?, "ki_other": im(0x80075C8C)?, "block_attacker": im(0x80075CA8)?, "hit_attacker": im(0x80075CC4)?,
        "heavy_attacker": im(0x80075CE0)?, "close_attacker": im(0x80075CFC)?, "counter_attacker": im(0x80075D18)?,
        "thrower": im(0x80075D34)?, "ko_other": im(0x80075D50)?, "fire_hit": im(0x80075E74)?, "fire_attacker": im(0x80075E84)?,
        "bone_own": im(0x80075E90)?, "bone_other": im(0x80075EA0)?, "ground": im(0x80075EAC)?, "breath": im(0x80075EB8)?,
        "test": im(0x80075EC4)?, "fire_descriptors": fire});
    Ok(json!({"routine": "0x80028EC0 (request: pad, pattern; pad >= 2 both), 0x800291D8 (tick), 0x8002A128 (output)",
              "small_masks": bytes_list(exe.bytes(0x80095414, 16)),
              "step": "[frames, level]", "patterns": patterns, "requests": requests}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_as_python() {
        assert_eq!(strip_fmt("%c%H%VTEAM %%c 100%", "cfHV"), "TEAM % 100%");
        assert_eq!(strip_fmt("%h%vA", "cfHVhv"), "A");
        assert_eq!(*pyget(&[1, 2, 3], -1).unwrap_or(&0), 3);
        assert!(pyget(&[1, 2, 3], 3).is_err());
    }

    #[test]
    fn immediates() {
        // addiu a1, zero, 6; addiu v0, s0, -14; addu a2, zero, zero; andi v0, v0, 0x8000; ori v0, zero, 0xFFFF;
        // slti v0, a0, -1; then not constants: lui v0, 0x8000; addu v0, a0, zero
        let words = [0x24050006u32, 0x2602FFF2, 0x00003021, 0x30428000, 0x3402FFFF, 0x2882FFFF, 0x3C028000, 0x00801021];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let e = Exe::with_base(&code, 0x80000000);
        let got: Vec<i64> = (0..8).map(|k| imm(&e, 0x80000000 + 4 * k).unwrap_or(-99)).collect();
        assert_eq!(got, [6, -14, 0, 0x8000, 0xFFFF, -1, -99, -99]);
    }

    #[test]
    fn sprite_arguments_from_the_code() {
        // a2 = 5, a3 = 7, v0 = 128 to sp+16 (w) and sp+36 (mode: 8-bit), jal 0x8004DB9C with sp+20 (h) = 0 in the
        // delay slot
        let code: Vec<u8> = [0x24060005u32, 0x24070007, 0x24020080, 0xAFA20010, 0xAFA20024, 0x0C0136E7, 0xAFA00014]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let got = sprites(&Exe::with_base(&code, 0x80000000), 0x80000000, 1).unwrap_or_default();
        assert_eq!(got, [json!({"x": 5, "y": 7, "w": 128, "h": 0, "vram": [0, 0], "bpp": 8, "clut": [0, 0]})]);
        assert!(sprites(&Exe::with_base(&code, 0x80000000), 0x80000000, 2).is_err(), "past the code");
    }

    #[test]
    fn printf_arguments_from_the_code() {
        // a0 = the format at 0x80000040, a1 = 2 (%c), a2 = 0 (%f), a3 = 117 (%H), sp+16 = 240 (%V), sp+20 = 6 (%c)
        let words = [0x3C048000u32, 0x24840040, 0x24050002, 0x24060000, 0x24070075, 0x240200F0, 0xAFA20010, 0x24020006];
        let mut code: Vec<u8> = words.iter().chain(&[0x0C01339D, 0xAFA20014]).flat_map(|w| w.to_le_bytes()).collect();
        code.resize(0x40, 0);
        code.extend(b"%c%f%H%VA %cB\0");
        let got = printf_call(&Exe::with_base(&code, 0x80000000), 0x80000000).unwrap_or_default();
        let want =
            json!({"text": "A B", "font": 0, "colour": 2, "x": 117, "y": 240, "parts": ["A ", "B"], "colours": [2, 6]});
        assert_eq!(got, want);
    }

    #[test]
    fn printed_pieces_keep_their_places_and_conversions() {
        // "%c%f%V%H%02d%H'" with colour 6, font 1, y 36, x 34, a number, x 57: two pieces
        let mut args = [0i64; 16];
        args[1..7].copy_from_slice(&[6, 1, 36, 34, 5, 57]);
        let got = printf_pieces(Some("%c%f%V%H%02d%H'"), &args).unwrap_or_default();
        let want = [
            json!({"text": "%02d", "font": 1, "colour": 6, "x": 34, "y": 36}),
            json!({"text": "'", "font": 1, "colour": 6, "x": 57, "y": 36}),
        ];
        assert_eq!(got, want);
        assert!(printf_pieces(Some("%c%H"), &[0; 16]).is_ok_and(|p| p.is_empty()), "codes alone print nothing");
        for bad in ["%h", "%u", "%9d", "%12d", "%-2d", "%2c", "A%"] {
            assert!(printf_pieces(Some(bad), &[0; 16]).is_err(), "{bad} is refused");
        }
    }

    /// T3_BIN: the rules of particular moves, move sets and sounds read from the code are the values it compares.
    #[test]
    fn rules_read_from_the_code() -> Result<()> {
        let Ok(bin) = std::env::var("T3_BIN") else { return Ok(()) };
        let game = crate::disc::Game::load(&conv_io::StdDisc(bin.into()))?;
        let (exe, rec) = (Exe::new(&game.exe), &game.rec);
        let (r, a, b, f) = (rules(&exe)?, ai(&exe, rec)?, ball(&exe, rec)?, force(&exe, rec)?);
        assert_eq!(r["moves"]["long_hitstop"]["alias"], 0x898);
        assert_eq!(
            r["moves"]["air_spikes"]["moves"][1],
            json!({"moveset": 16, "alias": 0x14A, "unless_queued": [0x848, 0x849]})
        );
        let pushes = json!([{"aliases": [0x18, 0x1A], "push": 30, "frames": [3, 9], "cancel": 0},
                            {"aliases": [0x1E, 0x20], "push": 150, "frames": [5, 12], "cancel": 18},
                            {"aliases": [0x1F, 0x21], "push": 0, "frames": [0, 0], "cancel": 18}]);
        assert_eq!(b["constants"]["moves"]["push_back"], pushes);
        assert_eq!(a["handlers"]["routines"]["0x80061B88"]["aliases"]["spin"], json!([0x459, 0x45A, 0x45B]));
        assert_eq!(a["rules"]["late_parry"]["until"], json!([22, 26]));
        assert_eq!(f["constants"]["ai_level"]["by_stage"], json!([0, 3, 4, 5, 5]));
        assert_eq!(f["constants"]["final"]["health"], 250);
        assert_eq!(f["constants"]["countdown"]["codes"], json!([0x8694, 0x8693, 0x8652, 0x8711, 0x86D0]));
        let s = sound(&exe)?;
        assert_eq!(s["impact_rules"]["arms"], json!({"light": [0x7041, 0x7064], "tiers": [0x7042, 0x7043, 0x7044]}));
        assert_eq!(s["announcer"]["continue"]["count"], 4);
        assert_eq!(
            rumble(&exe)?["requests"]["fire_descriptors"],
            json!({"routine": "0x8006F120", "breath": 0x18, "test": 0x1A})
        );
        assert_eq!(screens(&exe, rec)?["select"]["faces"]["question"], json!([22, 23]));
        Ok(())
    }

    /// T3_BIN: the screens' places, sizes and pictures read from the code (the values the port drew before them).
    #[test]
    fn screen_places_read_from_the_code() -> Result<()> {
        let Ok(bin) = std::env::var("T3_BIN") else { return Ok(()) };
        let game = crate::disc::Game::load(&conv_io::StdDisc(bin.into()))?;
        let (exe, rec) = (Exe::new(&game.exe), &game.rec);
        let s = screens(&exe, rec)?;
        let slash = &s["select"]["faces"]["slash"];
        assert_eq!((&slash["page"], &slash["clut"]), (&json!([960, 256]), &json!([464, 508])));
        assert_eq!(slash["sprites"][1], json!({"u": 32, "v": 224, "w": 32, "h": 32, "dy": 27}));
        assert_eq!(s["select"]["pieces"]["arrows"]["x"], json!([12, 346]));
        assert_eq!(s["hud"]["team"]["x"], json!([156, 202]));
        assert_eq!(s["grid"]["vs"]["gauge"]["draw"]["arrow"]["dx"], json!([-95, 87]));
        assert_eq!(s["modes"]["results"]["time_attack"]["total"]["face"], json!([289, 368]));
        assert_eq!(s["modes"]["results"]["force"]["layout"]["boss_faces"][3], json!([160, 380]));
        assert_eq!(s["key_config"]["draw"]["rows"]["y"], json!([340, 366, 392, 418]));
        assert_eq!(s["menu"]["bar"]["lines"], json!([[360, 362], [386, 388]]));
        let f = force(&exe, rec)?;
        assert_eq!(f["draw"]["keys"]["vram"][2], json!([688, 392]));
        assert_eq!(f["draw"]["map"]["colours"][3], json!([128, 16, 0]));
        Ok(())
    }
}
