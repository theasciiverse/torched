# Roadmap

Torched is built from scratch in Rust (David, 2026-11-06), each release played before the
next is started. Xscorch's rules and numbers are the reference throughout (see `CLAUDE.md`
for where each lives in its source).

## 0.1.0: one shot

- Ground from Xscorch's seven terrain generators, drawn in Braille
- Two to four tanks, hot-seat: aim, power, fire
- Gravity and flight as in `sphysics.c`; the Baby Missile
- Craters, loose dirt falling, tanks falling and taking damage
- Rounds and a winner; session logging

## 0.2.0: the arsenal

- All 44 weapons from `weapons.def`: explosives, MIRVs and split weapons, napalm that runs
  downhill, rollers, riot charges, diggers and sandhogs, dirt weapons, tracers
- Wind, air viscosity and the six wall kinds
- Weapons read from data files in the spirit of `weapons.def`, not hard-coded

## 0.3.0: the shop

- Money, interest and the three scoring rules
- The shop between rounds, with bundles and arms levels
- All 17 accessories: shields, fuel and driving, parachutes, Triple Turret, teleporter,
  batteries, Auto Defense, Contact Trigger, Solar Panel
- Play order options and the all-fire-at-once mode

## 0.4.0: computer players

- All seven, Moron to Insanity, buying and aiming as in `sai/`
- Mixed games of people and computer players; `--demo`

## 0.5.0: look and sound

- Skies, explosion and napalm effects, shot trails, screen shake
- Sound, synthesised; title screen with computer players behind the menu
- Options screen for every physics and game setting, remembered between runs
- Preview recording and the catalog entry

## 0.6.0: network play

- LAN and internet games over iroh, people and computer players mixed

## 1.0.0

- Everything above in, balance played through against Xscorch
