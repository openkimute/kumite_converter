# Tekken 2 (PS1, USA v1.1, SLUS-00213): engine reference

What is known so far, for the Tekken 2 converter `t2_converter/` (separate from Tekken 3's; `convert_all`). Addresses are
RAM addresses of SLUS_002.13 (loaded at 0x80010000, file offset = address - 0x8000F800). The converted
characters are shown by `t2_viewer/` (`serve 8767 t2_viewer`, port 8767), apart from the Tekken 3 viewer.

## 1. Disc

| File | Contents |
|---|---|
| SLUS_002.13 | the EXE (1 MB) |
| TEKKEN2/TEKKEN2.BNS | all records (sector 0: the copyright text) |
| TEKKEN2/TEKKEN2.XAS | XA streams on 8 channels: ADPCM audio (music, voices) and STR video sectors |
| TEKKEN2/TEKKEN2.DA, DUMMY.DA | point past track 1: the CD-audio tracks 2 and 3 |

**Record table:** file offset 0x960 (0x80010160), 400 x {u32 0, u32 sector, u32 size}.

- 0-23: system records (VAB banks 0 / 2 / 27..., an overlay with its own PS-X EXE header at record 4, load
  0x80140000).
- 24-399: 47 character blocks of 8 records. The loader `0x800A0018(player, character 0..46, stage 0..22, buffers)`
  takes the block number.

| +k | Contents |
|---|---|
| 0 | stage TIMs at their VRAM places: the panorama cells (x 512..) and the 4-bit floor tiles (x 960..) |
| 1 | stage description (u8 flag, 9 section offsets; see section 5) |
| 2 | model arc |
| 3 | VAB header |
| 4 | "nlz ": u32 size, an LZ stream (0x8006C46C) of 32 x 32 4-bit TIMs: the character's hit spark frames (uploaded by 0x80087280 / 0x800873B8 / 0x80087518) |
| 5 | texture TIMs |
| 6 | VAB body (SPU samples) |
| 7 | moves and motions (Devil / Angel: an "arc " container: part 0 the moves, parts 1 / 2 the wing frames of Devil / Angel) |

Block b = character b // 2, costume b % 2, except: 35 is Angel (Devil's second slot), 45 Alex (Roger's) and 46 a
third Kazuya.
Character table 0x8005C504: 25 x {u32 name, u8 id + 0x15, u8 id + 0x33, u8 home stage, u8 order, name}.

**arc** (`0x8006C538`): u32 n; n x {u32 offset, u32 size}. The "arc " variant has the magic first.

## 2. Model (arc part 0; drawn by 0x8006EB38)

25 rows of 32 bytes: {i32 vertex block, polygon block, UV block, 0, 0, tx, ty, tz}. 18 bones are drawn; parents
at 0x8004E964: [-1, 0, 1, 1, 3, 4, 5, 1, 7, 8, 9, 0, 11, 12, 13, 11, 15, 16] (root, chest, head, arm A, arm B,
pelvis, leg A, leg B). The bone's offset is (tx, ty, -tz), x and y negated for bones 2, 3, 7, and the
upper arms' (4, 8) turned once at load by Ry(+-90) Rx(-90) (0x8006ED64: RotMatrixY(+-0x400) of a -90 degree turn
about X, applied to the offset by ApplyMatrixLV), which puts the shoulder joints out at the shoulders. Bones are
GsCOORDINATE2 records (0x50 bytes; init 0x80071C7C). Rigid skinning: each
bone has its own vertices. Front faces keep the GPU order (Tekken 3's are wound the other way).

- vertices: {u32 shading, u32 n}, then n x {s16 x, y, z, pad}; shading 2 (smooth): each vertex followed by its
  normal.
- polygons: {u32 nt, triangles, u32 nq, quads}. Shading 1: 12-byte entries {u8 a, b, c(, d), s16 face normal,
  pad}; shading 2: 4-byte entries (the indices).
- UVs: {u32 shading, u32 nt, nt x 3 words, u32 nq, nq x 4 words}; word = u | v << 8 | attribute << 16; word 0's
  attribute is the CLUT, word 1's the texture page (as in a GPU packet).
- Texture: the TIM coordinates are relative; pictures and CLUTs go to separate places (the CLUT rows at y 0..3
  would otherwise overlap the 8-bit pictures at (0, 0)).
- Part 1: the shadow hull, the same for every character (rows as part 0's; per bone {u32 n, n x (s16 x, y,
  z, pad)}, {u32 nt, a b c pad}, {u32 nq, a b c d}, no UVs). 0x8006DBA8 makes flat POLY_F3 / F4 packets of the
  stage's shadow colour from it. The draw flattens it onto the floor with the matrix 0x800D659C (0x8006F26C):
  v = column 0 of RotMatrix(0, angle hi, angle lo); x' = x − y vx/vy, y' = 0, z' = z − y vz/vy. It is exported
  as the glb's `shadow` mesh.
