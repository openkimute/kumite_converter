# Tekken 3 (USA) — Disc Image Research

**Game:** Tekken 3, Namco, 1998 — Sony PlayStation (PS1)
**Serial:** SLUS-00402 (North America)
**Format:** Raw BIN/CUE (Redump-style), 1 data track + 2 CD-audio tracks
**Analysed:** 2026-10-07

---

## 1. Dump integrity — verified good

| File | SHA-1 | SHA-256 |
|---|---|---|
| Track 1 (data) | `68f32a8657376cb4d4504c160fd5e8ae4a4f18d6` | `6b660e62748d02779e9e08362a5ed202540af7fad134de2ec0a2184ad78bb496` |
| Track 2 (audio) | `ba8cbc6a371250a92b3d12c1154f489bd6b4b70b` | — |
| Track 3 (audio) | `a5b34e39603ed80fed8400d879b20b39aadfe1ba` | — |
| `SLUS_004.02` (EXE) | — | `fbda8b68e5799dbef4af39a161783bc670c15b0aa0e87dce65e210717da19b8c` |

Track 1 and the EXE hashes match the known-good reference values used by
[Tekken3Recompiled](https://github.com/FishB0nes98/Tekken3Recompiled) (`game.toml`, `tools/xas_mod.py`).

### ⚠️ Known issue: cue sheet filename mismatch

The `.cue` references `Tekken 3 (USA) (Track N).bin` (spaces), but the files on disk are
named `Tekken_3_(USA)_(Track N).bin` (underscores). Emulators will fail to load the image
until either the `.bin` files are renamed or the three `FILE` lines in the `.cue` are edited.

---

## 2. Disc layout

Cue sheet: Track 01 `MODE2/2352`, Tracks 02–03 `AUDIO` (each with a 2-second pregap).
ISO 9660 volume ID `TEKKEN3`, volume date 1998-03-31.

`SYSTEM.CNF`:
```
BOOT = cdrom:\TEKKEN3\SLUS_004.02;1
EVENT = 10
STACK = 801FFFF0
```

| Path | LBA | Size | Contents |
|---|---:|---:|---|
| `/SYSTEM.CNF` | 23 | 67 B | Boot config |
| `/TEKKEN3/SLUS_004.02` | 25 | 1,185,792 B | Main PS-X executable |
| `/TEKKEN3/TEKKEN3.XAS` | 604 | 249,552 raw sectors | Interleaved XA music + STR movies |
| `/TEKKEN3/TEKKEN3.BNS` | 250,156 | 38,150,144 B | Indexed asset archive (303 records) |
| `/TEKKEN3/TEKKEN3.DA` | 269,084 | 23,814,144 B | **Alias for CD-audio Track 2** |
| `/TEKKEN3/TEKKEN3.DMY` | 280,862 | 24,111,104 B | **Alias for CD-audio Track 3** |

### DA / DMY are directory entries over the audio tracks

Track 1 is 268,934 sectors. Track 2 INDEX 01 begins at absolute LBA 268,934 + 150 = **269,084**
(= `TEKKEN3.DA`), and its 11,628 audio sectors equal the DA size / 2048. Track 3 INDEX 01 is at
**280,862** (= `TEKKEN3.DMY`), 11,773 sectors. Both match to the sector.

- **Track 2** (~2:37): loud, full-bandwidth mastered music (integrated −10.5 LUFS, peak −0.2 dBFS).
- **Track 3** (~2:39): **pure digital silence** (peak −∞ dB) — a literal dummy/padding track.

---

## 3. Executable — `SLUS_004.02`

- Header: `PS-X EXE`, "Sony Computer Entertainment Inc. for North America area"
- Load address `0x80010000`, entry PC `0x80079C70`, text size 1,183,744 B
- Sony PsyQ libraries, CVS tags from 1997 (`bios.c 1.86`, `intr.c 1.76`, `sys.c 1.135`)
- No disc filenames are referenced; the game seeks by sector offset using internal tables.
- Plain-ASCII strings include the mode titles and the character names (the roster names the converter reads
  into `assets/gamedata/roster.json`).

### Internal tables

| Table | EXE file offset | Entries | Record layout |
|---|---|---:|---|
| XA music (BGM) | `0x15578` (RAM `0x80024D78`) | 50 | `u32 start_sector, u32 end_sector, u32 channel` (XAS-relative) |
| BNS index | `0x14C00` (RAM `0x80024400`) | 303 | `u32 sector, u32 size` (BNS-relative, 2048-byte sectors) |

No FMV table exists in the main EXE (likely in an overlay); movies are located by scanning.

---

## 4. `TEKKEN3.XAS` — streamed media

Sector breakdown (subheader file/channel/submode):

- **XA audio** (submode `0x64`): channels 0–7, all **37,800 Hz, stereo, 4-bit XA-ADPCM**
- **STR video** (submode `0x48`): channel 1 only, MDEC frames, with movie audio also on channel 1

Layout: sectors 0–~122,000 hold 8-way interleaved music; sectors 122,000–249,246 hold movies.

### 4.1 Music — 50 XA streams

Matches the 50-entry EXE table exactly. About 38 full-length BGM tracks (1–3.7 min) plus about 12
short jingles/stingers (1–20 s); a stream's duration is its sector count × 0.0533 s. The streams are spread over
the 8 channels (6–8 per channel), each starting at a low sector and continuing through the interleave. The EXE
table (`{start sector, end sector, channel}` per stream) is converted to `assets/gamedata/music.json` →
`xa_table.entries`, and the decoded streams to `assets/audio/music/NN.flac`.

### 4.2 Movies — 22 FMVs (decoded and identified visually)

All movies are on channel 1, MDEC video with its XA audio, at 15 or 20 fps, mostly 256×224 (a few 256×192 /
256×176; the prologue 320×240). In XAS order:

- 0–19: the character endings, one each (Paul, Law, Lei, King, Yoshimitsu, Nina, Hwoarang, Xiaoyu, Eddy, Jin,
  Julia, Kuma/Panda, Bryan, Heihachi, Ogre/True Ogre, Mokujin, Gun Jack, Anna, Gon, Tiger), 18–51 s;
- 20: the opening movie (98 s);
- 21: the prologue (18 s).

Example row: movie 0 (Paul's ending) spans XAS sectors 122000–126859, 256×224, 648 frames at 20 fps (32 s).
The sector spans are in the movie table in record 301 ([engine.md](engine.md) §10.3); the converted movies' frame counts,
rates and lengths are in `assets/audio/index.json` → `video` (the movies themselves in `assets/video/`), their
sizes in `assets/gamedata/theater.json`.

Movie ↔ character identification is from visual inspection of decoded frames.

---

## 5. `TEKKEN3.BNS` — asset archive

Starts with a Namco copyright notice (plain text), followed by RAM pointers (`0x800Dxxxx`) — i.e. memory
images loaded directly into RAM.
Indexed by the 303-entry table in the EXE; all records fit inside the file.

| Kind | Records | Size | IDs |
|---|---:|---:|---|
| Code/data overlay (Namco header) | 2 | 0.80 MB | 0, 10 |
| ARC sub-container | 3 | 2.17 MB | 1–3 |
| `3DMK` 3D model | 52 | 1.28 MB | 71–275 |
| VAB sound-bank header (`pBAV`, v7) | 48 | 0.16 MB | 72–276 |
| Empty | 6 | 0 | 4…295 |
| Unknown binary | 192 | 33.43 MB | 5–302 (36–50 stage textures + floor maps, 56–70 stage backdrops; see §10) |

Signature scan of the whole file: 49 `pBAV` VAB headers (9–22 tones each), ~8,556 structurally
valid TIM textures (8,099 × 4bpp, 457 × 8bpp; most common 32×32).

---

## 6. Related project: Tekken3Recompiled

<https://github.com/FishB0nes98/Tekken3Recompiled> (v0.1.4, 2026-09-23)

- Static recompilation + early native PC port of exactly this release (SLUS-00402),
  built on psxrecomp / recomp-ui.
- 16:9 widescreen, mod manager, and read-only tooling for BNS / XAS / TIM / VAB / ARC.
- Its documented tables (`0x15578` XAS, `0x14C00` BNS) and counts (50 XA, 22 movies) match
  the independent findings above.
- **Windows-only** (`.exe` / `.cmd` / PowerShell). First setup downloads build tools; the
  optional "Jun" character requires MAME arcade sets `tekken3.zip` and `tektagt.zip`.
- Expects the cue file named `Tekken 3 (USA).cue` with space-named BINs — fix the
  mismatch in §1 first.

---

## 7. Method

ISO 9660 parsed directly from raw 2352-byte sectors; XA subheaders tallied per sector;
STR frames decoded with ffmpeg (`psxstr` demuxer, MDEC + ADPCM-XA); audio tracks analysed
with ffmpeg `astats` / `ebur128`; EXE tables located by searching for sector values found
in the stream scan. No files in this folder were modified (besides this report).

---

## 8. 3D models + textures — format reverse-engineered, extractor written

Derived independently from the disc data and the EXE's MIPS renderer
(`0x80036CAC` bone renderer, `0x800379xx` per-character draw loop).

### Tool

```
t3_convert extract_models "Tekken_3_(USA)_(Track 1).bin" t3_models_out [--preview]
```

Reads the BIN directly (ISO 9660 → `SLUS_004.02` + `TEKKEN3.BNS`), writes per model
`model_NNN.obj` + `.mtl` + `.png` (texture atlas). All **52 models** export with every
face resolved (~500–850 faces each). `--preview` also renders a 4-angle textured PNG.

### Record grouping in `TEKKEN3.BNS`

Each character costume = 4 consecutive records starting at a `3DMK` record `m`
(m = 71, 75, … 275): `m` model · `m+1` VAB sound header · `m+2` texture container · `m+3` (likely moves/animation).

Names come from each motion record's **moveset id** (byte 1 of its header) matched to the EXE
roster strings (`PAUL, LAW, LEI, KING, …`):

Each moveset id is one character's move set (Ogre and True Ogre share 14; Eddy and Tiger share 8; Kuma and Panda 11);
most characters have two or three model records. Example: moveset 0 is Paul, models 071 and 075. The full
mapping is converted to `assets/gamedata/movesets/index.json` (model record → moveset file) and
`assets/gamedata/roster.json` (`selections`, `models`).

### `3DMK` model format

```
0x00 u32 nRows (27)   0x04 u32 ?   0x08 "3DMK"   0x10 u32 payload offset (base 8)
0x18 nRows × 56-byte rows (14 × u32):
  w0  → A: normals      [u32 n][n ref bytes] ×2, [u32 m][m × s16 nx,ny,nz,pad] (|n|=4096)
  w1  → B: polygons     4 lists, each [u32 count][records]:
                         flat tri  (8 B: idx word, light idx)   GPU 0x24
                         flat quad (8 B)                         GPU 0x2C
                         gouraud tri  (8 B)                      GPU 0x34
                         gouraud quad (12 B)                     GPU 0x3C
                         idx word: v0=bits2-8, v1=bits9-15, v2=bits16-22, v3=bits25-31
                         (quad polygon order v0,v1,v3,v2); bit24 = double-sided
  w2  → C: texture info  u16 nbytes + u16 CLUT words (last = TPAGE),
                         (u,v) byte table, then per poly list: u8 count +
                         [attr(CLUT idx), uv-index ×3/×4]
  w3..w5  s32 joint translation from parent (bone-local, +X along the limb)
  w6  parent bone index (bone list = rows minus {2, 4, 20})
  w10 has-geometry flag
  w12 → D: vertices for the NEXT row's section:
         u32 H, ref list 1 (scratch slots), ref list 2 (global slots),
         [u32 m][m × s16 x,y,z,pad], then weld lists L1/L2a/L2b/L3a/L3b (9-bit packed,
         bit8 = average) and L4 (2-bit) that share/average seam vertices between bones
```

Key rules found in the renderer:

- **Section k** = row k's transform + row k's A/B/C + the D block stored at row k−1.
- Polygon indices address a 110-entry scratch vertex buffer (`0x1F800000` SXY,
  `0x1F8001B8` Z). Ref bytes `r` → slot `r/2 − 1`; list 1 copies from the previous
  section's scratch (deliberately reusing stale slots), list 2 from a persistent global
  buffer filled by other sections' L3a exports.
- Draw order / matrix slot table at EXE `0x8001A074`.
- Row 20 = alternate head/face section textured from runtime-animated VRAM (skipped).

### Textures

Texture record `m+2` is a 5-part container: part 0 = chain of TIMs (4bpp with own
16-colour CLUT, 8bpp skin textures sharing CLUT (0,0)), part 1 = 30 × 32×32 animated
frames, part 2 = VAB sample body, parts 3–4 small tables.
Polygon CLUT word → TIM with that CLUT position (1:1; bit 8 = alternate palette row,
`0x8000` = 8bpp skin). UV is relative to a 64-halfword (256 px) texture page:
`u = (tim_x mod 64)·4 + px`. Verified: 100% of polygons' UVs fall inside their TIM.

### Known limitations

- **Pose is an improvised T-pose** (pelvis flipped, clavicles rotated ±90°, head stood up);
  animation data (`m+3`) is not decoded, so no skeleton/rig is exported — static meshes only.
- Alternate palettes (CLUT bit 8), semi-transparency, lighting/normals and the runtime
  facial-animation textures are not applied.
- Extra alternate-hand meshes (data after D13/D17) are not exported.

---

## 9. Animation system + THREE.js viewer

Reverse-engineered from the EXE (all addresses in `SLUS_004.02`):

| Stage | Code | What it does |
|---|---|---|
| Keyframe decode | `0x80038B4C`, spline `0x800387C8` | motion → 49 s16 channels per frame |
| Pose → local matrices | `0x8003AD10` | Euler angles (`R = Rz·Ry·Rx`, 65536 = 360°) + 2-bone IK |
| 2-bone IK | `0x8003BCA8` (+ `0x8003BA28`, `0x8003BB88`) | aim upper bone at target, twist, law-of-cosines bend |
| Skeleton compose | `0x8003AFCC` → `0x8003B5C0` | 18 matrix slots, `world = parent ∘ local` |

**Motion data** — record `m+3`, header u32[7]..u32[8] = motion area. Each motion:
`u16 prefix` · `u16 (frames | key0<<8)` · `u8 key counts[48]` · `s16 keys…`.
Per channel: 0 keys → 0; 1 → constant; 2 → linear; N → per-frame; else spline with
per-channel shift from table `0x80096C24`, weights at `0x8009671C`. Odd channels are
stored time-reversed. All values `<< 1`. 64–342 motions per character.

**49 pose channels**: `[0..2]` root position · `[3..14]` IK targets (R hand, L hand, R foot,
L foot) · `[15..18]` IK twist angles · `[19..48]` 10 × (x,y,z) angles for slots
`[0,1,11,2,3,7,6,10,14,17]` (root, chest, pelvis, head, clavicles, hands, feet).
Slots 2, 3 and 7 are post-multiplied by a fixed 90° Y rotation. Arms/legs (slots 4/5, 8/9,
12/13, 15/16) are solved by IK; knees bend opposite to elbows.

**Skeleton mapping**: 18 matrix slots — 0 root, 1 chest, 2 head, 3–6 arm A (clavicle, upper,
fore, hand), 7–10 arm B, 11 pelvis, 12–14 leg A (thigh, shin, foot), 15–17 leg B.
Mesh row → slot (draw table `0x8001A074`): 0→0, 1→1, 19→2, 11→3, 12→4, 13→5, 14→6, 15→7,
16→8, 17→9, 18→10, 3→11, 5→12, 6→13, 7→14, 8→15, 9→16, 10→17.
**Joint translations are stored with Z negated** relative to the meshes (row 20, the alternate
head, uses the head slot). Verified by rigidly fitting welded seam vertices in the bind pose
(residual ≈ 0): e.g. clavicle mesh 11 attaches at (349, 0, −100) while row 11 stores +100.
Getting this wrong crosses the legs or tears the shoulders; with it, all four IK end effectors
land exactly on target.

**Accessory bones** (rows not in the 18 slots: hair, ponytails, tails, dress panels, coats,
True Ogre's wings, Mokujin's stick) are not in the 49-channel pose. Their bind transform is fitted
from welded seams when they share any with an already-placed bone; otherwise they hang under
gravity (or stay rigid when attached to a hand).

**Skinning**: the engine's seam-weld lists average vertices from two bones in screen space;
the exporter turns these into per-vertex bone weights (e.g. 0.5/0.5), so the mesh stays
welded in every pose.

### Converter (glTF 2.0) + viewer

```
t3_convert export_gltf "Tekken_3_(USA)_(Track 1).bin" t3_viewer/models
t3_convert export_stages "Tekken_3_(USA)_(Track 1).bin" t3_viewer/stages
t3_convert export_media  "Tekken_3_(USA)_(Track 1).bin" t3_viewer/media
serve 8765 t3_viewer                  # then open http://127.0.0.1:8765 (Range support for seeking)
```

`export_gltf` writes one standard **`.glb` per character** (52 files, ~260 MB): skinned mesh,
embedded texture atlas, named skeleton (`root`, `chest`, `pelvis`, `thigh_A`, … plus accessory
bones), and every motion as a named animation (`motion_000` …, 60 fps). The game's arm/leg IK is
solved during conversion and baked into ordinary joint rotations, so the files play in any glTF
consumer (Blender, three.js, Unity, Godot) without game-specific code. Root motion is kept on the
`root` joint. `models/index.json` lists the files.

How the engine's data maps to glTF:
- **Bind pose** is built by chaining the rigid transforms fitted from welded seam vertices, so the
  engine's 50/50 seam blending becomes ordinary skin weights and stays exact in every pose.
  Main joints are fitted only against their parent and the fit is accepted only if it lands on
  the stored joint offset; some models have just 3–6 seam pairs at a joint (too few for a
  reliable fit), and those joints use the standard bind rotation, which is identical in every
  model where the fit is reliable (e.g. clavicles 29/29, pelvis 47/47).
- **Accessory bones** (rows 21+: hair, tails, dress panels, coat tails, skirts) start from their
  neutral angles (row words 7–9) and are driven by a port of the game's spring simulation
  (`0x80037CBC`, called per frame for slots 18–21 = rows 21–24 by the pose solver at `0x8003B418`).
  Each link keeps its tip from the previous frame. The new direction, in the frame
  parent × neutral (where the link points along +X), blends three vectors with per-link weights:
  the lagging tip direction, gravity `(k>>4, (256−k)<<4, k>>4)` (off while the tip is at or below
  the floor y = 0), and the rest direction. That direction becomes yaw/pitch via `ratan2`, with a
  sign table at `0x80095D20`, and the link's local rotation is neutral × RotZYX(0, yaw, pitch).
  Parameters are per player id = (model record − 71) / 4: pointer table `0x8009623C`
  (4 links × `{type, ?, w_lag, w_grav, length, k, collide, slot, point[3], cmp}`).
  Character-specific rules:
  - Thigh collision for Heihachi 2's coat tails and Xiaoyu 3's skirt: a limit point in thigh
    space; the link snaps to it past a half-plane in angle space (`0x80038430`).
  - Anna's dress front panel aims 128 units from the front knee toward the back knee
    (`0x8003866C`).
  - Yaw forced to zero on some Xiaoyu and Heihachi 2 links, angle clamps for Heihachi 1 and Anna, and True Ogre's
    first two links held static.
  Each exported clip runs 90 settle frames on its first pose before frame 0.
- **Row 20** is a second head section driven by the head bone (Law's neck, Lei's hair live there).
- **Extra sections on rows 2/4** (jackets, shirts, straps) are secondary meshes drawn in the frame
  of the section before them — chest for row 2, pelvis for row 4 (row 20 likewise uses the head).
  Where they share welded seams, the frame is confirmed by an exact bind-pose test (the right
  frame makes the seam copies coincide).
- **One-sided polygons**: the PS1 culls back faces unless a polygon's index word has bit 24 set
  (double-sided). The converter writes two primitives (one-sided / double-sided materials) and
  reverses the winding, since PS1 front faces are clockwise relative to glTF.
- Texture part 1 (30 × 32×32 frames) holds **hit-spark effect frames**, not facial animation; a few
  head polygons sample VRAM the game fills at runtime and are left transparent.

`t3_viewer/index.html` is a plain glTF player (GLTFLoader + AnimationMixer): character list,
animation picker, scrub, speed, root lock, lighting/filter/wireframe/skeleton toggles, and
drag-and-drop of any `.glb`.

Known gaps: each clip is simulated on its own, so hair can snap where a clip's data jumps
(in game, clips blend into each other); facial animation, alternate palettes and the
second motion stream (`0x800389C0`, likely hands/face) are not applied.

## 10. Arenas (stages)

15 stages; stage k (0–14) is two BNS records, loaded at `0x80036654` as file ids
`stage + 0x22` and `stage + 0x36` (file id → record via the u16 table at `0x80098574`; record = id + 2 here).

- **Record 36 + k: textures and floor map.** A container `[n = 2, offsets…]`.
  - Part 0: a chain of 4bpp TIM tiles plus their CLUTs, uploaded to VRAM (x ≥ 512, CLUTs at y ≥ 480).
  - Part 1: 24 bytes, unknown.
  - Part 2: the floor map, `u32 128` followed by 10 × 10 entries `(uv, tpage | flags, 0, clut)`.
    Each entry names a 64 × 64 texture tile; flags `0x4000` / `0x8000` mirror the tile and
    `0x400` turns it.
- **Record 56 + k: backdrop geometry.** Header `[65, ?, 36]`, then 36 objects of 7 words:
  `(vertex offset, vertex count, extra offset, poly count, poly offset, poly count, 0)`.
  Offsets are relative to byte 12. Vertices are `(x, y, z, 0)` shorts.
  - Polygons are GPU-packet-like: a colour word whose top byte is the command, then UV words
    `uv0|clut, uv1|tpage, uv2, uv3`, then 4 index bytes.
  - Commands: `0x2C` textured quad (20 B), `0x24` textured triangle (20 B), `0x28` flat quad (8 B).
  - The geometry is a small drum (radius about 1200, height about 860) that is drawn around the
    camera, with its bottom ring on the horizon: a panorama, not a walkable set.

`t3_converter/src/export_stages.rs` writes `stage_NN.glb` for 20 stages:
- 0–14: the arenas, named after their home fighters (character descriptor byte +10);
- 15–18: the Tekken Force stages (records 51–54);
- 19: the Tekken Ball beach (record 55).

How the exported files are built:
- **Sky:** the "sky" node holds the backdrop, to be kept centred on the camera.
- **Floor:** the "floor" node holds one copy of the floor map, repeated 13 × 13 as node instances of one mesh.
  - The tile size comes from the EXE's floor setup (`0x80048630`): **1800 units per tile on a 10 × 10 map**.
    Tekken Force uses 2500 units on 10 × 3.
  - The map is indexed by world position: tile (i, j) shows map row (−j) mod H, column (i + W/2) mod W.
  - Corner UVs come from the EXE corner table `0x8009E800` ([engine.md](engine.md) §5.4).
  - The floor renderer is resident (`0x80049848`, see [engine.md](engine.md) §5.4).
- **Textures:** go through an emulated VRAM (tpage/CLUT) and are packed into one atlas, with vertex colours
  as the PS1 modulation (`colour / 128`).
- **Force stages:** the scrolling backdrop panorama (part 2 of the record) is rebuilt as a textured wall at
  z = 3000 (node "backdrop_wall"), over the corridor floor ([engine.md](engine.md) §13).
  - The event script becomes nodes under "force_events": `enemy_*` (glTF extras: kind, variant, soldier,
    model record, single or stream spawn), `item_*` (the chicken sprite mesh) and `marker_*` (camera locks,
    waits, stage clear). They are placed at the trigger camera x + dx, lane z.
  - Spawn streams are laid out 1500 units apart. In the game they arrive one at a time while enemy slots
    are free, so the static view shows every spawn point at once.
  - The viewer puts the matching soldier model (idle animation) on each enemy node, turns pickups to face
    the camera, and has a level-position slider for the side-scrolling camera.

The viewer's **Arenas** tab loads `t3_viewer/stages/index.json`. It shows the arena with the
current character (animation keeps playing), plus an optional orbit camera.

## 11. Sound effects, music, movies (`export_media`) and viewer tabs

`t3_converter/src/export_media.rs` reads the BIN directly and writes `t3_viewer/media/` plus `index.json`.
- **Sound effects** (`sfx/<bank>/NN.wav`, 51 banks, 857 samples):
  - **System banks:** the system SFX (46 samples) and the announcer (25) are uncompressed inside the EXE:
    headers at file offsets 0x16050 / 0x17270, bodies at 0xC3E6C / 0x10081C.
  - **Character banks:** each character's VAB header is BNS record m+1. Its sample body is part 2 of
    texture container m+2, whose container header holds (offset, size) pairs.
  - **Credits bank:** record 301 holds one more bank with a single 6-second sample.
  - **Decoding:** samples are PS1 SPU ADPCM, decoded to 16-bit WAV at the rate the game plays them.
    libsnd keys note = tone + 60 with fine 0, so rate = 44100·2^((key − centre + shift/128)/12); the tone's
    fine-tune is added (libsnd `0x80089460`). The banks are laid out as drum kits (one key per sample).
  - **Rates:** about 16 092 Hz for system SFX, 7 974 Hz for the announcer and most voices, and 11 025 /
    7 104 / 16 092 Hz for the rest.
- **Music** (`music/NN.flac`): the 50 XA streams from the EXE table (`0x15578`), decoded by
  `psx_media` and kept as FLAC at their own 37.8 kHz, plus CD-DA track 2 (44.1 kHz; track 3 is
  digital silence). Names aren't on the disc.
- **Movies** (`video/NN.avi`, `NN.flac`): STR movies are found by scanning XAS for STR frames with frame 1 /
  chunk 0, and keep channel 1 (MDEC video and its XA audio). `psx_media` decodes the MDEC frames into a
  Motion-JPEG AVI (JPEG quality 85) and the XA audio into a FLAC; the frame rate is frames ÷ audio length
  (15 or 20 fps; the boundaries and rates match §4.2).

The viewer has **Sounds** (bank picker, a play button per sample, play all), **Music** (playlist with
auto-advance), **Video** (movie list: the AVI's frames shown at the soundtrack's time, pixelated or smooth scaling) and **Pictures** tabs.

**Pictures:** `t3_converter/src/export_pictures.rs` decodes the LZ-compressed 2D art ([engine.md](engine.md) §4.4)
from the EXE and records 0, 9–35, 301 and 302, plus the raw Theater backgrounds and the Tekken Ball rules card.
- **Sets:** large portraits, HUD and fonts, title and logos, character select, menus, records, lightning
  panels, Theater thumbnails, results stills; about 200 unique pictures.
- **Assembly:** pieces that tile VRAM, or a 368-pixel-wide screen grid, are joined into the full picture
  (title screen, result stills, lightning panels).
- **Banded palettes:** the 126×252 8bpp portraits store 6-bit pixels (0–63) with a 256-entry CLUT. Each 64-row
  band uses its own 64-colour sub-palette (rows 0–63 → colours 0–63, 64–127 → 64–127, …), which gives 4 × 64
  colours per portrait. Reading them with a single palette gives wrong colours in the lower bands. The viewer's server (`serve`)
answers HTTP Range requests: browsers need them to seek in audio/video.
