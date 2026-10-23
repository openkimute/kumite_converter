# Tekken 3 (SLUS-00402): the fight camera

> Detailed research spec, part of [engine.md](engine.md). Derived from the EXE and disc data only.
> Helper scripts named below were research tools and are not in the repository, except the reference
> implementations `t3_converter/src/reference/attackpath.rs` and `reference/camera` (they read the disc image).

Source: the EXE disassembly. Helper dumps are in `gap4/`: `cam_module.asm` (0x80062000–0x8006A400, annotated) and
`ko.asm` (0x80045EE8–0x80046D00). The prototype is `t3_converter/src/reference/camera.rs`; the engine port's
version is OpenKumite's `crates/kumite_sim/src/camera.rs` (sine/atan tables exported to `gamedata/camera.json` → `tables`).

Tags: **[C]** read from code, **[I]** inferred from code, **[G]** guess.

Conventions [C]:
- World Y points down. The floor is 0.
- A standing fighter's "chest" bone (player +0x994, bone slot 2 TY) is about −1425.
- Angles use 4096 units per turn. Inside the camera module, pitch and yaw are kept **shifted left by 6**
  ("<<6 units", 0x40000 per turn). They are wrapped to 18 bits when subtracted: `wrap18(v)`.
- The view direction for (pitch p, yaw y) is `f = (−sin y·cos p, sin p, cos y·cos p)`.
  - yaw 0 looks along +Z;
  - positive pitch looks down.
- Divisions truncate toward zero (MIPS `div`, plus the `bgez/addiu 2^n−1/sra n` idiom).
- `ratan2(x, y)` at `0x8004B39C` returns atan2(y, x) in 0…4096, using a 1025-byte table at `0x800210C8`.
- The sine table is at `0x8001E8C4` (5120 × s16); cos(i) = sin(i + 1024).
- isqrt is at `0x8002A324`.

---

## 0. Corrections to earlier notes

| Earlier note | Actual |
|---|---|
| `0x80069414` is "the fight camera controller" | It is the **replay camera** [C]. The fight handler `0x8002AE58` calls it only when the round state machine `0x8003CB84` returns non-zero (replay playback, driven by `0x80032178`/`0x800325E4`, which call its init `0x8006953C`). |
| "samples both players every 4th frame" | Only the replay camera does that (`0x80069304`). The fight camera samples every frame (`0x8006465C`). |
| `0x800630D8` = "stage-7 (Tekken Force) camera" | `0x800630D8` is the **camera director for every battle type** [C]. Battle type `0x800AFA88` = 8 diverts to an overlay camera (`0x800B4F78`/`0x800B5020`), which is Force per engine.md. Type 7 (Ball) uses the normal camera with a third framing point. |
| "eases distance toward 0x113D" | That is replay only. The fight camera uses a fixed framing depth of **0x158D = 5517**. |
| "zoom = projection H, clamp 0x800A8CD6/8" | Correct, but in the EXE `0x80063CBC` sets **H = min = max = 500**, and nothing else writes the limits (checked the EXE and overlays 5–8). **H is constant 500.** "Zoom" is done by moving the eye. Scripted camera motions convert their authored H into a dolly (§4.3). |
| `0x80064170` "blend (guess)" | Confirmed: an orbit-interpolating key blend (§3). |

---

## 1. Data structures

### 1.1 CAM block `0x800A8C70` [C] (offsets used by the director)

| Off | Meaning |
|---|---|
| +0x00 | director state within the fight phase (1…8, §4) |
| +0x02 | init flag for battle type 6 |
| +0x04 | enable |
| +0x06 | intro countdown |
| +0x08…+0x10 | **weights w0…w4** (4096 = 1) for key slots 0…4 |
| +0x12…+0x1A | previous-frame weights |
| +0x1C | specials allowed (1 for battle types 0–5; 0 for 6–8) |
| +0x1E | throw side (index of the thrower) |
| +0x20 | orientation flag (mirror the yaw offset) |
| +0x22 | sub-timer |
| +0x24 / +0x26 | blend counter / blend length |
| +0x28 | throw-side latch |
| +0x2A | cut flag |
| +0x2C / +0x2E | hold counter / length |
| +0x30 | selected camera ref (−1 = none) |
| +0x34 | camera type: <0 = scripted motion, 1…30 = situation type |
| +0x36 | random pick |
| +0x38 | move pointer that triggered it |
| +0x3C / +0x40 | slot-1 pitch / yaw (<<6) |
| +0x44 / +0x48 / +0x4A | camera-motion pointer / length / frame |
| +0x4C | mirror |
| +0x54…+0x5C | scripted-camera anchor position |
| **+0x64** | **projection H (500)** |
| +0x66 / +0x68 | H min / max (500 / 500) |
| +0x6A | "dolly instead of zoom" (1) |
| +0x6C | stage camera variant (0 in the EXE; overlays may set it) [G] |
| +0x6E | |
| +0x70 | player index sampled on odd frames only (−1) |

