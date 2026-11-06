//! The game itself, with no terminal in it: tanks, shells in flight,
//! blasts, falling dirt and tanks, turns and rounds. Rules and numbers are
//! Xscorch's (`sphysics.c`, `strack.c`, `sphoenix.c`, `splayer.c`,
//! `sexplosion.c`) unless marked as Torched's own.

use std::rc::Rc;

use log::info;
use rand::{Rng, SeedableRng, rngs::StdRng};

use crate::economy::{Accessories, Item, Scoring};
use crate::land::{self, Generator, Land};
use crate::physics::{self, BORDERS_EXTEND, GRAVITY, Path, TIMEOUT, VELOCITY_SCALE, Walls};
use crate::weapons::{Arsenal, Weapon};

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
/// Dirt and tanks fall this far per tick while settling. Xscorch allows
/// up to 48 and 32 per cycle; smaller here so the fall is seen.
const DIRT_DROP_PER_TICK: usize = 6;
const TANK_DROP_PER_TICK: i32 = 4;

/// Fall damage. Not Xscorch, which has none ("Currently, parachutes do
/// NOTHING", `accessories.def`; its Tanks Fall setting is "N/A"); the
/// Scorched Earth rule `ROADMAP.md` asks for. Falls up to the climb
/// height are free.
pub const FALL_FREE: i32 = 3;
pub const FALL_DAMAGE_PER_UNIT: i32 = 2;

/// Death explosions (`sweapon.h`, `sc_player_death`).
const SMALL_EXPLOSION: (f64, f64) = (40.0, 100.0);
const MEDIUM_EXPLOSION: (f64, f64) = (120.0, 300.0);
const LARGE_EXPLOSION: (f64, f64) = (200.0, 700.0);
const DEATH_NAPALM: (f64, f64, usize) = (13.0, 30.0, 400);

/// Slices of a tick: Xscorch steps in sixteenths (`SC_TRAJ_TIME_SUBSTEP`).
const SLICES: usize = 8;
/// Tunnelling (`sphysics.h`): speed kept per unit of dirt, and the speed
/// below which a shell stops and goes off.
const TUNNEL_DAMPING: f64 = 0.8;
const TUNNEL_MIN_VEL: f64 = 0.5;

/// Phoenix numbers (`sphoenix.h`).
const PROB_AT_RAND: f64 = 0.01;
const INCREASE_FACTOR: f64 = 0.85;
const MIN_FROG_POWER: f64 = 70.0;
const MAX_FROG_POWER: f64 = 350.0;
const SPIDER_ARC: f64 = 128.0;
const SPIDER_POWER: f64 = 0.25;
const SPIDER_MAX_LEG_ATTEMPTS: usize = 100;

/// Rollers (`sphysics.h`): speed gained per unit fallen, lost per unit
/// climbed.
const ROLLER_FALL: f64 = 0.5;
const ROLLER_GAIN: f64 = 1.3;
/// Torched's own: rollers move this many units per step for each unit of
/// speed (Xscorch moves them sixteen times per step, too fast to watch),
/// never slower than ROLLER_MIN_SPEED.
const ROLLER_PACE: f64 = 2.0;
const ROLLER_MIN_SPEED: f64 = 1.0;
/// Torched's own: diggers burrow this fast, units per tick, and change
/// direction this often per unit (Xscorch's `SC_TRAJ_DIGGER_PROB`).
const DIG_PER_TICK: usize = 3;
const DIGGER_TURN_PROB: f64 = 0.15;

/// Liquids spread this many cells per tick (`SC_EXPL_LIQUID_STEP`), and
/// napalm burns on this long once spread.
const LIQUID_STEP: usize = 32;
const NAPALM_BURN_TICKS: u32 = 90;

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
    /// Selected weapon, an index into the arsenal.
    pub weapon: usize,
    /// Where the last shot of this tank flew, kept for aiming.
    pub last_trail: Vec<(f64, f64)>,
    pub(crate) killed_by: Option<usize>,
    pub(crate) fall_from: Option<i32>,
    pub money: i64,
    /// Weapons held, by arsenal index.
    pub stock: Vec<u32>,
    /// Accessories held, by index.
    pub kit: Vec<u32>,
    pub shield: Option<Shield>,
    /// Fuel opened from fuel tanks, in units of distance.
    pub fuel: u32,
    /// Contact triggers switched on.
    pub triggers: bool,
    pub suicides: u32,
    /// All-fire-at-once mode: orders given this volley.
    pub ready: bool,
}

/// A raised shield: which accessory, and its strength left.
#[derive(Clone, Copy, Debug)]
pub struct Shield {
    pub acc: usize,
    pub life: i32,
}

impl Tank {
    /// Pivot of the turret.
    pub fn pivot(&self) -> (f64, f64) {
        (self.x as f64, (self.y + TANK_HEIGHT - 1) as f64)
    }

    pub(crate) fn muzzle(&self) -> (f64, f64) {
        let (px, py) = self.pivot();
        let a = (self.angle as f64).to_radians();
        (px + (TURRET_RADIUS + 1.0) * a.cos(), py + (TURRET_RADIUS + 1.0) * a.sin())
    }

    pub(crate) fn contains(&self, land: &Land, x: f64, y: f64) -> bool {
        self.alive
            && land.dx(self.x as f64, x).abs() <= TANK_RADIUS as f64
            && y >= self.y as f64
            && y <= (self.y + TANK_HEIGHT) as f64
    }
}

#[derive(Clone, Copy, Debug)]
enum Motion {
    Flight { path: Path, t: f64 },
    Roll { vx: f64 },
    Dig { dir: (i32, i32), left: f64 },
}

/// A warhead on its way: flying, rolling or burrowing.
#[derive(Clone, Debug)]
pub struct Shell {
    pub weapon: usize,
    pub owner: usize,
    motion: Motion,
    pub x: f64,
    pub y: f64,
    /// Last place it was that wasn't an impact.
    clear: (f64, f64),
    pub trail: Vec<(f64, f64)>,
    /// Yield, shrinking with each Leap Frog bounce (`exp_res`).
    exp_res: f64,
    /// Bounces so far, for chain weapons.
    chain: u32,
    /// Steps alive, for the timeout.
    age: f64,
    /// Ticks to wait before moving, so a bounce waits for its crater.
    pub delay: u32,
    /// The shell the tank fired; its trail is kept for aiming.
    root: bool,
    /// Fired with a contact trigger: goes off on touching dirt even when
    /// tunnelling is on.
    triggered: bool,
    /// Tunnelling through dirt just now.
    tunnel: bool,
}

impl Shell {
    pub fn rolling(&self) -> bool {
        matches!(self.motion, Motion::Roll { .. })
    }

