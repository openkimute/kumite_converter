# Tekken 3 (SLUS-00402): timed properties, cancel conditions, moveset header gaps

> Detailed research spec, part of [engine.md](engine.md). Derived from the EXE and disc data only.
> Helper scripts named below were research tools and are not in the repository, except the reference
> implementations `t3_converter/src/reference/attackpath.rs` and `reference/camera` (they read the disc image).

This file fills gaps in [engine.md §7](engine.md#7-combat-system). All findings come from reading the EXE's
machine code (`(research)/exe.asm`) and the moveset data (BNS 279 common, 280–300 packs; 295 is empty, so there are 20 packs plus common = 21).
No third-party code was used.

Confidence tags:
- **[V]**: read in code and checked against data.
- **[C]**: read in code, semantics inferred from the fields it touches.
- **[G]**: a guess.

Helpers are in `(research)/gap2/`:

| Script | Purpose |
|---|---|
| `ms` | Pack loader: alias resolution, cancel entries, command text. Common pack 279 has no alias table, so its dests resolve in Paul's context. |
| `props` | Timed-property op totals, plus a per-pack matrix |
| `plist pack O<i>\|C<i>\|alias…` | Dumps one record's property list and SFX list |
| `cstats` | Statistics for the cancel-entry fields |
| `bycond pack <cond>\|r=<req>\|t=<tr>` | Lists cancel entries by `cond`, `req` or `tr` byte |
| `sfx` | Voice/SFX list statistics |
| `jt addr n` | Groups a jump table by target |
| `xref [-s] off…` | Loads/stores of actor offsets |
| `bituse off` | `andi` masks applied to a field |
| `shared_lists.txt` | Full dump of the 672 shared EXE cancel entries (`0x80017C78`) |
| `cond_e0bc.asm`, `cond_e8d0.asm` | The two condition evaluators |

---

## 1. Timed-property lists (header +0x14, record +0x20)

### 1.1 Format and dispatch [V]

The list is a series of `u16 frame, u16 word` pairs, terminated by `frame == 0`.

- **`op = word >> 8`, `arg = word & 0xFF`.** The opcode is the high byte; the earlier notes treated the whole
  `u16` as the op.
- **Dispatcher `0x800458C8(actor)`** is called once per frame per actor from the fight loop (`0x8002B430/38/50`)
  and only while `actor+0xC3` (active) is set.
  - It first clears `actor+0xE4`.
  - An entry fires when **`prev_frame(+0x5A) < frame ≤ cur_frame(+0x58)`**, so each entry fires once.
- **Jump tables:**
  - ops 6..10 use `0x8001DF08` (5 entries);
  - ops 1..13 use `0x8001DF20` (13 entries);
  - ops 0x0E..0x3D take a computed path.
  - Any other op is ignored.
- **Other readers of the same lists:**
  - `0x8002D2C8(record)` returns `record+0x14 + Σ arg(op 0x0B)`. The relocator `0x8002CE14` (at `0x8002CF84`) stores
    this back into `+0x14`, so on-disc damage plus the 0x0B args equals the in-RAM damage.
  - The throw camera `0x80067530` scans the attacker's list for the first op 0x0B and the victim's list for op
    0x0C. It uses `frame − cur_frame` to time the camera cut.

### 1.2 Opcode table

`bone` means a skeleton slot: the bone world position is read from `actor+0x908 + 0x44·arg`.

| Op | Count | Packs | Handler / effect | Arg |
|---|---|---|---|---|
| 0x01 | 17 | 9 | `0x8004ACFC(0)`: **screen shake, small**. Sets the signed-byte script pointer `0x80097AA0 = 0x80097AA4[0]` (scripts at `0x8001E848…`, run by `0x8004AD34`, added to the camera at `0x800A85C0`). Also requests rumble pattern 1 on both pads (`0x80075D90` → `0x80028EC0`), whose two scripts are empty: no vibration. [C] | unused (0) |
| 0x02 | 229 | 20 | Same, **medium** shake (script 1), rumble pattern 2 [V] | 0 |
| 0x03 | 46 | 13 | Same, **large** shake (script 2), rumble pattern 3 [V] | 0 |
| 0x04 | 589 | 20 | `0x8004AC84(actor+0xF68)`: **ground-impact effect at the root position**. It runs `0x8004A990` (3-slot ring at `0x8009E850`, positions ×128) and `0x8004A5C8`, then rumbles the own pad (`0x80075DEC(player,2)`: pattern 0x19). Used by falls, slams and throws. [C] | 0 (1 entry 17) |
| 0x05 | 57 | 19 | **No handler**: the jump table sends it to the skip label. Often on the same frame as op 6. [V: ignored; G: removed effect] | 0 |
| 0x06 | 440 | 19 | **Effect at bone, owned by own pad slot (`+0x1E`)**: `0x80076B70(slot,&pos,0)` → `0x80076884(slot,0,pos)` (3-slot spark ring `0x800A3408`) and `0x8004A5A8`, plus rumble: own pad pattern 0x0F, the other's 0x1B. Example: Paul's Burning Fist O119 fires frames 7/12/17 on bones 4/5/6. [C] | bone slot |
| 0x07 | 21 | 3 | Same effect, owned by `+0x1F` (the other slot), no rumble [C] | bone slot |
| 0x08 | 615 | 20 | `0x80076BD0(0,&pos)`: neutral-owner effect, kind 0, at bone [C] | bone slot |
| 0x09 | 3 | 1 | Same, kind 1 [C] | bone slot |
| 0x0A | 349 | 18 | Same, kind 3 [C] | bone slot |
| 0x0B | 526 | 20 | **Damage event to the opponent**: `actor+0xE4 = 1`, `+0xEC = arg` (see §1.3) [V] | damage; 0 = the move's own damage |
| 0x0C | 96 | 10 | **Damage event to self**: `+0xE4 = 2`. Examples: Paul O182 (fall), common O320/321 arg 17 [V] | damage; 0 = own move damage |
| 0x0D | 73 | 5 | **Heal self**: `+0xE4 = 3`, the amount is subtracted from incoming damage. Examples: Yoshimitsu pack 284, O15/16/17 heal 10/15/22. [V] | HP |
| 0x0E..0x3D | – | – | **Hand pose**. Sets `actor+0xBC = 1` and calls `0x800344A4(actor, mask, kind, arg)`; details below. [C] | blend time |

**Hand-pose ops (0x0E..0x3D)** [C]. Let `v = op − 0x0E`.

- **`mask = v>>4`**, where 0 means 3 (both hands), 1 means hand A, 2 means hand B. Hand A/B correspond to `+0x127C/+0x127E`;
  which one is left is unknown.
- **`kind = v & 7`**, with two exceptions:
  - op 0x14 forces kind 0;
  - op 0x15 takes kind from the per-model table `0x80095950[model]` (values 0/1/2), i.e. "return to the model's default hand".
- **Target values (`+0x1280[hand]`):** kind 0 → 0 (open), kind 1 → 0x200 (closed fist), kind k ≥ 2 → 0x1FF+k
  (special shapes).
- **Blending:** `speed = 0x200/(arg+1)` per frame (`+0x1284`). It snaps instead of blending when the current or
  target value is above 0x200.
- **Rendering:** `0x80034970` blends `+0x127C`. The renderer swaps the hand draw entries 16/17 (and 12).
- **True Ogre** (`actor+0x18 == 0x14`) uses `+0x1296..+0x129A` instead [G: wings or extra parts].

Counts:

| Op | Count | Meaning |
|---|---|---|
| 0x0E | 245 | both hands open |
| 0x0F | 333 | both hands fist |
| 0x10 | 48 | both hands, shape 0x201 |
| 0x11 | 1 | both hands, shape 0x202 |
| 0x12 | 6 | both hands, shape 0x203 |
| 0x13 | 1 | both hands, shape 0x204 |
| 0x15 | 382 | both hands to model default |
| 0x1E | 19 | hand A open |
| 0x1F | 5 | hand A fist |
| 0x2E | 11 | hand B open |
| 0x2F | 3 | hand B fist |
| 0x30 | 5 | hand B, shape 0x201 |
| 0x35 | 6 | hand B, shape 0x206 |

Example: Paul throw O240 has open at frame 4, fist at frame 14 (the grab), and back to default at frame 20. Jumps O22
have fist at frame 9 and default at frame 38.

Total: **4126 entries**. The per-pack matrix is printed by `props`. The only op 0x09 user is pack 294 (Ogre). Ops
0x11 and 0x13 occur once each.

### 1.3 How the damage events are applied (`0x800449B4`, called from the per-actor hit pass `0x80044400`) [V]

Each frame the actor's health change is the normal hit damage plus a scripted extra. The amount of a scripted
event is its argument (`+0xEC`), or the current move's damage (`+0x5E`) when the argument is 0.
- Own event 2 (op 0x0C) adds its amount; own event 3 (op 0x0D) subtracts it (a heal).
- While the actor is linked to an opponent (`+0x74 < 0`, a throw or hold; partner from `0x80045C18`), the
  partner's event 1 (op 0x0B) adds the partner's amount.
- The total, in 16.16, is taken off health (`+0x3F4`) and clamped to 0…max. Nothing is applied while either
  global `0x8009547C` or `0x800954B4` is set.

**Throws work as follows** (Paul as the example):

1. The throw attempt (O240, damage type 1, so no direct damage) has an op-0x0F grab pose.
2. A cancel entry with `cond` 2 (§2.3) branches to the throw-success move O326 (mask 0x800, damage 30, camera 0x53).
3. O326 carries **op 0x0B, arg 0, at frame 75**, which applies its own 30 damage to the linked victim.

Multi-hit throws use several 0x0B entries with explicit args.

---

## 2. Cancel entries (header +0x04)

### 2.1 Layout and walkers [V]

```
+0 u16 cmd | +2 u8 req | +3 u8 cond | +4 u16 param | +6 u16 dest_alias | +8 u8 trans | +9 u8 win_start | +A u8 win_end | +B u8 exec
```

**Normal walker `0x8002DE24(list, frame, actor, opp)`** runs each frame from `0x8002DA3C`. A second pass at
`0x8002DC38` walks the *next* move's list on frame 1. For each entry it checks, in order:

1. `0x8002CC28` (input match);
2. `win_start ≤ frame ≤ win_end`;
3. `req`;
4. `cond`. Entries with **`cond ≥ 0x45` are skipped**; `cond 0` always passes; other values go to `0x8002E0BC(entry, actor, opp)`.

`0xC00C` jumps to `0x80017C78 + dest·12`, and `0xC00D` returns. Both nest one level.

**Reversal walker `0x8002E708`**, from `0x8002D8B4`:
- It runs only on the frame the actor is struck (`actor+0xCE`). Gon (`char 0x11`) is excluded.
- It walks the *current* move's list and accepts **only** entries with `cond ≥ 0x45`, evaluated by `0x8002E8D0`.
- On a match it cancels the hit: it clears `+0xCE/+0xCF/+0xD0` and the attacker's `+0xD1`, and queues `dest`.
- This is the parry/reversal mechanism.

**On a match** (`0x8002DA58`) [V]:

| Field | Effect |
|---|---|
| `dest` | Becomes `+0x1A0` (alias) and `+0x1A4` (record) |
| `win_end` | `+0x6C = win_end − frame` |
| `trans & 0x3F` | Passed through `0x80030F68`; the result is stored in `+0x19A`, the **transition type** (§2.5) |
| `trans` bit 7 | `+0x19C`: **turn 180°**. At commit, `+0x2C` and `+0x0E` get += 0x8000 |
| `trans` bit 6 | `+0x19E` [G: suppresses a blend or effect; read at `0x8003C9E4`] |
| `exec` | Stored in **`+0x198`**, the frame of the *current* move at which the queued move is committed (`0x8002F414`: commit when `frame ≥ +0x198`, or at animation end). **255 means "at the end of the animation"** (it is replaced by `record+0x18`). When playback runs backwards (`+0x6E < 0`), `win_end` is used instead. For transition type 0x1E the value is instead the start frame of the new move (`0x800305C4`). |

Example: Paul's jab O104 (active frame 8) has 1→2 accepted on frames 1–12 with `exec` 8. Its END entry is
`win 25-255, exec 25`, so recovery is cancellable from frame 25. [V]

Special `trans` handling in the walker:
- `0x2C` means "use this list's END entry" (it skips to `0xC000`);
- `0x1C` is rejected while `+0x128` is set;
- in Tekken Ball mode (8), `0x21` is rejected unless the opponent holds the ball (`+0xC4`).

### 2.2 `req` byte (character/moveset gate) [V]

The same code exists in both walkers (`0x8002DF00`, `0x8002E7D8`). `ms` is `actor+0x16` (moveset id) and `ch` is
`actor+0x18` (character id).

| req | Passes when | Example use |
|---|---|---|
| 0 | always | – |
| 0x01..0x17 | `ms == req−1` | Character-specific command throws inside shared stance lists. Paul's pack has `req=1` on SEQ n,f,n,f+1+2 → O243; King's pack has `req=4` on f,n,b,d/b,d,f+1 → O280 |
| 0x18..0x2E | `ms != req−0x18` | Exclusions; e.g. 29/30 means "not Nina/Hwoarang" |
| 0x2F..0x45 | `opp.ms == req−0x2F` | Victim-specific variants. Pack 293 uses 47/49/56/58/60 (opponent Paul, Lei, Jin, Julia, Heihachi) |
| 0x46..0x5C | `ch == req−0x46` | `90 (0x5A)` = True Ogre only; `84` = Ogre only |
| ≥ 0x5D | `ch != req−0x5D` | `107 (0x6B)` = not Ogre (14). The shared EXE lists use it 28 times |

Ogre and True Ogre share moveset 14, so the character gates separate them.

### 2.3 `cond` codes, normal walker (`0x8002E0BC`, jump table `0x80019BF8`, 69 entries) [V]

In this table, `self` is the walking actor and `opp` is the opponent.

**Geometry** (computed each frame by `0x80043A94`) [V]:

| Field | Meaning |
|---|---|
| `+0x2A` | world angle from self to the opponent |
| `+0x2C` | self's facing |
| `+0x2E` | `facing − dir_to_opp` (s16); `+0x30` = its absolute value |
| `+0x32` | self's quadrant: 0 front (\|a\| < 45°), 1 side (+45..135°), 3 back, 2 side (−) |
| `+0x40` | the **opponent's** quadrant relative to self (0 = opponent faces self … 3 = opponent's back turned to self) |
| `+0xF8` | distance between the pair (table `0x8009E660`) |
| `+0xF4` | `+0xF8` minus 40 for each of the two who is Kuma (`ch` 11) or True Ogre (`ch` 20) |

