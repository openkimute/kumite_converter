# Tekken 3 (SLUS-00402): attack descriptors and baked strike paths

> Detailed research spec, part of [engine.md](engine.md). Derived from the EXE and disc data only.
> Helper scripts named below were research tools and are not in the repository, except the reference
> implementations `t3_converter/src/reference/attackpath.rs` and `reference/camera` (they read the disc image).

Scope: the attack ("hitbox") descriptor that move record +0x28 points to, the packed point format, the
segment builder, and how the result is tested against hurt spheres. All addresses are RAM addresses in the
main EXE (file offset = addr − 0x80010000 + 0x800).

Confidence tags: **[V]** read in the disassembly or confirmed by decoding data; **[G]** a guess.

Helper scripts are in `(research)/gap1/`:
- `attackpath`: the decoder (library + CLI).
- `scan`: container/size checks.
- `verify*`, `local`, `fit`, `pathdist`, `report`: checks against the pose solver.
- `*_report.txt`: per-frame residual dumps.
- `fn_69714_6a3bc.asm`: the disassembly excerpt.

---

## 0. Summary (what changed versus engine.md §7)

- **There is only one point encoding.** `0x80069714` always decodes one 5-byte point: 13 + 13 + 14 bits,
  biased. The "15 encodings" are the 15 **layout codes** (fmt & 0xF) of `0x8006A3BC`. A layout code says how
  many points each frame stores, and how they are joined into at most **two** segments. [V]
- **Header bytes 0..3 are bone slots:** `[end0, start0, end1, start1]`. A start of 0 means "swept": the start is
  the same bone on the previous frame. Only byte 0 (< 0x18) and byte 2 (≠ 0 gives two segments) are read
  by the engine. [V for the engine reads; V by data for the bone meaning]
- **fmt bits 4..14 = byte offset of an alternate data set.** It is used only when the attacker's
  character id is 14 (Ogre) or 15 (Mokujin). [V]
- **No terminator and no limb list.** Each descriptor is referenced on its own, and its length follows from
  the layout code and the active-frame count. The "limbs 6, 0x0A, 0x11" seen near Paul's 0xD68 are
  *neighbouring* descriptors. [V]
- **No radius.** An attack "capsule" is a bare segment. The radius is the defender's hurt-sphere radius.
  The test is effectively segment versus a vertical **cylinder** (Y slab clip, then XZ distance). [V]
- **Coordinate frame:** points are in the attacker's local frame, Y down, with +Z towards the opponent. In
  the space of t3_converter's `skel.pose()` they map to `(−x, y, −z)`, a 180° turn about Y. They are
  then **scaled by the model's size byte** (3DMK header +4, percent). The striking point is the **joint
  origin** of the named slot (wrist, ankle, elbow, knee…). Measured against the pose solver, **game frame
  +0x58 = motion frame + 1**. [V by fit]

---

## 1. Call chain and per-frame gating

| Address | Role |
|---|---|
| `0x8002B110…` → `0x80041F4C` → **`0x8006AA9C`** | Start of each actor's frame. Saves the root `actor+0..8` to `0x800A0578 + 12·side`, segment-0 end `actor+0x1B8..` to `0x800A0528 + 12·side`, segment-1 end `actor+0x1D0..` to `0x800A0528 + 12·side + 0x24`, and copies both saved ends into the segment starts `+0x1AC` / `+0x1C4`. [V] |
| `0x80040140..0x8004019C` | Move update. `actor+0xE3` (segment count) = 0 outside `record+0x2D ≤ actor+0x58 ≤ record+0x2E`; otherwise 2 if `desc[2] ≠ 0`, else 1. [V] |
| `0x8002B2B0…` → **`0x80041F6C`** | After movement: if inside the active window, calls **`0x8006A3BC(actor)`**. Otherwise it fills default body volumes. [V] |
| `0x80047CBC` / `0x80047D98` | Collision. If `desc[0] < 0x18`: for each of `min(actor+0xE3, 4)` 24-byte segments at `+0x1AC`, for each of the 14 hurt spheres, call `0x8004770C`. If `desc[0] ≥ 0x18`: the volume comes from a spawned object (`0x800ADEAC/0x800ADEB0[side]` via `0x8006E51C`) instead; see §7. [V] |
| `0x8006A370` | Sets `0x800A05A8[side]` = 1 when `actor+0x18` (character id) ∈ {14, 15}, else 0. Called at round setup `0x8002AB1C`. [V] |

