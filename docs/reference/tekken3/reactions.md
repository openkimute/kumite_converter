# Tekken 3 (SLUS-00402): hit-reaction system

> Detailed research spec, part of [engine.md](engine.md). Derived from the EXE and disc data only.
> Helper scripts named below were research tools and are not in the repository, except the reference
> implementations `t3_converter/src/reference/attackpath.rs` and `reference/camera` (they read the disc image).

Scope: how a hit becomes a victim move. This covers which of the 14 reaction slots is chosen, what the 7 parameters
do, pushback, juggles, counter hits, trades, frame advantage, and throws/escapes.
Everything comes from the EXE disassembly plus BNS data (Paul = record 74, common bank = 279).
No third-party code was used.

Tags: **[V]** traced in code or checked against data. **[G]** guess or interpretation.

Helpers are in `(research)/gap3/`:
- `exe.py`: EXE reader.
- `rx.py <rec> <reaction-id>…`: dumps a reaction entry, resolving each slot through the victim's alias table.
- `clx.py <rec> <alias>…`: a move record plus its cancel list (shared sub-lists expanded).
- `fa.py <rec> [alias…]`: frame-advantage calculator.

---

## 0. Pipeline (per frame, `0x8002B2xx`)

1. **Hit detection runs in both directions before anything else is applied.** `0x8004406C(A,B)` and then
   `0x8004406C(B,A)` (3 actors in mode 8).
   - The attacker's mask & victim posture & 7 must be ≠ 0 (§3.1).
   - Each move can hit each victim only once: attacker `+0x83[victim side]` is set on contact and cleared on a
     move change (`0x8002F634`).
   - On contact the code sets:
     - victim `+0xCE` (was hit this frame);
     - attacker `+0xD1` (move connected);
     - victim `+0xEE` = attacker record byte `+0x2C`, the victim's **hit-stop** frames (0 in all of Paul's
       moves) [V].
2. `0x8004439C(actor)` runs for each actor. For each of the 2 contact slots (`+0x13C`, `+0x168`) it calls:
   - `0x80043A94`: computes the hit direction, victim `+0x32` (§1.2);
   - `0x80044A8C`: classification flags;
   - `0x80044BD8`: damage.

   Then:
   - `0x80044E34` picks the "best" contact. Score = 1·whiff + 2·blocked + 3·chip + 4·close + 5·clean + 6·counter
     + 7·air; a tie goes to the larger damage. The result is copied to the victim by `0x80044FE4`:
     - `+0xCF` blocked;
     - `+0xD0` clean;
     - `+0xD3` counter;
     - `+0xD4` close-range;
     - `+0xE8` = |damage|;
     - `+0x22` = attacker index.
   - **Reaction entry pointer → victim `+0x1A8`** (`0x8004459C..0x80044680`):
     - normal case: `0x80010CE8 + id·42`, where id = attacker record `+0x32`;
     - if `+0xD4` (close range): the id comes from the close-range table `0x800177DC[rec+0x34].u16`;
     - if `id & 0xF000`: `0x800174C4 + (id & 0xFFF)·2` (throw/victim alias list, §5);
     - if `+0xE7` (mode-8 body hit): the fixed entry `0x8001DEC0`.
   - HP −= (Σ damage + scripted damage `0x800449B4`) << 16.
3. `0x80045248(actor)` (from `0x800451B8`): if hit, it calls the **slot selector `0x8002EBA4(victim, attacker)`**.
   The selector only runs when attacker damage type `+0xB7` is:
   - 0 (normal); or
   - 2, and the victim is neither airborne nor grounded.

   Type 1 (throws) and type 3 never get a reaction.
4. `0x8002F3AC` (frame advance) switches into the queued reaction. The jump table `0x80019D40[+0x19A]` runs the
   per-mode setup (pushback, juggle physics, stun counters) at `0x800307DC..0x80030E30`.

Because detection runs both ways before any reaction is applied, **a trade is symmetric**:
- both fighters take the other's full damage in the same frame;
- neither hit is a counter hit, because both attackers are past start-up (`frame ≥ rec+0x2D`; §3.3);
- both enter their own reactions [V].

Simultaneous **throws** are resolved differently (§5.4).

---

## 1. Slot selection: `0x8002EBA4(victim, attacker)` [V]

Entry `E` = victim `+0x1A8` (14 × u16 victim aliases, then 7 × s16 params). Let `st = (victim+0x60 & 2) ? 0 : 1`,
where 0 = standing and 1 = crouching (any posture without the standing bit 0x02).

