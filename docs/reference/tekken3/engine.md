# Tekken 3 (PS1, USA, SLUS-00402): engine reference

This document describes how the Tekken 3 engine works, reconstructed from the main executable
`SLUS_004.02` and the `TEKKEN3.BNS` / `TEKKEN3.XAS` archives. Disc layout, the 3D model and animation
formats, stages and media are covered in [disc.md](disc.md). This file covers what
runs on top of them.

**Conventions**
- Addresses are RAM addresses. The EXE loads at `0x80010000`, so EXE file offset = addr − 0x80010000 + 0x800.
- Overlays are BNS records loaded into one of two fixed slots (§4); their file offset = addr − slot base.
- "Record N" means BNS record N. The engine refers to records by **file id**; see §4.1 for the mapping.
- Confidence: unmarked = confirmed in code or data. *(likely)* = strong evidence. *(guess)* = unverified.
- Everything here is derived from the EXE and disc data. Detailed specs for the move system, attack paths, hit
  reactions, camera and CPU AI are in [the other notes here](../README.md).
- Key values were spot-checked against the binary while writing this: max health 130 (`0x8002260C` = 0x820000),
  life table, damage-zone table, system VAB body and end flags, and the movie table.

---

## Contents
1. [Big picture](#1-big-picture)
2. [Boot, main loop and frame timing](#2-boot-main-loop-and-frame-timing)
3. [Game state machine and modes](#3-game-state-machine-and-modes)
4. [Memory map, overlays, file loading, compression](#4-memory-map-overlays-file-loading-compression)
5. [Rendering](#5-rendering)
6. [Characters at run time](#6-characters-at-run-time)
7. [Combat system](#7-combat-system)
8. [Round logic and CPU AI](#8-round-logic-and-cpu-ai)
9. [Audio](#9-audio)
10. [Music streaming and movies](#10-music-streaming-and-movies)
11. [Data catalogue: every BNS record](#11-data-catalogue-every-bns-record)
12. [Roster, unlocks, select screen, Theater](#12-roster-unlocks-select-screen-theater)
13. [Tekken Force and Tekken Ball](#13-tekken-force-and-tekken-ball)
14. [Pictures (2D assets)](#14-pictures-2d-assets)
15. [Corrections to disc.md](#15-corrections-to-discmd)
16. [Open questions](#16-open-questions)
17. [Quick address reference](#17-quick-address-reference)

---

## 1. Big picture

- **Platform layer:** stock Sony PsyQ libraries plus a thin Namco wrapper.
  - libapi/libc, libgpu, libgte and libgs-style helpers;
  - libetc, libpad, libcard;
  - libcd/libds (CD access) and libsnd/libspu (sound);
  - libpress (movies), which lives in the overlays.
- **Code layout:** game code is about `0x80028000`–`0x80079000`, with the libraries above it to about `0x80096000`.
- **Frame model:** one game-logic step per vertical blank (60 Hz NTSC). Drawing and the buffer flip are
  started from a timer interrupt shortly after VBlank, not from the main loop.
- **Scenes:** a top-level mode number (0–19) selects the current screen. Menus, select, results and the
  Theater are code overlays loaded on demand. The fight itself runs from resident EXE code plus a small
  per-mode overlay (versus, practice, Tekken Force, Tekken Ball).
- **Data:** nearly everything is in `TEKKEN3.BNS` (303 records, read by sector through an async queue).
  - Music and movies stream from `TEKKEN3.XAS`.
  - 2D art is LZ-compressed (§4.4).
  - 3D models, textures, motions and stages are stored raw.
- **Characters:** 3DMK models are skinned on the CPU and GTE by 18 bone matrices plus up to 4 spring
  accessories, lit by the GTE.
  - Moves come from per-character move tables with baked attack trajectories.
  - Hits are capsules (attack) against 14 spheres (body).

---

## 2. Boot, main loop and frame timing

### 2.1 Entry and init
| Address | What |
|---|---|
| `0x80079C70` | `__start`: clears BSS `0x8009B9A8–0x800B0548`, sets stack (RAM size 2 MB, `_stacksize` 0x8000), `gp = 0x8009B9A8`, calls `main` |
| `0x80028BA0` | **`main`**: `__main`, then `boot_init`, then the endless loop |
| `0x800B0548` | **`boot_init`**, one-shot code stored right after BSS. It is overwritten later by overlay slot A |

`boot_init`, in order:
1. CD init `0x8006AB64`. It looks up `\TEKKEN3\TEKKEN3.BNS;1` and `\TEKKEN3\TEKKEN3.XAS;1` with
   `CdSearchFile`/`DsSearchFile` (`0x80091558`); these are the only filename lookups the game ever does.
2. Graphics:
   - `ResetGraph(0)` `0x8007C028`, `SetDispMask(0)`, `ClearImage(0,0,384,480)`.
   - Display init `0x80080848` (GsInitGraph-like) using mode table `0x800B0CC8`: entry 0 =
     **384×480 interlaced**, entry 1 = 320×240 (unused).
   - `InitGeom` `0x80081C50` (ZSF3 0x155, ZSF4 0x100) and **`SetGeomScreen(500)`**.
3. Sound init `0x80074DA4`: SsInit, SS_NOTICK, master volume 127, and the two system VABs uploaded once (§9.2).
4. Memory card events (`InitCARD`/`StartCARD`/`_bu_init`, 8 × `OpenEvent`).
5. Pads `0x800B0B9C`: `PadInitDirect`, `PadSetAct` ×2, `PadStartCom`. Pad records are at `0x800A9130`.
6. Builds the OT descriptors and player slots, then seeds the RNG from the 1 KB scratchpad (`0x800B07E4`).
7. **Frame timer** `0x80029894`:
   - root counter 2 interrupt with handler `0x80029784`;
   - a VSync callback `0x800299E0` that arms it each vertical blank.

### 2.2 Main loop (`0x80028BCC`)
An endless loop; each pass:
1. **Wait** (`0x800296C4`): hands the finished OT to the frame interrupt and spins until the next tick; the spin
   also stirs the RNG.
2. **CD/XA tick** (`0x8006B6FC`), then the frame counter `0x800AFA4C` is incremented.
3. **Buffers:** selects the active buffer's packet area as the GS work base and clears its 1024-entry OT
   (reverse order); resets the 2D lists (`0x8004C684`).
4. **Mode handler:** calls `mode_table[mode]` (table `0x80010000`, mode = s16 `0x800AE204`, < 20).
5. **Links** 2D list A at OT entry 8 (HUD, in front) and 2D list B at entry 1006 (behind the 3D).

### 2.3 Frame timing
- The **VSync callback** `0x800299E0` arms root counter 2 with a delay from table `0x80095438`
  (0x13A7–0x1AE5 ticks, about 1.2–1.6 ms after VBlank).
- The **RCnt2 handler** `0x80029784` is the real frame tick:
  - If the main loop has finished a frame, it calls `ResetGraph(1)`, then the **flip** (`0x80080FB8`:
    PutDispEnv, SetDispMask, PutDrawEnv), then `DrawOTag` of the new OT.
  - If the frame is late, it re-draws the previous OT, so each interlaced field is still painted.
  - It then runs the libsnd tick, the pad actuators, and **pad read + button remap** (`0x80029DC0`, remap
    table `0x80097EB0`).
- **Result:** there is no `VSync()` in the loop. Logic runs at most once per field (60 Hz). A long frame
  drops to 30 Hz without tearing.
- **OT compaction:** `0x80029A28` optionally shortens runs of empty OT entries before DMA.
- **Buffers:** two OTs of 1024 entries (`0x800A6588`, `0x800A7588`) and two packet work areas, but a
  **single interlaced framebuffer**: `GsDefDispBuff(0,0,0,0)` puts both display buffers at the same VRAM origin.

---

## 3. Game state machine and modes

### 3.1 State variables
| Address | Meaning |
|---|---|
| `0x800AE204` (s16) | top-level **mode** 0–19 |
| `0x800AE224` (s16) | sub-state of the mode |
| `0x80097F38` (u8[8]) | scene-transition request: [0] target, [1] previous, [2] "use attract overlay" |
| `0x800AFA88` (u32) | **battle type**: 8 = Tekken Force, 7 = Tekken Ball (record 7's code compares it with 8; the stage loader `0x80036600` sends 7 to record 8); others take the generic versus path |
| `0x800ADC84` (u16) | stage id |
| `0x8009811C` | overlay load queue (≤ 8 × {flags, file id, dest}) |

Helpers:
- `set_mode(m, sub)` `0x80052C30`: switches immediately, or via the loader (mode 1) while overlays are queued.
- `request_scene(n)` `0x8004F8F8`: goes through the fade/transition (mode 2).

### 3.2 Mode table (`0x80010000`)
| Mode | Handler | Code lives in | Screen |
|---:|---|---|---|
| 0 | `0x800B0708` | boot code | boot: upload the HUD pictures packed in the EXE, then go to the title |
| 1 | `0x80052CC4` | EXE | overlay loader |
| 2 | `0x8004FA60` | EXE | scene transition / fade (13 sub-states) |
| 3–5 | `0x800DB1B8`, `0x800DB5B4`, `0x800DDE64` | record 10 | title, main menu, options *(exact split is a guess)* |
| 6 | `0x800D3228` | record 0 | title / attract demo *(likely)* |
| 7 | `0x80050318` | EXE | battle-type setup: load slot-A overlay (records 5–8) |
| 8 | `0x80050428` | EXE | load the select overlay |
| 9 | `0x8010FF4C` | record 9 | **character select** |
| 10 | `0x80055590` | EXE | **grid select** (select screen B: VS, Team Battle, Tekken Ball; draws the cells 0x80054380, the VS tally 0x800B287C) |
| 11 | `0x80052520` | EXE | match setup: load characters, stage and pictures |
| 12–15 | `0x800EEE8C`…`0x800F18E8` | record 302 | results: win/lose, team, time attack, survival |
| 16, 17 | `0x800501C0`, `0x800C2434` | EXE, record 11 | records / name entry |
| 18, 19 | `0x8004FF74`, `0x800FF0C4` | EXE, record 301 | **Theater** |

**Main menu** (its strings are in record 10): one item per mode — arcade, versus, team battle, time attack,
survival, Tekken Ball, Tekken Force, practice, Theater — plus the options.

**Options** cover gameplay (difficulty, round time, fight count, guard damage, continue and select behaviour),
audio (speaker output, BGM column: arcade / arranged / silent) and system (key config, display adjust, memory
card, records). The menu layouts and strings the port uses are converted to `assets/gamedata/screens.json`.

---

## 4. Memory map, overlays, file loading, compression

### 4.1 File id → BNS record (u16 table `0x80098574`)
| File ids | Records | Group |
|---|---|---|
| 0–6 | 5–11 | mode and screen overlays |
| 7 | 0 | title/attract overlay |
| 8, 9 | 301, 302 | Theater overlay, results overlay |
| 10–68 | 12–70 | UI backgrounds, portraits, stage textures, stage backdrops (record = id + 2) |
| 69–72 | 1–4 | character packs used by record 0 (4 is empty) |
| 73–94 | 279–300 | common motion set and canonical moveset packs |
| 95–302 | 71–278 | 52 character groups (record = id − 24) |

### 4.2 Memory map
| Range | Contents |
|---|---|
| `0x80010000` | mode jump table, then rodata (bone/draw table `0x8001A074`, strings) |
| `0x80024400` | BNS index (303 × {sector, size}). `0x80024D78`: XA table. `0x80024FD0`: BGM table |
| `0x80028000–0x80079000` | game code |
| `0x80079C70–0x80096000` | PsyQ libraries, Namco graphics wrapper (`0x80080848–0x80081C50`), libgte, libsnd, libcd, libpad |
| `0x80095000–0x8009B9A8` | initialised data (frame timer, pad remap, file-id table, character descriptors…) |
| `0x8009B9A8–0x800B0548` | BSS |
| `0x800A6588`, `0x800A7588` | ordering tables (2 × 1024 entries) |
| `0x800A9228` | **player structs**: 3 × 0x188C bytes (P1, P2, and P3 used only in Force/Ball) |
| `0x800B0548–~0x800B8B60` | **overlay slot A** (mode logic: records 5–8). Holds `boot_init` at boot |
| `0x800B8D58–~0x80144100` | **overlay slot B** (screens: records 0, 9, 10, 11, 301, 302). Holds the EXE's packed HUD pictures and the system VAB bodies at boot |
| `0x1F800000` | scratchpad: projected vertices and Z, lit colours, pose temporaries |

### 4.3 File loading
- **Async loader** `bns_load_async(id, dest, done_cb)` `0x8006BF20`:
  - maps the id to a record, then to (sector, size);
  - queues up to 8 requests, kept sorted by record number to reduce seeking;
  - reads with `CdlReadN` and `CdGetSector` in 2 KB steps (`0x8006C2A0` callback), and retries on disc errors.
- **Sync loader** `0x8006C470`. After loading code (file id < 10) the loader calls `FlushCache`.
- **Character load** `0x80036310`: file ids `4·model + 0x5F … 0x62` (model, VAB header, textures, motions).
  The motion record is skipped if the motion cache (`0x800A0510`) already holds it.
- **Stage load** `0x80036654` / `0x8006C864`: ids `stage + 0x22` (textures + floor) and `stage + 0x36`
  (backdrop; only for stages whose bit is set in mask 0x34AF).
- **Overlays are stored uncompressed** and linked for their slot (no relocation).

### 4.4 LZ compression (`0x80031BFC`)
All compressed 2D art uses one byte-oriented LZ77 scheme:
- **Flag bytes.** The stream is a sequence of groups, each led by a flag byte. A flag byte of 0 ends the stream
  (also as the very first byte). The flag bits are read from the least significant end; the highest set bit is a
  sentinel, not a flag, so a flag byte with k data bits describes k items.
- **Items.** Flag bit 1 = a literal byte copied to the output. Flag bit 0 = a 2-byte big-endian back-reference
  token: the low 11 bits are the distance (0 means 2048), the top 5 bits the length (0 means 32). The bytes are
  copied one at a time from that far back in the output, so a copy may overlap what it is writing.
- **Containers:** `u32 n; n × {u32 offset, u32 size}`. Each member is either an LZ stream (pictures) or raw
  data (stage and Force records, records 1–3).
- **Users:** `upload_lz_tims` `0x8004CA40` decompresses each member into a TIM and uploads it with
  OpenTIM/ReadTIM/LoadImage. Decompression is also called from overlays (record 9 `0x8010E56C`, record 0 `0x800D4254`).
- **Verification:** every stream ends exactly where the next member starts, and the outputs are valid TIMs.

---

## 5. Rendering

### 5.1 Display and ordering tables
- **Resolution:** 368×480 interlaced visible, inside a 384×480 VRAM area (clip 0,20–368,448). Floor,
  backdrop and sprites all clip to x < 368, y < 480.
- **Aspect:** the view matrix scales the Y row by 7122/4096 ≈ 1.739 to correct the tall interlaced pixels
  (`0x80045CA8`); billboards do the same.
- **Projection:** distance H defaults to 500. The fight camera zooms by changing H (§5.3).
- **Ordering tables:** cleared with `ClearOTagR`, so high entries are drawn first. Layout:

| OT entry | Content |
|---|---|
| 1023 | background TILE (clear / sky colour) |
| ~1021 − k | floor tiles (POLY_GT4) |
| 1010 | backdrop drum (its own 2-entry OT, spliced in) |
| 1007 | character shadows |
| 1006 | 2D list B (behind 3D) |
| (Za + Zb)/2 − bias | character polygons |
| SZ/4 − 0x6E | particles |
| 8 | 2D list A: HUD, text, health bars |
| 1 | front-most particles |

### 5.2 Character renderer (`0x80037924` → `0x80036CAC` per bone section)
1. **Skeleton:** `0x8003AFCC` writes, for each of 18 bones plus 4 accessories, a world matrix
   (`player+0x8F4+slot·0x44`) and a view matrix (camera ∘ world, at +0x20 of the same record).
2. **Light in bone space:** for each of 22 draw entries (bone from table `0x8001A074`), `SetLightMatrix(light ×
   boneRotation)`, so normals stay in mesh space.
3. **Transform:** vertices go through **RTPT**/RTPS. The engine stores screen XY and `Z = SZ >> 5` in the scratchpad.
4. **Seam welding in screen space:** shared vertices between two bones are averaged after projection.
   - One bone exports half of its projected vertex; the other adds its own half.
   - This is the 9-bit weld list format in disc.md §8, and the reason the glTF export uses 0.5/0.5 skin weights.
5. **Lighting:** **NCCT/NCCS**: colour = material × (ambient + colourMatrix × max(0, lightMatrix·n)).
   - Per-stage table `0x80096C78` (28 B per stage) gives the ambient, material colour, key-light colour and
     direction, and the shadow direction.
   - Light 1 is the key light; light 3 is a fixed vertical fill at half colour.
   - There is no depth cue or fog on characters.
   - **Colours are recomputed only every other frame** per packet buffer, staggered between the two players
     (`0x80037970`). This halves the lighting cost.
6. **Culling and depth:**
   - NCLIP culls one-sided polygons (bit 24 of the index word marks double-sided ones).
   - The OT index is computed in software: `(Za + Zb − bias)/2`, with a signed 7-bit per-polygon depth bias. AVSZ is not used.
7. **Packets:** POLY_FT3 0x24, FT4 0x2C, GT3 0x34, GT4 0x3C, **pre-built** at load. Each frame only the XY, tag
   and colour words are written.
8. **Faces** *(likely)*: expressions swap section descriptors for draw entries 12/16/17 (`0x80034970`). One
   character (id 0x11) also copies VRAM rectangles (`0x800293BC`); this is the runtime-filled texture area
   seen on a few head polygons.

### 5.3 Camera
Full spec: [camera.md](camera.md). Reference implementation:
`t3_converter/src/reference/camera.rs` (integer-exact). Rust port: OpenKumite's `crates/kumite_sim/src/camera.rs`
(versus camera + round intro; tables from the converted `gamedata/camera.json`), checked frame by frame
against golden traces of the prototype (`reference/camera_golden` → `assets/tests/camera_golden.json`).
- **View struct** "RVIEW" at `0x800A85C0`: pitch, yaw, roll (4096 per turn) and eye position. `0x80045CA8`
  builds the camera matrix `0x800ADF70`, applying the aspect factor.
- **Camera driver** `0x800630D8` runs for every battle type. Tekken Force hands off to its overlay; Tekken Ball
  adds the ball as a third framing point.
- **Key slots** (`0x800A02A8`, 24 B each: eye, pitch<<6, yaw<<6, H):
  - 0: normal fight camera;
  - 1: throw and replay angle cameras;
  - 2: scripted camera animations and the round intro;
  - 3: last output;
  - 4: frozen snapshot.

  Slots are weighted and mixed; a two-slot blend (`0x80064170`) orbits a blended look-at point.
- **Normal fight camera** (`0x80064810` yaw controller + `0x80064E10` framing solver):
  - **Position:** both fighters are sampled every frame. The eye sits **5517 units** behind their midpoint along
    the camera yaw, at about −1112 height, looking level.
  - **Yaw:** chases the P1→P2 line. It starts turning when 5° off, brakes inside 20°, and turns at most about 1.2°
    per frame. Errors over 150° freeze it (instant side swaps keep the players swapped).
  - **Spacing:** each fighter gets a 150-unit margin and the pair must fit in 280 px. The camera pulls back at
    1/8 of the excess per frame and returns at 1/32. There is a ±48 px dead zone when close.
  - **Jumps:** the higher fighter is kept 112 px below the top edge (dolly back 1/8, return 1/16).
  - **Pitch:** 0, except a 12–17° tilt while the yaw lags by more than 20° during fast moves.
- **Zoom:** the projection H stays at **500** in fights (clamp limits both 500). Authored zoom in scripted keys
  becomes a dolly.
- **Throw and special cameras:** move flags (+0x24 low byte) select weighted camera scripts (moveset header +0x20).
  They either pick one of 30 preset angles (yaw ±30°/±60°, with a 35-frame blend or a cut) or play keyframed
  paths (header +0x24, encoding not decoded).
- **Round intro:** 10 fly-in variants (120 frames in the first round, 53 otherwise), blended into the fight
  camera over the last 16 frames.
- **KO:** two alternating variants with randomised radius, height and drift (`0x800465BC` / `0x80046A20`,
  chosen by `0x8006870C`).
- **Win:** an orbit of the winner, or an overhead shot of the loser from 7853 units up.
- **Replay** `0x80069414`: starts at 11034 units and eases to 4413, with a random yaw sweep. A likely game bug
  stores the replay yaw in 16 bits, so it wraps every 90°.
- **Camera shake** `0x8004AD34`: scripts of 6, 24 and 66 frames added to the pitch.

### 5.4 Stage
- **Floor** (resident, `0x80049848`, selected through function pointer `0x8009E728`):
  - The 10×10 tile map (the 808-byte part of the stage texture record) is laid around the players'
    midpoint, snapped to the tile size and wrapping, projected with RTPT, and emitted as **POLY_GT4** near
    OT entry 1021.
  - Tile size is **1800 units** with a 10 × 10 wrap (Tekken Force: 2500 units, 10 × 3), set at `0x80048630`.
  - World tile (i, j) shows map row (−j) mod H, column (i + W/2) mod W.
  - Each entry is `{u32 w0, u32 w1}`: w0 = base UV | tpage << 16, with bits 30–31 selecting a row of the corner
    table `0x8009E800`; bits 0x600 of the tpage half permute the corners (0x200 mirror u, 0x400 mirror v);
    the CLUT is w1 >> 16.
  - Tile flags mirror and rotate the UVs.
  - **Vertex colours are a CPU "spotlight" falloff** `min(0xFE, K / dist²)`, from one light per player, their
    midpoint, a stage object, or a point in front of the camera depending on the stage. This is the only
    fog-like effect in the game; the GTE depth cue is never used.
- **Backdrop drum** `0x8006D7CC` → `0x8006D014` → `0x8006CC28`:
  - a world-oriented object in units of 8 world units (translation `>> 3`), centred on the fighters' root
    midpoint (x, z), its base ring `STAGE_Y` below the floor (per-stage table `0x80025138`, one u16 per 4 bytes
    for stages 0–14, a few hundred units; each converted stage's glb carries its value as the scene extra
    `drum_base_y`), projected with the camera rotation and
    H = 500 but a 5/3 Y row (the world uses 7122/4096); a small spin `0x800A9188` adds parallax
    (earlier notes said "camera rotation only, centred on the camera": wrong);
  - a per-object visibility test against yaw;
  - no NCLIP and no sorting: data order is painter order;
  - POLY_FT4/FT3/F4 into a private 2-entry OT linked at entry 1010.
- **Animated stage props:** about 20 per-stage routines (`0x8006F5C0` dispatch).
- **Shadows:** planar projected silhouettes.
  - A shear matrix built from the stage's shadow angles flattens each bone onto y = 0 (`0x80035F2C`, `0x800ADE48`).
  - The shadow vertices go into a pre-linked chain of flat POLY_F3/F4 at entry 1007.
  - The mesh is not the visible model: it is a low-poly mesh of its own, section 3 of the model's texture record
    (record m + 2, `0x80036510`). +8 face list, +0xC / +0x10 triangle and quad counts, +0x14 18 per-bone-slot
    vertex lists {u32 n; n × (s16 x, y, z, pad)} in the slot's local frame (indices count each list padded to a
    multiple of 3). Faces: u32 n then words with indices at bits 2, 10, 20 (triangles); u32 n then 4 index bytes
    (quads).
  - Most fighters share one mesh (77 vertices, 20 triangles, 44 quads). Yoshimitsu, the bears and True Ogre,
    Mokujin and Gon have their own; the Tekken Force soldiers have none.
  - Opaque, in the stage table's colour at +0x10. Airborne fighters still cast them.
  - Drawn only while sin(camera pitch + 0xEF) > 0, i.e. the camera looks no more than about 21° up.
  - Every mode draws them (arcade, VS, team, time attack, survival, practice, the attract fights, Tekken Ball on
    stage 19, whose light is straight down and colour black) except Tekken Force (battle type 8). The word at
    0x800ADCA4, read before the draw, is a skip-all flag the retail game never sets.
  - **Tekken Force blobs** (`0x800793B8`, for every drawn actor): four POLY_GT4 (code 0x3E, tpage 0x48: ABR 2,
    subtract) on the floor y = 0, straight down. They sample a white texel (CLUT (272, 511) all 0xFFFF), so they
    take colour × 31 / 128 off the floor, the colour being the table's +0x18 on the ridge (v0, v7) and black at
    the rim. M = the midpoint of the feet (slots 14 and 17); P, Q = the feet relative to it; t0, a3, a2 = 160,
    220, 300. Lying (the move's posture & 0x204 with the head at y ≥ −499): P = head − M pushed out 128 on each
    axis, Q = (∓64, ∓64), sizes 200, 400, 400. Then along x (dz ≤ dx / 2), along z (dz ≥ 2 dx), else diagonal
    (sizes × 71 / 100), each with its own eight vertices; quads (1,0,2,3), (3,0,4,7), (4,7,5,6), (6,7,1,0)
    (0x80098A54). Fixed size, no fade with height.
  - **The Tekken Ball ball** (record 8, `0x800B3640`): an octagon of radius 384 under the ball (0x800B5FE4 +
    0x90), turned and squashed as the ball and scaled in x and z by (y / 2 + 4096) / 4096; four flat opaque
    POLY_F4 (code 0x29) in the table's +0x10, OT 1007, no camera test.
  - Converted by `export_shadows` to `gamedata/shadows.json`. The port binds each vertex to its joint and
    flattens it in `vs_shadow`.

### 5.5 Effects and HUD
- **Billboard particles** `0x80076E70`:
  - each particle is a camera-facing POLY_FT4 with velocity, acceleration and a lifetime fade;
  - UVs animate through a texture atlas, so the 30 hit-spark frames in each character's texture container
    fit this system *(likely)*;
  - effect manager `0x8004A5C8`.
- **Text:** a printf-style renderer `0x8004CE74` draws each glyph as a SPRT (0x65). It supports colour,
  position, scale and font codes, and writes to 2D list A.
- **Health bars:** `0x8004E8B4`. Each bar is a sprite whose width is the health, plus a semi-transparent FT4.
- **HUD driver:** `0x8003D6B0` (bars, timer, round markers, plus overlay HUD code).
- **No swept weapon or limb trails** were found in resident code.

---

## 6. Characters at run time

### 6.1 Player structure (`0x800A9228` + n·0x188C)
| Offset | Meaning |
|---|---|
| +0x00 / +0x08 | world X / Z |
| +0x12 | player index |
| +0x16 / +0x18 / +0x1C | moveset key, character id, model id |
| +0x30 | angle relative to the opponent (< 0x4000 = facing them) |
| +0x54 / +0x58 | current move record / frame in the move |
| +0x5E, +0x60, +0x64, +0x66 | current damage, posture flags, attack mask, guard mask |
| +0xC5 | CPU-controlled |
| +0x1AC / +0x1C4 | up to 2 attack segments (start, end) |
| +0x20C | 14 hurt spheres |
| +0x330 | 8 body push spheres |
| +0x3F4 / +0x3F8 | **health / max health** (16.16 fixed point) |
| +0x402…+0x41C | input state + 60-frame input history |
| +0x4F0 | 22 draw entries (section and packet pointers) |
| +0x8F4 + 0x44·slot | bone world and view matrices |
| +0xF74 + 0x20·slot | bone local matrices |
| +0x1360… | accessory spring state |

### 6.2 Animation and pose
Documented in disc.md §9:
- motion decoder `0x80038B4C`, then the 49-channel pose;
- pose solver `0x8003AD10` (Euler angles plus 2-bone IK);
- skeleton `0x8003AFCC`;
- accessory springs `0x80037CBC`.

The per-player update is `0x8003C278`.

Motion data comes in three layers:
- each character's own record (m+3);
- a **common motion set** (record 279) that every character uses for shared moves and reactions;
- canonical moveset packs (records 280–300), loaded through a 2-player + common motion cache at `0x800A0510`.

---

## 7. Combat system

### 7.1 Move data (record m+3, relocated in place by `0x8002CE14`)
| Header | Contents |
|---|---|
| +0x00 | u8 relocated flag, u8 **moveset id**, s16 move-record count |
| +0x04 | command / cancel lists (12-byte entries) |
| +0x08 | **move records** (56 bytes) |
| +0x0C | **alias table** (4023 move ids → own record, common-bank record, or missing) |
| +0x10 / +0x14 | voice/SFX event lists / timed property lists |
| +0x18 | bitmap of moves this character lacks; at match load they are borrowed from the opponent's table (this is how throw-victim animations defined in the attacker's file become reachable) |
| +0x1C | motion area (keyframes) |
| +0x20 | weighted random tables that pick a camera script |
| +0x24 | packed camera keyframe data (encoding not decoded) |
| +0x28 | attack (hitbox) descriptors |

**Move record** (56 bytes). Field details are in [move-properties.md](move-properties.md).
- +0x00 motion (bit 30 = motion from the common bank);
- +0x04 posture flags:
  - bits 0–2: base posture (crouch / stand / down);
  - 0x08 / 0x10: guard;
  - 0x20 / 0x40 / 0x80: throwable crouching / standing / grounded;
  - 0x100: guard only while holding guard;
  - 0x400: airborne;
  - bits 11–15: state code;
  - higher bits: CPU-AI hints (range class, throw, string starter);
- +0x08 attack mask and hit level;
- +0x0C cancel-list start;
- +0x10 move to play when the animation ends;
- +0x14 base damage (14 bits) and damage type;
- +0x18 frame count;
- +0x1C sound list;
- +0x20 timed properties;
- +0x28 attack descriptor;
- +0x2D / +0x2E **first / last active frame**;
- +0x24 low byte = camera script; other bits = unthrowable, not parryable, ki charge, chain throw,
  guard-by-input, push spheres off;
- +0x2C hit-stop frames (0 for all of Paul's moves);
- +0x30 low half: body push-sphere control (shrink / disable / grow, optionally only around active frames);
- +0x32 hit-reaction id (633-entry table `0x80010CE8`, §7.5);
- +0x34 close-range variant.

**Timed properties** (list at header +0x14, record +0x20): `{u16 frame, u8 arg, u8 op}`. Each entry fires once,
when the move's frame passes it (`0x800458C8`).

| Op | Effect |
|---|---|
| 0x01–0x03 | screen shake small / medium / large (+ pad rumble) |
| 0x04 | ground-impact effect at the root (+ rumble) |
| 0x05 | no handler (ignored) |
| 0x06–0x0A | spark/effect spawn at a bone slot |
| 0x0B / 0x0C / 0x0D | damage to the opponent / damage to self / heal (arg 0 = the move's own damage) |
| 0x0E–0x3D | hand poses (fist, open, special shapes), arg = blend time; 0x15 = default hand |

For example, Paul's throw deals its 30 damage with op 0x0B at frame 75.

**Voice/SFX lists** (header +0x10): each entry is a 4-bit type plus a 12-bit index into a sound table (mapped to
`SePlay` codes, §9.1). They cover impact sounds on hit and block (chosen by arm or leg), voices on frame 1,
timed sound scripts, and voice control. Type-6 entries (on almost every attack, probably whooshes) are skipped
by the list processor.

**Attack descriptor** (record +0x28). Full spec: [attack-paths.md](attack-paths.md).
Reference decoder: `t3_converter/src/reference/attackpath.rs`.
- **Header:** 6 bytes. Bytes 0–3 are bone slots (end0, start0, end1, start1; a start of 0 means "this bone's end
  point on the previous frame", so fast limbs are swept). Bytes 4–5 (big-endian) hold a **layout code** in the
  low nibble and, in bits 4–14, the offset of a second data set.
- **Data:** absolute **5-byte points** (`0x80069714`: x 13 bits, y 13 bits, z 14 bits, biased; about 1 mm
  units) in character space, one or more per active frame.
- **Layout codes:** the 13 valid codes (jump tables `0x80024344` / `0x80024384` / `0x800243C4`) fix how many
  points a frame stores and how they form **at most 2 segments**.
- **Size:** paths are pre-scaled to the character's model size (3DMK header byte +4: King 106 %, Gon 45 %).
  The second data set (unscaled) is used only by Ogre and Mokujin, who borrow other characters' moves.
- **Placement:** `0x8006A230` rotates each point by the actor's facing and adds the root (with a pivot turn if
  the facing changed mid-move).
- **No capsule radius:** `0x8004770C` tests a bare segment against each hurt sphere (box reject, clip to the
  sphere's height band, then a horizontal distance check). Hurt spheres therefore act like vertical cylinders.
- **Verification:** decoded points match the animated wrist/ankle/elbow/knee joints within a few cm
  (Paul median about 40 units, 18 units from the bone's path).

**Example** (Paul, record 74, standing command list): input 1 (the jab) is a high with 5 damage, active on
frame 8 of 37. Every move's damage, level, active frames and frame count are in the converted movesets
(`assets/gamedata/movesets/moveset_NN.json`; Paul is `moveset_00.json`).

### 7.2 Input and command matching
- **Pad conversion** `0x8002C580`:
  - the pad becomes a **numpad direction** relative to facing (table `0x80010738`) plus buttons:
    □ = 1, △ = 2, × = 3, ○ = 4;
  - each frame is stored as `buttons<<4 | dir` in a **60-frame history ring**.
- **Single-frame commands** `0x8002CC28`: a bit set of accepted directions plus required buttons, for example
  `0x0083` = d/f+1+2.
- **Motion inputs** `0x8002CA84`: step sequences matched backwards through the history within a frame window
  (tables `0x800954D0`, `0x800955CC`), for example qcf+1+2.
- **Cancel walk** `0x8002DE24` (each frame) walks the current move's cancel list, including shared lists in the
  EXE (`0x80017C78`, 672 entries). A match queues the next move; `0xC000` links to the next move at animation end.
- **Cancel entry** (12 bytes: `u16 cmd, u8 req, u8 cond, u16 param, u16 dest, u8 transition, u8 win_start,
  u8 win_end, u8 takeover`):

| Field | Meaning |
|---|---|
| `req` | character/moveset gate: 0x01–0x17 = only moveset req−1; other ranges = not this moveset, opponent's moveset is, only character, not character |
| `cond` | state condition (0x01–0x44, `0x8002E0BC` / `0x8002E8D0`). 0x02–0x13 are throw connects (standing / crouching / grounded opponent × front / either side / back); 0x24 = back turned, 0x25/0x26 = back turned with the opponent on the left/right; 0x3A/0x3B/0x3C = press 1 / 2 / 1+2 while being thrown (escapes). 0x45–0x4B are checked only on the frame you are hit: parries and reversals |
| `win_start/end` | frames of the current move during which the input is accepted |
| `takeover` | frame at which the buffered move takes over (255 = at animation end). Paul's jab accepts 2 on frames 1–12 and switches at frame 8 |
| `transition` | bit 7 turns the fighter 180°; the low 6 bits pick one of 44 transition types |

- **Special commands:** 0xC001 / 0xC002 = forward dash (f, n, f) / backdash (b, n, b); 0xC00C calls a shared
  sub-list and 0xC00D returns from it.
- **CPU players** use the same parser with synthesised pad input (§8.2).

### 7.3 Hit detection
- **Hurt spheres:** 14 per character, radii from a per-character table (`0x80096FF0`, 22 pointers to 14 × s16;
  most humans share one set, from 100 to 400 units). The sphere index is the **body zone**. Radii and layout:
  `assets/gamedata/rules.json` → `hurt_spheres`.
- **Collision test** `0x8004770C`: each attack segment against each sphere (box reject, height-band clip,
  horizontal distance), so the spheres act like vertical cylinders. Hurtbox loop `0x80047CBC`. Both directions are
  tested before any reaction is applied, so **trades** hit both fighters at full damage, and neither counts as a
  counter hit.
- **Push spheres:** 8 per character (`0x80096F60`) keep the fighters apart; they are switched off by a move
  flag or during active frames.

### 7.4 Guarding and damage (`0x80044A8C`, `0x80044BD8`)
- **Block:** the defender's guard mask (standing 0x10 / crouching 0x08) ANDed with the attack mask, and the
  defender facing the attacker.
  - The attack mask's high byte is the level: 1 low, 2 mid, 3 special mid, 4 high, 6 unblockable.
  - Mask low bits are tested against the victim's posture: 0x01 hits crouching, 0x02 hits standing, **0x04 hits a
    grounded opponent**. Highs (0x412) are the only standard level without 0x01 or 0x04.
  - Back hits are never blocked (the guard needs the attacker within ±90° in front).
- **Damage:**
  - **Clean hit:** `base × zone% / 100`, with one percentage per body zone (table `0x8001DEEC`, 14 × u16, from
    90 to 140; `rules.json` → `damage_zones`).
  - **Counter hit** (defender hit during their own move's start-up): ×1.2, plus about 0.09 × the defender's own
    move damage × (their frame / their start-up). A 120-frame "ki charge" timer (+0x122) raises that to 0.3 × and
    forces counter hits and chip damage.
  - **Close-range variant:** ×1.5.
  - **Airborne (juggle):** the first air hit does 80 %, later ones 50 %. The counter at +0x94 is reset when a move
    starts that wasn't caused by a hit. Each extra air hit also pops the victim 7 units lower.
  - **Chip damage** on block (when guard damage is on): damage/10 + 2.
- **Health:** stored as 16.16 at +0x3F4. **Default max health 130.** The life option table (`0x800225EC`, 8 ×
  16.16) offers eight maximums in steps of 15 (`rules.json` → `health.life_options`).
- **Throws** are damage-type-1 moves at a normal level. Paul's are high (0x412), so crouchers avoid them.
  - **Connecting:** on contact (frames 12–14, within 1536 units) the thrower picks a front, back or side throw from
    the relative orientation. Table `0x800174C4` maps the thrower's move to the victim's animation, and the damage
    comes from a timed property.
  - **Escapes:** press 1 against 1+3 throws and 2 against 2+4 throws. The window is frames 1–15 for front throws,
    1–5 for side throws, and none for back throws. The first button pressed in the window decides the result,
    so a wrong button prevents the escape. A break is even: both fighters spend 30 frames.
  - **Simultaneous throws:** the game picks one at random.

### 7.5 Hit reactions (`0x8002EBA4`)
Full spec: [reactions.md](reactions.md). Each reaction entry (`0x80010CE8`, 633 × 42 B)
holds 14 victim move ids and 7 pushback parameters. The slot is chosen in this order (st = 0 standing,
1 crouching):

| Situation | Slot |
|---|---|
| blocked | 10 + st |
| airborne / juggled | a common air table at `0x8001A630` (direction, a downward "spike" strike, or already falling) |
| lying on the ground | 12 face-up, 13 face-down |
| counter hit | 2 + st |
| otherwise, by direction (±45° quadrants, `0x80043A94`) | front 0, side A 4, side B 6, back 8, each + st |

- **KO:** if the chosen reaction would return to neutral, the victim is sent to the air table instead (or a
  collapse animation when `0x800AFA93` is set). A chip-damage KO becomes a clean hit.
- **Parameters:** they only describe **pushback**: an angle offset and curves for normal, counter, grounded and
  block hits (p2 unused). Each of the 71 curves at `0x8001075C` is 10 s16: frames of constant velocity, the
  velocity, then 8 per-frame impulses.
- **No stun values:** hit and block stun are not stored. They follow from each move's first actionable frame
  (the start frame of its END cancel entry, with a 5-frame input buffer) and the hit-stop byte +0x2C.
  - Advantage = W_victim − W_attacker + A, where A is the attacker's first active frame.
  - Paul: 1 = +7 on hit, +2 on block, +11 on counter hit; 2 and d/f+1 = +6 / 0; d+4 = knockdown, −34 on block.
    These may be off by one frame (unconfirmed without an emulator trace).

---

## 8. Round logic and CPU AI

### 8.1 Round state machine (`0x8003CB84`, state `0x80096F2C`, table `0x8001A354`)
| State | Phase |
|---|---|
| 0 | intro |
| 1 | round call ("Round N… Fight") |
| 2 | **fighting**: clock (`0x800ADBCC`) counts down, KO when HP ≤ 0 |
| 3 | KO / time-up slow-down |
| 4 | replay / winner pose |
| 5 | round result (300 frames; waits for the win/lose motions to finish) |
| 6–9 | match end, next round, continue |

- **Round time:** (option + 2) × 600 frames, i.e. 20–60 s. The default is 40 s.
- **Time-out:** the winner is decided by health-bar units.
- **No ring-out.**
- **Win/lose motions:** fixed move ids 0xD67–0xD6A.

### 8.2 CPU AI (`0x80059890`)
Full spec, including all 30 profiles: [ai.md](ai.md).
- **Input:** the AI runs every frame for CPU-controlled players and **synthesises pad input** (`0x800618C0`),
  which goes through the normal command parser.
- **No per-character move lists.** Each frame the AI:
  1. collects its current move's cancel entries;
  2. filters them for the situation using flag bits in the move records (range class close / mid / long, throw
     `0x400000`, string starter in +0x24);
  3. picks one at random.
- **Priority each frame:**
  1. canned input playback;
  2. follow-ups after a hit;
  3. throw escape;
  4. stun recovery;
  5. wake-up;
  6. per-character handlers;
  7. reacting inside the opponent's string;
  8. reacting to the opponent's first attack;
  9. neutral offense;
  10. movement: walk, dash, run, backdash or sidestep by range zone.
- **Reactions:** interrupt, guard (down-back, or back against mids), evade unblockables and throws, sidestep,
  parry/reversal, duck. Each is gated by a profile probability and by "frames until his hit lands"; there is no
  separate guard delay.
- **Learning:**
  - every hit taken raises the guard and counter probabilities, up to caps in the profile;
  - the 3rd hit by the same move switches the AI to its stage-9 profile for 600 frames.
- **Profile** (110 bytes, 3 × 10 matrix difficulty × arcade stage at `0x800231AC`):
  - wake-up intervals;
  - attack cooldowns (EASY stage 0: 150/200/200/250 frames; HARD stage 9: 10/30/30/30);
  - maximum string length;
  - probabilities out of 4096: throw, throw escape, string continue, guard, counter, sidestep, in-string guard
    and interrupt (with learning caps), evade, parry, anti-air.
- **Spacing table** `0x80098260`: values 0–3 are reach boundaries (zones 1–5 for "can I reach him" / "can he
  reach me"); value 5 is a body offset (0); value 4 is unused. King, Xiaoyu, Kuma, Gun Jack, Gon and True Ogre
  differ from the default.
- **Per-character data:**
  - `0x80022D14`: a "can parry" flag and canned input sequences (Law's reversal, low parries, motion openers);
  - `0x80023134`: special handlers against Paul, Yoshimitsu, Ogre and Gon, and own routines for King (canned
    throws) and Hwoarang.
- **Original bug:** the planned follow-up executor never fires in the retail game (its match counter is never
  incremented), so profile fields 0x2E–0x38 are mostly dead.
- **Opponent selection** (record 5):
  - **Arcade** `0x800B1FCC`: stages 1–4 from the original ten characters; 5–8 from all unlocked characters except
    Heihachi / Ogre / True Ogre; stage 9 Heihachi (Jin if you play Heihachi); stage 10 Ogre. AI level = stage.
  - **Team** `0x800B225C`: picks from the most-used and least-picked characters, with a level from the
    difficulty plus position bonuses.
  - **Survival** `0x800B26D0`: the pool grows at 7 and 17 wins (never Mokujin, Gon or Dr. B). It avoids your
    last 4 opponents, forces MEDIUM (raised to HARD at random later), and sets the level from the streak.

---

## 9. Audio

### 9.1 Driver
- **Library:** stock **libsnd/libspu**. There are **no sequences** (no SEQ/SEP data on the disc); libsnd is
  used only for VAB management and per-voice key on/off.
- **SPU RAM** is laid out statically (table `0x8002790C`):

| VAB id | Contents | SPU address |
|---|---|---|
| 0 | system SFX (46 samples) | 0x01010 |
| 1 | announcer (25 samples) | 0x3D9C0 |
| 2 | fighter A's character bank | 0x56020 |
| 3 | fighter B's character bank | top of SPU RAM minus its size (`0x800278A8`) |

  Mirror matches share one bank.
- **`SePlay(player, code, atten)`** `0x800753BC` takes a 16-bit sound code:

| Bits | Meaning |
|---|---|
| 15–14 | bank (system / announcer / own character) |
| 13–10 | voice (voices ≥ 6 are offset by the player) |
| 9–6 | volume index (table `0x800278FC`) |
| 5–4 | program |
| 3–0 | tone |

  It calls `SsUtKeyOffV`, then `SsUtKeyOnV` with **note = tone + 60, fine = 0**.
- **Voices** are fixed by the code: voice 1 = announcer, voices 16–23 = a round-robin pool.
- **No panning:** left and right volumes are always equal.
- **Echo:** two helper tasks repeat a sound 15 frames later at ½ and ¼ volume.
- **Pitch:** `rate = 44100 · 2^((note − centre + shift/128) / 12)` (libsnd `0x80089460`). Every VAB is a drum
  kit (one key per sample), so each sample has one fixed rate:
  - system SFX ≈ 16 092 Hz;
  - announcer ≈ 7 974 Hz;
  - most character voices ≈ 7 974 Hz, others 11 025 / 7 104 / 16 092 Hz.
- **Reverb** (room mode, voice 1 only) is switched on only for the ending staff roll.

### 9.2 Where the sounds are
| Bank | Header | Sample body |
|---|---|---|
| System SFX | EXE `0x80025850` (file 0x16050) | **EXE `0x800D366C` (file 0xC3E6C), 0x3C9B0 bytes**, uploaded once at boot (`0x80074E10`) |
| Announcer | EXE `0x80026A70` (file 0x17270) | **EXE `0x8011001C` (file 0x10081C), 0x18660 bytes** |
| Character | record m+1 | record m+2, container part 2 |
| Credits | record 301 + 0x277C4 | follows its header |

- **Announcer** [C]: the calls are entries of the move-sound table played with `0x80040C00(index)`
  (gamedata `sound.json` → `announcer`):
  - round call `0x8003DAC4`: index 0x3B "ROUND", then the number from `0x8001A3BA[round]` (0x34..0x39, 0x3D,
    0x3E = one..eight), then 0x3C "FIGHT", at call frames 61/100/150 in round 1 and 1/40/90 later; when the round
    number reaches the round limit (`0x800ADF3C`) it is 0x3A "FINAL ROUND" without a number;
  - round result `0x8003DEC0`, from the flags of `0x8003E474` (1 time up, 2 K.O., 4 double K.O., 8 perfect,
    0x10 winner below 0xCD/0x1000 of the bar, 0x20 draw): time up 0x13, perfect 0x19, double K.O. 0x15,
    "great" 0x18, else K.O. 0x1A;
  - match end `0x8003E060`: equal → 0x14 (draw), else 0x11 "YOU WIN" if the winner is human, 0x12 "YOU LOSE";
  - continue `0x80050100`: 0x16 at the start, index 4 + seconds left while counting down, 0x17 at zero.
  Clip lengths agree (FINAL ROUND 1.5 s vs ROUND 0.6 s; DOUBLE K.O. 1.44 s vs K.O. 1.11 s). Labels of 0x16 / 0x17
  ("continue?" / "game over") and 0x18 ("great") follow from where they play [G].
  "&lt;NAME&gt; … WINS" is the character's own tone 0 followed by an announcer clip *(the words are a guess)*.
- **XA** carries only music and movie soundtracks.

---

## 10. Music streaming and movies

### 10.1 CD / XA layer
- **Library:** libds (`DsSearchFile`, `DsPacket` `0x8008F08C`, `DsControl`). State struct at `0x800A05D8`.
- **State machine:** `0x8006B6FC` runs every frame.
- **Playback:** XA is read with `CdlReadS` at double speed, and the channel is selected with `CdlSetfilter(file 1, channel)`.
- **Looping:** polled with `CdlGetlocP`; the stream restarts at the end sector.

### 10.2 Music selection (`PlayBgm(id)` `0x8006AE14`)
- **BGM table** `0x80024FD0`: 32 ids × 2 columns. Each column is `{xa index | 0x80 = CD-DA, volume | 0x80 = loop}`.
- **Column choice:** picked by the **BGM SELECT** option byte `0x80097F05`. Column 0 = long arranged tracks
  (REMIXED, the default *(likely)*); column 1 = shorter arcade tracks; SILENT = no music.
- **Fight music follows the character, not the stage.** BGM ids 7–27 belong to Paul … True Ogre (character
  descriptor byte +11).
  - Example: Paul plays XA 42 (218 s) in column 0, or XA 16 (127 s) in column 1.
  - Characters without an arcade theme fall back to XA 6 in column 1.
- **CD-DA track 2** (BGM id 4) is the **ending staff-roll music**. Track 3 is never used.

### 10.3 Movies
- **Player:** in overlays record 10 (attract loop: the opening alternating with the prologue) and record 301
  (endings, Theater, credits), using libpress (DecDCTin/out).
- **Movie table:** at offset 0 of record 301 (copy in record 10 + 0xAC4).
  - Entries are 16 bytes: `{XAS start sector, end sector, flag, name}`.
  - Entry 0 is the music area (sectors 0–121 999); entries 1–22 are the 22 STR movies.
- **Per-movie parameters:** width, height, frame count, XA volume and flags.
- **Theater order:** a 25-entry list. Two entries replay the Kuma and Gun Jack spans with other frame counts
  *(likely alternate cuts, e.g. Panda's ending)*.
- **Other discs:** the Theater has extra tables to play movies from **Tekken 1 and Tekken 2 discs** ("insert
  Tekken 1, 2 or 3 disc"); Tekken 1 movies are found by file name.

---

## 11. Data catalogue: every BNS record

| Records | Contents |
|---|---|
| 0 | title/attract overlay: title and namco logos, an attract scene (copies of stage 4 and some characters) |
| 1–3 | packs of 13/13/15 (model, texture) pairs plus 2 `TK3psSDW` blobs, used by record 0 |
| 4 | empty |
| 5 / 6 / 7 / 8 | slot-A mode overlays: versus fights / Practice / **Tekken Force** / Tekken Ball |
| 9 | character select overlay (+ compressed select art) |
| 10 | title / main menu / options overlay (+ compressed menu art, attract movie player) |
| 11 | records and name entry |
| 12, 13 | red / purple lightning background panels (compressed) |
| 14–35 | 22 large character portraits (compressed, one per record) |
| 36–50 | regular stages 0–14: textures + floor map |
| 51–54 | **Tekken Force stages** (stage ids 15–18) |
| 55 | Tekken Ball beach stage (19) |
| 56–70 | backdrop geometry for stages 0–14 |
| 71–278 | 52 character groups: model m, VAB header m+1, textures/VAB body m+2, moves/motions m+3 |
| 279 | common motion set (shared by every character) |
| 280–300 | canonical moveset packs (one per moveset id; 295 = Mokujin is empty) |
| 301 | Theater overlay (movie tables for Tekken 1/2/3, music player, credits, thumbnails, credits VAB) |
| 302 | results / ending screens |

---

## 12. Roster, unlocks, select screen, Theater

- **Character ids:**

| Id | Character | Id | Character | Id | Character |
|---:|---|---:|---|---:|---|
| 0 | Paul | 8 | Eddy | 16 | Gun Jack |
| 1 | Law | 9 | Jin | 17 | Gon |
| 2 | Lei | 10 | Julia | 18 | Anna |
| 3 | King | 11 | Kuma | 19 | Dr. B |
| 4 | Yoshimitsu | 12 | Bryan | 20 | True Ogre |
| 5 | Nina | 13 | Heihachi | 21 | Tekken Force soldiers |
| 6 | Hwoarang | 14 | Ogre | | |
| 7 | Xiaoyu | 15 | Mokujin | | |

- **Selection** = `character·4 + costume` (92 entries):
  - `0x80097D40` points to a 12-byte descriptor: name, moveset id, home stage, BGM id.
  - `0x800958C4` gives the **model index** (0–51, model record = 71 + 4·index; 255 = no such costume).
  - The spring and other per-model tables use this model index.
- **Costume slots:**
  - Tiger is Eddy's costume 2; Panda is Kuma's costume 1/3; Anna also appears as Nina's costume 2.
  - Ogre's costume slots 2/3 use the True Ogre descriptor.
  - Moveset ids differ from character ids (Anna 17, Dr. B 18, Gon 19, Force 20). Mokujin's descriptor says
    moveset 9 while its own data says 15 *(it may be randomised at run time)*.
- **Unlocks:**
  - The initial roster mask is `0x3FF` (characters 0–9), at `0x80097EF0`.
  - Unlock sequence `0x80098214`: 2-byte `{type, arg}` steps ending at type 0. Type 1 unlocks a character,
    2 a costume bit (Tiger), 3 / 4 a mode (*likely* Theater / Tekken Ball), 5 everything. The order is in
    `assets/gamedata/roster.json` → `unlocks.sequence`.
- **Select grid:** 7 × 3 slots at `0x80022768`, each `{s16 x, s16 y, s16 selection}`. The ten starting
  characters sit in the outer columns and the middle row, the unlockable ones mostly in the middle of the top and
  bottom rows (`roster.json` → `select_grid`).
- **Theater list** (record 301, `0x800BA164`):
  - entries hold the movie, the ending flag that unlocks it, and up to three short title lines;
  - parallel lists cover Tekken 2 (24 movies) and Tekken 1, plus a music player for all three games.

---

## 13. Tekken Force and Tekken Ball

- **Code:** Tekken Force logic is overlay record 7 (slot A): scoring, keys, chicken pickups, bonus and clear
  time, and the stage name strings.
  - Force uses the third player struct.
  - Enemies are character 21 (CROW / FALCON / HAWK / OWL, models 259–271, moveset 20).
- **Force stages** = records 51–54 (uncompressed containers, 4 parts each):
  0. 64×64 4bpp TIM tiles (backdrop, floor, chicken sprite, shadow), uploaded by `0x800B45A4`;
  1. floor map (same format as normal stages), used as 10 × 3 with **2500-unit tiles**;
  2. **backdrop panorama**: `u16 cols, u16 rows (5)`, then cols·rows tile codes and cols·rows CLUT words, row-major.
     For each code: tpage = `((code>>8)&0x10)|((code>>8)&0xF)`, u = (code&3)·64, v = ((code>>2)&3)·64.
     Decoded panoramas: a night street (51), a canyon (52), a cave with a railing (53), a fortress wall (54);
  3. **event script**: 128 × 20-byte records `{s32 x, s16 type, s16 dx, …, s16 z, …}`, absolute camera x:

| Type | Meaning |
|---|---|
| 0 | camera mode: 0 = scroll, 1 = frozen, 2 = arena lock (±5000) |
| 1 | spawn one enemy at camera x + dx, lane z |
| 2 | keep spawning from a list until a given x |
| 3 | pause the script until N enemies are defeated (−1 = all) |
| 4 | spawn a pickup |
| 5 | nothing |
| 6 | stage clear (x = 30720 in all four stages) |
| 7 | end marker (x = +32768, never reached) |

- **Enemies:** an event's kind (0/1) picks the soldier costume through the record 7 table `0x800B06E8`
  (stage × kind). The soldier is then loaded as selection `21·4 + costume` (`0x800B2A80`, `0x800B2184`):

| Force stage | kind 0 | kind 1 |
|---|---|---|
| 1 | CROW | FALCON |
| 2 | CROW | HAWK |
| 3 | FALCON | HAWK |
| 4 | HAWK | OWL |

  - The variant (`b` mod 5) is stored per enemy slot *(meaning unconfirmed)*.
  - Spawn streams (type 2) read 6-byte `{kind, variant, 0}` entries from record 7 at `a·6`, ending at `0x63`.
  - The four soldier models (259/263/267/271) differ in mesh but share a byte-identical texture record.
- **Pickups:** the roast chicken sprite is two 64×64 tiles at VRAM (512, 64) and (512, 128), with its shadow
  at (512, 0), in every Force stage. At most two pickups are active at a time (`0x800B3E44`). The keys won from
  bosses are the three 20×36 TIMs inside record 7 (copper, silver, gold).
- **Backdrop drawing:** the Force overlay draws the panorama itself (`0x800B4660` setup, `0x800B4914` draw, called
  from `0x8006D7CC` when the battle type is 8). It is not 3D:
  - `rows × 7` flat 64×64 sprites, wrapping horizontally;
  - scroll = camera x / 20 pixels;
  - the bottom edge is pinned to the projected floor point at z = 2500, and the strip is drawn over the floor;
  - record 54's strip is tilted about 19° and drawn behind the floor.
  - The equivalent 3D wall is at z = 3000, with panels 1280 wide and 736 tall; panel n spans
    x = 1280n − 3680. `export_stages` builds the wall this way.
- **Floor:** the normal floor code with the Force constants. Records 51, 52 and 54 use the variant `0x80048E58`
  (window pinned to z −5000…2500); record 53 follows the player (`0x80049848`).
- **Camera:** eye y −3500, z −7000, pitch 15° down, H = 500 (overlay camera key at `0x800B4F78`).
- **Boss routes:** table `0x800B6378` in record 7 lists four bosses per selection. The last boss is always
  Heihachi, except for Mokujin, who meets Mokujin.
- **Tekken Ball:** record 8 (logic: ball types BEACH / GUM / IRON) and record 55 (beach stage + rules card).

---

## 14. Pictures (2D assets)

| Where | Storage | What |
|---|---|---|
| EXE (`0x800AD628…`, and the slot-B boot container) | LZ | HUD: timer digits, 1P/2P/CPU tags, round gems, 20 small 32×58 portraits, fonts, hit-spark sheets |
| record 0 | LZ | title logo (368×480 in strips), namco logo |
| record 9 | LZ | select screen: 22 portraits 126×252, 32×68 thumbnails, frames, PLAYER SELECT, name sheet |
| record 10 | LZ | menus: panels, PlayStation pad pictures (key config), logos |
| record 11 | LZ | records banners |
| records 12 / 13 | LZ | lightning background panels 172×224 |
| records 14–35 | LZ | 22 large portraits 126×252 (VS / loading); 6-bit pixels, one 64-colour sub-palette per 64-row band |
| record 301 | LZ + raw | 61 movie thumbnails 48×64 (Tekken 1/2/3 endings), film-strip art |
| record 302 | LZ | two full-screen stills (a gold tournament title, a dark red one) |
| records 36–55, m+2 | raw TIM | stage and character textures, Force/Ball tiles |

**Fonts:** each character's texture record contains a name bitmap and a 128×120 4-plane 1-bit sheet holding an
8-pixel ASCII font plus that character's **move-name word list**. Practice mode's command list builds move
names from these words; there are no move-name strings anywhere on the disc.

---

## 15. Corrections to disc.md

All three below are now fixed in the tools and in [disc.md](disc.md).

1. **VAB pitch formula:** the tone's fine-tune (`shift`) is **added**: `44100·2^((key − centre + shift/128)/12)`.
2. **System sound banks** are not missing. Their bodies are uncompressed inside the EXE (§9.2).
3. **The floor routine is resident** in the EXE (`0x80049848`), not in an overlay. Its setup (`0x80048640`)
   writes the tile size `0x8009E780` = **1800** units with a 10 × 10 map. Tekken Force (battle type 8) uses
   2500 units on 10 × 3.
4. **A movie table exists** (record 301 / record 10), as do the BGM table and per-character music.
5. Records **51–54 are the Tekken Force stages** and 55 is the Tekken Ball stage (previously "unknown").
6. **Battle type:** 8 = Tekken Force and 7 = Tekken Ball (some earlier notes had them the other way round).
7. **Earlier engine notes, corrected by the detailed specs:**
   - cancel `cond` 0x24–0x26 mean back-turned states, not crouching / while rising;
   - `0x80069414` is the replay camera, not the fight camera, and fight zoom never changes H;
   - throws are not level 6;
   - attack segments have no radius and there are at most 2 per frame (not 4 capsules);
   - move record +0x30 low half controls push spheres, not reaction or throw data;
   - timed-property entries are `{frame, arg, op}` (the op is the high byte).

---

## 16. Open questions

- **Attack paths:** where the model-size percentage is applied to the hit paths at run time; what objects the
  built-in descriptors (byte 0 ≥ 0x18) use.
- **Move system:** most of the 44 transition types; the camera keyframe encoding (header +0x24); spark types
  for ops 6–10; type-6 sound entries; which hand is left.
- **Reactions:** which side slot (4 or 6) is left; frame advantage is ±1 frame until checked in an emulator.
- **Camera:** the start-of-round yaw-freeze trigger; the first-round flag; the rarer character-specific camera
  triggers; the Ball and Force overlay cameras.
- **AI:** where Ogre becomes True Ogre in Arcade; battle type 3; the Tekken Ball AI hooks.
- **Tekken Force:** the enemy variant field; the floor shading of variant `0x80048E58`.
- **Other:** which mode handler is title vs main menu vs options (3–5); what record 0's attract scene shows.
  (Answered: the voice rows are by actor +0x1A, the selection descriptor's byte 8, not the character id: Panda on,
  and Ogre / True Ogre, take the row after their id's.)

---

## 17. Quick address reference

| What | Address |
|---|---|
| main / boot init | `0x80028BA0` / `0x800B0548` |
| frame wait / frame IRQ / VSync callback | `0x800296C4` / `0x80029784` / `0x800299E0` |
| mode table / mode / sub-mode | `0x80010000` / `0x800AE204` / `0x800AE224` |
| set mode / scene request | `0x80052C30` / `0x8004F8F8` |
| BNS async / sync load | `0x8006BF20` / `0x8006C470` |
| file id → record | `0x80098574` |
| LZ decompress / upload LZ TIMs | `0x80031BFC` / `0x8004CA40` |
| player structs | `0x800A9228` (+0x188C each) |
| character renderer / section renderer | `0x80037924` / `0x80036CAC` |
| skeleton / pose / motion decode / springs | `0x8003AFCC` / `0x8003AD10` / `0x80038B4C` / `0x80037CBC` |
| stage lights / shadow matrix | `0x80096C78` / `0x800ADE48` |
| camera view / matrix / fight camera | `0x800A85C0` / `0x800ADF70` / `0x80069414` |
| floor / backdrop | `0x80049848` / `0x8006D7CC` |
| particles / text / health bars | `0x80076E70` / `0x8004CE74` / `0x8004E8B4` |
| move header relocation / command match / cancel walk | `0x8002CE14` / `0x8002CC28` / `0x8002DE24` |
| hit test / classify / damage | `0x8004770C` / `0x80044A8C` / `0x80044BD8` |
| round state machine / CPU AI | `0x8003CB84` / `0x80059890` |
| SePlay / VAB load / system VAB upload | `0x800753BC` / `0x80075970` / `0x80074E10` |
| PlayBgm / CD state machine | `0x8006AE14` / `0x8006B6FC` |
| XA table / BGM table | `0x80024D78` / `0x80024FD0` |
| character descriptors / model index | `0x80097D40` / `0x800958C4` |
