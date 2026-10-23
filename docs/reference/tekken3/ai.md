# Tekken 3 (SLUS-00402): CPU AI

> Detailed research spec, part of [engine.md](engine.md). Derived from the EXE and disc data only.
> Helper scripts named below were research tools and are not in the repository, except the reference
> implementations `t3_converter/src/reference/attackpath.rs` and `reference/camera` (they read the disc image).

Scope: the per-frame CPU opponent (`0x80059890` and helpers `0x80056764..0x80062a10`), the 3×10 behaviour
profiles, the per-character range thresholds, the per-character AI tables/handlers, and CPU opponent
selection for Arcade / Team Battle / Survival (overlay record 5).

Confidence tags: **[V]** read from code/data (decompiled and cross-checked against the disassembly);
**[G]** interpretation / naming guess. Nothing here comes from third-party sources.

Helpers (all in `(research)/gap5/`, not in the repository; no decompiler output is reproduced in this note):
- `ghidra/` — headless Ghidra project of the EXE (`exe.bin`, base 0x80010000) and EXE+record 5 (`exe_ovl5.bin`),
  scripts `Dec.java` (decompile list of functions) and `JT.java` (jump-table overrides).
  Decompiles: `ghidra/main2.c` (0x80059890 with all 6 jump tables recovered), `ghidra/ai2.c` (all helpers),
  `ghidra/cb2.c` (per-character handlers), `ghidra/ovl5.c` (opponent selection). `post.py` renames
  state-block accesses (`ST.fXXX` = state+XXX, `ST.P_XX` = profile+XX; `SELF`/`OPP` actors).
