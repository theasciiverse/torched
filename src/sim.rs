//! The game itself, with no terminal in it: tanks, the shot in flight,
//! blasts, falling dirt and tanks, turns and rounds. Rules and numbers are
//! Xscorch's (`sphysics.c`, `splayer.c`, `sexplosion.c`) unless marked.

use std::collections::VecDeque;

use log::info;
use rand::{Rng, SeedableRng, rngs::StdRng};

use crate::land::{self, Generator, Land};

/// Gravity in units per step per step (`SC_PHYSICS_GRAVITY_DEF`).
pub const GRAVITY: f64 = 0.5;
/// Units per step per point of power (`SC_PHYSICS_VELOCITY_SCL`).
pub const VELOCITY_SCALE: f64 = 0.027;
pub const MAX_POWER: i32 = 1000;
pub const MAX_LIFE: i32 = 1000;
pub const POWER_STEP: i32 = 1;
pub const POWER_BIGSTEP: i32 = 20;
pub const TURRET_STEP: i32 = 1;
pub const TURRET_BIGSTEP: i32 = 5;

/// The Standard tank profile.
pub const TANK_RADIUS: i32 = 9;
pub const TURRET_RADIUS: f64 = 13.0;
pub const SHELF_SIZE: i32 = 5;
/// Height of the hull above its base, from the profile's seven rows.
pub const TANK_HEIGHT: i32 = 7;

/// Flight steps per 60 Hz tick. Xscorch's nominal 20 steps a second
/// (50 ms cycle) is slow on a terminal; 30 a second crosses the field at
/// full power in about three seconds.
pub const STEPS_PER_TICK: f64 = 0.5;
/// Ticks a blast takes to reach full size, and again to fade.
const BLAST_TICKS: u32 = 9;
/// Dirt and tanks fall this far per tick while settling. Xscorch allows
/// up to 48 and 32 per cycle; smaller here so the fall is seen.
const DIRT_DROP_PER_TICK: usize = 6;
const TANK_DROP_PER_TICK: i32 = 4;
/// Steps before a shot that never lands fizzles out.
const SHOT_TIMEOUT: f64 = 4000.0;

/// Fall damage. Not Xscorch, which has none ("Currently, parachutes do
/// NOTHING", `accessories.def`); the Scorched Earth rule `ROADMAP.md` asks
/// for. Falls up to the climb height are free.
pub const FALL_FREE: i32 = 3;
pub const FALL_DAMAGE_PER_UNIT: i32 = 2;

/// The Baby Missile (`weapons.def`).
pub const BABY_RADIUS: f64 = 20.0;
pub const BABY_FORCE: f64 = 400.0;

/// Death explosions (`sweapon.h`, `sc_player_death`).
const SMALL_EXPLOSION: (f64, f64) = (40.0, 100.0);
const MEDIUM_EXPLOSION: (f64, f64) = (120.0, 300.0);
const LARGE_EXPLOSION: (f64, f64) = (200.0, 700.0);

pub const TANK_COLOURS: usize = 4;

#[derive(Clone, Debug)]
pub struct Tank {
    pub name: String,
    pub colour: usize,
    pub x: i32,
    /// Base of the hull.
    pub y: i32,
    /// Degrees, 0 right, 90 up, 180 left.
    pub angle: i32,
    pub power: i32,
    pub life: i32,
    pub alive: bool,
    pub wins: u32,
    pub kills: u32,
    /// Where the last shot of this tank flew, kept for aiming.
    pub last_trail: Vec<(f64, f64)>,
    killed_by: Option<usize>,
    fall_from: Option<i32>,
}

impl Tank {
    /// Pivot of the turret.
    pub fn pivot(&self) -> (f64, f64) {
        (self.x as f64, (self.y + TANK_HEIGHT - 1) as f64)
    }

    fn muzzle(&self) -> (f64, f64) {
        let (px, py) = self.pivot();
        let a = (self.angle as f64).to_radians();
        (px + (TURRET_RADIUS + 1.0) * a.cos(), py + (TURRET_RADIUS + 1.0) * a.sin())
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        self.alive
            && (x - self.x as f64).abs() <= TANK_RADIUS as f64
            && y >= self.y as f64
            && y <= (self.y + TANK_HEIGHT) as f64
    }
}

#[derive(Clone, Debug)]
pub struct Shot {
    pub owner: usize,
    ctr: (f64, f64),
    vel: (f64, f64),
    t: f64,
    pub x: f64,
    pub y: f64,
    pub trail: Vec<(f64, f64)>,
}