### 1.2 Other structures

- **Key slots** `0x800A02A8`, 5 × 24 bytes: `{eye.x, eye.y, eye.z, pitch<<6, yaw<<6, H}` [C].

  | Slot | Use |
  |---|---|
  | 0 | normal fight camera |
  | 1 | situation / throw "angle" camera, and the replay camera |
  | 2 | scripted camera motion and round intro |
  | 3 | the last applied (output) key |
  | 4 | frozen snapshot, used for cut-backs |

  - `0x80063FD4(dst, n)` loads slot n.
  - `0x80064020(key, n)` stores a key into slot n.
  - `0x8006460C(n)` copies slot n into slot 4.
  - `0x80064080(n)` applies slot n: copies it to slot 3, sets `RVIEW.eye = key.eye`,
    `RVIEW.pitch = key.p>>6`, `RVIEW.yaw = key.y>>6`, then calls `0x80063C64(key.H)`. That function clamps H to
    [CAM+0x66, CAM+0x68] and calls GsSetProjection `0x80081238`.
- **Slot state** `0x800A0328 + 0x30·slot` [C]:

  | Off | Meaning |
  |---|---|
  | +0 | yaw chase state |
  | +2 | "moving" flag (slot 0) |
  | +8 | settle counter |
  | +0xC | yaw velocity |
  | +0x10 | yaw acceleration |
  | +0x14 | pitch lag (slot-0 pitch) |
  | +0x18 | pitch-lag target |
  | +0x1C | pitch step |
  | +0x20 | \|yaw error\| |
  | +0x24 | vertical pull step |
  | +0x28 | vertical pull |
  | +0x2C | horizontal pull |

  Global `0x80098374` is the slot-1 vertical offset.
- **Samples** `0x800ADCA8`: 2 × {x, y, z, side}, with the previous copy at +0x30 [C].

---

## 2. The per-frame normal fight camera (slot 0)

Call chain each frame [C]: `0x8002AE58` → `0x800630D8(P1, P2, &phase 0x8009546C)`. Then:

- **phase 2** (fighting):
  1. `0x8006465C` samples.
  2. The director state machine runs (§4).
  3. If w1 > 0: update slot 1.
  4. If w0 > 0: `0x80066084(P1, P2, snap = (prev w0 < 1), 0x158D)`.
  5. Mixer `0x80062B74`.
- After the camera: shake `0x8004AD34`, then `0x80048240`, which builds the matrix from RVIEW (`0x80045CA8`).

`0x80066084` runs the yaw controller `0x80064810` for slot 0, then the framing solver `0x80064E10` with the
slot's pitch lag, the new yaw, the snap flag and depth 5517.

### 2.1 Sampling `0x8006465C(P1, P2, force)` [C]

Each frame every fighter is reduced to one sample `{x, y, z, side}`; last frame's samples are kept beside them.

- **Horizontal position.** The root bone (player +0xF68 / +0xF70) when the current move's flag word (+0x24) has
  bit 29, when the player's +0x74 (throw / special-move state [G]) is non-zero, or when `force` is set (it is also
  set when either player has +0x74 ≠ 0). Otherwise the logical position (+0x00 / +0x08).
- **Height.** The chest bone's Y (+0x994) raised by 290, about −1715 for a standing fighter. One special case
  (+0xA0 = 2 with moveset key 4) uses a fixed −1720 [G].
- **Side.** The pelvis's screen X (+0x1278), written when the fighter is drawn (`0x80037924` projects bone 11);
  the round init (`0x8002BE28`) seeds −100 / +100.
- **Skipping.** The player named by CAM+0x70 (normally −1, i.e. nobody) keeps its old sample on frames whose
  frame-in-move (+0x58) is even.
- If both samples land on the same XZ point they are pushed one unit apart.

### 2.2 Yaw controller `0x80064810` (slot 0) [C]

