//! Everything a player does that changes the game, as a value. At one
//! keyboard a command is applied at once; over a network every peer
//! applies the same commands in the same order and stays in step.

use serde::{Deserialize, Serialize};

use crate::ai::{AiKind, Brain};
use crate::kit::Aisle;
use crate::sim::{Game, MAX_POWER, Phase};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cmd {
    /// Where the turret points, the power and the weapon, sent now and
    /// then while a player aims, so others see it move.
    Aim { angle: i32, power: i32, weapon: usize },
    /// Fire with this aim.
    Fire { angle: i32, power: i32, weapon: usize },
    Shield,
    Battery,
    Triggers(bool),
    Drive(i32),
    Teleport,
    /// Buy or sell a bundle of an item, by its index in the weapons or
    /// accessories.
    Buy { aisle: Aisle, item: usize },
    Sell { aisle: Aisle, item: usize },
    ShopDone,
    /// Past this round's results; the round makes repeats harmless.
    Advance { round: u32 },
    /// The player left; a computer takes the tank over.
    Takeover,
}

impl Game {
    /// Whether the command applies now. Commands wait in line until it
    /// does: a shot waits for its tank's turn.
    pub fn can_apply(&self, tank: usize, cmd: &Cmd) -> bool {
        match cmd {
            Cmd::Takeover => true,
            Cmd::Advance { round } => matches!(self.phase, Phase::RoundOver | Phase::GameOver) && self.round == *round,
            Cmd::Buy { .. } | Cmd::Sell { .. } | Cmd::ShopDone => self.phase == Phase::Shop && self.shop.player == tank,
            _ => self.phase == Phase::Aim && self.turn == tank,
        }
    }

    /// Apply a command for a tank. Call only when `can_apply`.
    pub fn apply(&mut self, tank: usize, cmd: &Cmd) {
        match cmd {
            Cmd::Aim { angle, power, weapon } => self.set_aim(tank, *angle, *power, *weapon),
            Cmd::Fire { angle, power, weapon } => {
                self.set_aim(tank, *angle, *power, *weapon);
                self.fire();
            }
            Cmd::Shield => {
                self.raise_shield();
            }
            Cmd::Battery => {
                self.use_battery();
            }
            Cmd::Triggers(on) => {
                if self.tanks[tank].triggers != *on {
                    self.toggle_triggers();
                }
            }
            Cmd::Drive(d) => {
                self.drive(*d);
            }
            Cmd::Teleport => {
                self.teleport();
            }
            Cmd::Buy { aisle, item } | Cmd::Sell { aisle, item } => {
                let list = self.aisle(*aisle);
                if let Some(at) = list.iter().position(|i| i == item) {
                    self.shop.aisle = *aisle;
                    self.shop.cursor = at;
                    if matches!(cmd, Cmd::Buy { .. }) {
                        self.shop_buy();
                    } else {
                        self.shop_sell();
                    }
                }
            }
            Cmd::ShopDone => self.shop_done(),
            Cmd::Advance { .. } => self.advance(),
            Cmd::Takeover => {
                let t = &mut self.tanks[tank];
                if t.brain.is_none() {
                    let kind = AiKind::Chooser;
                    t.brain = Some(Brain { kind, real: kind, victim: None, offset: 0.0, last_aim: None });
                    t.name = format!("{} (computer)", t.name);
                    self.plan = None;
                }
            }
        }
    }

    fn set_aim(&mut self, tank: usize, angle: i32, power: i32, weapon: usize) {
        let owns = weapon < self.arsenal.weapons.len() && (self.cfg.free || self.owns(tank, weapon));
        let t = &mut self.tanks[tank];
        t.angle = angle.clamp(0, 180);
        t.power = power.clamp(0, MAX_POWER.min(t.life));
        if owns {
            t.weapon = weapon;
        }
    }

    /// What this player's aim is now, as a command.
    pub fn aim_of(&self, tank: usize, fire: bool) -> Cmd {
        let t = &self.tanks[tank];
        let (angle, power, weapon) = (t.angle, t.power, t.weapon);
        if fire { Cmd::Fire { angle, power, weapon } } else { Cmd::Aim { angle, power, weapon } }
    }