| cond | Test | Count | Meaning / typical use |
|---|---|---|---|
| 0x01 | `self+0xD1` | 0 | own attack made contact this move |
| 0x02..0x13 | `throwcode(self) == cond && self+0xF4 ≤ param` | 524 | **Throw connect**; see the throw-code table below. `param` = range (1536 typical, 640 for codes 0x12/0x13, 1792). Paul O240 → `cond 2 par 1536 win 12-14` → O326 |
| 0x14 | throwcode ∈ {2,6} | 23 | Front throw on standing or crouching opponent, `param 1408` |
| 0x15 | ∈ {3,7} | 21 | Side-2 throw |
| 0x16 | ∈ {4,8} | 18 | Side-1 throw |
| 0x17 | ∈ {5,9} | 18 | Back throw (standing or crouching) |
| 0x18 | `self+0xF8 ≤ param` | 6 | Opponent within distance |
| 0x19 | `self+0xF8 ≥ param` | 200 | Opponent farther than distance (e.g. backdash C002 `par 1800`; `par 3800`) |
| 0x1A | `self+0xD1 && !(self.attack_mask & opp.guard_mask)` | 125 | **Own hit landed (not blocked)**: hit-confirm follow-ups |
| 0x1B | `opp+0xCF` | 73 | Opponent is blocking / blocked |
| 0x1C | `self+0xD2` | 31 | Own attack was blocked [G] |
| 0x1D | `opp+0x19A != 0x21` | 2 | Opponent is not in a throw-link transition |
| 0x1E | `opp+0x5E != 0` | 0 | Opponent is in an attack |
| 0x1F | opponent attacking and `opp.posture&7 == 2` | 0 | …while standing |
| 0x20 | opponent attacking and `opp.posture&7 == 1` | 0 | …while crouching |
| 0x21 | `opp+0x5E == 0` | 0 | Opponent not attacking |
| 0x22 | `opp.posture & 0x407 == 2` | 21 | Opponent standing (grounded) |
| 0x23 | `opp.posture & 0x407 == 1` | 2 | Opponent crouching |
| **0x24** | `self+0x30 > 0x4000` | 1810 | **Self's back is turned to the opponent.** Used in jump and turning lists with `trans` bit 7 (turn-around attacks). The old note "crouching" was wrong. |
| **0x25** | `+0x2E ∈ [0x4E38, 0x8000)` | 552 | **Back turned, opponent on side +** (110–180°) |
| **0x26** | `+0x2E ∈ [0x8000, 0xB1C6]` | 554 | **Back turned, opponent on side −**. With 0x25 this picks left/right turn-around moves (walk C22 vs C24, punch O107 vs O108). The old note "while rising" was wrong: O107/O108 are back-turned turn-punches (`trans 0x8E`). |
| 0x27..0x2A | `self+0x32 == 0 / 1 / 3 / 2` | 838 / 12 / 12 / 26 | Self faces front / side+ / away / side− |
| 0x2B..0x2E | `self+0x40 == 0 / 1 / 3 / 2` | 1 / 1 / 1 / 3 | Opponent faces self / side / back / side (e.g. King `b,n,f+1+2` with 0x2E) |
| 0x2F | false | 0 | – |
| 0x30 | `opp.mask == 0x412 && opp.frame ≤ opp.rec+0x2E && self+0xF8 < 0x700` | 639 | **A high attack is incoming (active or pending) and close.** Gates the instant-crouch entries (d/d/b/d/f held → C4..C7 with `trans 06`) in the shared lists [C] |
| 0x31 / 0x32 | `self+0x118 == 0 / != 0` | 6 / 0 | `+0x118` = reaction/stun timer written by transition types 0x1E–0x27 [C] |
| 0x33 | `self+0xD5` | 36 | [G] contact flag from `0x80043EA8` |
| 0x34 | `self+0xD5 && !opp+0xDB && !(opp.posture & 4)` | 1 | …and opponent grounded |
| 0x35 / 0x36 | `opp.posture & 4` / not | 20 / 0 | Opponent lying down (posture bit 4) |
| 0x37 | `opp.posture & 4 && opp.rec+0x19 == 0 && self+0xF8 ≤ param` | 10 | Opponent down (not airborne) and in range: ground attacks/pick-ups |
| 0x38 | `opp.posture & 4 && self+0x30 ≥ 0x4000` | 0 | – |
| 0x39 | `opp+0xD3 && !opp+0xDB` | 88 | [G] opponent was hit or blocked and is not airborne |
| **0x3A / 0x3B / 0x3C** | `self+0xE0 / +0xE1 / +0xE2` (sets `+0x8C = 1`) | 96 / 97 / 52 | **Throw-escape input.** `0x80040A94`, called while being thrown, latches newly pressed **1** (`+0xE0`), **2** (`+0xE1`) or **1+2** (`+0xE2`). Victim lists branch to the escape motions (C323/C325/C327/C329). [V] |
| 0x3D | `self HP == 0` | 38 | KO'd (used in reaction lists) |
| 0x3E / 0x3F | `self+0xC6 == 0 / != 0` | 454 / 454 | `+0xC6 = (self+0x1278 ≥ opp+0x1278)`, set by `0x80046234` every frame: the two pelvises' screen X (+0x1278, written by the draw 0x80037924), i.e. which one is on the right of the screen [C]; with `+0xC7` set, `+0xC8` instead. It also picks the pad's forward (0x8002C580: table `0x80010738 + C6*18`). Picks get-up roll O39 vs O41 |
| 0x40 / 0x41 | `+0xC6 == 0 / != 0` and back turned | 235 / 235 | – |
| 0x42 | `self+0x8E == 0 && HP != 0` | 1 | – |
| 0x43 / 0x44 | `self+0x122 != 0 / == 0` | 3 / 1 | **Ki charge active.** `+0x122 = 120` is set by +0x24 bit 0x10000, see §4.2 [C] |