Inputs: both samples, the slot-0 key, the slot state and the snap flag. Output: the new yaw (<<6 units) and the
snap flag; as a side effect it updates the slot-0 pitch lag.

1. **Target.** The axis is the `ratan2` direction from P1's sample to P2's. Unless P1's side value is the smaller
   one, the axis is turned by half a turn, so the fighter who started on the left is kept on the left. The
   target yaw is the axis in <<6 units, and the error `d` is `wrap18(target − key0.yaw)`.
2. **Snap.** With the snap flag the target is returned as is, and velocity, acceleration and pitch lag are reset.
3. **Freeze.** The yaw is returned unchanged when the slot is not 0, when |d| exceeds 150° (0x1AAAA), or while
   the frame counter P1+0xF8 (`0x800AFA3C`, probably frames since FIGHT [G]) is below 200.
4. **Chase.** A two-state, second-order controller:
   - *Idle:* as soon as |d| reaches 5° (0xE38) it switches to chasing, with an acceleration kick of 0x600 towards
     the target.
   - *Chasing, |d| ≥ 20° (0x38E4):* the acceleration grows by 0x600 per frame towards the target.
   - *Chasing, |d| < 20°:* braking. Each frame the acceleration is halved, the velocity loses 14/128 of itself,
     and the acceleration then gets a smaller 0x300 push towards the target. Under 5° the acceleration is halved
     once more; after 32 consecutive frames under 5° the velocity is zeroed and the controller goes idle.
   - The velocity accumulates the acceleration and is capped at ±0xDDD0. A move with camera id 0x27…0x2A on
     either fighter replaces the cap with a smaller per-id value (scaled by 56784/256; the values are in
     `camera.json` → `fight.yaw.move_caps`). Hitting the cap backs the acceleration off by 0x600.
   - The yaw advances by velocity / 64, rounded toward zero.
5. **Pitch lag (slot 0 only).** When |d| is past about 20° (0x38E3), the target pitch is
   `sin(i) · 279616 / 4096` (<<6 units), where the sine-table index `i` is the excess over 20° divided by about
   3186 (a reciprocal multiply). At the 150° freeze limit that is about 200 angle units (≈ 17.6°). Below 20° the
   target is 0. While the slot's "moving" flag is set, the pitch eases toward the target by 1/16 of the gap per
   frame, at most 0x100.

What this means:
- The yaw is a second-order chase.
- The maximum turn rate is 0xDDD0/4096 ≈ 13.9 angle units per frame ≈ 1.2°/frame (73°/s).
- Moves with camera id 0x27…0x2A raise the cap to about 2.4°…0.2°/frame.
- **Pitch is 0 in normal play.** It tilts down (up to about 17°) only while the yaw lags by more than 20° and the
  eye is moving faster than about 31.6 units/frame. The "moving" flag is set by `0x80064E10`.

### 2.3 Framing solver `0x80064E10(P1, P2, pitch6, yaw6, flag, dist, slot)` [C]

Constants: H = 500, dist ≥ 800 (5517 for slot 0). Inputs: both samples, the wanted pitch and yaw, the snap
flag, the framing depth `dist` and the slot. Output: a new key for the slot (eye, pitch, yaw, H). The solver
works out a view-space correction vector `D` for the eye in four passes, then rotates `D` back to world space
(through the pitch, then the yaw) and adds it to the eye. All divisions truncate.

1. **Base eye.** On a snap the eye goes straight behind the fighters' XZ midpoint, `dist` units back along the
   new yaw. Otherwise the old eye is orbited about the new midpoint by the yaw change (new yaw − old key yaw).
   The eye height is the old key's.
2. **Depth.** Both fighters are moved into view space (X right, Y down, Z depth). The centre point — their mean
   horizontal position at world height −450 — is pushed to depth `dist`: the eye moves along Z by the
   difference, and the fighters' depths shift with it.
3. **Horizontal fit.** Each fighter is widened by 150 units on its outer side, and the screen X of the left and
   right edges is taken (`X·H/Z`).
   - *Too wide* (the edges more than 280 px apart): the eye must retreat. The depth at which the pair spans
     exactly 280 px gives a target "horizontal pull" (negative = backwards). Without a snap the stored pull eases
     toward that target, by 1/32 of the gap per frame when coming in and by 1/8 when going out. The pull is
     added to `D.z`, and `D.x` re-centres the pair at 1/8 easing.
   - *Fits:* when snapping, or when the edges are more than 96 px apart (48 px either side), `D.x` aims at the
     perspective midpoint of the pair at 1/8 easing. When they are closer, nothing happens while both stay inside
     ±48 px; an edge outside that dead zone is pulled back to it at 1/8 easing. The stored horizontal pull decays
     toward 0 (by 1/32 of itself when negative, 1/8 when positive; reset to 0 on a snap) and is still applied.