The selector checks these cases in order:

| # | Condition | Slot | Entry mode (`+0x19A`) |
|---|---|---|---|
| 1 | Blocked (`+0xCF`) | **10 + st** (10 standing guard, 11 crouching guard) | 0x23 |
| 2 | Airborne (`+0xDB` or current record `+0x04 & 0x400`) | **no slot**: common air table `0x8001A630` (§1.3) | 0x24 |
| 3 | Posture `& 0x200` (lying face-down [G]) | **13** | 0x25 |
| 4 | Posture `& 0x004` (lying on the ground) | **12** | 0x25 |
| 5 | Counter hit (`+0xD3`) | **2 + st** (direction ignored) | 0x26 |
| 6 | Direction `+0x32` = 0 (front) | **0 + st** | 0x2A |
| 7 | `+0x32` = 1 (side A) | **4 + st** | 0x27 |
| 8 | `+0x32` = 3 (side B) | **6 + st** | 0x28 |
| 9 | `+0x32` = 2 (back) | **8 + st** | 0x2B |

In short: blocked → 10 + st; airborne → the air table; lying → 12 or 13; counter hit → 2 + st; otherwise the
direction's base slot (front 0, side A 4, side B 6, back 8) + st. The same order is exported as
`assets/gamedata/reactions.json` → `slot_selection.order`.

Notes:
- **High/mid/low** is not a slot selector. The level acts only through the hit mask:
  - it decides whether the move hits at all, and whether it is blocked (§3.1);
  - the move's own reaction id provides the variation.