- Part 2 (360 bytes): the hanging chains. Fields, all s16:
  - 0xFEFF, the chain count, the link count, then the model row of each link (rows 18..20, beyond the 18 bones), 0, 0.
  - 0x6FFF, the bone they hang from, links per chain, link length (model units).
  - The rest is the same in every model.

  Which fighters have them:
  - Roger / Alex (tail): 1 x 3 links at the pelvis, 300.
  - Kunimitsu (ribbons): 2 x 2 at the head, 165.
  - Michelle (braid): 1 x 3 at the head.
  - Jun (ribbons): 2 x 1 at the head.
  - Belts: Kazuya 2 x 2 at the chest; Paul, Heihachi and Baek 2 x 2 at the pelvis.
  - Armor King, King, Nina and Lei have chains of their own too.

  How the game swings them:
  - 0x80080FDC copies part 2 to a per-player record (0x800C4638 + 0x168 * player); the chains' state lives in it.
  - 0x800810D4 lays the links out, hanging from the fighters' places.
  - 0x800811C4 (fighter, 1) steps them once a frame (0x8005FFC0). It is fixed-point Verlet: positions << 8, drag
    1/128 a frame, gravity, kept above the floor (y <= -10) and off planes on the body (points on bones,
    record +0x134 / +0x150).
  - The links hang from their bone's world matrix (fighter +0x600 + 0x50 * bone). Their world matrices go to the
    drawing 0x8006FC00 (player, count, MATRIX[count]).

  In OpenKumite: the export adds the chains as joints `chain<c>_<i>` on their bone. The transplant runs the game's
  own routine in the MIPS interpreter (`chain_sim`; its GTE helpers SetRotMatrix / SetTransMatrix 0x80071CF8,
  RotTrans 0x8001CF4C and SquareRoot0 0x80086FDC in Rust). Each clip feeds the bones' world matrices in and
  bakes the links' matrices into the clip; a clip starts with the links laid out and left to hang 60 frames.
  Where the game swings on across moves, OpenKumite's drawing takes the `chain*` / `wing*` joints from the pose
  last shown to the new clip's over 8 frames (`kumite_game::SecondaryBlend`).
- Devil's / Angel's wings: model row 18 (polygons and UVs; its vertex block is only the header) drawn in the
  chest's frame (0x800727B8) with one frame of the moves arc's part (costume & 1) + 1 (0x8006EEC8): {u32 36,
  192 offsets, 192 x 36 vertices}. 0x8006F9E8 picks the frame: 8 clips (0x800A14DC, frame bytes ending 0xFF) and
  a mode per fighter (+0xE44; set by the moves at 0x80065290). Mode 0: rest, flapping (clip 0, kept in step with
  the stance); 1: spread (1 → 3); 2: fly (4 → 5); 3: fold (6 → 7). Exported as the glb's `wings` mesh with 192
  morph targets; the clips and transitions are in its extras.