4. **Vertical fit.** The "top" fighter is the one higher on screen.
   - The centre-point drop `a3` is `cos(pitch)·(t1 − 450·H/dist)/4096` pixels, with `t1` = 100 (128 for a
     slot-1 shot pitched up), i.e. 60 px for slot 0 at pitch 0.
   - Slot 1 with a downward pitch adds a height offset `a2` from the top fighter's height, `(y·cos p + 1000)·368/640`
     when that is at least −1000, rate-limited to ±28 per frame through `0x80098374`.
   - A "vertical pull" is the extra depth needed to keep the top fighter at least 112 px above the centre (never
     negative). Without a snap it eases: out by 1/8 of the gap (at most 800 per frame), in by 1/16 (at most 1200).
     A positive pull moves the eye back; for slot 0 it also deepens `a3` slightly (pull·cos p·H/dist/16).
   - `D.y` then puts the centre point `a3` pixels below the screen centre, plus `a2`.
5. **Result.** The key becomes `{eye + world(D), pitch, yaw, H}`. For slot 0 without a snap, the "moving" flag
   is set when the eye moved more than about 31.6 units (squared XZ step > 1000).

Ball (type 7) adds a third framing point taken from overlay `0x800B4484` [I]:
- ±150 margin;
- it can become the vertical "top".

### 2.4 Resulting behaviour (slot 0, H = 500, pitch 0)

| Situation | Behaviour |
|---|---|
| Normal distance | Eye 5517 units behind the midpoint along the yaw. Height −1112 at rest. Level (pitch 0). The point (mid, y = −450) sits 60 px below screen centre. Standing chests (−1715) sit 47–55 px above centre. |
| Far apart | Once the margin-expanded pair exceeds 280 px, the camera pulls back. It retreats at 1/8 of the gap per frame and comes back in at 1/32 per frame. The pair is kept at about ±133–140 px. Lateral re-centring eases at 1/8 per frame. |
| Close together | No centring inside ±48 px (dead zone). Pull-back decays. |
| Jump / launch | When the higher fighter would rise above about −112 px, the camera dollies back. Smoothing is 1/8 out (≤ 800/frame) and 1/16 in (≤ 1200/frame). The vertical recentre then lifts the eye. |
| Sidestep / circling | The yaw follows the P1→P2 axis with the second-order chase (start threshold 5°, settles after 32 frames under 5°). The eye orbits the midpoint because it is rotated by the yaw delta around the new midpoint. |
| Side switch | The axis is oriented by the start-side markers, so a crossover makes the target yaw swing about 180°. If the error grows gradually (passing beside), the camera keeps turning and swings round so P1 is on the left again (about 2–3 s). If the error ever exceeds 150° (an instant swap), the yaw freezes and the fighters stay swapped until the error falls back. |
| First 200 frames of the round [G] | Yaw frozen. |
| Ground / KO | Handled by the director states and phases below. |

---

## 3. Key blend and mixer [C]

**`0x80063D00(key)` → target.**
- d = mean of \|fighter − eye\| over both samples.
- target = eye + d·f(pitch, yaw).

**`0x80064170(A, B, wA)`**, with wB = 4096 − wA:
- d, target and H are linear blends;
- pitch and yaw are `A + wrap18(B − A)·wB`;
- eye = target − d·f(pitch, yaw);
- the result is re-derived as a look-at key (`ratan2`);
- it is stored into slot 3, then applied.

So blends orbit around the blended look target instead of moving straight through space.

**Mixer `0x80062B74`.** The mask is built from the weights that are > 0:

| Mask | Action |
|---|---|
| {0} | apply 0 |
| {1} | apply 1 |
| {2} | apply 2 |
| {4} | apply 4 |
| {0,1} | blend(k0, k1, w0) |
| {0,2} | blend(k0, k2, w0) |
| {0,4} | blend(k0, k4, sin(w0/2 − 0x400)/2 + 0x800), an S-curve |
| anything else | reset `0x80062A10` |

`0x80063EEC(out, eye, target, H)` builds a look-at key.