- `mem.py` (read EXE/overlay by RAM address), `profiles.py` → `profiles.md` (all 30 profiles, labelled),
  `seqdump.py` (canned AI input sequences, per-moveset AI table), `aiflags.py <rec>` (AI flag bits of the
  moves in a moveset's standing list), `condscan.py <rec> <cond…>` (find cancel entries by `cond`).

---

## 1. Architecture in one paragraph

The CPU never "plays" moves directly. Every frame it builds a **candidate list from the cancel/command list of
its own current move** (the same list a human's input is matched against, record m+3 + the EXE shared
sub-lists), filters that list with situation-specific predicates, picks one entry uniformly at random, and
converts the entry's `cmd` back into **pad bits** that are fed to the normal input parser
(`0x8002C580`). Strategy is therefore data-driven by the movesets: what the CPU *can* do is exactly the
cancel list, and what it *prefers* is controlled by AI flag bits stored in each move record (+0x04, +0x08,
+0x24), by a 110-byte **profile** (difficulty × arcade stage), by 6 per-character **range thresholds**, and by
a handful of per-character special tables/handlers. Movement (walk, dash, backdash, sidestep) and guarding are
also produced as pad bits (guard = hold back / down-back, or simply neutral).

---

## 2. Globals, RNGs, entry points

| Address | Meaning |
|---|---|
| `0x80059890(actor,&held,&pressed)` | per-frame AI. Called from `0x8002C4C4` when actor+0xC5 (CPU) and HP>0, then `0x8002C580(actor, side=0, held, pressed)` [V] |
| `0x80059654()` | round setup: pairs actor slots (`actor+0x1886` = AI state index, `+0x1887` = opponent index), caches alias tables, calls `0x800593C8` per CPU [V] |
| `0x800593C8(actor,…)` | per-round reset of the state block + `0x80061488` init + per-character handlers (§8) [V] |
| `0x80061488(state, diff, level, mode)` | profile load (§5). `diff/level < 1` (incl. -1) → use globals `0x800AE208` / `0x800AE211` [V] |
| `0x8009F318 + idx·0x330` | AI state block (§3). `0x8009F2E8[idx]` = alias table of the actor [V] |
| `0x800AFA5C` / `0x800A867C` | `SELF` / `OPP` actor pointers for the current AI call [V] |
| `0x8009F978` | candidate table, ≤180 × 12 B: `{u32 move_rec*, u32 cancel_entry*, s16 state}` (state 0 = candidate, 1 = in pool, −1 = excluded) [V] |
| `0x800ADCA0` | **AI LCG**: `x = x*5 + 3`. Probability tests use the *old* value: `(x & 0xFFF) < P` (P/4096) [V] |
| `0x8007A318` | libc `rand()`: `x = x*0x41C64E6D + 0x3039; return (x>>16)&0x7FFF` [V] |
| random int | `randint(n) = (n * ((rand() + aiLCG) & 0x7FFF)) >> 15`, then the AI LCG steps (`0x800566E8`) [V] |
| `0x8004CE54` | selection RNG at `0x8009F2A8`: `x = x*5 + 1`, returns x (used by opponent selection, `% n`) [V] |
| `0x800AE208` u16 | difficulty option 0 EASY / 1 MEDIUM / 2 HARD (clamped to 2) [V] |
| `0x800AE211` u8 | AI "level" 0..9 (arcade stage index, or computed for Team/Survival, §10) [V] |
| `0x800ADED4` | practice-like flag: level forced to 4, cooldowns halved, no learning (§5.3) [V] |
| `0x800AFA88` | battle type; 7 = Tekken Ball (calls hooks `0x800B4494/4644/46A0/4720` in *its* slot-A overlay), 8 = Tekken Force (3 actors) [V] |

**Pad word the AI produces** [V]: PS1 bits in the *side-0* frame (CPU always parses with side 0):
`0x1000` up, `0x4000` down, `0x2000` **forward**, `0x8000` **back**, buttons `0x80`=1 (LP), `0x10`=2 (RP),
`0x40`=3 (LK), `0x20`=4 (RK). Output: `*held = out; *pressed = out & ~prev_out` (prev kept in state+0x06).

**Command → pad (`0x800618C0(state, cmd, arg)`)** [V]:
- `cmd < 0xC000` (single-frame command): direction = the *highest* numpad direction whose bit (bit 4+n) is set,
  through table `0x80023050` (numpad → pad: 1=0xC000, 2=0x4000, 3=0x6000, 4=0x8000, 6=0x2000, 7=0x9000,
  8=0x1000, 9=0x3000); buttons bits 0..3 via `0x80023064` = {0x80,0x10,0x40,0x20}.
- `cmd ≥ 0xC000` (motion sequence, looked up by `0x8002CD7C`): the step list is replayed one step per frame
  through state+0x08 (word format `dir | btn<<8`, buttons 0x100/0x200/0x400/0x800 = 1/2/3/4, converted by
  `0x800617DC` with table `0x8002301C`). Only `(cmd & 0xC000) == 0xC000` is accepted.
- `0x8006183C(state, ptr)`: plays a **canned sequence** of such words (one per frame, 0-terminated) [V].

---

## 3. State block (`0x8009F318 + idx·0x330`) — fields used

| Off | Meaning |
|---|---|
| 00 / 02 | slot index / counter of animation-frame changes (bit 0 used as a coin flip) |
| 04 / 06 / 08 | pad output this frame / last frame / canned-sequence pointer (non-null ⇒ AI skipped) |
| 0C / 10 | own alias table / opponent actor |
| 14 | **my range zone** 0..5 (can I reach him), §6 |
| 18 | **his range zone** 1..5 (can he reach me), §6 |
| 1C / 20 / 24 | distance (actor+0xF8) / previous / delta (negative = closing) |
| 28 / 2C | my move changed / frames in current move |
| 2A | random accumulator while in zone 4/5 |
| 2E | `SELF+0x30` relative angle (0 = facing, 0x8000 = back turned) |
| 30 | my HP last frame (for "was hit" detection) |
| 36 | lying-down flag; 6E wake-up tap timer |
| 38 / 3A / 40 / 48 | movement timer / movement mode (2 = running) / movement cooldown (40 default) / hold-direction timer (jump window) |
| 3C | crouching-posture flag `(rec+4 & 0x205)==1`; 3E standing-or-crouching neutral flag |
| 42 / 44 / 46 | cooldowns: combo-table (28..43), crouch-dash (18), jump-attack (1..13) |
| 4A / 4C | crouch timer (20..42) / crouch-hold after a crouching move (12) |
| 50 | I am in an attacking move (rec+0x2D ≠ 0) |
| 52 | my move is a stance-type move (rec+4 & rec+8 bit 0x200000) past its end |
| 54 | 30-frame window after my attack connected (`SELF+0xD2`) |
| 58 / 5C / 60 | my move record (prev) / cancel-entry arg of last command / last chosen move |
| 64 | situation bits for `cond` 0x24..0x27/0x30 (§4.2) |
| **68** | **attack cooldown** (frames until the next neutral attack; < 0 ⇒ attack now) |
| 6A | "already safe" vs the incoming attack (§7.1) |
| 6C | delayed-punish timer |
| 70 / 72 | candidate count / pool count |
| 74 / 78 / 7C | vs-opponent handler, own-character handler, candidate filter callback (§8) |
| 80 / 84 | frames opponent in start-up / frames I am in start-up |
| 88 | opponent is Yoshimitsu (moveset 4) in alias 0x456 |
| 8A / 8C | wake-up lock (16) / handler cooldown |
| 8E..91 | one-shot "attack now" flags (8F, 91 used) |
| 92 | guard latched (set by learning gate / reaction 2) |
| 94 | evade plan vs unblockable/throw (0 none, 1 planned, 2 too late, 3..6 sub-actions) |
| 96 / 9C / A0 / BC | hit-confirm string: length so far / frame to fire / move / cancel entry |
| C0..DC, 1A0..1A4 | three "planned follow-up" lists — **dead code** (§7.6) |
| D4 | exclude string-starter moves (rec+0x24 flag 0x2000) |
| 1D2 / 1D4 | consecutive attacks performed / limit = max(P+0x32, P+0x36) |
| 1D8 / 204 / 206 | opponent move record prev / changed / frame prev |
| 1DC[8] / 1FC | ring of opponent moves that hit me / index |
| 1FE / 202 | "rage" timer (600 frames) / active flag (§5.3) |
| 200 / 201 | opponent is moveset 14 (Ogre) doing attack-descriptor 0x18 / moveset 19 doing 0x1A [G: specials] |
| 208 | opponent is down and hittable (rec+4 & 0x204, not 0x4C02, OPP+0xDB==0) |
| **20A** | **frames until the opponent's attack becomes active** (first-active − frame; 999 none; 0x3E5/0x3E6 out of range) |
| 20C | opponent attack incoming (start-up) |
| 20E / 210 | frames opponent in rec+8 bit 0x80000 state |
| 214 | opponent's move has rec+4 bit 0x400000 (throw-type) and is not yet over |
| **216** | **opponent attacks-in-a-row counter**: −1 when he is idle, +1 each time one of his moves reaches its first active frame (cap 10) |
| 21A | opponent "far behind" (`OPP+0x994 < -0xC1B`) [G] |
| 21E | AI mode (init arg 4): 0 = idle dummy (no input), 1 = output `(rec+4&4)<<10` [G dummy], else full AI |
| 220 / 224 / 228 | my rec+8 (full) / my rec+4 / my attack mask |
| 22C / 230 / 234 | opponent rec+8 (full) / rec+4 / attack mask (low 16: 0x412 high, 0x217 mid, 0x31F s.mid, 0x10F low, 0x607 unblockable, 0x706 throw) |
| 238 | **profile** (110 B, §5) |
| 2A6 | copy of the HARD/L9 profile (`0x80023E22`) — never read by AI code [V] |
| 314 / 320 | my / opponent's 6 range thresholds (§6) |
| 32C | Tekken Force table pointer (`0x80022FA4 + diff*30 + min(level,3)*6`) |

---

## 4. Data the AI reads from the movesets

### 4.1 AI bits in move records [V by code; names G, confirmed on Paul's list with `aiflags.py 74`]
| Field | Bit | Use by AI |
|---|---|---|
| rec+0x08 (attack word) | 0x40000 / 0x80000 / 0x100000 | **range class** close / mid / long. None set ⇒ treated as close+mid (0xC0000) |
| rec+0x08 | 0x10000 | "hits grounded / special" — accepted out of range; used for wake-up and down-attacks |
| rec+0x08 | 0x200000 | evasive / special-stance move (Paul d+1, d+4, f+1+4) |
| rec+0x08 | **0x400000** | **throw** (1+3, 2+4, command throws, parries) — excluded from normal pools, used by throw logic |
| rec+0x04 (posture) | bit0 / bit1 | crouching / standing posture (0x2829 / 0x1052 families) |
| rec+0x04 | 0x400 | airborne; 0x4/0x404 lying down; 0x204 down-and-hittable |
| rec+0x04 | 0x10000 | always-in-range move; 0x20000 movement/non-attack (excluded from attack pools) |
| rec+0x04 | 0x80000 | hop/jump-type move; 0x800000 never selectable |
| rec+0x19 | ≠0 | jump window (jumping move) |
| rec+0x24 | >>8 & 0x20 | string starter (has a follow-up), >>8 & 0x08 unparryable, >>8 & 0x100 "desperation" move |
| rec+0x2D/0x2E | | first/last active frame; 0x2D==0 ⇒ not an attack |
| low16 of +0x04 | 0x4C02 | stun/stagger state [G] (self → stun-recovery logic; opponent → free-hit logic) |

### 4.2 Cancel-entry filtering (shared by every AI list walk) [V]
Entry = `u16 cmd | u8 req | u8 cond | u16 param | u16 dest | u8 trans | u8 win_start | u8 win_end | u8 extra`.
- `0xC00C` entries are expanded inline: `extra` entries from `0x80017C78 + dest*12`; `0xC000` ends the list.
- Window: `win_start ≤ SELF.frame ≤ win_end`.
- `req` (character filter): 0 any; 1..0x17 own moveset == req−1; 0x18..0x2E own moveset ≠ req−0x18;
  0x2F..0x45 opponent moveset == req−0x2F; 0x46..0x5C own char id (actor+0x18) == req−0x46;
  0x5D..0x73 own char id ≠ req−0x5D; ≥0x74 never.
- `cond`: 0 always; AI shortcuts (state+0x64 bits, computed each frame):
  `'$'(0x24)` bit0 = SELF+0x30 > 0x4000 (**my back is turned**); `'%'(0x25)` bit1 / `'&'(0x26)` bit2 = opponent
  in my left / right flank (SELF+0x2E in 0x4E38..0x8000 / 0x8000..0xB1C7); `'0'(0x30)` bit3 = opponent doing a
  0x412 move before its active end and `0x800AFA3C < 0x700`; `'''(0x27)` bit4 = SELF+0x32 == 0. Everything
  else → engine test `0x8002E0BC(entry, self, opp)`.
  *(As in [move-properties.md](move-properties.md) §2.3: 0x24/0x25/0x26 are back-turned / side-turned variants,
  not crouch/rising.)*
- Special conds the AI looks for: `'1'(0x31)` getting-up options (lying), `'5'(0x35)` attacks on a downed
  opponent, `':' ';' '<'(0x3A..0x3C)` throw-escape windows (→ press 1, 2, 1).
- Special `cmd`: `0xC001` = **dash forward** (f,n,f within 19 f, `0x8002C8B4`), `0xC002` = backdash
  (`0x8002C99C`).

### 4.3 Candidate builder `0x80056764(state, endLinks)` [V]
Walk the cancel list of `state+0x58` (my current move); keep entries passing §4.2; store
`{dest move record, entry, 0}` (≤180). If `endLinks==0`, also append entries with `win_start < 2` from the list
of the move that follows at animation end (the 0xC000 link), then mark −1 every candidate whose move has
rec+4 bit 0x800000, then call the filter callback `state+0x7C` (if set). Returns count.

### 4.4 Picker `0x80058A84(state)` [V]
`k = randint(pool)`; walk candidates with state>0 (from the end if `state+0x02` is even, from the start if odd)
and take the k-th; `0x800618C0(state, entry.cmd, entry.dest_record)`; then `0x80058958`:
if rec+8 has 0x400000 `state+0x96++`; if it is an attack (0x2D≠0): **attack cooldown**
`state+0x68 = P[0x04] + P[0x06 + 2*(aiLCG&3)]` and movement timers reset.

### 4.5 Pool predicates (mark candidates state 0 → 1) [V]
`inRange(m) = (rangeClass(m) & ZONEMASK[state+0x14]) != 0`, `ZONEMASK = 0x80098230 = {CML, CML, CM, L, 0, 0}`
(a second table `0x80098248 = {CML, CML, ML, L, L, 0}` is used for "can his move reach me").

| Fn | Pool |
|---|---|
| `0x80057538` fast poke | dmg>0, active start < 14, not jump, not throw, not 0x20000, inRange or 0x10000 |
| `0x80057D58` interrupt | not throw, dmg>0, attack, (not jump or hop), **start-up < frames-until-his-hit (+0x20A)**, inRange |
| `0x80057E94` interrupt, no highs | as above, attack mask ≠ 0x412 |
| `0x8005766C` guard-aware | attack, inRange; vs crouching (opp rec+4 bit0) mids only; vs standing (bit1) no highs; vs bit2 no high/mid |
| `0x800577DC` punish by posture | attack, inRange; opp 0x4C02 any; airborne(0x400) high/mid/s.mid (**juggle**); bit2 low/0x51F; crouching mid/low; standing low/mid/high |
| `0x800579A0` | same, no highs |
| `0x80057B60` | button-only commands (`cmd & 0x3FE0 == 0`), inRange |
| `0x800573F4` | non-high/non-throw attacks inRange (also used for "step in") |
| `0x800572A4` | general: dash (0xC001) or in-range attacks or always-in-range |
| `0x80057FB8` | **guard/evade** vs incoming: throw-type → crouch (posture&1, no attack) + backdash if his zone ≥3; s.mid → stand/crouch moves; mid → stand; low → crouch (+backdash); high → crouch if already crouching else stand (+1/50 crouch); throw 0x706 → crouch; closing fast (delta<−70) → backdash |
| `0x800587D8` | dash-in (zone 2: 1/128, zone 3: 1/4) + interrupt pool |
| `0x800613CC` (filter cb) | exclude low/0x10000 moves |

---

## 5. The 110-byte profile

### 5.1 Selection [V]
`profile = 0x800231AC + diff*1100 + level*110` copied to state+0x238 (diff ≤ 2, level ≤ 9).
Levels: **Arcade** = stage index 0..9 (10th = Ogre fight uses L9); Team/Survival computed (§10); practice-like
modes force L4. Mostly u16/s16; probabilities are /4096 tested as `(aiLCG & 0xFFF) < P`.

### 5.2 Field map
| Off | Type | Meaning | Where / how |
|---|---|---|---|
| 00..03 | 4×u8 | wake-up tap intervals (one picked at random) | lying with `'1'` options: every N frames tap 2 (pad 0x10) [V; effect G] |
| 04 | s16 | attack cooldown base (0 everywhere) | +0x68 = base + one of 06..0C after every attack [V] |
| 06..0C | 4×s16 | **attack cooldown choices (frames)** | EASY L0 {150,200,200,250} … HARD L9 {10,30,30,30} [V] |
| 0E | s16 | **max hit-confirm string length** | continue only while `+0x96 < P0E−1` [V] |
| 10 | s16 | **throw attempt prob** (zone ≤1, cooldown expired) | [V] |
| 12 | s16 | **throw-escape prob** (or always vs listed throws, §7.5) | [V] |
| 14 | s16 | **string continue prob** after a hit | [V] |
| 16 | s16 | throw *selection* prob (2nd stage) / King special prob | [V] |
| 18 / 1A | s16 | prob of "guard-aware attack" / "fast poke" pools in neutral; **both zeroed when the opponent's HP ≤ 10 %** | [V] |
| 1C / 1E | s16 | **guard prob vs a first attack** / learning cap of 1C and late-guard prob | [V] |
| 20 / 22 | s16 | **counter-attack (interrupt) prob** / cap; 22 also vs a move that already hit me (ring 1DC) | [V] |
| 24 | s16 | **sidestep prob** (vs highs at 9..11 frames) | [V] |
| 26 / 28 | s16 | in-string interrupt prob / cap | [V] |
| 2A / 2C | s16 | in-string guard prob / cap | [V] |
| 2E,30,34,38 | s16 | planned follow-up limits/probs — **only read by dead code** (§7.6) | [V] |
| 32 / 36 | s16 | consecutive-attack limit = max(32,36) → +0x1D4 (also dead-code counts) | [V] |
| 3A | s16 | evade unblockable/throw prob; per-character handler gate | [V] |
| 3C | s16 | pressure prob vs stunned (0x4C02) opponent | [V] |
| 46 | s16 | **0 in all 30 profiles** ⇒ every `rand ≥ P46` gate is always true | [V] |
| 48 | s16 | anti-air / jump-in-reaction prob | [V] |
| 4C | s16 | keep a chosen walk-back/backdash (else cancelled) | [V] |
| 5A | s16 | **parry / reversal prob** | [V] |
| 5E | s16 | counter-throw-startup prob (0 everywhere) | [V] |
| 62 | s16 | stun-recovery action prob (self in 0x4C02) | [V] |
| 4A, 4E, 50, 5C, 64 | | no reads found in AI code [V] |

The 30 profiles are converted to `assets/gamedata/ai.json` → `profiles.entries` (named fields per the map above).
Their shape: within each difficulty the cooldowns fall and the probabilities and limits rise with the level
(for example the cooldown set goes from {150, 200, 200, 250} frames at EASY L0 to {10, 30, 30, 30} at HARD L9),
EASY never guards or counters on purpose, and HARD starts where MEDIUM is in mid-game.

HARD L0 = MEDIUM L4, HARD L1 = MEDIUM L5, HARD L2 = MEDIUM L9 (identical rows) [V].

### 5.3 Run-time modification [V]
- Round start (`0x800593C8`): practice-like ⇒ 06..0C halved, 1C quartered. Initial cooldown +0x68 = P06;
  initial movement cooldown 30..59.
- **Learning when hit** (SELF+0xCE set, not practice): `P1C += 10 (≤P1E)`, `P20 += 10 (≤P22)`; if he is
  mid-string (+0x216 > 0) also `P2A += 40 (≤P2C)`, `P26 += 20 (≤P28)`. Per round (profile reloaded each round).
- **Rage** (not in practice/Force): on HP loss, the opponent's current move is pushed into an 8-entry ring
  (+0x1DC). If that move was already in the ring ≥2 times (the 3rd hit by the same move), the profile is
  reloaded with `level = 9` (same difficulty) for **600 frames** and +0x202 is set (extra counters/sidesteps);
  when the timer ends the normal profile is reloaded. Every hit also gives cooldown −3.
- Opponent HP ≤ 10 %: P18, P1A, P20, P22 zeroed (stops poking/countering, i.e. "lets you finish"?) [G intent].

---

## 6. Per-character range thresholds (`0x80098260`, 22 × 6 u16, index = actor+0x18 char id) [V]

Six u16 per character (index = actor+0x18); most characters share one row, a few big or small ones differ.
The values are in `assets/gamedata/ai.json`.

Meaning (computed every frame, `d` = actor distance +0xF8):
- **[0..3] = this character's reach boundaries** (close / mid / long / max). [5] = body-size offset added to the
  *other* side's distance (0 for everyone). [4] (0x700) is **never read** by the AI.
- My zone `+0x14`: `r = d − opp[5] − 0x80`; `1` if `r < my[0]` (0 instead if either actor has nonzero
  +0x3E8/+0x3EC/+0x3F0 [G: being pushed]), `2` if `< my[1]`, `3` if `< my[2]`, `4` if `< my[3]`, else `5`
  (zones 4/5 add a random 0..2/0..3 to +0x2A).
- His zone `+0x18`: `r = d − my[5] (+delta if closing)`; `1` if `r < opp[0]+0x80`, `2` `< opp[1]+0x100`,
  `3` `< opp[2]+0x200`, `4` `< opp[3]+0x280`, else `5`.
- Zone → usable move ranges: `0x80098230` (§4.5). Many decisions also use the hard distance **0xB87**
  (≈ his longest reach): above it the AI ignores his attacks.

---

## 7. Decision loop (`0x80059890`), in execution order

Each frame the routine produces one pad word. It takes the first of these steps that produces an output, in
this order:

1. **Setup:** pick the state block and the opponent; in Tekken Ball call the Ball overlay's hook; update the
   bookkeeping (§7.0).
2. **Canned input:** while a canned sequence is playing, output its next word.
3. **AI modes:** mode 0 outputs nothing (idle dummy); mode 1 outputs a fixed hold derived from the posture
   [G dummy].
4. **Hold:** while practice is paused, or Force's hold counter runs, repeat last frame's direction bits only.
5. **Hit-confirm strings** (§7.2) and, in the same step, the throw escape (§7.5).
6. **Stun recovery:** when stunned (posture 0x4C02), with probability P62.
7. **Wake-up** (§7.4).
8. **Per-character handlers** (§8): the opponent's moveset handler, then the own one, gated by P3A.
9. **Attack-now / already-safe:** if the opponent's attack connected, an "attack now" flag may be raised (with
   probability P20 plus a bonus growing with his string length and the difficulty). Otherwise, when a punish is
   being timed or he is idle, and the AI is already safe against his move, it just holds its direction.