    pub fn digging(&self) -> bool {
        matches!(self.motion, Motion::Dig { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlastKind {
    Normal,
    Plasma,
    Riot,
    Dirt,
    Napalm,
    LiquidDirt,
}

#[derive(Clone, Debug)]
pub struct Blast {
    pub kind: BlastKind,
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    force: f64,
    owner: usize,
    pub age: u32,
    /// Ticks before it starts.
    pub delay: u32,
    /// Ticks to full size, and again to fade.
    pub grow: u32,
    /// (direction, width) in radians for wedge blasts.
    pub wedge: Option<(f64, f64)>,
    /// Cells of a liquid, in the order they fill.
    pub spill: Vec<(i32, i32)>,
    /// A spider leg's arc from the bomb to where it lands.
    pub arc: Vec<(f64, f64)>,
}

impl Blast {
    fn new(kind: BlastKind, x: f64, y: f64, radius: f64, force: f64, owner: usize) -> Blast {
        Blast {
            kind,
            x,
            y,
            radius,
            force,
            owner,
            age: 0,
            delay: 0,
            grow: (6.0 + radius / 10.0).min(30.0) as u32,
            wedge: None,
            spill: Vec::new(),
            arc: Vec::new(),
        }
    }

    fn liquid(&self) -> bool {
        matches!(self.kind, BlastKind::Napalm | BlastKind::LiquidDirt)
    }

    /// Ticks until a liquid has finished spreading.
    fn spread_ticks(&self) -> u32 {
        self.spill.len().div_ceil(LIQUID_STEP) as u32
    }

    /// The tick its effect lands.
    fn peak(&self) -> u32 {
        if self.liquid() { self.spread_ticks().max(1) } else { self.grow }
    }

    fn lifetime(&self) -> u32 {
        match self.kind {
            BlastKind::Napalm => self.peak() + NAPALM_BURN_TICKS,
            BlastKind::LiquidDirt => self.peak(),
            _ => 2 * self.grow,
        }
    }

    pub fn active(&self) -> bool {
        self.delay == 0
    }

    /// Radius to draw now, growing then fading.
    pub fn shown_radius(&self) -> f64 {
        let a = self.age.min(2 * self.grow) as f64;
        let k = self.grow as f64;
        let f = if a <= k { a / k } else { (2.0 * k - a) / k };
        self.radius * f.max(0.15)
    }

    /// 0 to 1, how far into the fade.
    pub fn fade(&self) -> f64 {
        let end = self.lifetime().saturating_sub(self.peak()).max(1);
        (self.age.saturating_sub(self.peak()) as f64 / end as f64).min(1.0)
    }

    /// Liquid cells showing now.
    pub fn wet(&self) -> &[(i32, i32)] {
        let n = (self.age as usize * LIQUID_STEP).min(self.spill.len());
        &self.spill[..n]
    }
}

/// A star in the night sky, placed fresh each round.
#[derive(Clone, Copy, Debug)]
pub struct Star {
    pub x: f64,
    pub y: f64,
    /// 0 to 1.
    pub bright: f64,
    /// 0 white, 1 warm, 2 blue.
    pub tint: u8,
    /// Some stars twinkle: speed in radians per tick, 0 for steady.
    pub twinkle: f64,
    pub phase: f64,
}

const STARS: usize = 90;

/// Smoke left by a smoke tracer, for the round.
#[derive(Clone, Debug)]
pub struct Smoke {
    pub owner: usize,
    pub points: Vec<(f64, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Between rounds, each player in turn.
    Shop,
    Aim,
    /// Shells and blasts under way.
    Action,
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
    pub walls: Walls,
    pub max_wind: f64,
    pub dynamic_wind: bool,
    pub viscosity: f64,
    /// Sandbox: every weapon free and endless, no shop.
    pub free: bool,
    pub cash: i64,
    pub interest: f64,
    pub dynamic_interest: bool,
    pub scoring: String,
    pub arms_level: u32,
    pub order: Order,
    /// Everyone aims, then all fire at once (Xscorch's synchronous mode).
    pub simultaneous: bool,
    /// Shells burrow through dirt while fast enough (Xscorch's
    /// tunnelling, off by default there too).
    pub tunneling: bool,
}

/// Who goes first each round (`sc_config_order`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Random,
    LosersFirst,
    WinnersFirst,
    RoundRobin,
}

impl Order {
    pub const ALL: [Order; 4] = [Order::Random, Order::LosersFirst, Order::WinnersFirst, Order::RoundRobin];

    pub fn name(self) -> &'static str {
        match self {
            Order::Random => "random",
            Order::LosersFirst => "losers-first",
            Order::WinnersFirst => "winners-first",
            Order::RoundRobin => "round-robin",
        }
    }

    pub fn parse(s: &str) -> Option<Order> {
        Order::ALL.into_iter().find(|o| o.name().eq_ignore_ascii_case(s))
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            players: 2,
            rounds: 10,
            generator: None,
            bumpiness: land::BUMPINESS_DEF,
            seed: 0,
            walls: Walls::Wrap,
            max_wind: physics::WIND_DEF,
            dynamic_wind: true,
            viscosity: 0.0,
            free: false,
            cash: crate::economy::DEF_CASH,
            interest: crate::economy::DEF_INTEREST,
            dynamic_interest: false,
            scoring: "Standard".into(),
            arms_level: crate::economy::ARMS_LEVEL_DEF,
            order: Order::Random,
            simultaneous: false,
            tunneling: false,
        }
    }
}

enum Hit {
    Tank,
    Land,
    Wall,
    Ground,
    Shield,
}

pub struct Game {
    pub cfg: Config,
    pub arsenal: Rc<Arsenal>,
    pub land: Land,
    pub generator: Generator,
    /// This round's walls (Random resolved).
    pub walls: Walls,
    pub wind: f64,
    pub tanks: Vec<Tank>,
    pub turn: usize,
    pub phase: Phase,
    pub round: u32,
    pub shells: Vec<Shell>,
    pub blasts: Vec<Blast>,
    pub smoke: Vec<Smoke>,
    pub stars: Vec<Star>,
    /// Ticks since the game began, for anything that moves on its own.
    pub clock: u64,
    pub message: String,
    pub round_winner: Option<usize>,
    pub accessories: Rc<Accessories>,
    /// The scoring in play, scaled to the starting cash.
    pub scoring: Scoring,
    pub interest_rate: f64,
    /// This round's turn order, tank indices.
    pub order: Vec<usize>,
    pub shop: crate::kit::ShopState,
    pub(crate) rng: StdRng,
}

impl Game {
    pub fn new(cfg: Config, arsenal: Arsenal, accessories: Accessories, scoring: Scoring) -> Game {
        let mut rng = StdRng::seed_from_u64(cfg.seed);
        let arsenal = Rc::new(arsenal);
        let start = arsenal.default_weapon();
        let (nw, na) = (arsenal.weapons.len(), accessories.items.len());
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
                weapon: start,
                last_trail: Vec::new(),
                killed_by: None,
                fall_from: None,
                money: cfg.cash,
                stock: vec![0; nw],
                kit: vec![0; na],
                shield: None,
                fuel: 0,
                triggers: false,
                suicides: 0,
                ready: false,
            })
            .collect();
        let generator = cfg.generator.unwrap_or_else(|| Generator::random(&mut rng));
        let mut g = Game {
            land: Land::empty(),
            generator,
            walls: cfg.walls,
            wind: 0.0,
            tanks,
            turn: 0,
            phase: Phase::Aim,
            round: 0,
            shells: Vec::new(),
            blasts: Vec::new(),
            smoke: Vec::new(),
            stars: Vec::new(),
            clock: 0,
            message: String::new(),
            round_winner: None,
            scoring: scoring.scaled(cfg.cash),
            interest_rate: cfg.interest,
            order: Vec::new(),
            shop: Default::default(),
            accessories: Rc::new(accessories),
            rng,
            arsenal,
            cfg,
        };
        g.begin_round();
        g
    }

    /// Shop first, unless the arsenal is free.
    fn begin_round(&mut self) {
        if self.cfg.free {
            self.start_round();
        } else {
            self.begin_shopping();
        }
    }

    pub fn current(&self) -> &Tank {
        &self.tanks[self.turn]
    }

    pub fn weapon(&self, i: usize) -> &Weapon {
        &self.arsenal.weapons[i]
    }