`side` = `actor+0x1E`.

---

## 2. Locating the descriptor

`record+0x28` on disc is relocated by `0x8002CE14` (at `0x8002CF0C..0x8002CF5C`) [V]:
- If it equals a key of the 13-entry table at **`0x80017C10`** ({u16 key, pad, u32 ptr}), it becomes that
  pointer. Keys are 0, 0x7FF0..0x7FFB, and the pointers are `0x80017BDC + 4k`.
- Otherwise it becomes `header + header[+0x28] + value − 4`.

Built-in descriptors (`0x80017BDC`, entries overlap at 4-byte spacing):

| Key | Bytes 0..5 | Meaning |
|---|---|---|
| 0 | `00 00 00 00 18 00` | Moves with no attack. Their active window is 0..0, so no segment is ever built |
| 0x7FF0..0x7FF4 | `18..1C 00 00 00 …` | **byte 0 ≥ 0x18: object-driven hit** (§7). For example Paul 319 / Law 411 / King 402 (mask 0x607) use 0x7FF1; Dr. B 320/321 use 0x7FF4 |
| 0x7FF5..0x7FFB | e.g. `0a 02 00 00 04 00` | byte 0 < 0x18, so the "points" come from the EXE bytes that follow. They decode to coordinates near −8000 (behind the actor) and so never connect. They are used by dtype-1 (throw-type) moves and by a few King lows (0x7FF8) [G: these hits are resolved elsewhere] |

---

## 3. Descriptor layout

```
+0  u8  end0    bone slot of segment 0's end (the striking point); must be < 0x18 (else §7)
+1  u8  start0  bone slot of segment 0's start, 0 = swept from previous frame's end0
+2  u8  end1    bone slot of segment 1's end; != 0 => actor+0xE3 = 2 (second segment is tested)
+3  u8  start1  bone slot of segment 1's start, 0 = swept
+4  u16 BIG-endian fmt:  bits 0..3 = layout code, bits 4..14 = alt-data byte offset (mode 1),
                         bit 15 + bits 1..14 = alt offset for "mode 2" (dead code: mode 2 is never set)
+6  data:  [pre bytes: points for frame −1] [per bytes × nframes]  (primary set)
+6+alt: same layout again (alternate set for Ogre/Mokujin)
```

- Byte meanings [V]:
  - Byte 0 is checked `< 0x18` in `0x8006A3BC`, `0x80047BBC` and `0x80047D98`.
  - Byte 2 is checked `≠ 0` at `0x80040178`.
  - No other reads of bytes 1 and 3 were found.
- **Bone meaning** [V by data]. Each decoded point was matched to the nearest joint, across Paul, Law, King
  and Eddy (`second`):
  - code 0 `06 04`: points are slot 4 then slot 6;
  - code 6 `0e 00 0e 0d`: points are swept 14, then 13 → 14;
  - code 7 `11 00 10 11`: segment 1 = 0x11 → 0x10;
  - code 4 `05 00 04 02`: segment 1 = 2 → 4.
- Slots follow `skel`: 0 root, 1 chest, 2 head, 3–6 arm A (clavicle, upper, fore, hand), 7–10 arm B,
  11 pelvis, 12–14 leg A, 15–17 leg B. Values 0x12+ are accessory bones; for example Yoshimitsu's sword
  uses 0x12.
  - The joint origins are: upper arm = shoulder, fore = elbow, hand = wrist, shin = knee, foot = ankle.
  - Paul's jab (1) = slot 10 (left wrist) and 2 = slot 6.
- **Size** = `6 + pre + per·nframes`, with `nframes = record+0x2E − record+0x2D + 1`. If alt ≠ 0, the alt set
  follows and alt = the primary size, e.g. fmt 0x00A1 = code 1, alt 10 for one frame.
  - Checked over every descriptor of 20 movesets (`scan`). The only mismatches are one tail/padding
    case per moveset (the last descriptor before 4-byte alignment, or one unreferenced gap).
  - Alt sets are present in King (149/149), Anna (153/153), Law (62/170) and Paul (5/146); Mokujin, Dr. B
    and Force have none.