- Slot 11 can only be reached by moves the crouch guard can block (lows 0x10F, special mids 0x31F). For highs and
  mids the data often fills it with junk (for example the launcher alias in Paul u/f+4's reaction 0xF5).
- A block only counts if the defender faces the attacker (`+0x30 < 0x4000`), so back hits are never blocked
  (`0x80044B04`).
- Data check over all 633 entries:
  - slots 10/11 are almost always guard animations (0xA85…0xAB6, `posture 0x1052/0x2829`);
  - slots 12/13 are the ground-hit aliases 0xA49/0xA4C, which lead to 0x99 (face-up) and 0xA6 (face-down,
    posture 0x284);
  - slots 4/6 come in consistent pairs (0x91F/0x921, 0x8B9/0x8BA, 0x8D7/0x8D8);
  - CH = front in 389 of 633 entries.

### 1.1 KO
`0x80045420..0x80045548` [V]: when HP reaches 0 on a clean hit (`+0xD0`) and the chosen reaction would return to
neutral (alias 3 or 0x28, stubs `0x8002D204` / `0x8002D20C`), what happens depends on the global byte
`0x800AFA93` (meaning [G]):
- **0:** the code forces `+0xDB = 1` and calls the selector again, so the KO uses the **air table** (the victim is
  knocked off their feet).
- **≠ 0:** it plays fixed alias **0xD66** (`0x8002D82C`, KO collapse) instead. Grounded victims and moves with an
  airborne window are excluded.

A **chip-damage KO** is promoted to a clean hit (`0x80044730`: `+0xCF = 0`, `+0xD0 = 1`), so the victim does not die
in a block animation.

### 1.2 Direction `+0x32` (`0x80043A94`) [V]
- `+0x2A` = atan2 from the victim to the attacker (root positions at `+0xF68` / `+0xF70`).
- `d = victim.facing(+0x2C) − +0x2A`, and `q = (d + 0x2000) & 0xFFFF`.

| q | `+0x32` | Meaning |
|---|---|---|
| < 0x4000 | 0 | front (±45°) |
| < 0x8000 | 1 | side A |
| < 0xC000 | 2 | back |
| otherwise | 3 | side B |

Which of A and B is left or right is **[G]**: the sign of the angle convention was not established.

The same quadrant logic run on the opponent gives `+0x40`, which throws use (§5).

### 1.3 Airborne / juggle reactions: `0x8002EE94` + common air table `0x8001A630` [V]
Juggle hits **ignore the move's reaction entry.** The game picks an index `+0x90` into a table of 4-byte
entries `{u16 target height (→ +0x96), u16 victim alias}`; the six entries' values are in
`assets/gamedata/reactions.json` → `air_reactions`. Example: entry 0 is the normal juggle hit, a fall alias with a
target height of 0x1B8.

| `+0x90` | When |
|---|---|
| 0 | Normal juggle hit (front/back) |
| 1 | "Spike": attacker strike vector Y (`+0x3E0`, copied from attacker capsule delta `+0x3D4` by `0x80040A94`) ≥ 0x6E, or two specific special moves |
| 2 | Victim already in the late part of a fall and close to the ground (`|+0xF6C| < 0x400`) |
| 3 | Juggle hit from side 3 |
| 4 | Juggle hit from side 1 |
| 5 | Used by entry mode 0x1E (cancel-driven) |

The juggle physics `0x8002F074`:
- Juggle counter: `n = +0x94`.
  - It is incremented on every air-hit reaction (`0x80030ADC`).
  - It is reset when a move starts that was not caused by a hit (`0x8002F7C0`).
  - While `n ≠ 0` the victim counts as airborne (`0x800402E0`).
- Pop velocity:
  ```
  vy = clamp(±100, 4·dmg + 40 − 7·n − (victimVelY + strikeVecY)/8)
  ```
  stored in `+0x9C`.
- Gravity: `y(t) = vy·t − 3t²`.
- Air time is solved so that the victim reaches at least the target height `+0x96`. The fall animation is phased so
  its hold frame (record `+0x1B`) lines up with landing. Air time is stored in `0x80095770[side]`.
- Pushback in the air uses the fixed 8-sample curve `0x80019D2C`.
- The spike case uses a fixed `vy = 400` and 0x28 units/frame of horizontal movement.
- `+0x98 = clamp(0..100, dmg + 7(n+1) + 10)`: purpose [G] (juggle float/scale factor).

**Result:** every extra juggle hit pops 7 units/frame less, which acts as juggle decay [V for the formula, G for the
naming].

---

## 2. The 7 parameters (entry bytes 0x1C..0x29) [V]

Readers: the mode handlers at `0x800309E4`, `0x80030BA0`, `0x80030C8C` and `0x80030D08`.

| # | Off | Used in mode | Meaning |
|---|---|---|---|
| p0 | +0x1C | Normal hit (0x27–0x2B), CH (0x26), block (0x23) | **Pushback angle offset.** Direction `+0x10C = +0x2A + p0 − 0x8000` (away from the attacker, rotated by p0). For front hits (0x2A) the base is `+0x34`. Values 0, ±0x2AAA (60°), 0xE001 (−45°), 0x1FFF (45°), … |
| p1 | +0x1E | Normal hit | **Pushback curve index** (u16 index into `0x8001075C`) |
| p2 | +0x20 | – | Angle-like (same distribution as p0/p4) but **no reader found in the EXE**: unused [V for no reader; G for meaning] |
| p3 | +0x22 | Counter hit (0x26) | Pushback curve for CH (angle = p0) |
| p4 | +0x24 | Grounded victim (0x25) | Pushback angle offset for ground hits |
| p5 | +0x26 | Grounded victim (0x25) | Pushback curve for ground hits (0x122 in 604 of 633 entries) |
| p6 | +0x28 | Block (0x23) | **Block pushback curve** (0x5A in 446 entries) |

The 7 parameters do **not** encode hitstun, blockstun, sound, hit effect or camera shake. Stun comes from the
victim alias's own animation and cancel list (§4). Effects and sounds come from the victim move's sound list and
timed properties.

### 2.1 Pushback curve table `0x8001075C` (71 curves × 10 s16 = 0x58C bytes, up to `0x80010CE8`) [V]
Curve `c` is at `0x8001075C + idx·2`, and the indices are multiples of 10:
```
s16 n      ; frames of constant velocity
s16 v      ; constant velocity (units/frame) for frames 1..n
s16 k[8]   ; extra impulse added on frames 1..8
```
The per-frame mover is at `0x8003F798..0x8003F860`:
```
step = (n-- > 0 ? v : 0) + (frame ≤ 8 ? k[frame-1] : 0) + extra(+0x124)
```
It moves the root along `+0x10C` (trig scaled `<<3`, then `/0x7FFF`, so the values are world units).

Most curves have no constant phase and front-loaded impulses (the default hit curve 0 moves the victim 730
units in total, the default block curve 0x5A 550); the ground curve 0x122 adds a long constant phase, and a few
curves have negative values that pull the victim *toward* the attacker. All 71 curves are in
`assets/gamedata/reactions.json` → `pushback_curves`.

### 2.2 Other per-mode state
- `+0x11A` = the reaction's frame count. For throws (mode 0x22) it is the attacker's `rec+0x1B` + the victim's
  frames, or max(attacker, victim), capped at 300.
- `+0x118` = "stun pool".
  - Normal hits: `min(0x42, |dmg| + old/2)`. Throws: `clamp(0x22..0x42, dmg·32/45 + 0x22)`.
  - Per frame (`0x800403C0`): `+0x11A` counts down and is zeroed on a clean hit. Once it is below 31, `+0x118`
    drains by 1/frame, plus 3 if button bit 0x20 is pressed or 1 for any button. Mashing therefore shortens it.
  - Cancel conditions **0x31** (`+0x118|+0x11A == 0`) and **0x32** (≠ 0) gate stun-dependent recoveries in
    reaction cancel lists [V for the code; G that only stagger/crumple moves use it].
- Record flag `+0x24 & 0x1000` (`>>8 & 0x10`) turns off the stun pool and the pushback curve for that move.

---

## 3. Masks, counter hits, juggle damage

### 3.1 Hit-mask bits (`0x800442A4`: `mask & victim.posture(+0x60) & 7`) [V]
The mask's low 3 bits are matched against the **victim's posture bits**:

| Bit | Posture | Hits victims who are… |
|---|---|---|
| 0x01 | crouching (0x2829, 0x21, 0x29) | crouching |
| 0x02 | standing (0x1052, 0x42, 0x842) | standing |
| **0x04** | **lying on the ground** (0x84 face-up / 0x284 face-down; e.g. alias 0x99 = 0x3884, 0xA6 = 0x4284) | **down** |

The guard bits are separate: **0x10** standing guard, **0x08** crouching guard.

| Level | Mask | Behaviour |
|---|---|---|
| High | 0x412 | Hits standing only, so it whiffs on crouchers and grounded opponents |
| Mid | 0x217 | Hits all three; blocked standing only |
| Low | 0x10F | Hits all three; blocked crouching only |
| Special mid | 0x31F | Hits all three; blocked both ways |
| Unblockable | 0x607 | Hits all three; no guard bits |

**Bit 0x04 = "hits a grounded opponent"** [V]. In the data, highs are the only standard level without it.

Extra rules in `0x8004412C`:
- A low (mask exactly 0x10F) whiffs against a victim in jump state (`+0x69 == 0xC`) during
  `[rec+0x19, rec+0x1A−5]`, and against airborne victims.
- Attacker `+0xB6 == 2` forces `mask |= 1`.

### 3.2 Damage (`0x80044BD8`) [V]
Let `B` = attacker base damage (`+0x5E`, which already includes the op-0x0B bonus).

- **Blocked:**
  - chip: `B/10 + 2`, applied only if the global `0x800AFA58` ≠ 0 or attacker `+0x122` ≠ 0;
  - otherwise 0.
- **Close range** (`+0x26`): `B + B/2`, with **no zone scaling**.
- **Otherwise:** `B · zone% / 100` (`0x80045038`, table `0x8001DEEC`).
- **Counter hit:**
  ```
  D' = D·12/10 + bonus
  ```
  where:
  - `bonus = 0.3·|Vdmg|` if victim or attacker `+0x122` ≠ 0;
  - otherwise `bonus = (0.3·0.3·|Vdmg|) · f / max(startup, f)`, with Vdmg = the **victim's** current move damage,
    f = the victim's frame and startup = victim `rec+0x2D` (if startup = 0, the ratio is 1).

  In the normal case f < startup, so the bonus is about 0.09·Vdmg·f/startup (truncated integer maths).
- **Airborne** (`+0x27`, applied after the above):
  - `+0x94 == 0` (first air hit): ×80/100;
  - otherwise **×1/2**.

  The launcher itself hits a grounded victim and does full damage. In practice: 1st juggle hit 80%, then 50% [V].

### 3.3 Counter-hit condition (`0x80044B50`) [V]
A counter hit needs one of:
- the victim's current move has damage ≠ 0 **and** the victim's frame < its first active frame (`rec+0x2D`);
- victim `+0x122` ≠ 0;
- attacker `+0x122` ≠ 0.

`+0x122` is a 120-frame timer set by record flag `+0x24 & 0x10000`. For Paul only record 371 (alias 6, the
1+2+3+4 command) sets it [G: taunt]. While it runs, that player's hits count as counter hits and chip the
opponent, and hits on that player also count as counter hits.

Effects of a counter hit:
- damage per §3.2;
- reaction slot 2/3;
- pushback curve p3.

### 3.4 Hit-stop
- Victim `+0x76` = attacker `rec+0x2C`. It freezes the victim's frame counter (`0x8002F49C`).
- It is 0 for every Paul move checked.
- Special case: alias 0x898 sets it to 30 (`0x80044314`).

---

## 4. Frame advantage [V method, data-derived numbers]

Recovery is decided by the END (0xC000) entry of a move's cancel list (`0x8002DB0C..0x8002DD70`). Let
`W = win_start` and `E = win_end`.
- Input in frames `[W−5, W−1]` (button press) is buffered and executes at frame W.
- Input in `[W, E]` executes immediately. The game evaluates the **destination (neutral) move's** command list
  at frame 1, so any neutral command works.
- With no input, the move runs to its frame count and links to `dest`.
- A window of 0..0 means **no early action**: the move plays out (knockdowns, launch falls).

So **W is the first actionable frame**. This holds for the victim's reaction move and the attacker's move alike.

Timing:
- On the hit frame the attacker is at frame A (first contact, normally `rec+0x2D`).
- The victim switches to reaction frame 1 in the same update. The attacker advances to A+1.
- Both then advance one frame per frame.

The victim can act after `W_vic − 1` more frames, the attacker after `W_att − (A + 1)`, so

  advantage = W_vic − W_att + A.

Assumptions: hit-stop 0 (true for these moves); the victim is Paul; standing, front hit.

Worked example, Paul's jab (1): A = 8, W_att = 25; on a normal hit the victim's front reaction has W = 24, so
**+7**; on a standing block the guard reaction has W = 19, so **+2**. Every input to the formula is in the
converted data (`assets/gamedata/movesets/moveset_NN.json`: active frames and END windows; `reactions.json`:
which victim move each slot names), so the port computes the rest instead of tabulating it.

Plausibility: over Paul's basic moves the results (a jab slightly plus, mid pokes around 0, a low sweep heavily
punishable on block) match the game's commonly known frame data within about ±1 frame. That small difference is consistent with the off-by-one ambiguity in when
the attacker's frame increments relative to the hit test.