---

## 4. The director `0x800630D8`

### 4.1 Selection

- **Battle type** `0x800AFA88`:
  - 0–5 and 7 go to the phase switch;
  - 6 goes to the attract/demo director [G] (`0x800ADED4` set by `0x8004FA60`, which forces type 6);
  - 8 goes to the overlay camera.
- **Phase** `0x8009546C` (written by the round code `0x8003CB84`, the KO/win code and `0x8002A9DC`):

| Phase | When | Camera |
|---|---|---|
| 0 | round start | Reset (`0x80062A10`, `0x80065ED8`: H = 500, slot 0/1 = eye (0, −1450, −8000)). Intro init `0x80068394`, then phase 1. |
| 1 | round intro | Slot 2 = fly-in (§4.2), w2 → 0 over the last 16 frames while w0 → 1. Phase 2 when done. |
| 2 | fighting | State machine (§4.3). |
| 3 / 4 | KO (round state 2→3; 3 = camera on P1, 4 = on P2) | `0x800686B0(P)` alternates KO cam A and B on successive KOs (toggle `0x800A049C`). Then phase 5. |
| 5 | KO slow-down | `0x8006870C` runs cam A (`0x800465BC`) or cam B (`0x80046A20`). |
| 6 / 7 | win / lose pose (6 = P1, 7 = P2) | `0x8006895C(P, P.health == 0)`, primed by `0x8006875C`. |
| 8 | – | nothing |

The round state is non-zero while a **replay** plays. In that case the fight handler calls the replay camera
`0x80069414` instead of `0x800630D8` (§5.4).

### 4.2 Round intro (`0x80068394` / `0x800684B4`) [C]

- **Base.** The snapped slot-0 key, saved at `0x800A04A8`.
- **Duration.** T = 120 frames when `0x80095484` == 1 (first round [G]), otherwise 53.
- **Variant.** A round counter `0x8009854C` mod 10 (it must be < 8 if CAM+0x6C ≠ 0).
- **Eye.** `eye = base.eye + off[v]·(1 + sin(0xC00 + t·1024/T))`, where t counts down T…0, so the eye eases
  from base + off to base.
- **Look target.** (base.x, base.y, 0).
- **Offsets** `0x80098508`: 10 × `{s16 x, y, z}` world-space offsets, one per variant. They come in
  mirrored pairs: from straight behind, from the left and right, from above, and so on. For example variant 0
  starts 7500 units behind the base eye. Ball uses a single offset at `0x80098544`. The values are in
  `assets/gamedata/camera.json` → `intro.offsets` / `intro.ball_offset`.
- **Weight.** w2 = min(t·256, 4096), so the hand-off blend (k0, k2) lasts the final 16 frames.

### 4.3 Fight-phase states (CAM+0) [C]

Only battle types 0–5 run the special cameras (CAM+0x1C).

| State | Meaning |
|---|---|
| 1 | Normal (w0 = 1). **Throw/special start:** one player has +0x74 > 0 and the throw latch is free. The thrower's current move camera id (move +0x24 byte) is resolved by `0x800676A0`/`0x80067260` (below). A *situation type* gives state 3 (blend-in after a 16-frame delay). A *camera motion* gives state 6. **Situation cameras:** otherwise `0x80067CEC` checks player flags +0xD0/+0xD3/+0xD4/+0xDE/+0xE8 against the 4 entries of `0x800242A4` {moveset key, +0xA0 value, list 300–302}, weighted lists `0x8002428C`. A hit gives state 8 [I: rare character-specific shots]. |
| 2 | Hold slot 1 (w1 = 1) until the thrower leaves the throw (`0x80067530`), then state 4. |
| 3 | Blend k0→k1 over N frames (N from the type table). Aborted if a move has camera id 9. |
| 4 | Blend k1→k0 over N frames, then state 1. |
| 5 | S-curve blend from the frozen slot 4 back to k0 over 8 or 35 frames. |
| 6 | Scripted camera motion in slot 2. The motion's 7th channel (0…320) is the fight-camera share: w0 = p·4096/320, w2 = 1 − w0. It ends when p returns to 320, or with a freeze + state 5 (35 frames when the throw ends, 8 frames on camera id 9). |
| 7 | reset |
| 8 | Hold a situation camera for its hold time, then cut back to state 1. |