10. **Planned follow-ups** are collected (their execution is dead code, §7.6).
11. **Candidates** are built from the current cancel list (§4.3). After 220 frames in one move (a stance) any
    candidate is taken. Tekken Ball gets a second hook here.
12. **Stance continuation**, then **reactions inside his string** (§7.1b).
13. **Reaction to his first attack** (§7.1a): the rule table chooses one of nine reactions, or none.
14. **Neutral offense** (§7.3).
15. **Movement** (§7.7).

After the output is chosen the timers count down, learning is applied (§5.3), and the held / newly pressed bits
are written (pressed = held minus last frame's held).

### 7.0 Bookkeeping [V]
Distance/zones (§6), +0x20A (frames until his active frame), +0x20C (his attack incoming),
+0x216 (his attack counter), +0x080/+0x084 (start-up frame counters), +0x6A "already safe":
his 0x412 high & I'm crouching (0x2021/0x2829); 0x217 mid & I'm standing 0x1052; 0x10F low & crouching;
0x31F s.mid & standing or crouching; 0x706 throw & crouching; 0x607 unblockable → never safe.
Crouch-timer: while crouching, +0x4A = 20..42 random (holds d).

### 7.1 Reacting to the opponent's attack

**(a) First attack of a sequence** (`+0x216 < 0`, i.e. none of his current moves has reached its active frame
yet). `choose_reaction()` picks the first matching rule [V]:

| # | Condition | Reaction |
|---|---|---|
| — | he has passed his active window, or SELF+0x40==2 | evade plan = 2 (too late) |
| — | his move is 0x607 (unblockable), 0x706 (throw) or rec+4 0x200000, and rand < P3A | evade plan = 1 |
| 3 | evade plan ∉ {0,2} and +0x8C < 0 | **evade** (plan 1 → 3 if my zone < 3, else random from `0x80022E80` = {5,3,5,4,3,4,3}; see below) |
| 5 | +0x20A ∈ 8..9, I'm not attacking, my moveset has the parry flag (`0x80022D14[m].w0`), his move parryable (rec+0x24 bit 0x800 clear), he's not airborne, not already safe, his range reaches me, rand < P5A | **parry/reversal**: pool = my cancel entries whose dest alias is 0x1EE or 0x6A3 (b+1+3 / b+2+4 for Paul, Nina, King, Jin, Anna…) |
| 6 | Jin (moveset 9), +0x20A ∈ 22..26, same tests, rand < P5A | pool = dest alias 0x22C |
| 7 | +0x20A ∈ 4..10, high/mid/UB/throw, moveset has `+4` sequence, rand < P5A | canned **high/mid reversal** sequence |
| 8 | same window, low (0x10F), moveset has `+8` sequence | canned **low parry** sequence |
| 1 | he is starting a throw (rec+8 0x400000) whose alias ∈ {0x189,0x18E,0x17F,0x181, Jin/Paul 0x172, King 0x152}, rand < P3A | interrupt (attack him) |
| 2 | his throw-type move, +0x20A ≤ 29, rand < P5E (=0) | guard (never fires) |
| 2 | he is attacking (rec+8 ≠ 0, ≠0x607) and not already safe and I'm not attacking: if +0x92 latched, or **rand < P1C**, or (+0x20E > 30 and rand < P1E) | **guard** (+0x92 = 5) |
| 1 | 1/32 if +0x6C>8; 1/16 in rage; his move == ring slot 0 (a move that hit me) and rand < P22; or rand < P20 (not vs the Yoshimitsu special) | **counter-attack** (+ 1/64 vs highs → reaction 9 duck) |
| 4 | (+0x20E < 31 or rand ≥ P22), +0x20A ∈ 9..11, facing (<0x4000), not throwing, his zone == 2, I'm not crouching/airborne/attacking, his move high (0x412), rand < P24 | **sidestep** |
| 0 | otherwise | fall through to neutral |

Reaction bodies [V]:
1. pool = fast pokes if he is not actually attacking; vs UB/throw/0x200001 posture: interrupt-no-highs; else
   interrupt (`start-up < +0x20A`); vs highs also add crouching attacks (rec+4 bit0 attacks in range). Pick.
2. `0x80057FB8` (crouch/stand/backdash pool by attack level, §4.5); if empty: **hold d/b (0xC000)**, or
   **back (0x8000)** if his attack is a mid (0x217); nothing if SELF+0x32 ≠ 0 [G airborne].
3. evade vs unblockable / throw (does nothing if +0x21A; distance −300 vs Yoshimitsu; plans 3..6 give up when
   distance > 0xB86):
   - plan 3: if he is ≥29 f away from hitting: attack in his start-up (step-in pool `0x800573F4`, cooldown −1)
     if my zone < 3, else walk forward on even frames; 9..28 f: step-in pool, or switch to plan 4 + walk back;
     < 9 f: walk back (or nothing);
   - plan 4: dash in (canned n,f,n,f `0x80022C46`) when < 9 f or my zone > 1, else step-in pool; walk forward if
     SELF+0x3E > 0x4000 and his zone ≥ 2;
   - plan 5: 1/8 sidestep (when ≥ 7 f left and not throwing), otherwise walk forward;
   - plan 6: < 29 f: dash in (his zone > 1 or < 8 f) or **jump forward u/f (0x3000) for 20 f**; else walk back.
4. sidestep `0x80061A58(-1)` and cooldown −10.
5/6. parry pools; 7/8 canned sequences; 9 crouch-move pool.
After any attack pick: movement timers reset; if pick failed, the frame continues to neutral.

Note there is **no explicit "guard delay"**: the timing is implicit — reactions only exist in the window
+0x20A (frames until his hit) ∈ 2..29, sidestep 9..11, parry 8..9/4..10; guard is a pad hold that the engine
turns into a block if it is held when the hit arrives. Plain neutral (no input) also blocks highs/mids in
Tekken, which is why EASY (P1C = 0) still blocks often.

**(b) Inside his string** (`+0x20C && +0x216 ≥ 0 && hisZone < 4`) [V]:
`if (f8F or rand < P26) and 2 ≤ +0x20A < 30`: interrupt pool → attack.
`elif I'm not attacking and he is attacking and (f92 or rand < P2A)`: hold d/b (or back vs mid).
`elif rand < P24 and +0x20A ∈ 9..11 and facing and not throwing and hisZone ≥ 2`: sidestep, cooldown −10.

### 7.2 Hit-confirm strings [V]
When `SELF+0x74 > 0` (chain counter: my last chain move connected [G]) and my move changed:
if `+0x96 ≥ 0 && +0x96 < P0E−1` and rand < **P14**: collect ≤6 entries of my current cancel list passing
§4.2 (special per-alias rules for 0x3A7 (never), 0xD41/0xD24/0xD32/0xD25/0xD34/0xDE2 depending on the
opponent's moveset — Yoshimitsu, Paul, Heihachi — and a coin flip), choose one, remember it, and fire it when
`SELF.frame == entry.win_start + 1` (earliest buffer frame). Else stop (+0x96 = −1).

### 7.3 Neutral offense (`LAB_8005E304` …) [V]
**Gates.** While a crouch hold (+0x4C) or a crouch-dash setup (+0x44 ≥ 5) runs, the AI just holds its
direction. It goes to movement instead of attacking unless it is within reach (my zone < 4), the opponent is not
far behind, and it has not used up its consecutive-attack limit (+0x1D2 < +0x1D4). Even then it mostly approaches
first: from zone 3 or more it moves 11 times in 12, from zone 2 once in 8. While running it attacks only when no
punish is being timed and the opponent is idle or far; otherwise it stops the run by holding back (guard).

**Timing.** The attack cooldown (+0x68, §4.4) must have expired, with these overrides:
- when the opponent has been in start-up for 26+ frames, a delayed-punish timer (+0x6C) is armed to fire a few
  frames (4–11) before his move becomes active; while it counts down the AI chooses nothing;
- the "attack now" flags (§7, step 9), a long opponent start-up, or its own early start-up force the cooldown to
  expire;
- an anti-air chance (P48) when he is in the second half of a jump, facing, at zone 2;
- the cooldown shrinks by 2 (with P20) when the opponent is in his ki charge (alias 6) and my zone < 3.

Throws are ruled out (throw probability 0) when he is guarding, airborne, unthrowable, Gon, or about to hit
within 10 frames.

**Choice of pool**, first match:
1. *Opponent down* (+0x208): a random spread of down-attacks (cond `'5'`, about 20 %), sidestep, ground-hitting
   attacks, crouch, backdash, a canned okizeme jump attack at mid range, otherwise lows (movement moves when he is
   behind).
2. *Opponent stunned* (0x4C02), with P3C: an occasional dash or a ground-hitting attack, then one of the two
   punish-by-posture pools (§4.5).
3. *A jump attack is pending* (+0x46 ≥ 0): jump-in moves.
4. *He is in a rec+8 bit-0x80000 move* (+0x20E) or *attacking / throwing*: the counter pool (with the attack-now flag
   or P20), otherwise the step-in or general pool.
5. *Cooldown expired:* when not in a crouch-dash setup: a throw attempt (throw probability, my zone < 2, not vs
   Gon; P16 then picks from the throw pool, falling back to button-only throws), else the moveset's combo table
   (§9, under its own cooldown, standing, close and facing), else by a random draw: crouch moves (about 22 %),
   a dash/hop or punish-by-posture pick (about 28 %), or the neutral mix — a 1/8 crouch-dash setup, then the
   guard-aware pool (P18), fast pokes (P1A), button-only commands, a jump attack (1/8), rare non-attack
   specials, or the general pool. During a crouch-dash setup it picks hop moves.
6. *Desperation:* if nothing was chosen, he is out of reach, the AI's own health is at most 10 % and a 1/4 roll
   and P20 pass, the "desperation" moves.

The pick goes through §4.4. A string starter, a jump or a crouching move also arms the matching follow-up state
(+0xC0, +0xD8, or a 12-frame crouch hold).

### 7.4 Wake-up / lying [V]
If I'm lying (`rec+4 & 0x404`, SELF+0xDB==0):
- if my cancel list contains `'1'` entries: every `P[00..03]` frames tap 2 (0x10) [G: get up / stay down rhythm];
- else candidates (`endLinks=1`), then a category is drawn uniformly from `0x80022E74 = {1,2,4,8,0x10,0x20}`
  until it intersects an allowed mask (he isn't attacking: zone >2 ⇒ {1,2,4}, his zone <4 ⇒ all; coin flip;
  1/64 ⇒ {8,0x10,0x20}; he is attacking soon (+0x20A<12) ⇒ mid/low dependent):
  1 = up (0x1000, side roll), 2 = 1 or d+1 (0x80/0x4080), 4 = back (0x8000; Dr.B forward), 8 = general pool,
  0x10 = get-up attacks (0x10000 or low 0x10F), 0x20 = dash/spring moves. +0x8A = 16-frame lock.

### 7.5 Throw escape [V]
When `SELF+0x74 < 0` and my move changed: if the opponent's alias ∈ {0x189,0x18E,0x17F,0x181, Jin/Paul 0x172,
King 0x152} or rand < **P12**: pick a random entry of my current cancel list (≤48); cond `0x3A`/`0x3C` → press
**1**, `0x3B` → press **2**, else that entry's cmd. (Paul's lists: cond 0x3A..0x3C → common aliases 0xB52..0xB55,
the escape animations.)