**Caveat [G]:** pushback does not change frames but may make punishes whiff. Some reactions (crumples, stagger)
also gate recovery through cancel conditions 0x31/0x32 on the stun pool.

---

## 5. Throws [V]

### 5.1 Attempt
A throw is an ordinary move with **damage type 1** (`rec+0x14 = 0x4000`), so it gets no damage or reaction on
contact. Its hit mask is a normal level: Paul 1+3 / 2+4 = **0x412 (high)**, so throws whiff on crouchers.
The earlier "level 6" claim is wrong; 0x607 is unblockable. Guarding does not matter: type 1 skips the
block/reaction path.

On contact the attacker's `+0xD1` is set. Its cancel list (e.g. Paul O240, 1+3) holds entries:
- `cmd 0x0010`, `cond 2..0x13`, `param 0x600`, window 12..14, transition **0x21**;
- `cond` (`0x8002E118`) = relative position code from `0x8003115C` and distance `+0xF4 ≤ param` (1536 units).

Position codes:
- The thrower faces the target (`+0x30 < 0x2AAA`) and the target is standing (posture 0x40):

  | Target orientation (`+0x40`) | Code |
  |---|---|
  | front (0) | **2** |
  | back (2) | **3** |
  | side 1 | **4** |
  | side 3 | **5** |