    pub(crate) fn start_round(&mut self) {
        self.round += 1;
        self.generator = self.cfg.generator.unwrap_or_else(|| Generator::random(&mut self.rng));
        self.walls = self.cfg.walls.resolve(&mut self.rng);
        self.wind = physics::initial_wind(self.cfg.max_wind, &mut self.rng);
        self.land = Land::generate(self.generator, self.cfg.bumpiness, &mut self.rng);
        self.land.wrap = self.walls == Walls::Wrap;
        self.shells.clear();
        self.blasts.clear();
        self.smoke.clear();
        self.stars = (0..STARS)
            .map(|_| {
                let r = &mut self.rng;
                Star {
                    x: r.random_range(0.0..land::WIDTH as f64),
                    // Thinner towards the horizon.
                    y: land::HEIGHT as f64 * (1.0 - r.random::<f64>().powi(2) * 0.85),
                    bright: 0.25 + 0.75 * r.random::<f64>().powi(3),
                    tint: match r.random_range(0..10) {
                        0 => 1,
                        1 => 2,
                        _ => 0,
                    },
                    twinkle: if r.random_range(0..4) == 0 { r.random_range(0.03..0.09) } else { 0.0 },
                    phase: r.random_range(0.0..std::f64::consts::TAU),
                }
            })
            .collect();
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
            // Shields don't carry over; triggers start off (`sc_player_init_round`).
            t.shield = None;
            t.triggers = false;
            t.ready = false;
            let w = &self.arsenal.weapons[t.weapon];
            if !self.cfg.free && !w.infinite() && t.stock[t.weapon] == 0 {
                t.weapon = self.arsenal.default_weapon();
            }
        }
        self.order_round();
        self.turn = self.order[0];
        self.phase = Phase::Aim;
        self.message = format!(
            "Round {} of {}: {}, {} walls",
            self.round,
            self.cfg.rounds,
            self.generator.name(),
            self.walls.name()
        );
        info!("round {} start, terrain {:?}, walls {:?}, wind {:.3}", self.round, self.generator, self.walls, self.wind);
    }

    /// Start the next round, or a fresh game after the last.
    pub fn advance(&mut self) {
        match self.phase {
            Phase::RoundOver => {
                self.pay_interest();
                self.begin_round();
            }
            Phase::GameOver => {
                let (cash, nw, na) = (self.cfg.cash, self.arsenal.weapons.len(), self.accessories.items.len());
                for t in &mut self.tanks {
                    t.wins = 0;
                    t.kills = 0;
                    t.suicides = 0;
                    t.money = cash;
                    t.stock = vec![0; nw];
                    t.kit = vec![0; na];
                    t.fuel = 0;
                }
                self.interest_rate = self.cfg.interest;
                self.round = 0;
                self.begin_round();
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

    /// Step through the weapons a player can pick.
    pub fn cycle_weapon(&mut self, delta: i32) {
        if self.phase != Phase::Aim {
            return;
        }
        let list: Vec<usize> = self
            .arsenal
            .selectable()
            .into_iter()
            .filter(|&w| self.cfg.free || self.owns(self.turn, w))
            .collect();
        let t = &mut self.tanks[self.turn];
        let at = list.iter().position(|&w| w == t.weapon).unwrap_or(0) as i32;
        t.weapon = list[(at + delta).rem_euclid(list.len() as i32) as usize];
    }

    pub fn fire(&mut self) {
        if self.phase != Phase::Aim {
            return;
        }
        self.message.clear();
        if self.cfg.simultaneous {
            // Orders now; everyone fires once all have given theirs.
            self.tanks[self.turn].ready = true;
            if let Some(next) = self.next_alive(|t| !t.ready) {
                self.turn = next;
                self.start_turn();
                return;
            }
            let shooters: Vec<usize> = self.order.iter().copied().filter(|&i| self.tanks[i].alive).collect();
            for i in shooters {
                self.launch(i);
                self.tanks[i].ready = false;
            }
        } else {
            self.launch(self.turn);
        }
        self.phase = Phase::Action;
    }

    /// One tank's shot leaves the barrel.
    fn launch(&mut self, who: usize) {
        let t = &self.tanks[who];
        let wi = t.weapon;
        let ars = self.arsenal.clone();
        let w = &ars.weapons[wi];
        let (x, y) = t.muzzle();
        let a = (t.angle as f64).to_radians();
        let v = t.power as f64 * VELOCITY_SCALE;
        info!("{} fires {}: angle {} power {}", t.name, w.name, t.angle, t.power);
        let triggered = self.use_contact_trigger(who);
        self.use_weapon(who, wi);
        // Weapons that act as they leave the barrel: riot charges blow a
        // wedge in the turret's direction (`_sc_phoenix_detonate`).
        if w.phoenix.at_tank && w.phoenix.detonate {
            self.explode(wi, x, y, 1.0, who, Some(a));
            return;
        }
        let vel = (v * a.cos(), v * a.sin());
        // A Triple Turret fires three of a "triple" weapon, spread along
        // the line of fire (`_sc_weapon_fire_new`: scatter 3, three children).
        let shots = if w.state.triple && self.has(who, |a| a.triple_turret) { spread(vel, 3, 3.0) } else { vec![vel] };
        let middle = shots.len() / 2;
        for (k, vel) in shots.into_iter().enumerate() {
            let shell = self.new_shell(wi, who, (x, y), vel);
            self.shells.push(Shell { root: k == middle, triggered, ..shell });
        }
    }

    fn new_shell(&self, weapon: usize, owner: usize, at: (f64, f64), vel: (f64, f64)) -> Shell {
        let path = Path { ctr: at, vel, acc: (self.wind, -GRAVITY), drag: self.cfg.viscosity };
        Shell {
            weapon,
            owner,
            motion: Motion::Flight { path, t: 0.0 },
            x: at.0,
            y: at.1,
            clear: at,
            trail: vec![at],
            exp_res: 1.0,
            chain: 0,
            age: 0.0,
            delay: 0,
            root: false,
            triggered: false,
            tunnel: false,
        }
    }

    pub fn tick(&mut self) {
        self.clock += 1;
        match self.phase {
            Phase::Action => {
                self.tick_blasts();
                self.tick_shells();
                if self.shells.is_empty() && self.blasts.is_empty() {
                    self.phase = Phase::Settle;
                }
            }
            Phase::Settle => self.tick_settle(),
            Phase::Shop | Phase::Aim | Phase::RoundOver | Phase::GameOver => {}
        }
    }

    // ---- Shells -------------------------------------------------------

    fn tick_shells(&mut self) {
        let shells = std::mem::take(&mut self.shells);
        for mut s in shells {
            if s.delay > 0 {
                s.delay -= 1;
                self.shells.push(s);
                continue;
            }
            let alive = match s.motion {
                Motion::Flight { .. } => self.fly(&mut s),
                Motion::Roll { .. } => self.roll(&mut s),
                Motion::Dig { .. } => self.dig(&mut s),
            };
            if s.trail.last() != Some(&(s.x, s.y)) {
                s.trail.push((s.x, s.y));
            }
            if alive {
                self.shells.push(s);
            } else {
                self.retire(s);
            }
        }
    }

    /// A shell is done: keep the fired shell's path, and a smoke tracer's.
    fn retire(&mut self, s: Shell) {
        if self.arsenal.weapons[s.weapon].state.smoke {
            self.smoke.push(Smoke { owner: s.owner, points: s.trail.clone() });
        }
        if s.root {
            self.tanks[s.owner].last_trail = s.trail;
        }
    }

    /// Move a flying shell one tick. False when it's gone.
    ///
    /// The tick is cut into slices of a sixteenth of a step, as Xscorch
    /// steps (`SC_TRAJ_TIME_SUBSTEP`); magnetic shields push between
    /// slices. Within a slice the path is walked a unit at a time
    /// (`_sc_traj_traverse_path`), checking walls, shields, tanks and dirt.
    fn fly(&mut self, s: &mut Shell) -> bool {
        let Motion::Flight { mut path, mut t } = s.motion else { unreachable!() };
        let ars = self.arsenal.clone();
        let w = &ars.weapons[s.weapon];
        let going_up = s.age == 0.0 || path.at(t).1.1 >= 0.0;
        let (w_, h_) = (land::WIDTH as f64, land::HEIGHT as f64);
        let shield_r = (TANK_RADIUS + 1) as f64;
        let dt = STEPS_PER_TICK / SLICES as f64;
        let mut hit = None;
        'slices: for _ in 0..SLICES {
            let ((xe, ye), _) = path.at(t + dt);
            let n = (xe - s.x).abs().max((ye - s.y).abs()).ceil().clamp(1.0, 4000.0) as usize;
            // Local time on the current path; a bounce starts a new one.
            let (mut base_local, mut base_e) = (t, 0.0);
            for k in 1..=n {
                let e = dt * k as f64 / n as f64;
                let local = base_local + (e - base_e);
                let ((mut x, mut y), (vx, vy)) = path.at(local);
                let mut restart = |path: &mut Path, at: (f64, f64), vel: (f64, f64), acc: (f64, f64)| {
                    *path = Path { ctr: at, vel, acc, ..*path };
                    base_local = 0.0;
                    base_e = e;
                };
                let off_side = x < 0.0 || x >= w_;
                match self.walls {
                    Walls::Wrap if off_side => {
                        let shift = if x < 0.0 { w_ } else { -w_ };
                        path.ctr.0 += shift;
                        x += shift;
                    }
                    Walls::None | Walls::Random if off_side => {
                        if x < -BORDERS_EXTEND || x > w_ + BORDERS_EXTEND {
                            (s.x, s.y) = (x, y);
                            s.motion = Motion::Flight { path, t };
                            return false;
                        }
                    }
                    Walls::Concrete if off_side || y >= h_ => {
                        hit = Some((Hit::Wall, x.clamp(0.0, w_ - 1.0), y.min(h_ - 1.0)));
                        break 'slices;
                    }
                    walls => {
                        if let Some(el) = walls.elasticity()
                            && (off_side || y >= h_ || y < 0.0)
                        {
                            let (mut nvx, mut nvy) = (vx, vy);
                            if off_side {
                                x = x.clamp(0.0, w_ - 1.0);
                                nvx = -vx * el;
                            } else {
                                y = y.clamp(0.0, h_ - 1.0);
                                nvy = -vy * el;
                            }
                            let acc = path.acc;
                            restart(&mut path, (x, y), (nvx, nvy), acc);
                        }
                    }
                }
                if y < 0.0 {
                    hit = Some((Hit::Ground, x, 0.0));
                    break 'slices;
                }
                if !(0.0..w_).contains(&x) {
                    // Off the side with no walls: nothing to hit out there.
                    (s.x, s.y) = (x, y);
                    continue;
                }
                // Shields, crossed from outside to inside
                // (`sc_shield_would_impact`). Magnetic ones only stop a
                // sapper; a tank's own shield lets its shots out.
                if let Some(j) = self.shield_crossed(s, x, y, shield_r, w.state.sapper) {
                    let sh = self.tanks[j].shield.expect("crossed a shield");
                    let force = self.accessories.items[sh.acc].shield == Some(crate::economy::ShieldKind::Force);
                    if force && !w.state.sapper {
                        // Bounce off the curve (`sc_shield_get_reflection`).
                        let (tx, ty) = (self.tanks[j].x as f64, self.tanks[j].y as f64);
                        let (dx, dy) = (self.land.dx(tx, x), y - ty);
                        let d2 = (dx * dx + dy * dy).max(1e-6);
                        let radial = (dx * vx + dy * vy) / d2;
                        let vel = (vx - 2.0 * radial * dx, vy - 2.0 * radial * dy);
                        let d = d2.sqrt();
                        let at = (tx + dx / d * (shield_r + 0.1), ty + dy / d * (shield_r + 0.1));
                        self.absorb_hit(j, false);
                        let acc = path.acc;
                        restart(&mut path, at, vel, acc);
                        (s.x, s.y) = at;
                        continue;
                    }
                    if w.state.liquid {
                        // Liquids burst against it (`_sc_weapon_track`).
                        hit = Some((Hit::Shield, x, y));
                        break 'slices;
                    }
                    self.absorb_hit(j, w.state.sapper);
                    (s.x, s.y) = (x, y);
                    s.motion = Motion::Flight { path, t };
                    return false;
                }
                let (ix, iy) = (x.round() as i32, y.round() as i32);
                if self.tanks.iter().any(|tk| tk.contains(&self.land, x, y)) {
                    hit = Some((Hit::Tank, x, y));
                    break 'slices;
                }
                if self.land.get(ix, iy) {
                    // Tunnelling (`_sc_traj_hit_land`): slowed by the dirt,
                    // clearing it, until too slow; a contact trigger stops it.
                    let slowed = (vx * TUNNEL_DAMPING, vy * TUNNEL_DAMPING);
                    if self.cfg.tunneling && !s.triggered && slowed.0.hypot(slowed.1) >= TUNNEL_MIN_VEL {
                        self.land.set(ix, iy, false);
                        restart(&mut path, (x, y), slowed, (0.0, 0.0));
                        s.tunnel = true;
                        (s.x, s.y) = (x, y);
                        continue;
                    }
                    hit = Some((Hit::Land, x, y));
                    break 'slices;
                }
                if s.tunnel {
                    // Out in the air again: wind and gravity come back.
                    s.tunnel = false;
                    restart(&mut path, (x, y), (vx, vy), (self.wind, -GRAVITY));
                }
                s.clear = (s.x, s.y);
                (s.x, s.y) = (x, y);
            }
            t = base_local + (dt - base_e);
            // Magnetic shields push between slices (`sc_shield_get_deflection`).
            let v = path.at(t).1;
            if let Some(nv) = self.deflect(s.owner, s.x, s.y, v) {
                path = Path { ctr: (s.x, s.y), vel: nv, ..path };
                t = 0.0;
            }
        }
        s.age += STEPS_PER_TICK;
        s.motion = Motion::Flight { path, t };

        if let Some((kind, x, y)) = hit {
            (s.x, s.y) = (x, y);
            return self.impact(s, kind);
        }

        // At the top of its arc (`_sc_weapon_track`).
        if w.phoenix.at_apex && going_up && path.at(t).1.1 <= 0.0 {
            return self.phoenix_split(s);
        }
        // Somewhere along the way, maybe (Black Market MIRVs).
        if w.phoenix.at_rand && self.rng.random::<f64>() < 1.0 - (1.0 - PROB_AT_RAND).powf(STEPS_PER_TICK) {
            return self.phoenix_split(s);
        }
        if s.age >= TIMEOUT {
            return false;
        }
        true
    }

    /// A shell came down. False when it's gone.
    fn impact(&mut self, s: &mut Shell, hit: Hit) -> bool {
        let ars = self.arsenal.clone();
        let w = &ars.weapons[s.weapon];
        if w.phoenix.at_land && w.splits() {
            if w.phoenix.convert && w.state.roller {
                if matches!(hit, Hit::Tank) {
                    // A roller that lands on a tank goes off there.
                } else {
                    self.start_roll(s);
                    return true;
                }
            }
            if w.phoenix.convert && w.state.digger && matches!(hit, Hit::Land) {
                self.start_dig(s);
                return true;
            }
            if w.phoenix.chain && s.chain < w.children {
                self.leap(s);
                return false;
            }
            if w.phoenix.spider {
                self.spider(s);
                return false;
            }
            if w.phoenix.scatter {
                return self.phoenix_split(s);
            }
        }
        let (x, y, owner, exp_res, wi) = (s.x, s.y, s.owner, s.exp_res, s.weapon);
        self.explode(wi, x, y, exp_res, owner, None);
        false
    }

    /// Break into children (`_sc_phoenix_scatter`): spread along the line
    /// of flight, the outer ones fastest apart, the middle one keeping the
    /// parent's speed. The parent just fizzles.
    fn phoenix_split(&mut self, s: &mut Shell) -> bool {
        let ars = self.arsenal.clone();
        let w = &ars.weapons[s.weapon];
        let (Some(child), n) = (w.child, w.children) else { return true };
        if !w.phoenix.scatter || n == 0 {
            return true;
        }
        for vel in spread(self.velocity(s), n, w.scatter) {
            let c = self.new_shell(child, s.owner, (s.x, s.y), vel);
            self.shells.push(Shell { triggered: s.triggered, ..c });
        }
        false
    }

    fn velocity(&self, s: &Shell) -> (f64, f64) {
        match s.motion {
            Motion::Flight { path, t } => path.at(t).1,
            Motion::Roll { vx } => (vx, 0.0),
            Motion::Dig { .. } => (0.0, 0.0),
        }
    }

    /// Leap Frog (`_sc_phoenix_chain`): the warhead goes off with its
    /// child's blast, scaled down, and a smaller frog bounces on.
    fn leap(&mut self, s: &mut Shell) {
        let ars = self.arsenal.clone();
        let w = &ars.weapons[s.weapon];
        let child = &ars.weapons[w.child.expect("splits")];
        let exp_res = s.exp_res * INCREASE_FACTOR;
        let (vx, vy) = self.velocity(s);
        let (lo, hi) = (VELOCITY_SCALE * MIN_FROG_POWER, VELOCITY_SCALE * MAX_FROG_POWER);
        let dx = (vx.abs() * exp_res).clamp(lo, hi) * if vx < 0.0 { -1.0 } else { 1.0 };
        let dy = (vy.abs() * exp_res).clamp(lo, hi);
        let y = s.y.max(1.0);
        let mut frog = self.new_shell(s.weapon, s.owner, (s.x, y), (dx, dy));
        frog.exp_res = exp_res;
        frog.chain = s.chain + 1;
        let mut b = self.blast_for(w, s.x, y, exp_res, s.owner, None);
        b.radius = exp_res * child.radius;
        b.force = child.force;
        b.grow = (6.0 + b.radius / 10.0).min(30.0) as u32;
        // Wait for the crater before leaping out of it.
        frog.delay = b.grow + 1;
        self.add_blast(b);
        self.shells.push(frog);
    }

    /// Funky Bomb (`_sc_phoenix_spider`): the bomb goes off, and legs
    /// arc out to land on the ground around it, each its own blast.
    fn spider(&mut self, s: &mut Shell) {
        let ars = self.arsenal.clone();
        let w = &ars.weapons[s.weapon];
        let child = &ars.weapons[w.child.expect("splits")];
        let count = w.children as usize;
        let legs = count / 2 + if count / 2 > 0 { self.rng.random_range(0..count / 2) } else { 0 };
        let scatter = (w.scatter * s.exp_res * w.radius * SPIDER_POWER).max(1.0);
        let (x, y) = (s.x, s.y);
        let main = self.blast_for(w, x, y, s.exp_res, s.owner, None);
        let start_delay = main.grow;
        self.add_blast(main);
        let srcy = (self.land.top(x.round() as i32) as f64).max(y);
        for i in 0..legs {
            for _ in 0..SPIDER_MAX_LEG_ATTEMPTS {
                let cx = x - scatter + self.rng.random_range(0.0..2.0 * scatter);
                let cy = self.land.top(cx.round() as i32) as f64;
                if cy <= y + SPIDER_ARC {
                    let kind = if child.state.plasma { BlastKind::Plasma } else { BlastKind::Normal };
                    let mut b = Blast::new(kind, cx, cy, s.exp_res * child.radius, child.force, s.owner);
                    b.delay = start_delay + 3 * i as u32;
                    b.arc = arc((x, srcy), (cx, cy), SPIDER_ARC);
                    self.add_blast(b);
                    break;
                }
            }
        }
    }

    /// Roller lands (`sc_trajectory_convert`): it rolls from the last
    /// clear point. Torched's choice: it heads downhill, whichever way it
    /// was flying, as the weapons' descriptions say.
    fn start_roll(&mut self, s: &mut Shell) {
        let (vx, _) = self.velocity(s);
        (s.x, s.y) = (s.clear.0.round(), s.clear.1.round().max(0.0));
        let (x, y) = (s.x as i32, s.y as i32);
        let below = |xx: i32| self.height_below(xx, y);
        let (l, r) = (below(x - 3), below(x + 3));
        let dir = if l < r {
            -1.0
        } else if r < l {
            1.0
        } else if vx < 0.0 {
            -1.0
        } else {
            1.0
        };
        s.motion = Motion::Roll { vx: dir * vx.abs().max(ROLLER_MIN_SPEED) };
        s.age = 0.0;
    }

    /// Top of the ground at or below height y in column x.
    fn height_below(&self, x: i32, y: i32) -> i32 {
        (0..=y.min(land::HEIGHT as i32 - 1)).rev().find(|&yy| self.land.get(x, yy)).map_or(0, |yy| yy + 1)
    }

    /// A rolling shell (`_sc_traj_traverse_roller`): along the ground,
    /// falling where it can and gaining speed, climbing small steps at a
    /// cost, going off when it can't go on.
    fn roll(&mut self, s: &mut Shell) -> bool {
        let Motion::Roll { mut vx } = s.motion else { unreachable!() };
        let mut distance = vx.abs().max(ROLLER_MIN_SPEED) * ROLLER_PACE * STEPS_PER_TICK;
        let mut done = false;
        while distance > 0.0 && !done {
            distance -= 1.0;
            let dir = if vx < 0.0 { -1 } else { 1 };
            let (x, y) = (s.x as i32, s.y as i32);
            let mut nx = x + dir;
            if nx < 0 || nx >= land::WIDTH as i32 {
                match self.walls {
                    Walls::Wrap => nx = nx.rem_euclid(land::WIDTH as i32),
                    w if w.elasticity().is_some() => {
                        vx = -vx * w.elasticity().unwrap_or(1.0);
                        continue;
                    }
                    Walls::Concrete => {
                        done = true;
                        continue;
                    }
                    _ => {
                        s.motion = Motion::Roll { vx };
                        return false;
                    }
                }
            }
            let blocked = |g: &Game, xx: i32, yy: i32| g.land.get(xx, yy);
            let tank_at = |g: &Game, xx: i32, yy: i32| g.tanks.iter().any(|t| t.contains(&g.land, xx as f64, yy as f64));
            if tank_at(self, nx, y) {
                s.x = nx as f64;
                done = true;
                continue;
            }
            if !blocked(self, nx, y) {
                s.x = nx as f64;
                if y > 0 && !blocked(self, nx, y - 1) {
                    s.y -= 1.0;
                    vx += ROLLER_FALL * dir as f64;
                }
                continue;
            }
            // Something in the way: climb a step if there's speed for it.
            let climb = |g: &Game, up: i32| !blocked(g, nx, y + up) && !tank_at(g, nx, y + up);
            if vx.abs() > ROLLER_GAIN && climb(self, 1) {
                s.x = nx as f64;
                s.y += 1.0;
                vx -= ROLLER_GAIN * dir as f64;
            } else if vx.abs() > 2.0 * ROLLER_GAIN && climb(self, 2) {
                s.x = nx as f64;
                s.y += 2.0;
                vx -= 2.0 * ROLLER_GAIN * dir as f64;
            } else {
                done = true;
            }
        }
        s.motion = Motion::Roll { vx };
        s.age += STEPS_PER_TICK;
        if done || s.age >= TIMEOUT {
            let (x, y, owner, exp_res, wi) = (s.x, s.y, s.owner, s.exp_res, s.weapon);
            self.explode(wi, x, y, exp_res, owner, None);
            return false;
        }
        true
    }

    /// Digger lands. Xscorch's digger code was never finished ("probably
    /// broken"); this is Torched's: it burrows on the way it was going,
    /// down or sideways.
    fn start_dig(&mut self, s: &mut Shell) {
        let (vx, vy) = self.velocity(s);
        let dir = if vx.abs() > vy.abs() * 1.5 { (if vx < 0.0 { -1 } else { 1 }, 0) } else { (0, -1) };
        let w = &self.arsenal.weapons[s.weapon];
        s.motion = Motion::Dig { dir, left: w.dig_length.max(1.0) };
    }

    /// A burrowing shell: clears a tunnel, turns now and then
    /// (`SC_TRAJ_DIGGER_PROB`), stops when it runs out of dirt or length
    /// or meets a tank. Sandhogs then detonate; diggers just stop.
    fn dig(&mut self, s: &mut Shell) -> bool {
        let Motion::Dig { mut dir, mut left } = s.motion else { unreachable!() };
        let ars = self.arsenal.clone();
        let w = &ars.weapons[s.weapon];
        let r = w.tunnel_radius.max(1.0);
        let mut done = false;
        for _ in 0..DIG_PER_TICK {
            let ahead = |g: &Game, d: (i32, i32)| {
                let (ax, ay) = (s.x + d.0 as f64 * (r + 1.0), s.y + d.1 as f64 * (r + 1.0));
                g.land.get(ax.round() as i32, ay.round() as i32)
            };
            let tank_ahead = |g: &Game, d: (i32, i32)| {
                (1..=(r as i32 + 1)).any(|k| {
                    let (ax, ay) = (s.x + (d.0 * k) as f64, s.y + (d.1 * k) as f64);
                    g.tanks.iter().any(|t| t.contains(&g.land, ax, ay))
                })
            };
            if tank_ahead(self, dir) {
                done = true;
                break;
            }
            let turns: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];
            if self.rng.random::<f64>() < DIGGER_TURN_PROB || !ahead(self, dir) {
                let start = self.rng.random_range(0..4);
                let next = (0..4).map(|k| turns[(start + k) % 4]).find(|&d| d != (-dir.0, -dir.1) && ahead(self, d));
                match next {
                    Some(d) => dir = d,
                    None if ahead(self, dir) => {}
                    None => {
                        done = true;
                        break;
                    }
                }
            }
            let (nx, ny) = (s.x + dir.0 as f64, s.y + dir.1 as f64);
            if ny < 0.0 || (!self.land.wrap && !(0.0..land::WIDTH as f64).contains(&nx)) {
                done = true;
                break;
            }
            s.x = if self.land.wrap { nx.rem_euclid(land::WIDTH as f64) } else { nx };
            s.y = ny;
            self.land.carve(s.x, s.y, r);
            left -= 1.0;
            if left <= 0.0 {
                done = true;
                break;
            }
        }
        s.motion = Motion::Dig { dir, left };
        if done {
            let (x, y, owner, exp_res, wi) = (s.x, s.y, s.owner, s.exp_res, s.weapon);
            self.explode(wi, x, y, exp_res, owner, None);
            return false;
        }
        true
    }

    // ---- Blasts -------------------------------------------------------

    /// A weapon's warhead going off (`sc_weapon_get_explosion`).
    fn explode(&mut self, weapon: usize, x: f64, y: f64, exp_res: f64, owner: usize, dir: Option<f64>) {
        let ars = self.arsenal.clone();
        let w = &ars.weapons[weapon];
        if !w.explodes() {
            return;
        }
        let b = self.blast_for(w, x, y, exp_res, owner, dir);
        self.add_blast(b);
    }

    fn blast_for(&mut self, w: &Weapon, x: f64, y: f64, exp_res: f64, owner: usize, dir: Option<f64>) -> Blast {
        let kind = if w.state.plasma {
            BlastKind::Plasma
        } else if w.is_napalm() {
            BlastKind::Napalm
        } else if w.state.liquid {
            BlastKind::LiquidDirt
        } else if w.state.dirt {
            BlastKind::Dirt
        } else if w.state.riot {
            BlastKind::Riot
        } else {
            BlastKind::Normal
        };
        let mut b = Blast::new(kind, x, y, exp_res * w.radius, w.force, owner);
        if w.angular_width != 0.0 {
            b.wedge = Some((dir.unwrap_or(std::f64::consts::FRAC_PI_2), w.angular_width.to_radians()));
        }
        if b.liquid() {
            b.spill = self.land.spill(x.round() as i32, y.round() as i32, w.liquid, &mut self.rng);
        }
        b
    }

    fn add_blast(&mut self, b: Blast) {
        info!("blast {:?} at ({:.0}, {:.0}) r {:.0} f {:.0}", b.kind, b.x, b.y, b.radius, b.force);
        self.blasts.push(b);
    }

    fn tick_blasts(&mut self) {
        let mut blasts = std::mem::take(&mut self.blasts);
        for b in &mut blasts {
            if b.delay > 0 {
                b.delay -= 1;
                continue;
            }
            b.age += 1;
            if b.kind == BlastKind::LiquidDirt {
                let from = ((b.age - 1) as usize * LIQUID_STEP).min(b.spill.len());
                let to = (b.age as usize * LIQUID_STEP).min(b.spill.len());
                self.land.add_dirt(&b.spill[from..to]);
            }
            if b.age == b.peak() {
                self.detonate(b);
            }
        }
        blasts.retain(|b| b.delay > 0 || b.age < b.lifetime());
        // Blasts added while these ran (deaths) go after them.
        blasts.append(&mut self.blasts);
        self.blasts = blasts;
    }

    /// A blast at full size: its mark on the ground, and damage.
    fn detonate(&mut self, b: &Blast) {
        match b.kind {
            BlastKind::Normal | BlastKind::Plasma => self.land.carve(b.x, b.y, b.radius),
            BlastKind::Riot => self.land.disc(b.x, b.y, b.radius, b.wedge, false),
            BlastKind::Dirt => self.land.fill(b.x, b.y, b.radius),
            BlastKind::Napalm | BlastKind::LiquidDirt => {}
        }
        let mut hits = Vec::new();
        for i in 0..self.tanks.len() {
            let t = &self.tanks[i];
            if !t.alive {
                continue;
            }
            let raw = self.damage_at(b, t.x as f64, t.y as f64);
            if raw <= 0 {
                continue;
            }
            // Shields take it first (`sc_shield_absorb_explosion`).
            let damage = self.absorb_damage(i, raw, b.kind == BlastKind::Napalm);
            if damage <= 0 {
                hits.push(format!("{} shield -{raw}", self.tanks[i].name));
                continue;
            }
            let t = &mut self.tanks[i];
            t.life -= damage;
            t.killed_by = Some(b.owner);
            hits.push(format!("{} -{}", t.name, damage));
            info!("{:?} by {} hits {} for {damage}", b.kind, b.owner, t.name);
            // Money changes hands on every hit (`_sc_player_damage`).
            let (loss, bonus) = (self.scoring.damage_loss, self.scoring.damage_bonus);
            self.tanks[i].money -= loss;
            if b.owner != i {
                self.tanks[b.owner].money += bonus;
            }
            if self.tanks[i].life > 0 {
                self.auto_defense(i);
            }
        }
        if !hits.is_empty() {
            say(&mut self.message, &hits.join(", "));
        }
        self.queue_deaths();
    }

    /// `sc_expl_damage_at_point`, at a tank's base point.
    fn damage_at(&self, b: &Blast, x: f64, y: f64) -> i32 {
        let r2 = b.radius * b.radius;
        if r2 <= 0.0 {
            return 0;
        }
        let at = |cx: f64, cy: f64| {
            let (dx, dy) = (self.land.dx(cx, x), y - cy);
            (b.force * (1.0 - (dx * dx + dy * dy) / r2)).round().max(0.0) as i32
        };
        match b.kind {
            BlastKind::Normal | BlastKind::Plasma => at(b.x, b.y),
            // Burning napalm hurts from every cell near the tank.
            BlastKind::Napalm => b.spill.iter().map(|&(cx, cy)| at(cx as f64, cy as f64)).sum(),
            BlastKind::Riot | BlastKind::Dirt | BlastKind::LiquidDirt => 0,
        }
    }

    /// Tanks out of life explode: a small blast, a medium one half the
    /// time, a large one a tenth of those; or one time in ten, napalm
    /// (`sc_player_death`).
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
            self.tanks[i].shield = None;
            // `sc_player_died`, and the kill or suicide.
            self.tanks[i].money -= self.scoring.death_loss;
            if owner != i {
                self.tanks[owner].kills += 1;
                self.tanks[owner].money += self.scoring.kill_bonus;
            } else {
                self.tanks[i].suicides += 1;
                self.tanks[i].money -= self.scoring.suicide_loss;
            }
            let killer = &self.tanks[owner].name;
            let line = if owner == i { format!("{name} destroyed itself!") } else { format!("{killer} destroyed {name}!") };
            say(&mut self.message, &line);
            info!("{name} destroyed by {killer}");
            if self.rng.random_range(0..10) == 0 {
                let (r, f, liquid) = DEATH_NAPALM;
                let mut b = Blast::new(BlastKind::Napalm, x, y + 1.0, r, f, owner);
                b.spill = self.land.spill(x as i32, y as i32 + 1, liquid, &mut self.rng);
                self.add_blast(b);
                continue;
            }
            let mut stages = vec![(SMALL_EXPLOSION, BlastKind::Normal)];
            if self.rng.random::<f64>() < 0.5 {
                stages.push((MEDIUM_EXPLOSION, BlastKind::Normal));
                if self.rng.random::<f64>() < 0.1 {
                    stages.push((LARGE_EXPLOSION, BlastKind::Plasma));
                }
            }
            let mut delay = 0;
            for ((radius, force), kind) in stages {
                let mut b = Blast::new(kind, x, y, radius, force, owner);
                b.delay = delay;
                delay += b.grow;
                self.add_blast(b);
            }
        }
        if any && self.phase == Phase::Settle {
            self.phase = Phase::Action;
        }
        any
    }

    // ---- Settling and turns ----------------------------------------------

    fn tick_settle(&mut self) {
        let dirt = self.land.drop_cycle(DIRT_DROP_PER_TICK);
        let mut falling = false;
        for i in 0..self.tanks.len() {
            if !self.tanks[i].alive {
                continue;
            }
            let chute = self.accessories.items.iter().position(|a| a.parachute);
            let t = &mut self.tanks[i];
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
                // A parachute (Torched's own) saves the tank the fall.
                if damage > 0
                    && let Some(c) = chute
                    && t.kit[c] > 0
                {
                    t.kit[c] -= 1;
                    say(&mut self.message, &format!("{}'s parachute opens", t.name));
                    continue;
                }
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
                // `sc_player_inc_wins`.
                self.tanks[w].money += self.scoring.survival_bonus;
            }
            self.message = match self.round_winner {
                Some(w) => format!("{} wins round {}", self.tanks[w].name, self.round),
                None => format!("Nobody survives round {}", self.round),
            };
            info!("{}", self.message);
            self.phase = if self.round >= self.cfg.rounds { Phase::GameOver } else { Phase::RoundOver };
            return;
        }
        let next = if self.cfg.simultaneous {
            for t in &mut self.tanks {
                t.ready = false;
            }
            self.next_alive_from(0, |_| true)
        } else {
            self.next_alive(|_| true)
        };
        self.turn = next.expect("two tanks are alive");
        if self.cfg.dynamic_wind {
            self.wind = physics::update_wind(self.wind, self.cfg.max_wind, &mut self.rng);
        }
        self.start_turn();
        self.phase = Phase::Aim;
    }

    /// The next living tank after the current one in this round's order
    /// that passes `want`.
    pub(crate) fn next_alive(&self, want: impl Fn(&Tank) -> bool) -> Option<usize> {
        let pos = self.order.iter().position(|&i| i == self.turn).unwrap_or(0);
        self.next_alive_from(pos + 1, want)
    }

    fn next_alive_from(&self, pos: usize, want: impl Fn(&Tank) -> bool) -> Option<usize> {
        let n = self.order.len();
        (0..n).map(|k| self.order[(pos + k) % n]).find(|&i| self.tanks[i].alive && want(&self.tanks[i]))
    }

    /// A tank's turn begins (`sc_player_init_turn`): power capped by life,
    /// a solar panel tops up its shield.
    pub(crate) fn start_turn(&mut self) {
        let i = self.turn;
        self.solar(i);
        let t = &mut self.tanks[i];
        t.power = t.power.min(t.life);
    }

    /// Standings: indices by wins, then money.
    pub fn standings(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.tanks.len()).collect();
        order.sort_by_key(|&i| (std::cmp::Reverse(self.tanks[i].wins), std::cmp::Reverse(self.tanks[i].money)));
        order
    }
}

