# Torched

Artillery on destructible ground, a total conversion of Xscorch (Justin David Smith and
Jacob Luna Lundberg, 2000 to 2009; GPL-2.0-only), itself a free remake of Scorched Earth
(Wendell Hicken, 1991). Rust + Ratatui, the playfield in Braille.

**Status** (2026-11-06): notes and plan only, no code. Built from scratch in Rust (David,
2026-11-06), not as a fork of Gorched. Build order in `ROADMAP.md`.

## Name

Xscorch, then Zscorch (an X-to-Z sequel name), then **Torched**: "-orched" after Gorched, the
T for Terminal as in Terminal Orbit. Checked against itch.io and Steam, no Scorched-Earth-style
game called Torched.

## Next to Gorched

Gorched (in the catalog) is the plain artillery title: two players, one weapon, no wind, no
shop, no computer players, terrain in whole cells. Torched is the deep one: the shop, the
whole arsenal, wind, walls, computer players and smooth Braille ground.

## Source

`references/scorched/`: the 0.2.1 release tarball and its Debian patches, and
`xscorch-lschierer` (github.com/lschierer/xscorch, 0.2.1 ported to GTK3). Where the rules live:

| What | Where |
|---|---|
| Weapons, accessories, scorings, tank profiles | `data/*.def` (plain text, data-driven) |
| Flight, wind, viscosity, walls, tunnelling | `sgame/sphysics.c` |
| Explosions, napalm, dirt | `sgame/sexplosion.c`, `sspill.c`, `sland.c` |
| MIRVs and split weapons | `sgame/sphoenix.c` |
| Terrain generators | `sgame/slscape.c`, `slscapetools.c` |
| Shop, money, interest | `sgame/seconomy.c`, `sinventory.c` |
| Computer players | `sai/` (`saibuy.c`, `saitraj.c`, `saiturn.c`), `doc/AI` |
| Network play | `snet/`, `doc/NETWORK` |

What the original has, checked in its source:

- 44 weapons: missiles and nukes, Leap Frog, Funky Bomb, MIRV and Black Market MIRV, Death's
  Head, Annihilator, The Chaos and The Chaos^2, four napalms, tracers, Shield Sapper, rollers,
  four riot charges, diggers and sandhogs, four dirt weapons
- 17 accessories: magnetic, standard and force shields in three sizes, fuel, Triple Turret,
  Auto Defense, Solar Panel, Contact Trigger, batteries, teleporter, parachutes
- Physics settings: gravity, air viscosity, wind (with how much it changes per turn), and six
  wall kinds (concrete, padded, rubber, spring, wrap, none) or random
- Seven terrain generators (none, canyon, double mountain, hillside, mountain, plains,
  traditional) and several skies
- Play order (random, losers first, winners first, round robin), turn-based or all players
  firing at once
- Scoring rules Basic, Standard and Greedy
- Seven computer players: Moron, Shooter, Spreader, Chooser, Calculater, Annihilater, Insanity
- Network play over TCP, on by default. Its console mode (`sconsole`) is a server stub that
  prints "This program doesn't actually do anything (yet)"; there is no terminal edition

## Rendering

- Ratatui owns the frame. The playfield is one Braille canvas: ground, craters, napalm, shot
  trails, tanks. Shop, HUD, scores and menus are ordinary Ratatui widgets, never drawn in
  Braille.
- The world runs in its own units, at Xscorch's scale so its numbers carry over, and is
  scaled to the canvas, so the game plays the same in any window. 80x24 plays; the catalog
  preview is 100x30.
- Colour per tank and per weapon, amber HUD.

## Conventions

- Session logging per the Asciiverse convention (`log` + `env_logger`)
- Keyboard, mouse and gamepad (`gilrs`), sound with `rodio`, as in Braillestorm and Panzer
- Network play with iroh, wired in directly, as in Panzer
- `--demo` (computer players only) for the preview recording
- Version per `games/CLAUDE.md`: 0.x.0 while unfinished, 1.0.0 when `ROADMAP.md` is done