- Crouching targets (0x20): 6, 7 (back), 8, 9.
- Grounded targets: face-up 0xA–0xD, face-down 0xE–0x11.
- Oblique standing target: 0x12 / 0x13.
- No code if the target is airborne (`+0xDB`) or `+0xDF`.

Each throw input therefore has four follow-ups, one per position code, each a separate move with its own
damage. Example: Paul's 1+3 (O240) from the front leads to alias 0xAEB (30 damage). The others are in Paul's
cancel lists (`assets/gamedata/movesets/moveset_00.json`).

### 5.2 Pairing
- When a fighter's queued move with mode 0x21 starts, `0x80045550` links the pair:
  `+0x21` = partner index on both, and `+0xC4 = 1` (locked) on both.
- `0x800455A0` then calls `0x8002D75C(victim, thrower)`. It reads the thrower move's `+0x32` id (flag 0x1000) and
  takes the **victim alias from `0x800174C4[id & 0xFFF]`** (397 × u16; `reactions.json` → `throw_victims`).
  Example: id 0x100D gives victim alias 0xBC5.
- The victim enters that alias with mode 0x22: `+0x74 < 0` = "being thrown".
- **Damage** is not a hit. Timed-property op **0x0B** on the thrower move sets `+0xE4 = 1`, and `0x800449B4` then
  takes the thrower's `+0xEC` (or its move damage `+0x5E`) off the thrown victim. Example: O326 at frame 75 takes
  30. There is no zone, counter-hit or juggle scaling.