/// Velocities for n children spread along the line of flight
/// (`_sc_phoenix_scatter`): outer ones fastest apart, the middle one, if
/// any, keeping the parent's.
fn spread((mut vx, mut vy): (f64, f64), n: u32, scatter: f64) -> Vec<(f64, f64)> {
    let (mut xs, mut ys) = (0.0, 0.0);
    if n > 1 {
        let speed = vx.hypot(vy);
        if speed == 0.0 {
            xs = scatter;
        } else {
            xs = scatter * vx / speed;
            ys = scatter * vy / speed;
        }
        vx -= 0.5 * xs;
        vy -= 0.5 * ys;
        xs /= n as f64;
        ys /= n as f64;
    }
    (0..n)
        .map(|_| {
            vx += xs;
            vy += ys;
            (vx, vy)
        })
        .collect()
}

/// A parabola from a to b rising `height` above a, for spider legs.
fn arc(a: (f64, f64), b: (f64, f64), height: f64) -> Vec<(f64, f64)> {
    let top = a.1.max(b.1) + height;
    let n = 24;
    (0..=n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let x = a.0 + (b.0 - a.0) * f;
            // Quadratic through a, the peak and b.
            let base = a.1 + (b.1 - a.1) * f;
            (x, base + 4.0 * f * (1.0 - f) * (top - (a.1 + b.1) / 2.0))
        })
        .collect()
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
pub(crate) mod tests {
    use super::*;
    use crate::weapons::DEFAULT_DEF;