### 7.6 Dead code: planned follow-ups [V]
After a string-starter / jump / some moves, the AI collects up to 4/48/10 follow-up entries (+0xC4/+0xDC/+0x1A4
lists, profile 0x2E..0x38 control count/probability). The executor at `0x8005D4BC..0x8005DEE0` filters them
into `0x800A01E8` but tests the count register `$s7`, which is set to 0 and **never incremented**, so the
three executors never fire in the retail game; the only effect is that the AI holds its direction on those
frames. A reimplementation that wants retail behaviour must keep this disabled.

### 7.7 Movement [V]
Only when no attack was chosen and movement cooldowns (+0x40, +0x48) expired. `u = randint(4096)`:
- Nothing at all if +0x200 (special Ogre move) is set.
- If cooldown +0x68 < 36 (attack soon), by **my zone**: z≤1: u<20 → walk back; z2: u<122 long walk forward,
  <491 dash, <532 walk back, <P24·4096/1000 sidestep; z3: <1638 short walk forward, <2867 dash; z4: <204 short
  walk forward, <1433 run, else dash; z5: run, except dash when +0x2A < 721, opponent not down and u > 285.
- Else by **his zone**: z≤1: u<409 backdash, <819 walk back; z2: <40 walk forward, <368 walk back;
  z3: <491 walk forward, <532 long walk forward, <536 walk back, <557 dash, <577 sidestep (if my posture bit1);
  z4: <128 walk forward, <144 long walk forward, <192 sidestep, <960 dash; z5: (if +0x2A<721, not okizeme,
  u>80) <204 walk forward, <1024 dash, <1515 long walk forward; otherwise run.
  In this branch a chosen walk-back/backdash is dropped unless rand < P4C. (A further cancel gated by
  `rand < P46` never fires.) Everything not listed = mode 0 (no movement).
