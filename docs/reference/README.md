# Reference: the original games

What the original executables do and what the converter reads, reverse-engineered from the disc (no other sources).
The converter and OpenKumite follow these notes; code cites the EXE addresses they give. Confidence tags: **[V]**
verified in the code, **[C]** confirmed by data or a prototype, **[I]** inferred, **[G]** a guess (engine.md:
*(likely)*, *(guess)*).

## Tekken 3 (PS1, USA, SLUS-00402)

| | |
|---|---|
| [tekken3/engine.md](tekken3/engine.md) | The engine as a whole: main loop, modes, memory and files, rendering, characters, combat, rounds, AI, audio, movies. **Start here.** |
| [tekken3/move-properties.md](tekken3/move-properties.md) | Moves: timed properties, cancel conditions, the moveset header, move-record fields |
| [tekken3/attack-paths.md](tekken3/attack-paths.md) | Attack descriptors and the baked strike paths (hit detection) |
| [tekken3/reactions.md](tekken3/reactions.md) | Hit reactions, damage, counter hits, frame advantage, throws |
| [tekken3/camera.md](tekken3/camera.md) | The fight camera, the director, K.O., win and replay cameras |
| [tekken3/ai.md](tekken3/ai.md) | The CPU's AI: profiles, decision loop, opponent selection |
| [tekken3/disc.md](tekken3/disc.md) | The disc image's layout, archives (BNS, XAS), model, texture, animation and stage formats |

## Tekken 2 (PS1, USA v1.1, SLUS-00213)

| | |
|---|---|
| [tekken2/engine.md](tekken2/engine.md) | The disc, models, moves, media and arenas, as far as the Tekken 2 converter and the fighters brought into OpenKumite need them |

The small scripts some notes name (`mem.py`, `fa.py`, `scan.py`, ...) were throwaway research tools, not in the
repository; what they established is in the notes and in the converters.

New research goes into the note on its subject (or a new one, linked here); a finding that changes OpenKumite's
behaviour is cited from its code by address.

## Open questions

Where OpenKumite still guesses (the player-visible gaps: OpenKumite's docs/gameplay/not-done.md):

- **Facing guesses:** the sidestep lean's sign, tracking mode 6's "has dealt damage", and the reaction angle of
  transition types 0x1E / 0x24 are guesses.
- **Moves:** swept attack starts across turns between frames, the spark at a segment's end (rare cases); the
  chain-throw victim's placement on later parts is unverified.
- **Unconfirmed:** whether a throw break swaps the fighters' sides, and which side direction is "A" and which "B".
- **AI:** its Tekken Ball and Tekken Force hooks; the attract demo's halved cooldowns; the game's other users of its
  two random streams (effects, camera) do not draw from them yet.