- Stage lighting 0x800A1030[stage], 24 bytes: model colour (the packets' colour code), shadow colour, shadow
  angles, light angles, light colour, ambient << 4 (0x80071338 → SetBackColor). It is exported as the arena glb's
  `lighting`.

## 3. Moves and motions (+7)

Header: u8 relocated, u8, s16 move count, 7 offsets made pointers on load (0x80066F38): slot table (0x703 slots
of u32: a move index; | 0x40000000 the EXE's common moves at 0x80042864), moves (0x2C bytes), three more tables,
the motion area. Move +0: the motion's offset in the record.

**Motion decoding:** `0x8006C410(motion, dest, frame)` → `0x8006BEC8(motion, dest, frame, 3)` (+ `0x8006B944`):
u16 header (frames in the low byte, a bit-width table index in the top 2 bits → 0x8004E4C4 + type * 0xE4),
variable-width keys, 33-frame segments. Output: 57 s16: the root's place, then 18 x 3 angles (65536 a turn).
The root's place is polar (0x8006234C): channel 0 a direction relative to the facing (any value, often 0x7FF0,
when the distance is 0), channel 1 the height, channel 2 the distance (1 and 2 halved by the decoder):
x = -d sin(dir + 0x8000 - yaw), z = d cos(dir + 0x8000 - yaw), from where the motion started. Bone k's matrix (`0x80072234`, sine table 0x80056DA4) is
Rz(c) Ry(b) Rx(a) called with (a, -b, -c). The converter runs these routines on a MIPS interpreter
(`t2_converter/src/mips.rs`).

## 4. Media (`t2_converter/src/export_media.rs` → `assets_t2/media`, the viewer's media tabs)

- **Sounds:** VAB banks: records 0 + 1 and 2 + 3 (header, body); record 5 starts with one whole VAB (the rest
  of it is other data); each block's +3 / +6. 50 banks, 580 samples.
- **Music:** the EXE's XA table 0x80011490: 84 x {s32 first, s32 last, s32 channel} (TEKKEN2.XAS sectors; 56
  music streams, then the 28 movies), turned into CD positions at 0x80037868 by 0x80013A00. BGM n (0..29) plays
  through 0x80012E50: table 0x80011888, 2 music sets (0x800A3701) x {u8 1 = CD audio, u8 volume, s16 stream}.
  A fight's BGM is the character record's byte +7 (pointers at 0x800A39CC, by block; +6 is the home stage);
  BGMs 15-18 are called from 0x80093424 and the overlays (records 20, 21). CD-DA track 2 is BGM 16.
- **Movies:** XA table entries 56.., 28 STR movies (video and XA audio on channel 1, 15 fps), played by
  0x80013774(n). The overlays (loaded at 0x801BDFD8) pick them: record 17's table 0x801E9F10 [title part][music
  set] = (0, 1), (2, 2) (the opening for each set, then a second title movie); record 20's 0x801D00E4 [block]
  the endings (block 35 = Angel).
- **Pictures:** TIMs in the EXE and records 0-23 (plain and LZ, 0x8006C46C). Stage panorama: record +1
  section 1 = 192 cells of 64 x 32 {u8 u, u8 v, u16 CLUT, u16 tpage} over the +0 TIMs → 1536 x 256, 11 stages.

## 5. Arenas (`t2_converter/src/export_stages.rs` → `assets_t2/stages`, the viewer's Arenas tab)

Stage k (0..22) comes from the block of the character whose home it is (character table byte +6). The
description is relocated by 0x80082074 and kept at 0x800D2E18 (sections at 0x800D2E1C..).

- **Floor** (hrfld.c), section 0: u32 64, 10 x 10 x {u32 u | v << 8 | tpage << 16, u32 CLUT << 16}, s16 mode, s16
  shade, s32. The tpage's bits 14-15 turn the tile (the corner table that 0x80082790 builds is Tekken 3's).
  - Mode 0 (0x80082E60): 2500-unit tiles, an 11 x 11 grid around the fighters' midpoint snapped to whole tiles.
    World tile (i, j) shows map column (i + 5) mod 10 and row (4 - j) mod 10. The quad's corners are points
    p + 11, p + 12, p, p + 1.
  - Shade != 0: colour = 128 + T[min(1023, (sz - 10000) * 6710 >> 16)] * shade / 128, with T at 0x80056564.
  - Modes 1 and 2 (0x8008364C): 2000-unit tiles. Mode 1: min(254, 2^27 / d1² + 2^27 / d2²), d to each fighter.
    Mode 2: min(192, 0x7A120000 / d²), d to the fighters' midpoint.