**Move camera id → list.**
- Ids below 0x2B use the EXE table `0x80098370`. Higher ids use per-character data (`0x800ADC20[p]->+0x20/+0x24`).
- Entries are 8 bytes: `{u16 last, u16 cumulative probability /4096, u32 value}`. A random number & 0xFFF picks
  an entry.
- If value < 30, it is a situation type (via `0x80024170`, identity + 1). Otherwise it is a camera-motion pointer.
- The EXE list mostly uses types 15, 20, 21, 25 and 26.

**Situation types** `0x80023F18` (30 × 20 B) [C]: `{in-frames, ?, pitch[4], yawOff[4]}`.
- A random 1-of-4 pitch and yaw offset is chosen (`0x80067AFC`).
- pitch = v<<5, i.e. v/2 angle units; it is halved when CAM+0x6C ≠ 0.
- yaw = axis ± yawOff, with the sign from the side flag.

In outline: types 1–15 blend in over 35 frames, types 16–30 cut (1 frame). Within each group of five the yaw
offset steps through 0 and ±30° / ±60°; the groups differ in pitch (level, slightly up, or 5°…20° down). The
30 entries are in `assets/gamedata/camera.json` → `situation_presets.entries`, and the EXE selector rows
(`0x80098370`) in `script_selectors.rows`.

Slot 1 is then framed by `0x80066280` → `0x80064E10(…, pitch, yaw, snap, dist = 5517·114/128 = 4913, slot 1)`.
This is the "a2" vertical offset path for pitched shots.

### 4.4 Scripted camera motions (`0x800663F8` / `0x800666F0` / `0x80066A18`) [C]

- Camera motions are decoded with the character motion decoder (`0x80038CD8`/`0x80038CE4`).
- Seven channels: eye xyz ·20/32, target xyz ·20/32, weight (/8, clamped 0…320).
- The result is stored in `0x800A0428` {eye s16 ×3, tgt s16 ×3, weight, refH = 450}.
- Positions are relative to an anchor:
  - anchor = CAM+0x54… (the thrower root);
  - yaw = CAM+0x40, which comes from `0x80066DFC` from the move's facing (the 6-way switch at `0x80023F00`);
  - z is negated when mirrored.
- With CAM+0x6A = 1 the eye is pulled along the eye→target line by H/450. This turns the authored zoom into a dolly
  at the fixed H = 500.
- The output is a look-at key in slot 2.
- `0x800670FC`/`0x80067070` play motions by id. Id 0x2B + 2n is common; id 0x4B is used for the character with
  moveset key 0xE [G: Gon/Dr B]. Callers are the attract/demo paths.

---

## 5. Other cameras

### 5.1 KO camera A (`0x80046460` init, `0x800465BC` per frame) [C]

The camera block is at `0x8009E6A0`, in RVIEW format with angles <<6.

- **Eye.** P.root + 3953·(cos a, sin a), where a = 60° + rnd(0…60°) − P.angle/16. y = −850, or chest − 290 when
  CAM+0x6C ≠ 0.
- **Each frame.** If \|eye − P.root\| > 2513, the eye moves 8 units/frame toward P.
- **Aim.** A look-at at P.root through `0x80046270`, eased 1/32 in pitch and yaw.
- **Output.** `0x8004610C` writes it to RVIEW.

### 5.2 KO camera B (`0x800467A0`, `0x80046A20`) [C]

- **Random parameters:**
  - radius 4500 / 5000 / 5500 (`0x8001DF54`);
  - side offset 1800 / 2000 / 2200 (`0x8001DF5C`);
  - eye height −950 / −1100 / −1650 (`0x8001DF64`).
- **Placement.** The eye is placed perpendicular to P's facing, on a random side.
- **Drift.** The eye drifts 8 units/frame (vector at `0x8009E6F0`). It is pushed out if closer than 2308.
- **Aim.** Look-at on P, yaw eased 1/32, pitch 1/16. A pitch-dependent height clamp follows.

### 5.3 Win / lose camera (`0x8006875C` prime, `0x8006895C` per frame) [C]

**Winner** (P.health ≠ 0):
- Eye at P.root + R·(cos a, sin a). R = 3978, or 6596 for moveset key 11 [G: large character].
- a = 45°…135° relative to P's facing (LCG `0x800A912C`). y = −1500.
- The target starts at (P.x, −1425, P.z) and eases 6/128 per frame toward (P.root.x, chest, P.root.z).
- If farther than 3664 (6282), the eye approaches at 2 units/frame.
- If CAM+0x6C == 2, the target is kept at most 400 below the eye.
- Output through the look-at `0x80045EE8` and `0x800461C0`.

