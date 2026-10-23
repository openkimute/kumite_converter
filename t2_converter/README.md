# Tekken 2 converter

Converts a Tekken 2 disc (USA v1.1, SLUS-00213) into standard files, separately from the Tekken 3 converter in
`../t3_converter/`. Nothing here imports the Tekken 3 converter. A conversion to play takes the Tekken 2 fighters
with `kumite_convert --t2 BIN` (the README); `t2_convert` is a developer tool: `assets_t2/` for the Tekken 2 viewer,
and its modules one at a time.

```sh
target/release/t2_convert "Tekken 2 (USA) (v1.1) (Track 1).bin"   # -> assets_t2/ (characters, stages, media)
target/release/t2_convert transplant "Tekken 2 (USA) (v1.1) (Track 1).bin" assets   # the fighters into OpenKumite
target/release/t3_convert --fingerprint assets
target/release/serve 8767 t2_viewer                    # the viewer, http://127.0.0.1:8767
```

The transplant reads the movies of `assets_t2/` beside `assets/` (the first command's media).

| Module (`src/`) | Output |
|---|---|
| `export_characters` | `characters/t2_block_NN.glb`: model, shadow hull, wings (Devil / Angel), every motion |
| `export_stages` | `stages/t2_stage_NN.glb`: floor patch, panorama drum, fills, lighting |
| `export_media` | `media/`: VAB sounds, XA music, STR movies, pictures, hit sparks |
| `transplant` | every Tekken 2 fighter as a playable fighter of OpenKumite (`model_9NN.glb`: costume block NN): glb on Tekken 3's skeleton, move set, roster entries, pictures (`t2_convert transplant BIN assets`, then `t3_convert --fingerprint assets` as it says) |
| `transplant_moves` | their move sets: Tekken 2's attacks on a Tekken 3 donor's system |
| `ps1` | the PS1 formats these share (ISO 9660, TIM, glTF writer, SPU ADPCM, CD-XA, LZ) |
| `mips` | the MIPS interpreter that runs the game's own decoders |

The formats are in [../docs/reference/tekken2/engine.md](../docs/reference/tekken2/engine.md).