**Throw codes** (`0x8003115C`, stored in `0x80095764[side]`) [V]. A code is produced only when **self+0xD1** is set
(own attack connected), the opponent is not airborne (`+0xDB`), the opponent is not unthrowable (`+0xDF`), and
`self+0x30 < 0x2AAA` (facing the opponent within 60°). The value depends on the opponent's posture class and
`self+0x40` (0 front, 2 side, 1 side, 3 back):

| Opponent posture | front | side | side | back |
|---|---|---|---|---|
| `0x40` (standing throwable) | 2 | 3 | 4 | 5 |
| `0x20` (crouching throwable) | 6 | 7 | 8 | 9 |
| `0x80` (grounded) | 0xA | 0xB | 0xC | 0xD |
| `0x80 + 0x200` | 0xE | 0xF | 0x10 | 0x11 |
| self behind (`+0x30 ≥ 0x5556`) and opponent posture bit 2 | 0x12 (front) | 0x13 (side) | – | – |

Every Tekken 3 throw therefore has four direction variants (codes 2/3/4/5: 174/151/151/150 uses).

### 2.4 Reversal `cond` codes (`0x8002E8D0`, table `0x80019D10`) [V]

All of them, except 0x45, also require **both** fighters to face each other (`+0x30 < 0x2AAA`).