- Code usage over 20 movesets:
  - 1: 1682
  - 6: 852
  - 7: 241
  - 2: 100
  - 0: 97
  - 4: 29
  - 5: 8
  - 10: 5
  - 14: 5
  - 3: 4
  - 9: 1
  - 13: 1

---

## 4. Point format: `0x80069714(src, dst) → src+5` [V]

The 5 bytes form one little-endian 40-bit value holding three biased fields:

| Field | Bits | Bias | Range |
|---|---|---|---|
| x | 0–12 | −0x1000 | −4096…4095 |
| y | 13–25 | −0x1000 | −4096…4095 |
| z | 26–39 | −0x2000 | −8192…8191 (forward reach) |

Every point is absolute in the attacker's local frame; nothing is delta-coded and there is no scale factor.
The unit is the engine unit (≈ 1 mm; Paul's jab wrist is at y ≈ −1400, z ≈ +1300).

---

## 5. Layout codes: `0x8006A3BC` jump tables `0x80024344` / `0x80024384` / `0x800243C4` [V]

`f = actor+0x58 − record+0x2D` (0-based active frame). The data cursor starts at `desc + alt + 6 + pre + per·f`.
"N" means "decode the next point at the cursor". "prev" is a swept start:
- at f = 0, the point `back` bytes before the cursor is decoded and transformed with the **saved previous
  root** `0x800A0578[side]`;
- at f > 0, the end saved last frame is reused (`0x800A0528[side]`, +0x24 for segment 1).

| Code | per, pre | Segment 0 (`+0x1AC`) | Segment 1 (`+0x1C4`) | Typical header |
|---|---|---|---|---|
| 0 | 10, 0 | N → N | — | `06 04 00 00` (fore→hand) |
| 1 | 5, 5 | prev(−5) → N | — | `0a 00 00 00` (one swept wrist/ankle) |
| 2 | 10, 10 | prev(−10) → N | prev(−10) → N | `0a 00 02 00` (two swept points) |
| 3 | 15, 5 | N → N | prev(−15) → N | `06 04 05 00` |
| 4 | 15, 5 | prev(−5) → N | N → N | `05 00 04 02` |
| 5 | 20, 0 | N → N | N → N | `0d 09 10 05` |
| 6 | 10, 5 | prev(−5) → N | N → seg0.end | `0e 00 0e 0d` (swept foot + knee→foot) |
| 7 | 10, 5 | prev(−5) → N | seg0.end → N | `11 00 10 11` |
| 8 | invalid | (`0x80024344` → return) | — | |
| 9 | 10, 5 | N → N | prev(−5)\* → seg0.start | `12 0a 0a 00` (Yoshimitsu sword) |
| 10 | 15, 0 | N → N | N → seg0.end | `09 06 09 0a` |
| 11 | invalid | (return) | (seg0.end → N) | |
| 12 | 15, 0 | N → N | N → seg0.start | |
| 13 | 15, 0 | N → N | seg0.start → N | `0e 01 11 01` |
| 14 | 15, 0 | N → N | N → same N (a point) | `11 10 0e 0e` |

Codes ≥ 15 return without building anything.

\* **Code 9 quirk:** at f = 0 it re-decodes `cursor−5`, which is *this* frame's segment-0 end, not the
initial point at +6. So the 5-byte "pre" point is never read, and on the first frame segment 1 duplicates
segment 0. It looks like an EXE bug (it would have been −15); it is harmless.

---

## 6. Local → world: `0x8006A230(actor, dst, P, O)` [V]

Globals at `0x800A05B8` are filled at the top of `0x8006A3BC`. Sin/cos come from table `0x8001E8C4`
(4096 entries per turn; cos = +0x800 bytes; angles are 16-bit):
- `[0] = sin(−a)`, `[1] = cos(−a)`, with `a = actor+0x2C` (facing);
- `[4] = diff = actor+0x2C − actor+0x0E`; if diff ≠ 0, `[2],[3] = sin/cos(−diff)` and `[5..7] = pivot actor+0xF68..F70`.

A local point P becomes a world point by a turn about the vertical axis plus the origin O:
- If the facing changed during the move (`diff ≠ 0`), O is first turned about the pivot by −diff.
- With s, c = sin/cos(−facing) (4096 = 1, products shifted down by 12 rounding toward zero):
  `x = Ox + Px·c − Pz·s`, `y = Oy + Py`, `z = Oz + Px·s + Pz·c`.
- `O` is the current root `actor+0..8`, or the saved previous root for a swept start at f = 0.
- There is **no scale and no radius**.
- Note that 0x8006A3BC stores the *world* positions. The f > 0 swept starts are last frame's world ends,
  so root motion between frames is swept correctly.

---

## 7. Collision semantics

- **`0x8004770C(seg, sphere)`** [V]:
  1. Box reject per axis (x, z, y): `|mid − c| > |half-extent| + r`.
  2. If `y0 ≠ y1`, clip the segment parametrically to `[c.y − r, c.y + r]`.
  3. **XZ-only** point-to-segment distance² ≤ `r²` (sphere +0x10).

  So each hurt "sphere" (radius s16 at +0xC) is in practice a vertical cylinder of radius r and height 2r,
  and the attack segment has zero thickness. `attackpath.segment_hits_sphere()` reproduces it in floating
  point.
- The first hit records the struck sphere index (body zone) at `contact+0x18` and the segment direction.
  See [reactions.md](reactions.md) §0.
- **byte 0 ≥ 0x18** (`0x80047BBC` / `0x80047D98`):
  - the segment list comes from the side's object (`*(0x800ADEAC + 4·side)` through `0x8006E51C(side, 0)`);
  - the contact direction comes from the object's segment;
  - so these are projectile or extra-object hits [G: Ball, Force or special-weapon objects].

---

## 8. Mode 1: Ogre / Mokujin alternate paths

- `0x800A05A8[side]` = 1 for character ids 14 (Ogre) and 15 (Mokujin). In that case the data start is
  `desc + ((fmt & 0x7FFF) >> 4) + 6` [V].
- Ogre and Mokujin borrow other characters' movesets and are drawn at **size 100 %**. Some owners are not:
  the 3DMK header +4 byte gives King 106 %, Eddy and Anna 105 %, Dr. B and Kuma 90 %, Gon 45 %.
- The **primary** set is baked at the owner's size. The **alt** set is the same motion at 100 %.
  - King's primary set fits his decoded motion with a best uniform scale of **1.071**. Scaling the pose by
    1.06 brings the median residual from 162 down to 89.
  - King's alt set fits at scale **1.004**, with a median of 75.
  - Eddy's set (no alt data) fits pose × 1.05 exactly (best scale after correction 1.000).
  - Dr. B's set fits pose × 0.905. With the 90 % model (247), the median is 86.
  - [V by fit; the place where the engine applies the size byte to the skeleton was not located.]

---

## 9. Verification against the pose solver

Method:
1. Decode with `attackpath.capsules()` (actor at the origin, facing 0).
2. Pose with `anim.decode` + `skel.Skeleton.pose` at **motion frame = actor+0x58 − 1**. A swept start uses
   −2.
3. Map pose → game with `(−x, y, −z) × size/100`.
4. Compare with the joint origin of the slot that the header byte names.

The frame offset was selected statistically. Offset −1 gives a local-frame median offset of < 10 units per
slot. Offset 0 gives 170–370. A fractional-frame search puts 70 % of the points at exactly −1.0
(`stats`, `local`).

Paul (record 74, model 71; `paul_report.txt`). Coordinates are game space; |d| is in units (≈ mm):

Distance |d| between the decoded point and the named joint (the decoded coordinates themselves are not
reproduced; `t3_convert attackpath` prints them from your disc):

| Move | Frame | Segment | \|d\| start / end |
|---|---|---|---|
| 1 (rec 104, code 1, slot 10) | 8 | 0 | 52 / **10** |
| 2 (rec 106, code 1, slot 6) | 10 | 0 | 22 / 142 (8 at motion frame +0) |
| d/f+1 (rec 140, code 1, slot 10) | 13 / 14 | 0 | 126 / 17, then 17 / 19 |
| d+4 (rec 139, code 6, 14 + 13→14) | 14 | 0 / 1 | 19 / 30; 114 / 30 (knee) |
| b+1+2 (rec 119, code 0, 4→6) | 63 / 64 | 0 | 9 / 5, then 3 / 2 |
| f+1+2 (rec 116, code 6, 10 + 9→10) | 20–21 | 0, 1 | 39–106 |
| (rec 238, code 3) | 34 | 0, 1 | 8–84 |

Whole-character statistics over every own-motion descriptor (`pathdist`):

| Character (record / model / mode) | Points | Fixed-frame \|d\| median / p90 | Distance to the bone's animated path, median / p90 |
|---|---|---|---|
| Paul 74 / 71 / 0 | 574 | 40 / 152 | **18 / 63** |
| Law 82 / 79 / 0 | 960 | 49 / 156 | **23 / 78** |
| King 98 / 95 / 0 (unscaled pose) | 880 | 159 / 282 | 114 / 193 |
| King 98 / 95 / 1 (alt set) | 880 | 75 / 204 | **43 / 122** |

Spot checks (`king_report.txt`, `king_alt_report.txt`, `law_report.txt`):
- King d+1 at ×1.06: end |d| = 12.
- King d+4 alt: end |d| = 5.
- Law d+4: end |d| = 4, knee 12.
- Law d+1: 31.

Reading the residuals:
- **Axis flip:** there is a constant 180° turn about Y between the extractor's pose space and the actor
  frame: −x and −z, with y unchanged (Y down, floor at y = 0).
- **Scale:** the model size byte; see §8.
- **No constant offset:** the bone-local median offset is ≤ 10 units for wrist and ankle slots.
- **What remains is timing, not shape.**
  - The errors lie along the direction of motion.
  - They are largest for swept starts of fast limbs; a kick moves 700+ units per frame.
  - The distance to the animated *path* is 2–3× smaller than the fixed-frame error.
  - [G] The paths were baked by the developers from the source (pre-compression) animation, possibly with
    blend-in, so they lead or lag our keyframe-decoded motion by a fraction of a frame. Airborne moves and
    moves from the common bank have the largest errors.

---

## 10. Per-frame algorithm (re-implementation)

1. **Frame start** (`0x8006AA9C`): save the actor's root and the end points of both segments for its side.
2. **Move update** (`0x80040140`): the segment count is 0 outside the active window (record +0x2D…+0x2E),
   otherwise 2 when descriptor byte 2 is non-zero, else 1.
3. **After movement, inside the active window** (`0x80041F6C` → `0x8006A3BC`): nothing is built when the actor's
   +0xB4 gate is clear or descriptor byte 0 is ≥ 0x18 (object-driven, §7). Otherwise:
   - read the layout code (low nibble of the big-endian word at +4) and, for Ogre/Mokujin (mode 1), the
     alternate-set offset (bits 4–14);
   - the cursor for active frame f is `desc + alt + 6 + pre + per·f` (§5);
   - decode the points the layout code names and place them in the world (§6) with the current root; a swept
     start uses the saved previous root on the first active frame and the saved end point afterwards;
   - store segment 0 and, if present, segment 1.
4. **Collision** (`0x80047CBC`): every built segment (at most 4 are looked at) against each of the 14 hurt
   spheres: box reject, clip to the sphere's height band, XZ distance ≤ r; the first hit wins.

`attackpath` implements all of the above:
- `Moveset(rec)`;
- `capsules(M, move, mode, roots, angles)` yields `(frame, [(start, end, 0), …])` with exactly `actor+0xE3`
  segments;
- `build()` is one call of 0x8006A3BC;
- `decode_point()`, `to_world()` (with pivot turn) and `segment_hits_sphere()`.

CLI: `t3_convert attackpath 74 104 106` prints the segments of Paul's 1 and 2.

---

## 11. Open questions

- Where the engine applies the 3DMK size byte (header +4) to the skeleton. That it is applied is inferred
  from the path fits.
- What the objects behind byte 0 ≥ 0x18 descriptors are (Ball/Force/weapon objects), and what
  `actor+0xB4` (the gate for building segments) means.
- Whether the 0x7FF5..0x7FFB built-ins are deliberately "never hit" sentinels.
- The source of the sub-frame timing differences (§9).