**Loser** (health == 0):
- Overhead shot. RVIEW = {pitch 0x400 (straight down), yaw 0xC00 − P.angle/16 + rnd(±64) − 0x400}.
- Eye (P.x, −7853, P.z).
- Sets `0x800B040C` = 1 (except for Ball). That flag makes characters use a fixed matrix [G: verify].

### 5.4 Replay camera (`0x8006953C` init, `0x80069414` per frame) [C]

The block is at `0x800A04C8` {dist, toggle, enable, snap, yaw s16 +8, pitch s16 +0xA, yawOfs s16 +0xE, yawOfsTgt +0x10}.

- **Init:**
  - dist = 0x2B1A (11034);
  - axis = atan2 P1→P2;
  - the variant alternates every replay.
- **Variant A:**
  - yaw = axis ± 40° (random);
  - yaw offset sweeps from +(25…54)° to −(25…54)° (degrees converted with the /360 magic), eased 1/128 per frame;
  - randomly mirrored.
- **Variant B:** yaw = axis + 30°…150°, no sweep.
- **Each frame:**
  - samples every 4th frame (`0x80069304`);
  - root x/z, y = min(chest Y, root Y), from `0x800AA17C` = P + 0xF54;
  - framing via `0x80064E10(…, pitch +0xA (0), yaw +8 + +0xE, snap, dist, slot 1)`;
  - dist eases toward **0x113D (4413)** by 6/256 per frame;
  - applies slot 1 directly.
- **Bug [C].** The yaw values are stored as **s16** (`sh`) although they are <<6 angles. Replay yaws are therefore
  taken modulo 90° (−45°…+45° plus the offset).

### 5.5 Attract / demo director (battle type 6, `0x80069100`) [I]

- A 720-frame (0x2D0) program cycles through cameras via table `0x800242C4`:
  - the normal fight camera;
  - `0x80068BA8`: a random orbit, pitch from {±960, ±1920, ±3840, ±5760} in <<6 units (`0x80098550`), i.e.
    ±15…90 angle units, with the range narrowed by CAM+0x6C; yaw speed ±64 (<<6) per frame;
    dist 0x17F2 = 6130;
  - `0x80068D50`: a dolly/track at radius 0x11F5 with a sine height.
- Program lengths come from `0x800242BC` {10, 20, 30, 40}.

### 5.6 Camera shake `0x8004AD34` [C]

- Triggered by `0x8004ACFC(n)`: hit and event code at `0x8003F360`, `0x80040888`, `0x80045B10–30`.
- Each frame adds a signed byte to RVIEW.pitch (angle units). This happens after the camera has rebuilt RVIEW, so it
  is a per-frame offset, not cumulative. 0x80 ends the script.
- Scripts (pointer table `0x80097AA4`): three, of 6, 24 and 66 frames, small alternating offsets (up to ±4) that
  decay. The bytes are in `assets/gamedata/camera.json` → `shake.scripts`.

---

## 6. Reimplementation spec (versus, slot 0), condensed

- **Init:** H = 500; the slot-0 key's eye is (0, −1450, −8000) with pitch and yaw 0; slot state cleared.
- **Each frame:**
  1. sample both fighters (§2.1);
  2. run the yaw controller (§2.2) for the new yaw and snap flag;
  3. frame slot 0 (§2.3) with the slot's pitch lag, that yaw, the snap flag and depth 5517;
  4. during the intro, blend the fight key with the intro key (§4.2, §3);
  5. write RVIEW from the resulting key (angles back to 4096-per-turn units, roll 0), keep H = 500;
  6. add the shake byte to RVIEW's pitch (§5.6).

The constants below are also in `assets/gamedata/camera.json` → `fight`.

**Easing and limit constants:**

| Quantity | Value |
|---|---|
| Framing depth | 5517 |
| Margins | 150 units |
| Fit window | 280 px |
| Dead zone | ±48 px |
| Lateral easing | 1/8 |
| Pull-back easing | out 1/8, in 1/32 |
| Vertical keep-in | 112 px |
| Centre-point drop | 100 − 450·H/dist px |
| Vertical pull easing | out 1/8 (≤ 800), in 1/16 (≤ 1200) |
| Yaw chase thresholds | 5° (0xE38) start/settle, 20° (0x38E4) brake zone, 150° (0x1AAAA) freeze |
| Yaw acceleration | 0x600 / 0x300 |
| Yaw damping | 14/128 |
| Yaw velocity cap | 0xDDD0 |
| Pitch lag | ≤ ±0x100 (<<6) per frame toward 68.3·sin(…) |

