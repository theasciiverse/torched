# Torched

Artillery on destructible ground, a total conversion of Xscorch (Justin David Smith and
Jacob Luna Lundberg, 2000 to 2009; GPL-2.0-only), itself a free remake of Scorched Earth
(Wendell Hicken, 1991). Rust + Ratatui, the playfield in Braille.

**Status** (2026-11-07): 0.1.0 to 0.5.0 written, not yet played by a person: terrain,
hot-seat aiming, the whole arsenal, wind, air, walls, the shop, money, accessories and
shields, play orders and all-fire-at-once, the seven computer players and `--demo`,
skies, effects and synthesised sound, a title screen and remembered options, mouse and
gamepad. `--free` gives the old sandbox (every weapon free, no shop). The catalog entry
is written in `../hub/catalog/torched.{toml,md}`, not committed there. Built from scratch in Rust (David, 2026-11-06), not as a fork of Gorched.
Build order in `ROADMAP.md`.

Code: `src/land.rs` (terrain generators, falling dirt, liquid spills), `src/physics.rs`
(flight, wind, drag, walls), `src/defs.rs` (Xscorch's `.def` format), `src/weapons.rs`
and `src/economy.rs` (read `data/*.def`: weapons, accessories, scorings, prices),
`src/sim.rs` (the game, no terminal), `src/kit.rs` (shop, money, shields and other
accessories, also `impl Game`), `src/ai.rs` (computer players, also `impl Game`),
`src/fx.rs` (skies, particles, flash, shake, sound cues), `src/sound.rs` (rodio,
synthesised), `src/menu.rs` (title, options, saved to `~/.config/torched/options.toml`),
`src/input.rs` (gamepads via gilrs, mapped to keys), `src/render.rs` (Braille field, HUD, shop), `src/main.rs`. Session log
at `~/.local/state/torched/session.log`, level from `TORCHED_LOG`.
With no options `torched` opens on the title, computers playing behind it; any game option
on the command line, or `--demo`, starts a game directly (and `q` then quits rather than
returning to the title).

Preview: `asciiverse/torched.cast`, recorded headless at 100x30 per `hub/RECORDING.md`
with `torched --demo --seed 50 --cash 300000 --mute --for 48` (seed 50 picked by
`ai::preview`, an ignored search for lively seeds).

Measurements, not checks (`cargo test --release <name> -- --ignored --nocapture`):
`sim::timing` times every weapon's turn, `ai::timing` the length and outcome of demo
rounds, `ai::accuracy` each computer player's first-shot hit rate.

Found in the source while building 0.1.0:

- Xscorch has eight generators in code, not seven: Valley is there too. Double Mountain is
  drawn with the Mountain profile (`slscape.c`), so the two look alike.
- Xscorch has no fall damage ("Currently, parachutes do NOTHING", `accessories.def`). The
  fall damage here (2 per unit past 3) is ours, for `ROADMAP.md`; see `sim.rs`.
- Power is capped by life, so a badly hurt tank can't reach far (`sc_player_advance_power`).
- Xscorch's `weapons.def` has 39 entries, not 44: 36 weapons, plus No Weapon, the BMM
  Child (only made by a Black Market MIRV) and a Roller Tester.
- Unfinished in Xscorch, so Torched's own: diggers and sandhogs (hidden there, "partially
  written"; sandhogs had no behaviour at all), air viscosity ("N/A" in its dialog). Ours:
  diggers and sandhogs burrow `digLength` units clearing a `tunnelRadius` tunnel, turning
  15% of the time, and sandhogs then detonate; drag is linear, `dv/dt = a - k v`.
- Also ours: rollers head downhill on landing and move at a watchable pace (Xscorch moves
  them sixteen times per step). They gain 0.5 speed per unit fallen, as in Xscorch, so
  they get fast.
- Unfinished in Xscorch, so ours (0.3.0): Teleporter (to a random clear spot, the turn
  goes on), Parachute (spent to cancel a fall's damage; the fall damage is ours too),
  Greedy scoring (Standard, plus a tenth of unused stock's resale value each round,
  `stockRate` in `scorings.def`).
- Xscorch plays all-fire-at-once by default; Torched defaults to turn-based, which reads
  better at one keyboard (`--together` for Xscorch's way). Play order defaults to random
  as in Xscorch. Batteries restore only 1% of life each, as in Xscorch: weak, but faithful.
- Contact triggers only matter with `--tunneling` (off by default in Xscorch too).
- Computer players (0.4.0) follow `sai/`: Xscorch's budgets, buying orders, yield and
  precision ratings, victim choice and aiming rules. Insanity is Annihilater in Xscorch
  ("No one knows ...") and here. Shooter, Spreader and Chooser ignore wind, as in
  Xscorch, and hit about a quarter of first shots in the default wind; Calculater,
  Annihilater and Insanity allow for it. Torched's own additions, each because Xscorch's
  AI fails without it: lobs never reach a ceiling (Xscorch checks only direct shots, so
  its lobs hit concrete ceilings); victim-keepers learn to aim off from their last miss
  (the hook Xscorch's `doc/AI` asks for); when the formulas find no shot, the aiming
  kinds try shots out by simulation; the wind kinds always refine by simulation
  (Xscorch's optional scan refinement), and the simulation knows about magnetic shields.
  When only computers are left and no tank loses life for six turns each, the round is a
  stalemate. `--demo` uses Chooser, Calculater, Annihilater and Insanity: about a minute
  a round, five rounds in six won.
- Skies are Xscorch's two, Night (default, as there) and Fire, with its gradient colours
  toned down for the terminal. The ground stays Torched's amber-brown, not Xscorch's
  green. Terrain is random each round by default (Xscorch defaults to Traditional).
- Defaults as in Xscorch: wrap walls, wind up to 0.05 changing every turn. With wrap
  walls, blasts, spills and damage wrap around the edges too.

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

- 36 weapons: missiles and nukes, Leap Frog, Funky Bomb, MIRV and Black Market MIRV, Death's
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