    pub(crate) fn game_with(players: usize, f: impl FnOnce(&mut Config)) -> Game {
        let mut cfg = Config {
            players,
            rounds: 3,
            generator: Some(Generator::Plains),
            seed: 1,
            max_wind: 0.0,
            walls: Walls::Concrete,
            free: true,
            ..Config::default()
        };
        f(&mut cfg);
        new_game(cfg)
    }

    pub(crate) fn new_game(cfg: Config) -> Game {
        let acc = Accessories::parse(crate::economy::DEFAULT_ACCESSORIES).unwrap();
        let scorings = Scoring::parse_all(crate::economy::DEFAULT_SCORINGS).unwrap();
        let scoring = scorings.into_iter().find(|s| s.name == cfg.scoring).unwrap();
        Game::new(cfg, Arsenal::parse(DEFAULT_DEF).unwrap(), acc, scoring)
    }

    fn game(players: usize) -> Game {
        game_with(players, |_| {})
    }

    pub(crate) fn run_turn(g: &mut Game) {
        g.fire();
        for _ in 0..200_000 {
            g.tick();
            if g.phase != Phase::Action && g.phase != Phase::Settle {
                return;
            }
        }
        panic!("turn never ended");
    }

    pub(crate) fn select(g: &mut Game, name: &str) {
        let me = g.turn;
        g.tanks[me].weapon = g.arsenal.find(name).unwrap();
    }