---

## 7. Prototype results (`t3_converter/src/reference/camera.rs`, integer-exact port; numbers are the steady state)

| Scenario (fighters at y = −1715, z = 0) | RVIEW (eye; pitch, yaw; H) | Screen x / y of P1, P2 |
|---|---|---|
| 1500 apart | (0, −1112, −5517); 0, 0; 500 | ±68 / −55 |
| 3000 apart | (0, −1157, −5892); 0, 0; 500 | ±127 / −47 (horizontal pull −375) |
| 6000 apart | (0, −1800, −11250); 0, 0; 500 | ±133 / +4 (pull −5733, applied from the first frame by the snap) |
| 1500 → 6000 over 90 f | z −5517 → −7949 (f60) → −10620 (f90) → −11235 (f120) | |
| 3000 apart, P2 jumps 1200 high (apex f80) | f80 (0, −1310, −6723); f90 (0, −1333, −6795) (vertical pull 903); back to −5922 by f140 | P2 at apex −119 px |
| Sidestep: axis turns 45° in 60 f | yaw 22 (f20), 124 (f40), 288 (f60), 447 (f100), 511 (f140) | Lags about 0.5–1 s, no overshoot |
| Crossover passing 200 units beside | Yaw error peaks at 1701/1706 (just under the freeze). The camera swings about 180° (yaw 4031 → 2175 by f160). Pitch lag up to 137 (12° down), eye up to y −2225. | P1 back on the left |
| Intro variant 0 | z −13017 (f0) → −9259 (f40) → −6520 (f80) → −5517 (f119) | |

---

## 8. Corrections from the round-end research

These supersede the matching statements above.

1. Phases 3/4 aim at the **round winner**, not the KO'd player. They run during the win pose, after the replay.
   Phase 3 = P1 won, 4 = P2 won; only the winner is drawn.
2. Phases 6/7 are only the **loser** camera, used when a CPU player wins the match (6 = P1 is the loser, 7 = P2).
   The overhead shot is used when the loser was knocked out; after a time-up loss the orbit camera (§5.3 "winner"
   branch) runs on the standing loser.
3. KO cam A: the 3953 radius is immediately pushed out to **5800** by the aim helper `0x80046270`, which also clamps
   the pitch (chest +4004, floor −6917, in `<<6` units). The 3D approach goes down to 2513. Both eases are 1/32.
4. KO cam B: **pitch 1/32, yaw 1/16**. The push-out is horizontal by (2308 − d)/16, and the height rule raises the eye
   toward −7500·sin(pitch).
5. The replay camera samples **every frame** (every 4th only for a key-4 move with camera id 0x53), with
   y = min(chest, root.y). Its yaw and offsets are stored as **s16**, so angles above 45° wrap.
6. The A/B and replay variant toggles start at 0, so the first use of each is **B**.
7. The intro variant is `++counter % 10` (the first round after boot uses variant 1); the counter is never reset.
8. Distance checks use the approximate GTE square root (`0x8004AEDC`, table `0x8004AF40`), not `isqrt`.
9. The round-start fly-in runs **inside** the 151/91-frame round call.
10. After a KO: 90 frames of the normal fight camera, then the half-speed replay (from 30 frames before the KO move,
    at least 150 frames; START skips; none after a time up), then the win pose. Every camera change is a cut.

## 9. Guesses and open points

- **P1+0xF8** (`0x800AFA3C`). The < 200 yaw freeze is probably "frames since FIGHT".
- **+0x74.** Read as the throw/special-camera state.
- **`0x80095484`.** Read as the first-round flag.
- **CAM+0x6C.** Never written in the EXE. It may come from stage data or an overlay (record 6, Practice, writes
  near `0x800B8CD8` with another base).
- **`0x800B040C`** on the loser shot: the fixed-matrix flag is suspicious.
- **Situation cameras via `0x80067CEC`/`0x8002428C`.** The trigger flags +0xD0…+0xE8 are not identified.
  The type-1 "fixed point dolly" (`0x80067F5C`) was only skimmed.
- **Ball** third point (`0x800B4484`) and the Force overlay camera were not decoded.