- Modes (table `0x80022F80`): 1 dash (cmd 0xC001 pool, else 2), 2 walk forward 4..11/10..19/15..29 f
  (d/f if crouching), 3 walk back 20..24 f, 4 run (0xC001 with +0x3A=2, 250/310 f), 5 walk forward 40..59 f,
  6 backdash (0xC002 pool, else 3), 8 sidestep if his zone > 2 or +0x20A > 10.
- Default when idle: hold d if crouch timer running, else keep previous direction while +0x48 ≥ 0, else
  **neutral (0)**.
- Running (+0x3A==2): keep forward; within his zone <3 1/128 chance to stop and back.
- Angle correction: if +0x2E > 0x1000 with probability +0x2E/32768: walk 8 frames (Xiaoyu: forward/back by
  angle; others forward, back if facing away in close range).

---

## 8. Per-character handlers (`0x80023134`, 10 × {u16 moveset, fn vsThatMoveset, fn whenISelf}) [V]
`state+0x74` = handler for the opponent's moveset, `+0x78` = handler for my own moveset; called by
`0x800591DC` when +0x8A < 0 and rand < **P3A**. Return −1 nothing, −2 handled (sequence started),
−3 pick pool, 0x2000/0x8000 walk 16 f, other = pad word.

| Moveset | vs-it handler | self handler |
|---|---|---|
| 0 Paul | `0x800628C4`: his alias 0x2C7 and his zone < 3 → walk back if his frame > 15, else attack now (cooldown −1) | — |
| 3 King | — | `0x8006251C`: with P16, vs standing opponent ≤ zone 2: canned throws `0x800230DE` (f,d/f+1+3 / f,d/f+2+4 / f,d/f+1+2 — needs walk alias 0x15 available); vs crouching: d/b+1+3, d/b+2+4 (`0x800230A6`); downed: d+2+4 (`0x800230C2`) |
| 4 Yoshimitsu | `0x80061B88`: reacts to about a dozen of his special-move aliases (sidestep / dash / back / filter lows) | `0x80062994` (none) |
| 6 Hwoarang | — | `0x8006299C`: in alias 0xDE3 (stance) → forward 15/16 |
| 10, 13, 16 | stubs returning −1 | — |
| 14 Ogre/True Ogre | `0x80061EF0`: vs alias 0xDE4 and 0x16E: sidestep side by stage/angle, back off, punish with random d/f+2 / d/f+1 / d/f+4 (`0x8002309C`) | — |
| 19 Gon | `0x80062940`: vs alias 0x137: attack now if his zone <2, back off if ≥4 | — |