- **Panorama** (hrbg.c; init 0x80083F04, draw 0x80084560): drawn in screen space (512 x 240).
  - Section 7 holds the rows, 0x2C bytes each: {distance −1, scroll, x, y, w 64, h 32, map entries 96, first
    cell, map offset, packet buffers}. Section 1 holds the cells {u, v, CLUT, tpage}; section 2 the maps (s8 per
    64 px, 6144 px a turn).
  - Left-edge column = (row x + sec8 scroll − 1.5 yaw) mod 6144.
  - The row's top = s5 + y − h, where s5 = 120 − 597 tan(pitch) + below.
  - The GTE H is 955 (0x8007BE20), with y scaled 5/8, so 597 is the vertical focal length. 1.5 px per yaw unit is
    978 px a radian ≈ H, so a drum around the camera matches.
  - Section 8: {rows, ?, ?, scroll, below, sky_y, sky_h, sky rgb, sky2 rgb, ground_y, ground rgb, gy, gh, g1, g2,
    effect}. These are the TILE / G4 fills behind the sprites: sky above s5 + sky_y, then a gradient to sky2;
    ground below s5 + ground_y, plus a gradient.
  - Rows per stage: 8 (King, Yoshimitsu and Lee: 16). Kazuya's and Devil's stages have no panorama; Devil's has
    effect 2 (0x80085020, not decoded).

## 6. Tekken 2 fighters in OpenKumite (`t2_converter/src/transplant.rs`, `transplant_moves`)

`t2_convert transplant "Tekken 2 ... (Track 1).bin" assets` brings Kazuya (models 900 / 901, character
23) and Jun (938 / 939, character 24) into OpenKumite. It writes `characters/model_9NN.glb`,
`gamedata/movesets/moveset_t2_<name>.json`, the moveset index and roster entries, a portrait and a face made from
Tekken 2's select face (record 19).

- **Skeleton:** the same 18 bones and sides as Tekken 3 (which adds head_b); each bone's axes differ by a fixed
  quarter-turn rotation D. D was measured from the stances of the fighters both games share; with it, every child
  offset lies along +x as in Tekken 3. Rotations become D_parent^T L D_bone, and inverse binds (B D)^-1. Poses are
  identical; Tekken 3 motions play on these fighters by joint name, and theirs on Tekken 3's.
- **Moves:**
  - The donor's set supplies the system: Jin for Kazuya, Julia for Jun. Its motions are copied into the glb.
  - On top come every Tekken 2 move reachable from the stance through the command lists. Calls into the EXE's
    shared lists (0xFFF3 → 0x80047488) are expanded. Common moves come from 0x80042864, their motions at
    0x800D660C, decoded by the game's own routine.
  - Each move takes its motion, damage (+0xE & 0xFFF), active frames (+0x23 / +0x24), next move (+0xC), the
    hit level from +6 and the strings and END windows. The strike path is baked from the striking bone (+0x25),
    elbow / knee to wrist / ankle.
  - The reaction, sounds and flags come from the donor's closest attack (same level and limb).
  - The stance list puts the Tekken 2 commands first, then the donor's movement, throws and the rest without the
    donor's attacks.
  - Throws: attempt slot 0x299 (1+3) / 0x29A (2+4) → common 329 / 334 → on cond 0x2C (grabbed) 330 / 335 →
    cond 0x02 front / 0x03 side (distance 1536) → the character's throw. Its reaction entry (0x8003E5B4, 18 s16:
    7 parameters, 11 victim slots) names, last, the victim's move in the thrower's slot table. In OpenKumite the
    donor's attempts are pointed at these throws (front cond 0x02, sides 0x03 / 0x04), the victim's move joins the
    set with a new `throw_victims` entry (reactions.json, after the original 397), and the damage comes at the
    impact property. Throw damage is move +0xE's low byte (the high bits are flags).
  - Timed properties: move +0x1C indexes the +7 table at header +0x18, 4-byte {u16 frame, u8 arg, u8 op} up to a
    zero word. Op 3 = Tekken 3's op 8 (effect at a bone), op 1 = op 6, op 0 (arg 4) = the impact.
  - Jump attacks (byte +0x13 = 9) are taken with Tekken 3's airborne posture; plain jumps stay the donor's.
  - Conditions (handlers 0x8004E230; a1 the fighter, a2 the opponent): 0x19 back turned (+0x28 > 0x4000) →
    Tekken 3's 0x24; 0x1E opponent attacking close; 0x27 / 0x2A opponent flag 0x40; 0x2C grabbed (+0x8B);
    0x2D / 0x2E +0xBA == 0 / != 0; 0x33 +0x1DC == 0; 0x35 +0xBC == 0. Taken: 0x19 and 0x2D.
  - Hit reactions: each attack takes the reaction of the donor's closest attack with the same outcome. The
    outcome is the front standing victim of its Tekken 2 reaction (slot 7): lying (posture & 7 = 4) = knocked
    down or launched, else a hit. Level, limb and damage decide among those.