    /// Fire at a high lob on flat ground so it comes down in the middle.
    pub(crate) fn lob(g: &mut Game, name: &str) {
        g.land = Land::empty();
        g.land.level(400, 400, 100);
        for t in &mut g.tanks {
            t.y = 100;
        }
        let me = g.turn;
        g.tanks[me].x = 200;
        g.tanks[me].angle = 60;
        g.tanks[me].power = 500;
        for (i, t) in g.tanks.iter_mut().enumerate() {
            if i != me {
                t.x = 760;
            }
        }
        select(g, name);
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
        while !g.shells.is_empty() {
            g.tick();
            if let Some(s) = g.shells.first() {
                top = top.max(s.y);
            }
        }
        let v = 500.0 * VELOCITY_SCALE;
        let expect = start + v * v / (2.0 * GRAVITY);
        assert!((top - expect).abs() < 1.5, "apex {top}, expected {expect}");
    }

    #[test]
    fn wind_pushes_a_shot() {
        let land_x = |wind: f64| {
            let mut g = game_with(2, |c| {
                c.max_wind = 0.05;
                c.dynamic_wind = false;
            });
            g.wind = wind;
            lob(&mut g, "Tracer");
            let me = g.turn;
            g.tanks[me].power = 600;
            run_turn(&mut g);
            g.tanks[me].last_trail.last().unwrap().0
        };
        assert!(land_x(0.05) > land_x(0.0) + 50.0);
        assert!(land_x(-0.05) < land_x(0.0) - 50.0);
    }