## 9. Per-moveset AI table `0x80022D14` (22 × 16 B) [V]
`{s16 canParry, u16 pad, ptr highReversalSeq, ptr lowParrySeq, ptr comboList}`; sequences are u16 words
(`dir | btn<<8`) played one per frame. Dump: `seqdump.py`.

Some movesets can parry, some have a reversal or low-parry input sequence (e.g. `d, d+1+3`), and a few have a
combo list of motion preludes (quarter circle, crouch dash). The table is in `assets/gamedata/ai.json`.

Combo-list use: entries are tried in order with `rand < prob`; the first whose alias is reachable from my
current cancel list (`0x80058BE8`) is played; cooldown +0x42 = 28..43. These are *motion preludes*
(qcf / crouch-dash): the actual attack is then picked from the moves those motions unlock.
Other canned sequences: sidestep `0x8002306E` (n, d×4, n×5) / `0x80023086` (n, u×4, n×5), dash
`0x80022C46` (n,f,n,f,n,n), backdash `0x80022C3E`, OTG `0x80022C2A` (u/f, u/f+2).

No other per-character AI move tables exist: move choice is entirely from the cancel lists + flags (§4).

---

## 10. CPU opponent selection and level (overlay record 5, slot A `0x800B0548`)

### 10.1 Arcade `0x800B1FCC(out[10×4], playerSel=char*4+costume)` [V]
- **Boss:** Heihachi (character 13); Jin (9) when the player is Heihachi.
- **Pool:** the unlocked characters (`0x80097EF0`, 21 bits; battle type 3 [G: Time Attack] uses a narrower mask),
  plus Gon when flag `0x80097F1E` is set, minus Heihachi, Ogre, True Ogre, the player's character and the boss.