- Ops 0x0C and 0x0D (`+0xE4` = 2/3) are self-damage and self-heal (`0x8001DF20` op jump table).

Port findings (OpenKumite's `crates/kumite_sim`, checked by rendering Paul's and Law's throws) [I]:
- The victim alias comes from the **thrower's** alias table, and the victim plays the thrower's move record:
  Paul's set holds both halves of his front throw (O326 `motion_123`, victim O327 `motion_124`), while Law's alias
  0xBC5 is empty. The victim's skeleton plays the thrower's animation (bone names are shared).
- Both actors end up on one origin (`0x800306B8` copies the partner's root bone and facing), and the paired
  animations' root translations are **absolute** in that space: the thrower's O326 root starts 0.91 m behind the
  origin, the victim's at the origin. Applying them relative to their first frame pulls the pair apart.
- The thrower's facing is the direction to the victim, turned by 0 / ±90° / 180° for the victim's orientation.
- Damage: op 0x0B's argument (Law's throw: 7 at frame 32, 23 at frame 64), the move damage when it is 0.

### 5.3 Escapes
The victim's thrown move has a cancel entry `cmd 0x0010, cond 0x3A/0x3B/0x3C → break alias, transition 0x21`.

| Victim alias | Escape cond | Window |
|---|---|---|
| 0xBC5 (vs 1+3 front) | cond 0x3A | frames 1–15 |
| 0xBE5 (vs 2+4 front) | cond 0x3B | frames 1–15 |
| 0xC0D / 0xC1C (side throws) | 0x3A / 0x3B | frames **1–5** |
| 0xCB2 (back throw) | – | **no escape** |

Escape flags `+0xE0/0xE1/0xE2` are set by `0x80040AD0` from **new presses this frame**:
- 0x10 (button 1) → `+0xE0`, so **cond 0x3A = press 1**;
- 0x20 (button 2) → `+0xE1`, so **cond 0x3B = press 2**;
- both held and one newly pressed → `+0xE2`, so cond 0x3C = 1+2.

Evaluating cond 0x3A–0x3C sets `+0x8C`. After that, any non-zero flag is **latched**: the first press inside the
window decides the outcome. A wrong button locks the escape out [V for the code, G for the gameplay reading].

On success the victim queues its break move (e.g. 0xB52, 30 frames, mode 0x21). That move is itself a pairing
move: its id 0x105C sends the thrower into the matching break animation. Both return to neutral after
30 frames, so the break is even.

### 5.4 Throw vs throw
When more than one fighter has a pending mode-0x21 move in the same frame, `0x800455A0` picks one at random
(`0x800AFA4C % count`) [V]. Strike vs throw on the same frame resolves as in §0: the strike damages and the throw
contact just sets `+0xD1`. Whether the throw still links depends on the thrower's cancel window and whether a
reaction has overridden its move [G].

---

## 6. Corrections to earlier notes
- The 14 slots are **not** used for air hits. Air hits use the common table `0x8001A630`, indexed by direction and
  special cases.
- Throws are not "level 6". Paul's throws are high (0x412) and damage type 1. The `0x800402B8` "HP −10, 0x607"
  case runs only in game mode 8: Ball/Force body collisions, reaction entry `0x8001DEC0`.
- Hit-mask bit 0x04 = hits grounded (lying) opponents.
- Record byte `+0x2C` is the victim hit-stop, not "unused" (0 in Paul's data).
- Record byte `+0x1B` is the "hold frame". The animation pauses there while `+0x6A > 0` or while airborne
  (`+0x92`), and it is used to sync juggle landings.
- The reaction-entry params are pushback angle/curve selectors only. p2 has no reader.

## 7. Open
- Left/right naming of direction quadrants 1/3.
- The global `0x800AFA93` (KO style).
- Exact use of `+0x98`, and the full list of cancel `cond` codes.
- Off-by-one confirmation of the frame-advantage formula against an emulator trace.
