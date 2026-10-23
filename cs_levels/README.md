# cs_levels — Counter-Strike levels as OpenKumite arenas

Installs Counter-Strike levels as arenas of the CS LEVELS menu. A conversion to play takes them with
`kumite_convert --cs GAME_DIR` (the README); the `cs_levels` commands below (in `target/release/`) are developer
tools: one level at a time, in a converted folder.

From your copy of the game (the Half-Life folder with `cstrike/` and `valve/`, or its `cstrike/` folder; the
installer CD is refused: install or unpack it first), all its levels and the two Tekken Force stages below in one go:

```
cs_levels from_game /path/to/Half-Life assets
```

Each level's `.bsp` is converted in memory as the Half-Life project's `bsp2glb.py` converts it (the same glTF; its
textures from the BSP, else from the WADs the map names, looked up in `cstrike/` then `valve/` in any case; a missing
texture gets a placeholder and a warning). The 17 levels below get stages 40..56 in that order, any others follow
(20 at most). `cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]` writes one level's glb, to look at.

One level at a time, from a `.glb` with the level's entities (`info_player_*`, `func_illusionary`, ...) kept as node
names; each install also refreshes `assets/manifest.json`'s fingerprint:

```
L=/path/to/levels
id=40; for m in cs_assault de_dust de_train as_oilrig cs_747 cs_backalley cs_italy cs_militia cs_office cs_siege \
                de_aztec de_cbble de_prodigy tr_1 tr_1a tr_2 tr_3; do
  cs_levels install $L/$m/$m.glb assets $id; id=$((id+1)); done
```

Installed: stages 40..56 in that order (the menu lists them by stage id). Arenas from 10.75 m (de_prodigy) to 29.75 m
(cs_747). The training maps (tr_*) have one spawn in a corridor, so their arenas come from their largest floors.

What the installer does:

- Copies the level to `assets/stages/stage_<id>.glb`, keeping its meshes and textures.
- Hangs the level under a root node that puts the fight spot at the origin.
- Writes the scene extras OpenKumite reads:
  - `level`: the level's name.
  - `walls`: the walk grid.
- Registers the stage in `gamedata/stages.json` with `"menu": "CS LEVELS"` (the menu OpenKumite lists it in, by its
  title). The game's own stage lists do not show it.

How the arena and walls are found:

- **Floor:** the highest upward-facing surface under the level's `info_player_*` spawns.
- **Free floor** (cells of 0.25 m): a floor lies within 0.3 m of that height. No solid surface crosses 0.25–1.7 m above it; `func_illusionary` and ladders don't count as solid. Walls grow by 0.5 m for the fighters' bodies. Only the cells reachable from a spawn count.
- **Fighters' arena (`walls`):** the free cell farthest from any wall, its free square grown into a maximal free rectangle (cs_assault: 22.75 × 22.75 m). The rectangle is convex, so no wall ever stands between the fighters. The fight starts in its middle, the fight line along its longer side.
- **Camera space (`camera_walls`):** all the free floor.

In OpenKumite:

- **Textures:** the level's glb sampler (linear, repeat) gives them linear filtering, mipmaps and tiling.
- **Fighters:** they stay on the arena's free cells. A fighter that walks into a wall stops, and the push between the two then moves the other one (`kumite_sim::Walls`).
- **Camera:** it stays over the free floor. When the free run from the fighters' midpoint towards it is shorter than its distance, it rises, keeping that distance (both fighters stay framed), until it stands over the free floor 0.3 m short of the wall. It then looks at the midpoint chest high, and everything above the floor nearer than the nearer fighter is cut away.
- **Cutaway:** stage surfaces above the floor are cut away inside a 2 m tube from the camera to either fighter's chest, when nearer the camera than that fighter by more than 0.5 m. Walls and props between the camera and the fight don't hide it.
- **Menu:** CS LEVELS → the level → P1's fighter → the CPU opponent → one match → back to the list.

## Tekken Force extra stages

`force_stage` makes an installed level a Tekken Force stage played after the original's fourth (before the final
stage, when it was earned):

```
cs_levels force_stage assets 41 1 DE_DUST       # stage 5: de_dust, BADLANDS' script
cs_levels force_stage assets 40 0 CS_ASSAULT    # stage 6: cs_assault, BACKSTREETS' script
```

- **Route:** Tekken Force runs along +x from x = 0; its camera stands 7 m to the side and looks across the route. The
  route is a straight free run with 1.75 m free on either side of its line (the installer's walk grid at one of the
  level's floor heights), along either direction of either axis, 42 m long or, failing a view, shorter (the script's
  distances are then scaled). Runs are scored by the view: open floor beyond the run on the far side (nothing solid
  0.5–3.5 m above it), and room towards the camera. de_dust: 38 m across a courtyard, 17 m of view (script x 0.85);
  cs_assault: 42 m along a street, 25 m of view.
- **Port:** the level is laid along the route (`Route::local`). The fighters keep within the route's half width. On the
  camera's side, the level's surfaces above the floor are not drawn past the route's edge plus 0.5 m, and walls between
  the camera and the fighters are cut away.
- **Script:** the base stage's waves, plus a roast chicken every 4 m.
- **Tables:** stage 4's soldiers, health, AI level and bonus.
- **Bosses:** two come in together, drawn at random from Heihachi, Ogre, True Ogre, Kazuya and Devil (those in the roster).
  Both must go down to clear the stage.
- Written to `gamedata/force.json` `extra_stages` (in the order they are made).