    /// A fingerprint of everything that decides what happens next, for
    /// peers to compare. Leaves out the aim of whoever is aiming, which
    /// moves on screen before it's sent.
    pub fn checksum(&self) -> u64 {
        let mut h = Fnv::default();
        h.u64(self.round as u64);
        h.u64(self.turn as u64);
        h.u64(self.phase as u64);
        h.f64(self.wind);
        for t in &self.tanks {
            for v in [t.x as i64, t.y as i64, t.life as i64, t.alive as i64, t.money, t.fuel as i64, t.wins as i64, t.kills as i64] {
                h.u64(v as u64);
            }
            h.u64(t.shield.map_or(-1, |s| s.life as i64) as u64);
            t.stock.iter().chain(&t.kit).for_each(|&n| h.u64(n as u64));
        }
        for x in 0..crate::land::WIDTH as i32 {
            h.u64(self.land.top(x) as u64);
        }
        if let Ok(bytes) = postcard::to_allocvec(&self.rng) {
            h.bytes(&bytes);
        }
        h.0
    }
}

/// FNV-1a, 64-bit: small and the same on every machine.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Config;
    use crate::sim::tests::new_game;

    fn pair() -> (Game, Game) {
        let cfg = Config { players: 2, ai: vec![AiKind::Calculater], seed: 11, rounds: 2, ..Config::default() };
        (new_game(cfg.clone()), new_game(cfg))
    }

    /// Two games fed the same commands stay identical, whatever the
    /// timing: one runs extra ticks between commands.
    #[test]
    fn same_commands_same_game() {
        let (mut a, mut b) = pair();
        let mut log: Vec<(usize, Cmd)> = Vec::new();
        // Play game a by "hand", logging every command.
        for step in 0..20_000 {
            a.tick();
            let cmd = match a.phase {
                Phase::Shop if !a.is_ai(a.shop.player) => {
                    let item = a.aisle(Aisle::Weapons)[step % 3];
                    if step % 2 == 0 { Some((a.shop.player, Cmd::Buy { aisle: Aisle::Weapons, item })) } else { Some((a.shop.player, Cmd::ShopDone)) }
                }
                Phase::Aim if !a.is_ai(a.turn) => {
                    let t = &a.tanks[a.turn];
                    Some((a.turn, Cmd::Fire { angle: 40 + (step % 100) as i32, power: 300 + (step % 400) as i32, weapon: t.weapon }))
                }
                Phase::RoundOver | Phase::GameOver => Some((0, Cmd::Advance { round: a.round })),
                _ => None,
            };
            if let Some((tank, c)) = cmd
                && a.can_apply(tank, &c)
            {
                a.apply(tank, &c);
                log.push((tank, c));
            }
            if a.phase == Phase::GameOver || log.len() > 60 {
                break;
            }
        }
        assert!(log.len() > 10);
        // Game b gets the same commands, applied whenever they fit, with
        // a different rhythm of ticks.
        let mut queue: std::collections::VecDeque<_> = log.into_iter().collect();
        for step in 0..200_000 {
            if step % 7 == 0 {
                b.tick();
            }
            while let Some((tank, c)) = queue.front() {
                if b.can_apply(*tank, c) {
                    let (tank, c) = queue.pop_front().unwrap();
                    b.apply(tank, &c);
                } else {
                    break;
                }
            }
            b.tick();
            if queue.is_empty() {
                break;
            }
        }
        assert!(queue.is_empty(), "b got stuck");
        // Let both settle the same way.
        while a.phase == Phase::Action || a.phase == Phase::Settle {
            a.tick();
        }
        while b.phase == Phase::Action || b.phase == Phase::Settle {
            b.tick();
        }
        assert_eq!(a.round, b.round);
        assert_eq!(a.checksum(), b.checksum());
    }

    #[test]
    fn a_snapshot_restores_the_same_game() {
        let (mut a, _) = pair();
        for _ in 0..500 {
            a.tick();
        }
        let bytes = postcard::to_allocvec(&a).unwrap();
        let mut b: Game = postcard::from_bytes(&bytes).unwrap();
        b.arsenal = a.arsenal.clone();
        b.accessories = a.accessories.clone();
        assert_eq!(a.checksum(), b.checksum());
        for _ in 0..3000 {
            a.tick();
            b.tick();
        }
        assert_eq!(a.checksum(), b.checksum());
        assert!(bytes.len() < 200_000, "snapshot is {} bytes", bytes.len());
    }
}