    #[test]
    fn mirv_splits_at_the_apex_into_five() {
        let mut g = game(2);
        lob(&mut g, "MIRV");
        g.fire();
        let mut most = 0;
        while g.phase == Phase::Action {
            g.tick();
            most = most.max(g.shells.len());
        }
        assert_eq!(most, 5);
    }

    #[test]
    fn rubber_walls_bounce_and_concrete_stops() {
        for (walls, expect_inside) in [(Walls::Rubber, true), (Walls::Concrete, true), (Walls::Wrap, true)] {
            let mut g = game_with(2, |c| c.walls = walls);
            lob(&mut g, "Tracer");
            let me = g.turn;
            g.tanks[me].angle = 20;
            g.tanks[me].power = 900;
            run_turn(&mut g);
            let end = g.tanks[me].last_trail.last().copied().unwrap();
            assert_eq!((0.0..800.0).contains(&end.0), expect_inside, "{walls:?} ended at {end:?}");
            if walls == Walls::Rubber {
                // It came back off the right wall.
                assert!(g.tanks[me].last_trail.iter().any(|p| p.0 > 790.0));
                assert!(end.0 < 790.0);
            }
        }
    }

    #[test]
    fn napalm_floods_and_burns() {
        let mut g = game(2);
        lob(&mut g, "Napalm");
        g.fire();
        let mut wet = 0;
        while g.phase == Phase::Action {
            g.tick();
            wet = wet.max(g.blasts.iter().map(|b| b.spill.len()).max().unwrap_or(0));
        }
        assert_eq!(wet, 256);
    }