- **Arenas:** their Tekken 2 home stages become OpenKumite's stages 30 (Kazuya's, T2 stage 22) and 31 (Jun's,
  T2 stage 3), their home stages. `export_stages.convert(port=True)` writes the panorama and fills as a 'sky' drum
  that OpenKumite centres on the camera and draws first. The floor map is laid 3 x 3 times and clipped by OpenKumite
  to the 10 x 10 tile patch around the midpoint, snapped to whole tiles. The floor lighting (mode 0 depth fade, 1 / 2 lit from
  the fighters) is computed by OpenKumite's floor shader from the fighters' places.
- **Who:** every Tekken 2 character whose name no Tekken 3 character has (`transplant.plan`): Kazuya (23), Jun (24),
  then in Tekken 2's order Jack-2, Michelle, Wang, Lee, P.Jack, Armor King, Ganryu, Kunimitsu, Devil, Bruce, Baek
  and Roger (25..36). Marshall Law, King and Kuma keep their Tekken 3 namesakes.
  - Angel and Alex are Devil's and Roger's second costumes, as in Tekken 2 (blocks 35 / 45, the pairs' second slots).
    Each keeps its own model, moves, voices (its Tekken 2 id), select name, portrait and ending.
  - Endings: record 20's table 0x801D00E4 gives the ending movie by block. Its frames and audio come from
    `assets_t2/media/video` as OpenKumite's `video/1NN`, with a theater.json movie entry and
    `ending.extra[character][costume]`.
  - The Theater shows one cell per Tekken 2 ending, from cell 24: a 48 x 64 thumbnail from the select face
    (`pictures/theater_t2`) and the costume's name. It unlocks with its clear: save `endings_t2`, bit
    `ending.extra_bits[character][costume]`, seen by the Theater as bit 32 + n of its locks. The music player
    still opens with the original's movies.
  - Each costume is a block (`block_name`): model record 900 + block, moveset id = character id + 7.
  - System moves come from a donor of a like style (`DONORS`: Gun Jack for the Jacks, King for Armor King and Ganryu, Kuma for
  Roger and Alex, Jin for Devil and Angel, ...).
  - Home arena: the character table's +6 stage. Kazuya's and Jun's are 30 / 31, the others' 60 + the Tekken 2 stage.
  - Select faces: record 19 by Tekken 2 id. Alex's id holds the random "?", so he takes Roger's (his block pair's, block // 2).
  - Voices are laid out afresh in program 0 of the fighter's bank: attack voices tones 0.., hit voice 2 tone 12, hit voices 0 / 1
  tones 14 / 15. Armor King's VAB has two programs, and programs 1..3 hold the system sounds.
- **Wings and chains in OpenKumite:**
  - Devil's / Angel's wings: a joint per wing vertex (36, on the chest), the corners skinned to it. OpenKumite skins
    up to 64 joints. Each clip moves them through the wing frames as 0x8006F9E8 steps them.
    - The wing mode each frame (0x80065250, Devil only):
      - 2, fly: in the move's air window (move +0x13 .. +0x14, fighter +0x8C) and rising (+0x34 > 0).
      - 1, spread: in the air window, falling.
      - 3, fold: when hit (+0x5C) while blocking (+0x5F, from +0x89 set in the hit pass). Baked into the block
        reactions (reactions.json slots 10 / 11 through the donor's aliases): clip 6, then clip 7 held at its first
        frame.
      - 0 otherwise.
    - By mode and current wing clip (tables 0x8004EB98 / B8 / D8), the game switches to a clip now, or plays the
      current one out and goes on with another.
      - Clips: 0 stance, 1 spread, 2 close, 3 spread held, 4 take off, 5 flap, 6 fold, 7 unfold.
      - Mode 0's clip 0 follows the stance motion's frame in the stance (else holds its first frame).
  - Chains: their joints and baked tracks (see part 2).
- **Select screens:** the Tekken 2 fighters have their own page. The character select's rows 2 / 3 hold 8 faces each
  (Tekken 2's select order). The VS grid's page 2 keeps the original's 7 x 3 layout (`grid_slot` [row, column]).
  Up / Down moves on through the pages; the page shown is the last moved cursor's.
- **Music:** the character table's +7 BGM. Its two sets (0x800A3701: 0 is the default, set 1) become the
  port's music.json columns 0 and 1 (`audio/music/t2_NN.flac`). Kazuya gets BGM 32 (XA 4 / 34), Jun BGM 33
  (XA 5 / 17), played for the character by name.
- **Voices and sounds** (move sound list +0x18, player 0x80073174; u16 type << 9 | index):
  - Type 4 / 5: a voice on frame 1. Both wait for the voice lock 0x800A1AF4[player]; type 4 sets it to
    0x80050CB8[character] frames (Kazuya 10, Jun 30). The lock runs down each frame. Type 6: a voice script.
    Type 7 arg 2 sets the lock to 1.
  - Random voice (0x80073C50): when a move starts (+0x93) with damage (+0x48 = +0xE low byte), the lock is
    clear and +0x58 == 0 (cancel byte +8 kinds 1 / 3 / 4 / 5 set it; Tekken 2's lists don't use them), it
    plays on rand & 1. The voice comes from the list 0x800A1A8C[character] (Kazuya 4, 5, 7; Jun 4..7), then
    sets the lock. In OpenKumite this is the voice row's `random` plus sound entry type 9.
  - Scripts 0x800A1644[id]: s16 {time, flags | kind, value} up to time 0. Counter +0xB8 starts at the move's
    first frame and advances by +0x5A. Flags bit 15 limits an entry to the character in bits 13..8. Kinds
    (0x80050CF8): 0 table 0x8004EC10; 2 table 0x8004EC98; 4 the same at the opponent (0x3C = the floor's fall
    sound 0x800D64E8); 1 / 5 / 6 own voice; 3 the opponent's voice; 7 the opponent's 0x80050C38. The throws
    use them. In OpenKumite they are sound.json `scripts.t2` (type 8).
  - Table entries are bank << 8 | index. Bank 2 is the system VAB A (records 0 / 1), entries 0x80050D64
    {key, volume}, played as SsUtKeyOn(vab = bank, program s0 >> 14, tone (s0 >> 2) & 15, note (s0 >> 6) & 127).
    Bank 6 is the player's voice VAB. OpenKumite copies bank-2 sounds 0..47 into each Tekken 2 fighter's own bank
    at programs 1..3 (`mNNN/sII.wav`), with codes at sound.json `sfx_table.t2_system`.
- **Pictures:** Tekken 2's 160 x 100 select faces are square on its 512 x 240 screen, so on OpenKumite's 368 x 480
  select screen they are 126 x 219 (portrait) and 32 x 56 (face).
- **Port:** extra select slots at the end of a row (`select_row`), VS grid cells (`grid_cell`), pictures
  `portraits/NN.png` and `faces/NN.png`, and the HUD / select names in the font when there is no name picture.

## 7. Open

Moves (the 0x2C records, the command tables) and the fight rules; the BG effect 0x80085020 (Devil);
how the hit spark frames are played; the chains' own swing routine and where on their bone they hang; the moves that set the wing mode; the common moves' motions.