| cond | Test | Count | Meaning |
|---|---|---|---|
| 0x45 | `self+0xCE` and facing | 0 | Struck by anything |
| 0x46 | opponent mask == 0x412 (high), opponent record +0x24 bit 0x800 clear, limb test | 0 | High parry |
| 0x47 | mask == 0x217 (mid) | 0 | Mid parry |
| 0x48 | mask == 0x10F (low) | 200 | **Low parry** |
| 0x49 | mask 0x412 or 0x217 | 107 | **High/mid reversal** |
| 0x4A | `param == opp+0xA0` (opponent's current alias) | 213 | Counter one specific move |
| 0x4B | opponent's motion id (u16 before the motion data) in list `0x80095694[param]` (s16, ≤ 0 terminated) | 877 | Counter a family of motions |

The **limb test** for 0x46–0x49 is `0x8002EB4C(opp, param)`: `param == 0` means any limb; otherwise the attack
descriptor's first 4 bytes must contain the bone slot `param`. Data uses 6, 10, 14 and 17 (both hands, both feet).

### 2.5 `trans` low 6 bits → transition type (`+0x19A`) [C]

`0x80030F68` maps the raw value (table `0x80019DF0`):

| Raw value | Result |
|---|---|
| 2 | 0x14 when `0x80031028` says the queued attack's active window is still ahead of the start frame, else 2 |
| 0xC..0xF | 0xB when the queued move has no active frames |
| 0x14, 0x15 | 0xB unless `0x80031028` is true |
| 0x19 | 0x16 when `0x80031028` is true |
| others | unchanged |

The commit code (`0x8002F5DC…0x80030F00`) then switches on `+0x19A` (table `0x80019D40`, 44 cases). The common
behaviours are:

| Type (count as raw `trans`) | Handler | Behaviour |
|---|---|---|
| 0 (5155) | `0x8002FA30` | Hard cut. Frame 1, no blend (`+0x74 = 0`); facing snaps to the direction to the opponent; root position committed. |
| 2/3 (2197/329) | `0x8002FA74` | Continue at the matching frame (walk/crouch loops); `+0xB9 = 0xB` blend [G] |
| 4/5 | `0x8002FAF4` | Blend from the current pose. Auto-turns toward the opponent until the first active frame (`+0x114/+0x116`) when the opponent is within about 112° |
| 6 (12434) | `0x8002FC14` | Restart at frame 1 and keep facing. Root is committed only when the old record has +0x24 bit 28. Used by most stance/END links |
| 0xB/0xC/0xD/0xE (2196/4233/–/3142) | `0x8002FD2C…` | **Attack starts.** Frame 1; facing snaps to the opponent (or to `+0x34`) with a 4- or 6-frame blend (`+0xB9`) |
| 0x10..0x1D (0x18 2091, 0x1C 957) | – | Locomotion: dash/run/backdash/get-up (0x1C writes `+0x128`) [G] |
| 0x1E, 0x24..0x27 | – | **Hit reactions.** Use the reaction table entry (`+0x1A8`), record +0x24 bit 0x1000, and `+0x118` stun [C] |
| 0x21/0x22 (2441) | `0x800306B8`, `0x800307DC` | **Throw / hold link.** Aligns the victim using `+0x106..+0x10A`; record +0x24 bit 0x80000 marks chain throws; `0x80041998` [C] |
| 0x11 | `0x80030E6C` | Commit only (shared tail) |

The full per-type semantics were not decoded.

### 2.6 `param`

`param` is used by these conditions:
- cond 2..0x17: range against `+0xF4`;
- cond 0x18, 0x19, 0x37: distance against `+0xF8`;
- cond 0x48/0x49: limb slot;
- cond 0x4A: opponent alias;
- cond 0x4B: motion-list index.

Elsewhere it is 0. [V]

### 2.7 Special commands [V]

| Command | Meaning |
|---|---|
| `0xC001` | `0x8002C8B4`, **forward dash**: the input history (`+0x41C` ring) must read `f`, then `n`, then within 19 frames another `f` (f,n,f). Paul O3 (dash, posture 0x90842) |
| `0xC002` | `0x8002C99C`, **backdash**: the same pattern with `b`. Paul O7/O9 (posture 0x21052) |
| `0xC000` | END link. `exec` = start of the recovery cancel; `win_start` = the earliest frame |
| `0xC00C` / `0xC00D` | Call / return a shared list. The EXE block holds 672 entries and 36 entry points (`shared_lists.txt`) |

---

## 3. Header sections +0x18, +0x20, +0x24

### 3.1 +0x18: "alias missing" bitmap (504 bytes = 4023 bits) [V]

- **Reader:** `0x8002D07C`, called once at match load from `0x8006A1F0`.
- **Bit layout:** bit *i* (u16 word *i*/16, LSB first) is set **exactly when alias *i* is missing**
  (`0x3FFFFFFF`) in that character's alias table. Checked for all 3274/3198/2986 set bits of packs 280/281/283.
- **Merge per alias** (A = P1 table, B = P2 table):
  - A missing, B present: `A[i] = B[i]`;
  - B missing, A present: `B[i] = A[i]`;
  - both missing: both are set to their own entry `0xDC5`, the "missing" fallback.

So each fighter **borrows the moves it lacks from the opponent's table**, at the opponent's record addresses. This is
how throw-victim and other cross-character animations, which are defined in the attacker's file, become reachable.

### 3.2 +0x20: camera-script selector rows (8 bytes) [V]

Rows are `u16 type, u16 cum_prob(0..0xFFF), s32 value`. Groups start with a header row `{0, 0x1000, 0x1000}`.

| Type | Meaning |
|---|---|
| 1 | `value` = built-in camera preset number (15..29) |
| 2 | `value = 0x80000000 \| offset` into **+0x24** |

**Selection** is done by `0x80067260(index, flag, player, out)`:

1. `rand & 0xFFF` is drawn (`0x8007A318`).
2. If `index < 0x2B`, rows come from the **EXE table `0x80098370`** (row index = `index`): e.g. index 0x0B gives 50%
   preset 20 and 50% preset 25.
3. If `index ≥ 0x2B`, rows come from the player's header+0x20, at row `index − 0x2B`.
4. The first row after the header whose `cum_prob ≥ rand` is taken.
5. When `flag` is set, a type-2 row may be replaced by the next row, its paired side variant (`0x800A8CBC`).
6. Indices 0x19..0x2A and game mode 5 return −1.
7. A negative `value` other than −1 is made into a pointer: `(value & 0xFFFF) + header+0x24`.

**Who supplies `index`:**
- **record +0x24 low byte** (§4.2): throw and cinematic moves;
- `0x80067070`: intro/win cameras, with index `0x2B + 2n`, or 0x4B for Ogre (moveset 14).

For Paul (record 74), the +0x24 low bytes 43, 51, 60, 65, 74, 83, 90 and 99 minus 0x2B give 0, 8, 17, 22, 31, 40, 47 and
56, exactly the header rows of his 8 groups. 291 records across all packs use a camera script; all are throws or
mask-0x800 moves.

### 3.3 +0x24: packed camera keyframe scripts (s16 stream) [C]

Paul's section is 7292 bytes. Scripts start with a small count/flag word, then 0x80D/0x80E/0x810-tagged channel
blocks of s16 deltas. They are consumed by the fight camera (`0x80064xxx–0x80067xxx`,
globals at `0x800A8C70`). The exact channel encoding was **not decoded** [G: per-channel eye/target curves].

---

## 4. Move-record fields

### 4.1 +0x04 posture word (`actor+0x60`) [V unless marked]

| Bits | Meaning | Evidence |
|---|---|---|
| 0..2 | Base posture: **1 crouching, 2 standing, 4 lying/down** | `&7` in cond 0x1F/0x20, `&0x407` in 0x22/0x23, `&4` in 0x35..0x38. Code 9 (jump) sets bit 4 at runtime once `frame ≥ rec+0x19` (`0x800409A0`) |
| 3, 4 (0x08, 0x10) | **Guard mask**: crouch-guard and stand-guard; copied to `+0x66` | `0x8002F6B8` |
| 5 (0x20) | **Throwable crouching** (throw codes 6..9) | `0x8003115C` |
| 6 (0x40) | **Throwable standing** (codes 2..5); set on almost every standing state (0x842) | – |
| 7 (0x80) | **Throwable grounded** (codes 0xA..0x11) | – |
| 8 (0x100) | **Guard needs the guard input**: when set and `+0xCA == 0`, guard bits are cleared (`0x8002F70C`, `0x80040610`). Set on neutral stance 0x1952 and crouch 0x3129. See "Guard" below | – |
| 9 (0x200) | Sub-class of 0x80 (throw codes 0xE..0x11) [G: face-down vs face-up] | – |
| 10 (0x400) | **Airborne** (juggle scaling, `0x80044BB8`) | – |
| 11..15 | **5-bit state code → `actor+0x69`** | `0x8002F6D0` |

**Guard** [C] (`0x8002F6DC`, `0x80040514`):
- At every move commit `+0xCA = (+0xC5 == 0)`, except in game mode 5 (`0x800AFA88`), where it is left alone.
  `+0xC5` is set per player at round start (`0x8002A63C`) from a per-player setup value and also makes the
  routine at `0x8002C278` skip its work. What it marks is **[G]**: not simply "CPU", because the AI analysis
  (ai.md §7.1) shows the CPU relying on the neutral guard (EASY profiles never roll a guard yet block often);
  a practice-dummy "no guard" setting or a demo/replay player fit better.
- With `+0xCA` set (the normal case) the neutral stance (0x1952) keeps its stand-guard bit, so **standing still
  blocks highs and mids**, and the crouch (0x3129) blocks lows. With it clear, only moves whose posture has
  guard bits without 0x100 guard: walking back (0x1052) and crouch guard (0x2829, reached with d/b).
- Moves with `+0x24` bit 0x4000 ("guard by input") take the guard from the stick each frame: with `+0xCA`,
  directions 4/5 give 0x1052 and 1/2 give 0x2829; without it only 4 and 1 do. A second path (`+0xB6 != 0`) does
  the 4/1 case and otherwise reloads the record posture. No move of Paul's set has bit 0x4000.
- A hit is blocked when `mask & victim(+0x66 = posture & 0x18)` is non-zero and the victim faces the attacker
  (reactions.md §1, §3.1). OpenKumite: `kumite_sim::Fighter::posture`, `Fight::resolve_hits`.

State codes:

| Code | Value | Meaning |
|---|---|---|
| 1 | 0x842 | normal move/attack |
| 2 | 0x1052 | standing guard |
| 3 | 0x1952 | neutral stand |
| 4 | 0x2021 | crouching move |
| 5 | 0x2829 | crouch guard |
| 6 | 0x3129 | neutral crouch |
| 7 | 0x3884 | down/knocked-down |
| 9 | 0x4C02 | airborne/jump state; `0x8004096C` sets bit 4 after `rec+0x19` |
| 0xA | 0x5000 | – |
| 0xC | 0x6042 | special-cased against low attacks (mask 0x10F) in the hit pass (`0x8004413C`) |

The bits from 16 up are **AI move-class hints**: they are read only by the CPU AI (`0x80056xxx–0x80062xxx`) and by
the camera.

| Bit | Meaning | Example |
|---|---|---|
| 0x10000 | moves forward | walk f, dash, f-jump |
| 0x20000 | moves backward | walk b, backdash |
| 0x80000 | [G] low-profile/advancing | crouch-dash O37, d+4 O139, O74 |
| 0x100000 | rare | – |
| 0x200000 | [G] unblockable/special | Burning Fist O119 |
| 0x400000 | evasive roll/get-up; also tested by the throw camera (`0x8006758C`) | O39, O41, O221 |
| 0x800000 | [G] throw follow-ups | f+1+3 / f+2+4 → O339/O340 |

### 4.2 +0x24 flag word [V/C]

| Bits | Meaning | Reader |
|---|---|---|
| 0..7 | **Camera-script index** (§3.2); 0 = none | `0x800673EC`, `0x800676F8`, `0x80066F30`; value 9 is special-cased in `0x80067558`/`0x800636C8` |
| 0x100 | [C] predicate for the `+0x181C` per-actor state (with `+0xBA`, `+0x88`) | `0x8003C884` |
| 0x200 | [C] predicate for the same state (with `+0xBB`, `+0x19E`, `+0x94`, `+0xDB`) | `0x8003C9BC` |
| 0x400 | **Unthrowable during this move**: sets `+0xDF = 1` | `0x800404CC` |
| 0x800 | **Not parryable / throw-class.** Reversal conds 0x46..0x49 fail against it; on commit `+0x11C = 9`; also read by the AI. Set on throw attempts O240/O241 and get-ups | `0x8002E938…`, `0x8002F970`, `0x8005C404` |
| 0x1000 | Reaction-class move (reaction transition types 0x1E/0x24..0x27 test it) | `0x800305F8…` |
| 0x2000 | String follow-up: on commit `+0xB5 = 1` and `0x8002C0E8` resets the input state at `+0x410..+0x418`; otherwise `0x8002C10C` | `0x8002F74C` |
| 0x4000 | **Guard by input during the move.** If the held direction is back/neutral-back (4/5), posture becomes 0x1052 (stand guard); down-back/down (1/2) gives 0x2829. 36 records, mostly pack 288 stances | `0x80040514` |
| 0x10000 | **Ki charge** (universal alias 6, `1+2+3+4`; Paul O371). On commit `+0x122 = 120`. While `+0x122 > 0` guard bits are cleared each frame (`0x800407CC`), and the next hit counts as a counter-hit (`0x80044A8C`). `0x8004028C` turns the move into an unblockable 10-damage hit and takes 10 HP when `+0xDA` [C; G for `+0xDA`]. Rumble pattern 0xE (`0x80075BF0`) | – |
| 0x20000 | Disable body push spheres 4..7 (1 record) | `0x8003EFF4` |
| 0x40000 | Keep the hand pose across the transition (hand code `0x80034128`; `0x8002F7F0`) [C] | – |
| 0x80000 | Chain-throw link (King O610..O614); throw transition types 0x21/0x22 | `0x8003076C` |
| bit 28 | Commit the root position at a type-0/6 transition (`0x8002F94C`) | – |
| bit 29 | Camera tracks the root bone (`+0xF68`) instead of the actor origin | `0x800646FC` |
| bit 30 | [C] render effect around the active frames (`0x800390C0`, frames `+0x2D..+0x2E`) [G: motion trail] | – |
| bit 31 | 138 records (turn-arounds, common jumps); no reader found | – |

Typical values:
- 0x70000000: ordinary moves;
- 0x30000000: throw links (no trail);
- 0x10000000: KO;
- 0: win/lose poses.

### 4.3 +0x30 low half: body push-sphere control [V] (`0x8003EC64`)

The earlier note "aux reaction/throw data" was wrong. This word controls the 8 push spheres (`actor+0x330 + 16·k`,
base radii from `0x80096F60[ch]`).

**Bits 0..3 = mode:**

| Mode | Effect |
|---|---|
| 0 | Normal |
| 1 | Only spheres 0 and 3 keep their base radius; the others become 1 |
| 2 | All 1 |
| 3 | All halved |
| 4 | All 0 |
| 5 | Spheres 0 and 3 halved; the others 1 |
| 6 | Sphere 0 +60, spheres 4/5 = 1, the others base |
| 7 | All +100 |

**Mode 0 only:**
- Bits 4..11 = per-sphere disable mask (bit 4+k removes sphere k).
- Bit 12 restricts that mask to frames `[first_active−8, last_active)`.

**Always:**
- Moves with active frames drop spheres 1 and 2.
- Airborne-and-flagged actors (`+0xDB && +0x87`) drop spheres 6 and 7.
- Record +0x24 bit 0x20000 drops spheres 4..7.

Data: 0 (8161), 1 (381), 0x1F00 (263), 0xC00 (98, jumps: spheres 6 and 7 off), 0x1C10 (80), 0xF70/0xF80, …

The **high half** is the hit-reaction id. If `(id & 0xF000) != 0`, `id & 0xFFF` indexes the alternate table
**`0x800174C4`** instead of `0x80010CE8` (`0x80044568`). Throws and jumps use 0x1175, 0x1145, 0x100D, …

---

## 5. Voice/SFX event lists (header +0x10, record +0x1C)

`0x80040EA0(actor, opp)` is called every frame per active actor (`0x8002B4C4/E8/51C`). It reads **at most the first 3**
`u16` entries of the list (terminated by 0xFFFF). Each entry is **`type<<12 | index`**, and the index is **not** a
SePlay code. [V]

| Type | Count | Trigger and action |
|---|---|---|
| 0, 6 | 0 / 4166 | Skipped by `0x80040EA0`. Type 6 (indices 0x49..0x4C) is on nearly every attack (jab: `0x6049, 0x7041`) and matches the whoosh entries in the EXE scripts (`0x8001BF4C…`). **No consumer found** [G: whiff sound] |
| 1, 7 | 9 / 4389 | **Impact sound** when own attack connects (`+0xD1`), once. **Blocked** (`opp+0xCF`): fixed index 0x4D when the attack descriptor's bone is < 0xC (arm), else 0x4E (leg); Mokujin uses 0x7B/0x7A. On a KO (`opp HP == 0 && opp+0xD0`) the index is 0x44 for bone < 0xB, else 0x48. **Hit**: Gun Jack and Mokujin get material handlers (`0x80041B70`, `0x80041C40`); indices 0x000/0x041/0x042/0x045 go to the zone-dependent hit sound `0x80041CB4`; any other index is played from the table. Most common: 0x7042 ×2810, 0x7047, 0x7046, 0x7044, 0x7000 |
| 2 | 271 | **Voice, category 0**, on frame 1 of the move, when alive and the voice lock `0x800971E8[player]` is 0. Sets the lock from the voice row (+0xC) |
| 3 | 2 | Voice, category 1, frame 1, no lock |
| 4, 5 | 0 / 1 | Play the SFX table index every call |
| 8 | 998 | **Start a timed sound script**: `+0x70/+0x72 = index`, `+0x120 = frame−1`. Scripts come from the pointer table `0x800971F8[index]` and hold u32 `{u16 cmd<<12\|frame, s16 code}`, stepped at `0x800413E0`. Index 255 ×286 [G: "none/continue"] |
| 9 | 439 | Voice-state control `0x800419B0(self, opp, index)`: 0 sets `0x800971F0[player]`, 1 plays the opponent's pending script voice, 2 sets the lock, 3 random. Mostly index 0 on hit-reaction records [C] |

**Mapping to SePlay codes** (engine.md §9.1) [V]:

- **SFX indices:** `0x80040DF4(player, entry)` looks up `0x80097050[index & 0xFFF]` (u16 SePlay codes) and calls
  `SePlay(player, code, 0)`. A zero type nibble routes to `0x800755A4` instead. Examples:

  | Index | SePlay code | Decoded |
  |---|---|---|
  | 0x42 | 0x60C8 | bank 1 (VAB 0, system SFX), voice 8, vol 3, prog 0, tone 8 |
  | 0x41 | 0x61A2 | – |
  | 0x4D/0x4E | 0x60C3 | – |
  | 0x5C | 0xE0CD | bank 3 (own character VAB) |

- **Bank field** (checked in SePlay `0x80075474`):
  - 0 = silent;
  - 1 = VAB 0 (system SFX);
  - 2 = VAB 1 (announcer);
  - 3 = the player's character VAB (`0x800A8C38[player]`).

  Mokujin codes are remapped by `0x80075548`.
- **Voices:** `0x80040C90(player, voice_row(+0x1A), entry)`. The row is `0x8001AD54 + row·24`; its first pointer
  leads to `{s16 count[4]; s16 codes[]}`. Category = type − 2. The code is
  `codes[Σ count[<cat] + index]`, and `index` must be below `count[cat]`.
  - Row 0 counts are 8/5/3/5.
  - Its codes are e.g. −9916 = 0xD944: bank 3, voice 6, vol 5, tone 4, i.e. the own-character bank.
  - Category 2 additionally calls `0x8006F208` and `0x8004B770`.
- All plays are logged with `0x8003352C(kind, player, code)` for replays. Replay playback is at `0x800336E8`.

---

## 6. Corrections to earlier notes

- The timed-property **op is the high byte**: `0x0B00` is op 0x0B with arg 0.
- **cond 0x24** = back turned (not crouching).
- **cond 0x25/0x26** = back turned with the opponent on the left/right side (not while rising). Paul O107/O108 are
  back-turned turn punches.
- **cond 0x19** with `param` = distance at least `param`.
- **req** is a moveset/character gate, not a requirement count.
- **exec** (byte +0xB) = commit frame; 255 means animation end.
- Record **+0x30 low** = push-sphere control.
- Record **+0x24 low byte** = camera script.
- Header **+0x20/+0x24** = camera scripts.
- Header **+0x18** = missing-alias bitmap used to borrow the opponent's aliases.
- **0xC001/0xC002** = forward dash / backdash detectors.

## 7. Still open

- Full per-case semantics of the 44 transition types (`0x80019D40`).
- The camera-script channel encoding (+0x24).
- The consumer of SFX type 6.
- `+0x181C` state (+0x24 bits 0x100/0x200).
- Exact meaning of `+0xD2`, `+0xD3`, `+0xD5` (`+0x1278` is the pelvis's screen X, see condition 0x3E).
- Which hand is A and which is B.
- The effect kinds for ops 6..10 (spark types).