- **Stages 1–4:** four distinct random picks (a Fisher–Yates shuffle driven by `0x8004CE54` modulo the count)
  from the pool's original ten characters; **stages 5–8:** four more from what remains (which may include
  unlocked hidden characters); **stage 9:** the boss; **stage 10:** Ogre (14).
- **Entry:** `{character, costume, stage index}`; the costume is 0 for Ogre / True Ogre, otherwise 1 when the
  player's costume is non-zero.
- **AI level** (`0x800AE211`) = the stage index (0…9).

True Ogre is not in the table; the Ogre→True Ogre change happens elsewhere [open].

### 10.2 Team Battle `0x800B225C(isCPU, team, otherTeam)` [V]
Team struct: `[0..9]` members (char*4+costume), `[10]` already chosen, `[11]` costume, `[12]` size.
Pool = unlocked & 0x1F7FFF minus already chosen. If the CPU side has >2 members: sort by the per-character
record counter `0x80051660(c)` and take one of the top two at random. Fill the rest from the least-picked
characters (CPU-pick counters `0x800AFAFC[22]`, choose uniformly among those whose count ≤ min + k so the
pool is large enough); if the other team already has the same char+costume, flip the costume; counters ++.
Level per CPU fight (`0x800B09B0`): `diff + 3 + (0x800AE218 & 3)`, +1 for the CPU's second-to-last member,
+3 for its last member, clamp 9 (0 when no CPU side).