impl Shot {
    /// Position at step t: `ctr + v t - 0.5 g t^2` (`sc_traj_get_current_*`).
    fn at(&self, t: f64) -> (f64, f64) {
        (
            self.ctr.0 + t * self.vel.0,
            self.ctr.1 + t * (self.vel.1 - 0.5 * t * GRAVITY),
        )
    }
}

#[derive(Clone, Debug)]
pub struct Blast {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    force: f64,
    owner: usize,
    pub age: u32,
}

impl Blast {
    /// Radius to draw now, growing then fading.
    pub fn shown_radius(&self) -> f64 {
        let a = self.age.min(2 * BLAST_TICKS) as f64;
        let k = BLAST_TICKS as f64;
        let f = if a <= k { a / k } else { (2.0 * k - a) / k };
        self.radius * f.max(0.15)
    }

    /// 0 to 1, how far into the fade.
    pub fn fade(&self) -> f64 {
        (self.age.saturating_sub(BLAST_TICKS) as f64 / BLAST_TICKS as f64).min(1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Aim,
    Flight,
    Blast,
    Settle,
    RoundOver,
    GameOver,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub players: usize,
    pub rounds: u32,
    /// None draws a random one every round.
    pub generator: Option<Generator>,
    pub bumpiness: f64,
    pub seed: u64,
}

pub struct Game {
    pub cfg: Config,
    pub land: Land,
    pub generator: Generator,
    pub tanks: Vec<Tank>,
    pub turn: usize,
    pub phase: Phase,
    pub round: u32,
    pub shot: Option<Shot>,
    pub blast: Option<Blast>,
    pending: VecDeque<Blast>,
    pub message: String,
    pub round_winner: Option<usize>,
    rng: StdRng,
}

impl Game {
    pub fn new(cfg: Config) -> Game {
        let mut rng = StdRng::seed_from_u64(cfg.seed);
        let tanks = (0..cfg.players)
            .map(|i| Tank {
                name: format!("Player {}", i + 1),
                colour: i % TANK_COLOURS,
                x: 0,
                y: 0,
                angle: 90,
                power: 200,
                life: MAX_LIFE,
                alive: true,
                wins: 0,
                kills: 0,
                last_trail: Vec::new(),
                killed_by: None,
                fall_from: None,
            })
            .collect();
        let generator = cfg.generator.unwrap_or_else(|| Generator::random(&mut rng));
        let mut g = Game {
            land: Land::empty(),
            generator,
            tanks,
            turn: 0,
            phase: Phase::Aim,
            round: 0,
            shot: None,
            blast: None,
            pending: VecDeque::new(),
            message: String::new(),
            round_winner: None,
            rng,
            cfg,
        };
        g.start_round();
        g
    }

    pub fn current(&self) -> &Tank {
        &self.tanks[self.turn]
    }

    fn start_round(&mut self) {
        self.round += 1;
        self.generator = self.cfg.generator.unwrap_or_else(|| Generator::random(&mut self.rng));
        self.land = Land::generate(self.generator, self.cfg.bumpiness, &mut self.rng);
        self.shot = None;
        self.blast = None;
        self.pending.clear();
        self.round_winner = None;

        // Spread the tanks out, a slot each in random order, and level a
        // platform under each (`sc_land_level_around`).
        let n = self.tanks.len();
        let mut slots: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            slots.swap(i, self.rng.random_range(0..=i));
        }
        let slot_w = land::WIDTH as f64 / n as f64;
        for (i, t) in self.tanks.iter_mut().enumerate() {
            let jitter = self.rng.random_range(-0.25..0.25) * slot_w;
            let x = ((slots[i] as f64 + 0.5) * slot_w + jitter)
                .clamp(TANK_RADIUS as f64 * 3.0, land::WIDTH as f64 - TANK_RADIUS as f64 * 3.0) as i32;
            let ht = self.land.top(x).max(1);
            self.land.level(x, TANK_RADIUS, ht);
            t.x = x;
            t.y = ht;
            t.angle = self.rng.random_range(0..=180);
            t.power = MAX_POWER / 5;
            t.life = MAX_LIFE;
            t.alive = true;
            t.killed_by = None;
            t.fall_from = None;
            t.last_trail.clear();
        }
        self.turn = self.rng.random_range(0..n);
        self.phase = Phase::Aim;
        self.message = format!("Round {} of {}: {}", self.round, self.cfg.rounds, self.generator.name());
        info!("round {} start, terrain {:?}, first {}", self.round, self.generator, self.turn);
    }

    /// Start the next round, or a fresh game after the last.
    pub fn advance(&mut self) {
        match self.phase {
            Phase::RoundOver => self.start_round(),
            Phase::GameOver => {
                for t in &mut self.tanks {
                    t.wins = 0;
                    t.kills = 0;
                }
                self.round = 0;
                self.start_round();
            }
            _ => {}
        }
    }

    pub fn adjust_angle(&mut self, delta: i32) {
        if self.phase == Phase::Aim {
            let t = &mut self.tanks[self.turn];
            t.angle = (t.angle + delta).clamp(0, 180);
        }
    }

    pub fn adjust_power(&mut self, delta: i32) {
        if self.phase == Phase::Aim {
            let t = &mut self.tanks[self.turn];
            // Power is capped by life (`sc_player_advance_power`).
            t.power = (t.power + delta).clamp(0, MAX_POWER.min(t.life));
        }
    }

    pub fn fire(&mut self) {
        if self.phase != Phase::Aim {
            return;
        }
        let t = &self.tanks[self.turn];
        let (x, y) = t.muzzle();
        let a = (t.angle as f64).to_radians();
        let v = t.power as f64 * VELOCITY_SCALE;
        info!("{} fires: angle {} power {}", t.name, t.angle, t.power);
        self.shot = Some(Shot {
            owner: self.turn,
            ctr: (x, y),
            vel: (v * a.cos(), v * a.sin()),
            t: 0.0,
            x,
            y,
            trail: vec![(x, y)],
        });
        self.message.clear();
        self.phase = Phase::Flight;
    }

    pub fn tick(&mut self) {
        match self.phase {
            Phase::Flight => self.tick_flight(),
            Phase::Blast => self.tick_blast(),
            Phase::Settle => self.tick_settle(),
            Phase::Aim | Phase::RoundOver | Phase::GameOver => {}
        }
    }

    fn tick_flight(&mut self) {
        let Some(shot) = self.shot.as_mut() else {
            self.phase = Phase::Settle;
            return;
        };
        let t1 = shot.t + STEPS_PER_TICK;
        let (x0, y0) = (shot.x, shot.y);
        let (x1, y1) = shot.at(t1);
        shot.t = t1;
        // Walk the segment no more than one unit at a time, as
        // `_sc_traj_traverse_path` does.
        let n = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
        let mut impact = None;
        let mut gone = false;
        for i in 1..=n {
            let f = i as f64 / n as f64;
            let (x, y) = (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
            // Walls: none. Off either side and the shot is lost; the sky
            // is open above.
            if x < 0.0 || x >= land::WIDTH as f64 {
                gone = true;
                break;
            }
            if y < 0.0 {
                impact = Some((x, 0.0));
                break;
            }
            let (ix, iy) = (x.round() as i32, y.round() as i32);
            if self.land.get(ix, iy) || self.tanks.iter().any(|t| t.contains(x, y)) {
                impact = Some((x, y));
                break;
            }
            shot.x = x;
            shot.y = y;
        }
        if shot.trail.last() != Some(&(shot.x, shot.y)) {
            shot.trail.push((shot.x, shot.y));
        }
        if shot.t > SHOT_TIMEOUT {
            gone = true;
        }

        if impact.is_none() && !gone {
            return;
        }
        let shot = self.shot.take().expect("shot in flight");
        let owner = shot.owner;
        self.tanks[owner].last_trail = shot.trail;
        match impact {
            Some((x, y)) => {
                self.pending.push_back(Blast { x, y, radius: BABY_RADIUS, force: BABY_FORCE, owner, age: 0 });
                self.phase = Phase::Blast;
            }
            None => {
                say(&mut self.message, "The shot sailed off the field.");
                self.phase = Phase::Settle;
            }
        }
    }

    fn tick_blast(&mut self) {
        if self.blast.is_none() {
            self.blast = self.pending.pop_front();
            if self.blast.is_none() {
                self.phase = Phase::Settle;
                return;
            }
        }
        let b = self.blast.as_mut().expect("blast");
        b.age += 1;
        if b.age == BLAST_TICKS {
            let b = b.clone();
            self.detonate(&b);
        }
        if self.blast.as_ref().is_some_and(|b| b.age >= 2 * BLAST_TICKS) {
            self.blast = None;
        }
    }

    /// Crater and damage, at the blast's full size.
    fn detonate(&mut self, b: &Blast) {
        self.land.carve(b.x, b.y, b.radius);
        let mut hits = Vec::new();
        for t in self.tanks.iter_mut() {
            if !t.alive {
                continue;
            }
            // `sc_expl_damage_at_point`, measured to the tank's base point.
            let (dx, dy) = (t.x as f64 - b.x, t.y as f64 - b.y);
            let damage = (b.force * (1.0 - (dx * dx + dy * dy) / (b.radius * b.radius))).round() as i32;
            if damage > 0 {
                t.life -= damage;
                t.killed_by = Some(b.owner);
                hits.push(format!("{} -{}", t.name, damage));
                info!("blast at ({:.0}, {:.0}) r {} hits {} for {damage}", b.x, b.y, b.radius, t.name);
            }
        }
        if !hits.is_empty() {
            say(&mut self.message, &hits.join(", "));
        }
        self.queue_deaths();
    }

    /// Tanks out of life explode: a small blast, a medium one half the
    /// time, a large one a tenth of those (`sc_player_death`; its
    /// one-in-ten napalm death waits for napalm).
    fn queue_deaths(&mut self) -> bool {
        let mut any = false;
        for i in 0..self.tanks.len() {
            let t = &self.tanks[i];
            if !t.alive || t.life > 0 {
                continue;
            }
            any = true;
            let owner = t.killed_by.unwrap_or(i);
            let (x, y) = (t.x as f64, t.y as f64);
            let name = t.name.clone();
            self.tanks[i].alive = false;
            self.tanks[i].life = 0;
            if owner != i {
                self.tanks[owner].kills += 1;
            }
            let killer = &self.tanks[owner].name;
            let line = if owner == i { format!("{name} destroyed itself!") } else { format!("{killer} destroyed {name}!") };
            say(&mut self.message, &line);
            info!("{name} destroyed by {killer}");
            let mut stages = vec![SMALL_EXPLOSION];
            if self.rng.random::<f64>() < 0.5 {
                stages.push(MEDIUM_EXPLOSION);
                if self.rng.random::<f64>() < 0.1 {
                    stages.push(LARGE_EXPLOSION);
                }
            }
            for (radius, force) in stages {
                self.pending.push_back(Blast { x, y, radius, force, owner, age: 0 });
            }
        }
        if any {
            self.phase = Phase::Blast;
        }
        any
    }

    fn tick_settle(&mut self) {
        let dirt = self.land.drop_cycle(DIRT_DROP_PER_TICK);
        let mut falling = false;
        for t in self.tanks.iter_mut().filter(|t| t.alive) {
            let mut moved = 0;
            while moved < TANK_DROP_PER_TICK && t.y > 0 && !supported(&self.land, t) {
                t.fall_from.get_or_insert(t.y);
                t.y -= 1;
                moved += 1;
            }
            if moved > 0 {
                falling = true;
            } else if let Some(from) = t.fall_from.take() {
                let dist = from - t.y;
                let damage = (dist - FALL_FREE).max(0) * FALL_DAMAGE_PER_UNIT;
                if damage > 0 {
                    t.life -= damage;
                    say(&mut self.message, &format!("{} fell {} for {}", t.name, dist, damage));
                    info!("{} fell {dist} for {damage}", t.name);
                }
            }
        }
        if dirt || falling {
            return;
        }
        if self.queue_deaths() {
            return;
        }
        self.end_turn();
    }

    fn end_turn(&mut self) {
        let alive: Vec<usize> = (0..self.tanks.len()).filter(|&i| self.tanks[i].alive).collect();
        if alive.len() <= 1 {
            self.round_winner = alive.first().copied();
            if let Some(w) = self.round_winner {
                self.tanks[w].wins += 1;
            }
            self.message = match self.round_winner {
                Some(w) => format!("{} wins round {}", self.tanks[w].name, self.round),
                None => format!("Nobody survives round {}", self.round),
            };
            info!("{}", self.message);
            self.phase = if self.round >= self.cfg.rounds { Phase::GameOver } else { Phase::RoundOver };
            return;
        }
        let n = self.tanks.len();
        let mut next = (self.turn + 1) % n;
        while !self.tanks[next].alive {
            next = (next + 1) % n;
        }
        self.turn = next;
        // Power can't exceed what's left of life.
        let t = &mut self.tanks[next];
        t.power = t.power.min(t.life);
        self.phase = Phase::Aim;
    }

    /// Standings: indices by wins, then kills.
    pub fn standings(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.tanks.len()).collect();
        order.sort_by_key(|&i| (std::cmp::Reverse(self.tanks[i].wins), std::cmp::Reverse(self.tanks[i].kills)));
        order
    }
}

/// Add to the turn's running message.
fn say(message: &mut String, s: &str) {
    if !message.is_empty() {
        message.push_str(". ");
    }
    message.push_str(s);
}

/// A tank rests on ground under the middle of its hull, within the shelf
/// (`sc_land_support` lets a tank overhang by `shelfSize`).
fn supported(land: &Land, t: &Tank) -> bool {
    let reach = TANK_RADIUS - SHELF_SIZE;
    (-reach..=reach).any(|dx| land.get(t.x + dx, t.y - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(players: usize) -> Game {
        Game::new(Config { players, rounds: 3, generator: Some(Generator::Plains), bumpiness: 50.0, seed: 1 })
    }

    fn run_turn(g: &mut Game) {
        g.fire();
        for _ in 0..100_000 {
            g.tick();
            if g.phase != Phase::Flight && g.phase != Phase::Blast && g.phase != Phase::Settle {
                return;
            }
        }
        panic!("turn never ended");
    }

    #[test]
    fn tanks_start_on_the_ground() {
        let g = game(4);
        for t in &g.tanks {
            assert!(supported(&g.land, t), "{} floats", t.name);
            assert!(!g.land.get(t.x, t.y), "{} buried", t.name);
        }
    }

    #[test]
    fn straight_up_comes_back_down_on_the_shooter() {
        let mut g = game(2);
        let me = g.turn;
        g.tanks[me].angle = 90;
        g.tanks[me].power = 300;
        run_turn(&mut g);
        assert!(g.tanks[me].life < MAX_LIFE, "a shot straight up should land on its tank");
    }

    #[test]
    fn flight_matches_the_closed_form() {
        // Height reached is v^2 / 2g (`_sc_traj_height`).
        let mut g = game(2);
        let me = g.turn;
        g.tanks[me].angle = 90;
        g.tanks[me].power = 500;
        let start = g.tanks[me].muzzle().1;
        g.fire();
        let mut top: f64 = 0.0;
        while g.phase == Phase::Flight {
            g.tick();
            if let Some(s) = &g.shot {
                top = top.max(s.y);
            }
        }
        let v = 500.0 * VELOCITY_SCALE;
        let expect = start + v * v / (2.0 * GRAVITY);
        assert!((top - expect).abs() < 1.5, "apex {top}, expected {expect}");
    }

    #[test]
    fn a_game_plays_to_the_end() {
        let mut g = game(3);
        let mut rounds_seen = 0;
        // A scripted gunner: lob at 45 degrees at the nearest tank and
        // correct the power from where the last shot came down.
        let mut guess = vec![None::<f64>; g.tanks.len()];
        for _ in 0..3000 {
            match g.phase {
                Phase::Aim => {
                    let me = g.turn;
                    let mx = g.tanks[me].x;
                    let target = (0..g.tanks.len())
                        .filter(|&i| i != me && g.tanks[i].alive)
                        .min_by_key(|&i| (g.tanks[i].x - mx).abs())
                        .unwrap();
                    let tx = g.tanks[target].x;
                    g.tanks[me].angle = if tx < mx { 135 } else { 45 };
                    // Range on flat ground is v^2 / g at 45 degrees.
                    let flat = ((tx - mx).abs() as f64 * GRAVITY).sqrt() / VELOCITY_SCALE;
                    let power = *guess[me].get_or_insert(flat);
                    g.tanks[me].power = (power as i32).clamp(0, g.tanks[me].life);
                    run_turn(&mut g);
                    if let Some(&(lx, _)) = g.tanks[me].last_trail.last() {
                        let short = (tx - mx).abs() as f64 - (lx - mx as f64).abs();
                        guess[me] = Some(power + short * 0.4);
                    }
                }
                Phase::RoundOver => {
                    rounds_seen += 1;
                    guess.iter_mut().for_each(|p| *p = None);
                    g.advance();
                }
                Phase::GameOver => return assert_eq!(rounds_seen, 2),
                p => panic!("stuck in {p:?}"),
            }
        }
        panic!("game never ended: round {}, tanks {:?}", g.round, g.tanks.iter().map(|t| (t.x, t.life)).collect::<Vec<_>>());
    }
}