    #[test]
    fn dirt_adds_ground_and_riot_takes_it_away() {
        let mut g = game(2);
        lob(&mut g, "Dirt Ball");
        let before: i32 = (0..800).map(|x| g.land.top(x)).sum();
        run_turn(&mut g);
        let after: i32 = (0..800).map(|x| g.land.top(x)).sum();
        assert!(after > before + 1000, "dirt ball added {}", after - before);

        let mut g = game(2);
        lob(&mut g, "Riot Bomb");
        run_turn(&mut g);
        let after: i32 = (0..800).map(|x| g.land.top(x)).sum();
        assert!(after < before - 500);
        assert!(g.tanks.iter().all(|t| t.life == MAX_LIFE), "riot bombs don't hurt");
    }

    #[test]
    fn riot_charge_clears_a_wedge_in_front() {
        let mut g = game(2);
        lob(&mut g, "Riot Charge");
        let me = g.turn;
        g.tanks[me].angle = 0;
        // A low bank of dirt to the right of the tank, low enough that
        // nothing above the wedge falls back in.
        let x0 = g.tanks[me].x;
        for x in x0 + 15..x0 + 60 {
            for y in 100..112 {
                g.land.set(x, y, true);
            }
        }
        run_turn(&mut g);
        let (px, _) = g.tanks[me].pivot();
        assert!(!g.land.get(px as i32 + 30, 104), "cleared in front");
        assert!(g.land.get(px as i32 - 30, 99), "untouched behind");
    }

    #[test]
    fn roller_rolls_downhill_into_the_valley() {
        let mut g = game(2);
        lob(&mut g, "Roller");
        // A slope down to the right from x 400 to a cliff at 600. Rollers
        // gain half a unit of speed for every unit they fall, so only a
        // cliff stops one at the bottom.
        g.land = Land::empty();
        for x in 0..800 {
            let h = if x < 400 { 300 } else if x < 600 { 300 - (x - 400) } else { 300 };
            for y in 0..h {
                g.land.set(x, y, true);
            }
        }
        for t in &mut g.tanks {
            t.y = g.land.top(t.x);
        }
        let me = g.turn;
        g.tanks[me].x = 100;
        g.tanks[me].y = 300;
        g.tanks[me].angle = 70;
        g.tanks[me].power = 420;
        g.fire();
        let mut rolled = false;
        let mut end = (0.0, 0.0);
        while g.phase == Phase::Action {
            g.tick();
            if let Some(s) = g.shells.first() {
                rolled |= s.rolling();
            }
            if let Some(b) = g.blasts.first() {
                end = (b.x, b.y);
            }
        }
        assert!(rolled);
        assert!(end.0 > 580.0 && end.0 < 600.0 && end.1 < 110.0, "roller went off at {end:?}");
    }

    #[test]
    fn digger_tunnels_and_sandhog_blows_up() {
        let mut g = game(2);
        lob(&mut g, "Digger");
        let before: usize = (0..800).map(|x| (0..400).filter(|&y| g.land.get(x, y)).count()).sum();
        g.fire();
        let mut dug = false;
        while g.phase == Phase::Action {
            g.tick();
            dug |= g.shells.iter().any(|s| s.digging());
        }
        let after: usize = (0..800).map(|x| (0..400).filter(|&y| g.land.get(x, y)).count()).sum();
        assert!(dug);
        assert!(before - after > 500, "digger cleared {}", before - after);

        let mut g = game(2);
        lob(&mut g, "Sandhog");
        g.fire();
        let mut blasted = false;
        while g.phase == Phase::Action {
            g.tick();
            blasted |= !g.blasts.is_empty();
        }
        assert!(blasted);
    }

    #[test]
    fn leap_frog_bounces_twice() {
        let mut g = game(2);
        lob(&mut g, "Leap Frog");
        g.fire();
        let mut blasts = 0;
        let mut seen = std::collections::HashSet::new();
        while g.phase == Phase::Action {
            g.tick();
            // The frog's own blasts, not any tank it destroys.
            for b in g.blasts.iter().filter(|b| b.force >= 1200.0) {
                if seen.insert((b.x as i64, b.y as i64)) {
                    blasts += 1;
                }
            }
        }
        assert_eq!(blasts, 3);
    }

    #[test]
    fn funky_bomb_throws_legs() {
        let mut g = game(2);
        lob(&mut g, "Funky Bomb");
        g.fire();
        let mut most = 0;
        while g.phase == Phase::Action {
            g.tick();
            most = most.max(g.blasts.len());
        }
        assert!((7..=12).contains(&most), "{most} blasts");
    }

    #[test]
    fn every_weapon_fires_and_the_turn_ends() {
        let names: Vec<String> = {
            let a = Arsenal::parse(DEFAULT_DEF).unwrap();
            a.selectable().into_iter().map(|i| a.weapons[i].name.clone()).collect()
        };
        for walls in [Walls::Wrap, Walls::Rubber, Walls::None] {
            for name in &names {
                let mut g = game_with(3, |c| c.walls = walls);
                select(&mut g, name);
                run_turn(&mut g);
            }
        }
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

#[cfg(test)]
mod timing {
    use super::*;
    use crate::weapons::DEFAULT_DEF;

    /// Not a check, a measurement: `cargo test --release timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ticks_per_weapon() {
        let a = Arsenal::parse(DEFAULT_DEF).unwrap();
        for i in a.selectable() {
            let mut g = super::tests::new_game(Config { players: 4, seed: 3, free: true, ..Config::default() });
            let me = g.turn;
            g.tanks[me].weapon = i;
            g.tanks[me].angle = 70;
            g.tanks[me].power = 500;
            g.fire();
            let start = std::time::Instant::now();
            let (mut ticks, mut worst, mut shells) = (0, std::time::Duration::ZERO, 0);
            while g.phase == Phase::Action || g.phase == Phase::Settle {
                let t = std::time::Instant::now();
                g.tick();
                worst = worst.max(t.elapsed());
                shells = shells.max(g.shells.len());
                ticks += 1;
            }
            println!("{:<18} {:>5} ticks {:>5} shells  worst tick {:>7.2?}  total {:>7.2?}", a.weapons[i].name, ticks, shells, worst, start.elapsed());
        }
    }
}