### 10.3 Survival `0x800B26D0(state)` + level `0x800B0B84` [V]
- **Pool by wins** (state +0x24): the original ten below 7 wins, a wider set below 17, everyone from 17; always
  without Mokujin, Gon and Dr. B (Ogre and True Ogre only from 17 wins). The last 4 opponents (a ring at
  state +0x50) are removed.
- **Pick:** uniformly among the least-used characters (counters at state +0x60, with a slack of 4 once the
  streak passes 19); the costume comes from bit 0 of `0x800AE218`.
- **Difficulty and level by streak** (state +0x44): difficulty is set to MEDIUM, and the level is drawn from a
  range that widens with the streak (1–3 below 5 wins, up to 3–7 below 50). From 20 wins some fights are raised
  to HARD (from bits of `0x800AE218`); from 50, every tenth fight is level 0, fights ending in 3 or 8 are 6–9, the
  rest 3–9. Level ≤ 9, difficulty ≤ 2. The rules are also in `assets/gamedata/ai.json` → `survival`.

(`0x800AE218` = a global counter used as a random source [G].) The unlock mask is *not* applied in Survival.

### 10.4 Tekken Force (battle type 8) [V]
Profile level clamped to 3; `+0x32C → 0x80022FA4 + diff*30 + level*6` = {s16 attack interval, s16 cooldown,
s16 prob}. Higher difficulty and level mean shorter intervals and cooldowns (EASY level 0 waits 180 frames
between attacks). Values: `assets/gamedata/ai.json` → `force_ai`.

---

## 11. Reimplementation checklist
1. Port the two RNGs exactly (AI LCG `x*5+3` + `rand()`), keep their call order if bit-exactness matters.
2. Per round: §8 handlers, profile load (§5), round-start tweaks; per frame: §7 order.
3. Candidate machinery (§4) operates on the real cancel lists — needs `0x8002E0BC` conditions and the
   window/req checks shared with the human parser.
4. Keep §7.6 disabled; keep P46 = 0 semantics (all those gates pass).
5. Opponent selection §10.

## 12. Open items
- Exact semantics of actor fields read here: +0x32, +0x3E, +0x40, +0x74, +0x87, +0xCE, +0xD2, +0xDB, +0xDF,
  +0x122, +0x994, +0x1278 (named by usage only).
- Battle type 3 identity; where Ogre becomes True Ogre in Arcade.
- Tekken Ball hooks (`0x800B4494…`) live in the Ball overlay, not decoded.
- cond 0x31 ('1') lying options: confirm the "tap 2" meaning in play.

The 30 profiles' values are not reproduced here: the converter reads them from your disc into
`assets/gamedata/ai.json` (field map in §5.2).
