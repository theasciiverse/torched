# Torched

Artillery on destructible ground, in the terminal. Tanks on a range of
hills drawn in Braille, taking turns to lob shells, nukes, napalm, rollers
and diggers at each other while the ground is blown away beneath them.

Torched is a total conversion of Xscorch (Justin David Smith and Jacob Luna
Lundberg, 2000 to 2009), the free remake of Wendell Hicken's Scorched Earth
(1991), written from scratch in Rust with Ratatui. Xscorch's weapons,
accessories and scoring rules are read from its own `.def` files.

## Playing

```
torched                       title screen, computer players behind the menu
torched --ai shooter,chooser  you against two computer players
torched --demo                the game plays itself
torched --host                host a network game; others --join the ticket shown
torched --help                every option
```

Keys: arrows aim (Shift for big steps), PgUp and PgDn change power by 20,
Tab and Shift+Tab pick a weapon, Space or Enter fires, q quits. s raises a
shield, b uses a battery, c toggles contact triggers, a and d drive on
fuel, x teleports, m turns the sound off and on.

Shop: up and down to choose, Enter or Right buys, Left sells, Tab switches
between weapons and accessories, Space is done.

Mouse and gamepads work too. Options set on the title screen are kept in
`~/.config/torched/options.toml`.

## Building

```
cargo build --release
```

On Linux, sound needs ALSA (`libasound2-dev` to build) and gamepads need
udev (`libudev-dev`).

## Licence

GPL-2.0-only, as Xscorch. See `LICENSE`.
