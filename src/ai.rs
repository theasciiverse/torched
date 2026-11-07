//! Computer players (`sai/`): what each buys, which weapon it picks and
//! how it aims. Part of `Game`.

use log::info;
use rand::Rng;

use crate::economy::{self, Item, ShieldKind};
use crate::kit::Aisle;
use crate::land;
use crate::physics::{GRAVITY, Path, TIMEOUT, VELOCITY_SCALE, Walls};
use crate::sim::{Game, MAX_POWER, Phase, TANK_HEIGHT, TANK_RADIUS, TURRET_RADIUS};
use crate::weapons::{Arsenal, Weapon};

/// The seven computer players, and Random (`sc_ai_type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiKind {
    /// Fires at random.
    Moron,
    /// Aims for line of sight; buys weapons by yield for the price,
    /// fires the one with the best yield.
    Shooter,
    /// Like Shooter, but buys by yield alone.
    Spreader,
    /// Picks a victim and keeps at it; buys precise weapons.
    Chooser,
    /// Like Chooser, and allows for the wind.
    Calculater,
    /// Allows for the wind, spends on the biggest bangs first.
    Annihilater,
    /// Annihilater in Xscorch ("No one knows ..."); here too.
    Insanity,
    /// A different one each round.
    Random,
}

impl AiKind {
    pub const ALL: [AiKind; 8] = [
        AiKind::Moron,
        AiKind::Shooter,
        AiKind::Spreader,
        AiKind::Chooser,
        AiKind::Calculater,
        AiKind::Annihilater,
        AiKind::Insanity,
        AiKind::Random,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AiKind::Moron => "Moron",
            AiKind::Shooter => "Shooter",
            AiKind::Spreader => "Spreader",
            AiKind::Chooser => "Chooser",
            AiKind::Calculater => "Calculater",
            AiKind::Annihilater => "Annihilater",
            AiKind::Insanity => "Insanity",
            AiKind::Random => "Random",
        }
    }

    pub fn parse(s: &str) -> Option<AiKind> {
        AiKind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s.trim()))
    }

    /// Allows for the wind, and refines every shot by trying it out
    /// first (Xscorch's scan refinement, an option there, always on for
    /// these here).
    fn wind(self) -> bool {
        matches!(self, AiKind::Calculater | AiKind::Annihilater | AiKind::Insanity)
    }

    fn keeps_victim(self) -> bool {
        matches!(self, AiKind::Chooser | AiKind::Calculater | AiKind::Annihilater | AiKind::Insanity)
    }
}

/// Computer player state on a tank.
#[derive(Clone, Copy, Debug)]
pub struct Brain {
    pub kind: AiKind,
    /// This round's kind, when `kind` is Random.
    pub real: AiKind,
    pub victim: Option<usize>,
    /// Torched's own, the hook Xscorch's AI notes ask for: how far to aim
    /// off the victim, learnt from where the last shots at it landed.
    pub offset: f64,
    /// The victim and the point aimed at, last shot.
    pub last_aim: Option<(usize, f64)>,
}

/// A computer player's shot, being lined up on screen.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub angle: i32,
    pub power: i32,
    /// Ticks to wait once lined up.
    pub wait: u32,
}

/// AI tuning (`saiint.h`).
const POWER_DELTA_MAX: i32 = 100;
const ANGLE_DELTA_MAX: i32 = 60;
const MORON_MIN_POWER: i32 = 200;
const SHOOTER_START_A: i32 = 3;
const SHOOTER_DELTA_A: i32 = 45;
const SHOOTER_SIGHT_A: i32 = 70;
const SCAN_MIN_POWER: i32 = 200;
const VALIDATE_DIST: f64 = 100.0;
const CONSERVATIVE_BUDGET: f64 = 0.10;
const BUY_MAX_OF_WEAPON: usize = 3;
const INVENTORY_HIGH: u32 = 2;
const MAX_BATTERIES: u32 = 30;
/// Torched's own: how fast a computer swings its turret and power on
/// screen, and how long it holds before firing.
const SWING_DEGREES: i32 = 4;
const SWING_POWER: i32 = 30;
const HOLD_TICKS: u32 = 12;
const THINK_TICKS: u32 = 15;
/// How close a searched shot must come to count.
const SEARCH_NEAR: f64 = 60.0;
/// Share of a miss corrected next time, and the furthest it aims off.
const LEARN_RATE: f64 = 0.8;
const LEARN_MAX: f64 = 150.0;

/// A weapon's destructive potential (`_sc_weapon_stat_yield`).
pub fn yield_of(ars: &Arsenal, w: &Weapon, triple: bool) -> f64 {
    let mut y = w.radius * w.radius * w.force;
    if w.state.liquid {
        y *= w.liquid as f64;
    }
    if triple && w.state.triple {
        y *= 3.0;
    }
    if let (true, Some(c)) = (w.splits(), w.child) {
        let child = &ars.weapons[c];
        let n = w.children as f64;
        if w.phoenix.chain {
            for k in 1..=w.children {
                y += yield_of(ars, child, triple) * 0.85f64.powi(k as i32);
            }
        } else if w.phoenix.convert {
            // Rollers and sandhogs: the child's yield, or their own
            // warhead's where the child is only a tracer.
            y = yield_of(ars, child, triple).max(y);
        } else if w.phoenix.scatter {
            y = n * yield_of(ars, child, triple) * (10.0 + w.scatter) / 10.0;
        } else if w.phoenix.spider {
            y += 0.75 * n * yield_of(ars, child, triple);
        }
    }
    y
}

/// How tightly a weapon hits, bigger is better (`_sc_weapon_stat_precision`).
pub fn precision_of(ars: &Arsenal, w: &Weapon, triple: bool) -> f64 {
    let mut p = if w.state.liquid { w.liquid as f64 } else { 2.0 * w.radius };
    if triple && w.state.triple {
        p *= 2.0;
    }
    if let (true, Some(c)) = (w.splits(), w.child) {
        let child = &ars.weapons[c];
        let n = w.children as f64;
        let cp = precision_of(ars, child, triple);
        let inv = |q: f64| if q > 0.0 { 100.0 / q } else { 0.0 };
        if w.phoenix.chain {
            for k in 1..=w.children {
                p += 0.85f64.powi(k as i32) * inv(cp);
            }
        } else if w.phoenix.convert {
            p *= 100.0;
        } else if w.phoenix.scatter {
            p = n * inv(cp) * w.scatter;
        } else if w.phoenix.spider {
            p += 0.75 * n * inv(cp);
            p *= 2.0 * w.scatter;
        }
    }
    if p > 0.0 && w.force > 0.0 { 100.0 / p } else { 0.0 }
}

impl Game {
    pub fn is_ai(&self, tank: usize) -> bool {
        self.tanks[tank].brain.is_some()
    }

    /// Any people playing.
    pub fn has_humans(&self) -> bool {
        self.tanks.iter().any(|t| t.brain.is_none())
    }

    /// A new round: Random picks its kind; victims are forgotten.
    pub(crate) fn ai_new_round(&mut self) {
        for i in 0..self.tanks.len() {
            if let Some(mut b) = self.tanks[i].brain {
                if b.kind == AiKind::Random {
                    b.real = AiKind::ALL[self.rng.random_range(0..7)];
                }
                b.victim = None;
                b.offset = 0.0;
                b.last_aim = None;
                self.tanks[i].brain = Some(b);
            }
        }
        self.plan = None;
    }

    /// Called every tick: computer players shop and take their turns.
    pub(crate) fn ai_tick(&mut self) {
        match self.phase {
            Phase::Shop if self.is_ai(self.shop.player) => {
                self.ai_shop(self.shop.player);
                self.shop_done();
            }
            Phase::Aim if self.is_ai(self.turn) => self.ai_aim_tick(),
            _ => {}
        }
    }

    fn ai_aim_tick(&mut self) {
        let Some(mut plan) = self.plan else {
            self.think_wait += 1;
            if self.think_wait >= THINK_TICKS {
                self.think_wait = 0;
                let p = self.ai_think(self.turn);
                self.plan = Some(p);
            }
            return;
        };
        // Swing the turret and power onto the plan, then hold and fire.
        let t = &mut self.tanks[self.turn];
        t.angle += (plan.angle - t.angle).clamp(-SWING_DEGREES, SWING_DEGREES);
        t.power += (plan.power - t.power).clamp(-SWING_POWER, SWING_POWER);
        if t.angle == plan.angle && t.power == plan.power {
            if plan.wait == 0 {
                self.plan = None;
                self.fire();
                return;
            }
            plan.wait -= 1;
        }
        self.plan = Some(plan);
    }

    /// Decide a turn (`sc_ai_player_turn`): shields up, triggers armed,
    /// batteries spent, a weapon picked, then the aim.
    pub fn ai_think(&mut self, me: usize) -> Plan {
        let brain = self.tanks[me].brain.expect("a computer player");
        let kind = brain.real;
        if self.tanks[me].shield.is_none() {
            self.raise_shield();
        }
        if !self.tanks[me].triggers {
            self.toggle_triggers();
        }
        while self.use_battery() {}
        if kind == AiKind::Moron {
            self.select_last_weapon(me);
        } else {
            self.select_weapon_by_score(me);
        }
        let start = (self.tanks[me].angle, self.tanks[me].power);
        if kind.keeps_victim() {
            self.learn(me);
        }
        let aim = match kind {
            AiKind::Moron => None,
            k if k.keeps_victim() => self.fire_at_victim(me, k.wind()),
            k => self.calculated_fire(me, k.wind()),
        };
        // Torched's own: when the formulas find no shot (a cliff by the
        // barrel, a target hidden in a canyon), try shots out by
        // simulation before falling back to Xscorch's random fire.
        let aim = aim.or_else(|| if kind == AiKind::Moron { None } else { self.search(me) });
        let (mut angle, mut power) = aim.unwrap_or_else(|| {
            self.tanks[me].brain.as_mut().expect("brain").victim = None;
            self.random_fire(start)
        });
        if kind.wind()
            && let Some(v) = self.tanks[me].brain.and_then(|b| b.victim)
            && let Some((a, p)) = self.scan(me, v, angle, power)
        {
            (angle, power) = (a, p);
        }
        let b = self.tanks[me].brain.expect("brain");
        let aimed = b.victim.map(|v| (v, self.tanks[v].x as f64 + if kind.keeps_victim() { b.offset } else { 0.0 }));
        self.tanks[me].brain.as_mut().expect("brain").last_aim = aimed;
        let t = &self.tanks[me];
        info!("{} ({}) aims {angle} at {power} with {}", t.name, kind.name(), self.arsenal.weapons[t.weapon].name);
        Plan { angle, power: power.min(t.life).min(MAX_POWER), wait: HOLD_TICKS }
    }

    /// After a shot at the same victim, aim off by where it came down.
    fn learn(&mut self, me: usize) {
        let mut b = self.tanks[me].brain.expect("brain");
        let landed = self.tanks[me].last_trail.last().map(|p| p.0);
        match (b.last_aim, b.victim, landed) {
            (Some((was, aimed_x)), Some(v), Some(lx)) if was == v && self.tanks[v].alive => {
                // On the tank is right; leave the aim alone. Otherwise the
                // miss is measured from the point aimed at.
                if self.land.dx(self.tanks[v].x as f64, lx).abs() > 2.0 * TANK_RADIUS as f64 {
                    let miss = self.land.dx(aimed_x, lx);
                    b.offset = (b.offset - miss * LEARN_RATE).clamp(-LEARN_MAX, LEARN_MAX);
                }
            }
            _ => b.offset = 0.0,
        }
        self.tanks[me].brain = Some(b);
    }

    /// Moron: a random swing from where it was (`_sc_ai_random_fire`).
    fn random_fire(&mut self, (angle, power): (i32, i32)) -> (i32, i32) {
        let lo = MORON_MIN_POWER.max(power - POWER_DELTA_MAX);
        let hi = MORON_MIN_POWER.max(power + POWER_DELTA_MAX);
        let p = self.rng.random_range(lo..=hi);
        let a = (angle + self.rng.random_range(-ANGLE_DELTA_MAX..=ANGLE_DELTA_MAX)).clamp(0, 180);
        (a, p)
    }

    /// The last (furthest down the list) weapon held: the naive belief that
    /// later must be better (`_sc_ai_select_last_weapon`).
    fn select_last_weapon(&mut self, me: usize) {
        if let Some(w) = (0..self.arsenal.weapons.len()).rev().find(|&w| self.owns(me, w) && !self.arsenal.weapons[w].state.null) {
            self.tanks[me].weapon = w;
        }
    }

    /// The best yield held, sometimes the second best
    /// (`_sc_ai_select_weapon_by_score`).
    fn select_weapon_by_score(&mut self, me: usize) {
        let triple = self.has(me, |a| a.triple_turret);
        let mut scored: Vec<(f64, usize)> = (0..self.arsenal.weapons.len())
            .filter(|&w| self.owns(me, w))
            .map(|w| (yield_of(&self.arsenal, &self.arsenal.weapons[w], triple), w))
            .filter(|(y, _)| *y > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let pick = match scored.as_slice() {
            [] => return,
            [(_, w)] => *w,
            [(best, w), (next, w2), ..] => {
                if self.rng.random::<f64>() < next / (next + best) {
                    *w2
                } else {
                    *w
                }
            }
        };
        self.tanks[me].weapon = pick;
    }

    /// Try victims in random order; aim at the first reachable
    /// (`_sc_ai_calculated_fire`).
    fn calculated_fire(&mut self, me: usize, wind: bool) -> Option<(i32, i32)> {
        let mut victims: Vec<usize> = (0..self.tanks.len()).filter(|&i| i != me && self.tanks[i].alive).collect();
        for i in (1..victims.len()).rev() {
            victims.swap(i, self.rng.random_range(0..=i));
        }
        for v in victims {
            if let Some(aim) = self.aim_at(me, v, wind) {
                self.tanks[me].brain.as_mut().expect("brain").victim = Some(v);
                return Some(aim);
            }
        }
        None
    }

    /// Keep at the same victim while it lives (`_sc_ai_fire_at_victim`).
    fn fire_at_victim(&mut self, me: usize, wind: bool) -> Option<(i32, i32)> {
        if let Some(v) = self.tanks[me].brain.and_then(|b| b.victim)
            && self.tanks[v].alive
            && let Some(aim) = self.aim_at(me, v, wind)
        {
            return Some(aim);
        }
        self.calculated_fire(me, wind)
    }

    /// With line of sight, the cheapest shot; without, the steepest lob
    /// that reaches (`sc_ai_trajectory*`).
    fn aim_at(&self, me: usize, v: usize, wind: bool) -> Option<(i32, i32)> {
        let b = self.tanks[me].brain.expect("brain");
        let offset = if b.real.keeps_victim() && b.victim == Some(v) { b.offset } else { 0.0 };
        // The way to fire is settled by where the victim really is; the
        // learnt offset then moves the aim along that line.
        let me_t = &self.tanks[me];
        let dx = self.land.dx(me_t.x as f64, self.tanks[v].x as f64) + offset;
        let ty = (self.tanks[v].y + TANK_HEIGHT / 2) as f64;
        let tx = dx;
        let dy = ty - me_t.y as f64;
        let sight = self.line_of_sight(me, v);
        let w = if wind { self.wind } else { 0.0 };
        let sgn = if dx < 0.0 { -1 } else { 1 };
        if wind {
            // Every angle either side of the vertical; with sight the
            // cheapest, without the highest arc.
            let span = if sight { SHOOTER_SIGHT_A } else { SHOOTER_DELTA_A };
            let mut best: Option<(f64, i32, i32)> = None;
            for angle in 90 - span..=90 + span {
                if !self.angle_clear(me, angle) {
                    continue;
                }
                let Some((p, apex)) = self.power_for(me, angle, (tx, ty), w) else { continue };
                let score = if sight { -p } else { apex };
                if best.is_none_or(|(s, _, _)| score > s) {
                    best = Some((score, angle, p.round() as i32));
                }
            }
            return best.map(|(_, a, p)| (a, p));
        }
        let vector = dy.atan2(dx).to_degrees();
        let bound = |maxdelta: i32| {
            let d = if dx >= 0.0 { 90 - vector.max(0.0).ceil() as i32 } else { (if vector < 0.0 { 180.0 } else { vector }).floor() as i32 - 90 };
            d.min(maxdelta)
        };
        let mut angle = 90 - sgn * SHOOTER_START_A;
        if sight {
            let maxdelta = bound(SHOOTER_SIGHT_A);
            let mut best: Option<(f64, i32)> = None;
            while (angle - 90).abs() <= maxdelta {
                if self.angle_clear(me, angle)
                    && let Some((p, _)) = self.power_for(me, angle, (tx, ty), 0.0)
                    && best.is_none_or(|(bp, _)| p < bp)
                {
                    best = Some((p, angle));
                }
                angle -= sgn;
            }
            best.map(|(p, a)| (a, p.round() as i32))
        } else {
            let maxdelta = bound(SHOOTER_DELTA_A);
            while (angle - 90).abs() <= maxdelta {
                if self.angle_clear(me, angle)
                    && let Some((p, _)) = self.power_for(me, angle, (tx, ty), 0.0)
                {
                    return Some((angle, p.round() as i32));
                }
                angle -= sgn;
            }
            None
        }
    }

    /// Power to carry a shell at `angle` from this tank's muzzle through
    /// a target `rel_x` across from the tank and at height `ty`, with
    /// gravity and an optional wind, and the height of its arc. Solves x = v c t + w t²/2, y = v s t - g t²/2 for t and v.
    /// Arcs that would reach a ceiling are no good. (Xscorch only checks
    /// this for direct shots, so its lobs hit concrete ceilings; Torched
    /// checks always.)
    fn power_for(&self, me: usize, angle: i32, (rel_x, ty): (f64, f64), wind: f64) -> Option<(f64, f64)> {
        let t = &self.tanks[me];
        let a = (angle as f64).to_radians();
        let (c, s) = (a.cos(), a.sin());
        if s < 0.05 {
            return None;
        }
        let (px, py) = t.pivot();
        let (mx, my) = (px + (TURRET_RADIUS + 1.0) * c, py + (TURRET_RADIUS + 1.0) * s);
        let (dx, dy) = (rel_x - (mx - px), ty - my);
        let cot = c / s;
        let denom = 0.5 * (GRAVITY * cot + wind);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t2 = (dx - dy * cot) / denom;
        if t2 <= 0.0 {
            return None;
        }
        let time = t2.sqrt();
        let v = (dy + 0.5 * GRAVITY * t2) / (time * s);
        if v <= 0.0 {
            return None;
        }
        let power = v / VELOCITY_SCALE;
        if power > MAX_POWER.min(t.life) as f64 {
            return None;
        }
        let apex = my + (v * s).powi(2) / (2.0 * GRAVITY);
        let ceiling = matches!(self.walls, Walls::Concrete | Walls::Padded | Walls::Rubber | Walls::Spring);
        if ceiling && apex >= land::HEIGHT as f64 - 2.0 {
            return None;
        }
        Some((power, apex))
    }

    /// Clear of dirt for a little way out of the barrel, so it doesn't
    /// fire into a cliff beside it (`_sc_ai_validate_angle`).
    fn angle_clear(&self, me: usize, angle: i32) -> bool {
        let t = &self.tanks[me];
        let a = (angle as f64).to_radians();
        let (px, py) = t.pivot();
        let (x0, y0) = (px + (TURRET_RADIUS + 1.0) * a.cos(), py + (TURRET_RADIUS + 1.0) * a.sin());
        (0..VALIDATE_DIST as i32).all(|k| {
            let (x, y) = (x0 + k as f64 * a.cos(), y0 + k as f64 * a.sin());
            !self.land.get(x.round() as i32, y.round() as i32)
        })
    }

    /// A straight line between the tanks' flanks with no dirt in it
    /// (`_sc_ai_line_of_sight`).
    fn line_of_sight(&self, me: usize, v: usize) -> bool {
        let (a, b) = (&self.tanks[me], &self.tanks[v]);
        let r = TANK_RADIUS as f64;
        let ya = (a.y + TANK_HEIGHT / 2) as f64;
        let yb = (b.y + TANK_HEIGHT / 2) as f64;
        [(-r, -r), (-r, r), (r, -r), (r, r)].iter().any(|&(oa, ob)| {
            let (x0, x1) = (a.x as f64 + oa, a.x as f64 + oa + self.land.dx(a.x as f64 + oa, b.x as f64 + ob));
            let n = (x1 - x0).abs().max((yb - ya).abs()).ceil().max(1.0) as i32;
            (0..=n).all(|k| {
                let f = k as f64 / n as f64;
                !self.land.get((x0 + (x1 - x0) * f).round() as i32, (ya + (yb - ya) * f).round() as i32)
            })
        })
    }

    /// Every living enemy, nearest first, tried by simulation over a coarse
    /// sweep of angles and powers, then refined; the first that lands
    /// close becomes the victim.
    fn search(&mut self, me: usize) -> Option<(i32, i32)> {
        let mut victims: Vec<usize> = (0..self.tanks.len()).filter(|&i| i != me && self.tanks[i].alive).collect();
        let x = self.tanks[me].x as f64;
        victims.sort_by(|&a, &b| {
            let d = |i: usize| self.land.dx(x, self.tanks[i].x as f64).abs();
            d(a).total_cmp(&d(b))
        });
        let top = MAX_POWER.min(self.tanks[me].life);
        for v in victims {
            let target = (self.tanks[v].x as f64, self.tanks[v].y as f64);
            let mut best: Option<(f64, i32, i32)> = None;
            for angle in (10..=170).step_by(5) {
                for power in (100..=top).step_by(50) {
                    if let Some((lx, ly)) = self.predict(me, angle, power) {
                        let (dx, dy) = (self.land.dx(target.0, lx), ly - target.1);
                        let d = dx * dx + dy * dy;
                        if best.is_none_or(|(bd, _, _)| d < bd) {
                            best = Some((d, angle, power));
                        }
                    }
                }
            }
            if let Some((d, a, p)) = best
                && d < SEARCH_NEAR * SEARCH_NEAR
            {
                let (a, p) = self.scan(me, v, a, p).unwrap_or((a, p));
                self.tanks[me].brain.as_mut().expect("brain").victim = Some(v);
                return Some((a, p));
            }
        }
        None
    }

    /// Scan refinement (`sc_ai_trajectory_scan`): try shots around the
    /// planned one, coarse then fine, and keep the closest landing.
    fn scan(&self, me: usize, v: usize, angle: i32, power: i32) -> Option<(i32, i32)> {
        let target = (self.tanks[v].x as f64, self.tanks[v].y as f64);
        let pass = |a0: i32, p0: i32, da: i32, sa: i32, dp: i32, sp: i32, maxd: f64| -> Option<(i32, i32)> {
            let mut best: Option<(f64, i32, i32)> = None;
            let mut a = a0 - da;
            while a <= a0 + da {
                let mut p = p0 - dp;
                while p < p0 + dp && p < MAX_POWER.min(self.tanks[me].life) {
                    if p >= SCAN_MIN_POWER
                        && let Some((x, y)) = self.predict(me, a.clamp(0, 180), p)
                    {
                        let (ddx, ddy) = (self.land.dx(target.0, x), y - target.1);
                        let d = ddx * ddx + ddy * ddy;
                        if best.is_none_or(|(bd, _, _)| d < bd) {
                            best = Some((d, a.clamp(0, 180), p));
                        }
                    }
                    p += sp;
                }
                a += sa;
            }
            best.filter(|(d, _, _)| *d <= maxd * maxd).map(|(_, a, p)| (a, p))
        };
        let (a, p) = pass(angle, power, 20, 10, 100, 50, 150.0)?;
        pass(a, p, 4, 1, 20, 5, 50.0)
    }

    /// Where a shot would come down, flying it through the current wind,
    /// walls, dirt and tanks without changing anything. Shields and
    /// splitting are ignored.
    pub fn predict(&self, me: usize, angle: i32, power: i32) -> Option<(f64, f64)> {
        let t = &self.tanks[me];
        let a = (angle as f64).to_radians();
        let (px, py) = t.pivot();
        let start = (px + (TURRET_RADIUS + 1.0) * a.cos(), py + (TURRET_RADIUS + 1.0) * a.sin());
        let v = power as f64 * VELOCITY_SCALE;
        let mut path = Path { ctr: start, vel: (v * a.cos(), v * a.sin()), acc: (self.wind, -GRAVITY), drag: self.cfg.viscosity };
        let (mut base, mut t_local, mut last) = (0.0, 0.0, start);
        let dt = 1.0 / 16.0;
        let (w_, h_) = (land::WIDTH as f64, land::HEIGHT as f64);
        let mut step = 0.0;
        while step < TIMEOUT {
            step += dt;
            t_local += dt;
            let ((mut x, mut y), (vx, vy)) = path.at(t_local - base);
            let off = x < 0.0 || x >= w_;
            match self.walls {
                Walls::Wrap if off => {
                    let shift = if x < 0.0 { w_ } else { -w_ };
                    path.ctr.0 += shift;
                    x += shift;
                }
                Walls::Concrete if off || y >= h_ => return Some((x.clamp(0.0, w_ - 1.0), y.min(h_ - 1.0))),
                Walls::None | Walls::Random if off => {
                    if !(-w_..2.0 * w_).contains(&x) {
                        return None;
                    }
                    last = (x, y);
                    continue;
                }
                walls => {
                    if let Some(e) = walls.elasticity()
                        && (off || y >= h_)
                    {
                        let (nvx, nvy) = if off { (-vx * e, vy) } else { (vx, -vy * e) };
                        x = x.clamp(0.0, w_ - 1.0);
                        y = y.min(h_ - 1.0);
                        path = Path { ctr: (x, y), vel: (nvx, nvy), ..path };
                        base = t_local;
                    }
                }
            }
            if y < 0.0 {
                return Some((x, 0.0));
            }
            // Check the cells between the last point and this one.
            let n = (x - last.0).abs().max((y - last.1).abs()).ceil().max(1.0) as i32;
            for k in 1..=n {
                let f = k as f64 / n as f64;
                let (cx, cy) = (last.0 + (x - last.0) * f, last.1 + (y - last.1) * f);
                if self.land.get(cx.round() as i32, cy.round() as i32) || self.tanks.iter().any(|tk| tk.contains(&self.land, cx, cy)) {
                    return Some((cx, cy));
                }
            }
            last = (x, y);
            // Magnetic shields push, as in flight.
            let ((dvx, dvy), costs) = self.magnetic_push(me, x, y);
            if !costs.is_empty() {
                let (vx, vy) = path.at(t_local - base).1;
                path = Path { ctr: (x, y), vel: (vx + dvx, vy + dvy), ..path };
                base = t_local;
            }
        }
        None
    }

    // ---- Buying ---------------------------------------------------------

    /// How much a computer will spend this time (`_sc_ai_budget`): a
    /// tenth to start, more as the rounds run out.
    fn budget(&self, me: usize) -> i64 {
        let mut pct = CONSERVATIVE_BUDGET;
        if self.cfg.interest > pct {
            pct = self.interest_rate;
        }
        pct += (self.round + 1) as f64 / self.cfg.rounds as f64 * (1.0 - pct);
        (self.tanks[me].money as f64 * pct.min(1.0)) as i64
    }

    /// Shop for a computer player (`sc_ai_player_buy`).
    pub fn ai_shop(&mut self, me: usize) {
        let Some(brain) = self.tanks[me].brain else { return };
        let mut budget = self.budget(me);
        let m0 = self.tanks[me].money;
        match brain.real {
            AiKind::Moron => {
                self.buy_triggers(me, &mut budget);
                self.buy_auto_defense(me, &mut budget);
                self.buy_shield(me, &mut budget, false);
                self.buy_last_weapons(me, &mut budget);
            }
            AiKind::Shooter | AiKind::Spreader | AiKind::Chooser | AiKind::Calculater => {
                self.buy_triggers(me, &mut budget);
                self.buy_auto_defense(me, &mut budget);
                self.buy_shield(me, &mut budget, true);
                let rank = match brain.real {
                    AiKind::Shooter => Rank::Value,
                    AiKind::Spreader => Rank::Yield,
                    _ => Rank::Precision,
                };
                self.buy_weapons(me, &mut budget, rank);
                if matches!(brain.real, AiKind::Chooser | AiKind::Calculater) {
                    self.buy_batteries(me, &mut budget);
                }
            }
            AiKind::Annihilater | AiKind::Insanity | AiKind::Random => {
                budget = (self.tanks[me].money as f64 * 0.85) as i64;
                self.buy_triggers(me, &mut budget);
                self.buy_weapons(me, &mut budget, Rank::Yield);
                budget = self.tanks[me].money;
                self.buy_auto_defense(me, &mut budget);
                self.buy_shield(me, &mut budget, true);
                self.buy_batteries(me, &mut budget);
            }
        }
        let t = &self.tanks[me];
        let bought: Vec<String> = (0..self.arsenal.weapons.len())
            .filter(|&i| t.stock[i] > 0)
            .map(|i| format!("{} {}", t.stock[i], self.arsenal.weapons[i].name))
            .chain((0..self.accessories.items.len()).filter(|&i| t.kit[i] > 0).map(|i| format!("{} {}", t.kit[i], self.accessories.items[i].name)))
            .collect();
        info!("{} ({}) spends {}, holds {}", t.name, brain.real.name(), m0 - t.money, bought.join(", "));
    }

    /// Buy a bundle if it fits both purse and budget.
    fn buy(&mut self, me: usize, aisle: Aisle, i: usize, budget: &mut i64) -> bool {
        let (q, cost) = match aisle {
            Aisle::Weapons => {
                let w = &self.arsenal.weapons[i];
                let q = economy::buy_quantity(w, self.tanks[me].stock[i]);
                (q, economy::buy_price(w, q))
            }
            Aisle::Accessories => {
                let a = &self.accessories.items[i];
                if a.permanent && self.tanks[me].kit[i] > 0 {
                    return false;
                }
                let q = economy::buy_quantity(a, self.tanks[me].kit[i]);
                (q, economy::buy_price(a, q))
            }
        };
        if q == 0 || cost > self.tanks[me].money || cost > *budget {
            return false;
        }
        let t = &mut self.tanks[me];
        match aisle {
            Aisle::Weapons => t.stock[i] += q,
            Aisle::Accessories => t.kit[i] += q,
        }
        t.money -= cost;
        *budget -= cost;
        true
    }

    fn buy_triggers(&mut self, me: usize, budget: &mut i64) {
        if !self.cfg.tunneling {
            return;
        }
        if let Some(i) = self.aisle(Aisle::Accessories).into_iter().find(|&i| self.accessories.items[i].contact_trigger) {
            self.buy(me, Aisle::Accessories, i, budget);
        }
    }

    /// Only worth it when turns come one at a time.
    fn buy_auto_defense(&mut self, me: usize, budget: &mut i64) {
        if self.cfg.simultaneous {
            return;
        }
        if let Some(i) = self.aisle(Aisle::Accessories).into_iter().find(|&i| self.accessories.items[i].auto_defense) {
            self.buy(me, Aisle::Accessories, i, budget);
        }
    }

    /// One set of shields: the last listed that fits, or the strongest.
    fn buy_shield(&mut self, me: usize, budget: &mut i64, best: bool) {
        let mut shields: Vec<usize> =
            self.aisle(Aisle::Accessories).into_iter().filter(|&i| self.accessories.items[i].shield.is_some()).collect();
        if best {
            let key = |i: &usize| {
                let a = &self.accessories.items[*i];
                (a.shielding, matches!(a.shield, Some(ShieldKind::Force)))
            };
            shields.sort_by_key(|i| std::cmp::Reverse(key(i)));
        } else {
            shields.reverse();
        }
        for i in shields {
            if self.buy(me, Aisle::Accessories, i, budget) {
                return;
            }
        }
    }

    fn buy_batteries(&mut self, me: usize, budget: &mut i64) {
        if self.count(me, |a| a.battery) >= MAX_BATTERIES {
            return;
        }
        if let Some(i) = self.aisle(Aisle::Accessories).into_iter().find(|&i| self.accessories.items[i].battery) {
            self.buy(me, Aisle::Accessories, i, budget);
        }
    }

    /// Moron: the weapons furthest down the list, up to three rounds of
    /// buying (`_sc_ai_buy_last_weapons`).
    fn buy_last_weapons(&mut self, me: usize, budget: &mut i64) {
        let list = self.useful_weapons(me);
        for _ in 0..BUY_MAX_OF_WEAPON {
            for &i in list.iter().rev() {
                self.buy(me, Aisle::Weapons, i, budget);
            }
        }
    }

    /// Weapons ranked best first; the first pass skips what's already
    /// well stocked (`_sc_ai_buy_weapons_from_list`).
    fn buy_weapons(&mut self, me: usize, budget: &mut i64, rank: Rank) {
        let triple = self.has(me, |a| a.triple_turret);
        let ars = self.arsenal.clone();
        let mut list: Vec<(f64, usize)> = self
            .useful_weapons(me)
            .into_iter()
            .map(|i| {
                let w = &ars.weapons[i];
                let y = yield_of(&ars, w, triple);
                let score = match rank {
                    Rank::Yield => y,
                    Rank::Value => y * w.bundle as f64 / w.price.max(1) as f64,
                    Rank::Precision => y * precision_of(&ars, w, triple),
                };
                (score, i)
            })
            .collect();
        list.sort_by(|a, b| b.0.total_cmp(&a.0));
        for pass in 0..BUY_MAX_OF_WEAPON {
            for &(_, i) in &list {
                let w = &ars.weapons[i];
                if pass == 0 && self.tanks[me].stock[i] / w.bundle().max(1) >= INVENTORY_HIGH {
                    continue;
                }
                self.buy(me, Aisle::Weapons, i, budget);
            }
        }
    }

    /// On sale and with some bang in them (no tracers, dirt or riots).
    fn useful_weapons(&self, me: usize) -> Vec<usize> {
        let triple = self.has(me, |a| a.triple_turret);
        self.aisle(Aisle::Weapons)
            .into_iter()
            .filter(|&i| {
                let w = &self.arsenal.weapons[i];
                !w.useless && yield_of(&self.arsenal, w, triple) > 0.0
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Rank {
    Yield,
    Value,
    Precision,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::tests::{game_with, run_turn};
    use crate::sim::{Config, MAX_LIFE};
    use crate::weapons::DEFAULT_DEF;

    #[test]
    fn yields_rank_as_in_xscorch() {
        let a = Arsenal::parse(DEFAULT_DEF).unwrap();
        let y = |n: &str| yield_of(&a, &a.weapons[a.find(n).unwrap()], false);
        assert!(y("Nuke") > y("Baby Nuke") && y("Baby Nuke") > y("Missile") && y("Missile") > y("Baby Missile"));
        assert!(y("Death's Head") > y("MIRV"));
        assert_eq!(y("Tracer"), 0.0);
        let p = |n: &str| precision_of(&a, &a.weapons[a.find(n).unwrap()], false);
        // "This will give a Missile a precision of 1.25."
        assert!((p("Missile") - 1.25).abs() < 1e-9);
        assert!(p("Missile") > p("MIRV"));
    }

    #[test]
    fn the_solver_lands_where_it_aims() {
        for wind in [0.0, 0.04, -0.04] {
            let mut g = game_with(2, |c| c.max_wind = 0.0);
            g.wind = wind;
            crate::sim::tests::lob(&mut g, "Tracer");
            let me = g.turn;
            for (angle, target) in [(60, 600.0), (45, 450.0), (75, 350.0)] {
                let rel = target - g.tanks[me].x as f64;
                let (p, _) = g.power_for(me, angle, (rel, 103.0), wind).unwrap();
                g.tanks[me].angle = angle;
                g.tanks[me].power = p.round() as i32;
                let (x, _) = g.predict(me, angle, p.round() as i32).unwrap();
                assert!((x - target).abs() < 6.0, "wind {wind} angle {angle}: landed {x}, aimed {target}");
            }
        }
    }

    fn ai_game(kinds: &[AiKind], f: impl FnOnce(&mut Config)) -> crate::sim::Game {
        game_with(0, |c| {
            c.ai = kinds.to_vec();
            f(c);
        })
    }

    #[test]
    fn every_kind_hits_on_open_ground() {
        // Each aiming kind should land a Baby Missile on a sitting tank
        // on flat ground most of the time.
        for kind in [AiKind::Shooter, AiKind::Chooser, AiKind::Calculater, AiKind::Insanity] {
            let mut hits = 0;
            for seed in 0..6 {
                let mut g = ai_game(&[kind, AiKind::Moron], |c| {
                    c.seed = seed;
                    c.generator = Some(crate::land::Generator::None);
                    c.max_wind = 0.03;
                });
                let me = (0..2).find(|&i| g.tanks[i].brain.unwrap().kind == kind).unwrap();
                g.turn = me;
                let other = 1 - me;
                let plan = g.ai_think(me);
                g.tanks[me].angle = plan.angle;
                g.tanks[me].power = plan.power;
                run_turn(&mut g);
                if g.tanks[other].life < MAX_LIFE {
                    hits += 1;
                }
            }
            assert!(hits >= 4, "{kind:?} hit {hits} of 6");
        }
    }

    #[test]
    fn computers_shop_within_their_means() {
        for kind in AiKind::ALL {
            let mut g = ai_game(&[kind, kind], |c| c.free = false);
            assert_eq!(g.phase, Phase::Shop);
            g.ai_tick();
            g.ai_tick();
            assert_eq!(g.phase, Phase::Aim, "{kind:?}");
            for t in &g.tanks {
                assert!(t.money >= 0 && t.money < 100_000, "{kind:?} left {}", t.money);
            }
        }
    }

    #[test]
    fn a_demo_game_plays_itself_to_the_end() {
        let mut g = ai_game(&[AiKind::Moron, AiKind::Shooter, AiKind::Calculater, AiKind::Random], |c| {
            c.free = false;
            c.rounds = 2;
            c.seed = 4;
        });
        for _ in 0..400_000 {
            g.tick();
            if g.phase == Phase::GameOver {
                return;
            }
        }
        panic!(
            "stuck in round {} {:?}: turn {} plan {:?} tanks {:?}",
            g.round,
            g.phase,
            g.turn,
            g.plan,
            g.tanks.iter().map(|t| (t.alive, t.angle, t.power, t.life, t.x, t.y)).collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod learning {
    use super::*;
    use crate::sim::tests::{game_with, run_turn};
    use crate::sim::MAX_LIFE;

    #[test]
    fn a_chooser_learns_the_wind() {
        // Chooser ignores wind, so a steady crosswind makes it miss; it
        // should learn to aim off and hit within a few shots.
        let mut g = game_with(0, |c| {
            c.ai = vec![AiKind::Chooser, AiKind::Moron];
            c.generator = Some(crate::land::Generator::None);
            c.walls = Walls::Wrap;
            c.seed = 2;
            c.max_wind = 0.05;
            c.dynamic_wind = false;
        });
        g.wind = 0.05;
        let (me, other) = (0, 1);
        g.tanks[me].x = 150;
        g.tanks[other].x = 600;
        let mut hits = Vec::new();
        for _ in 0..6 {
            g.turn = me;
            g.phase = Phase::Aim;
            g.tanks[other].life = MAX_LIFE;
            g.tanks[other].alive = true;
            let plan = g.ai_think(me);
            g.tanks[me].angle = plan.angle;
            g.tanks[me].power = plan.power;
            run_turn(&mut g);
            hits.push(g.tanks[other].life < MAX_LIFE);
            // Put the ground back so the target doesn't sink away.
            g.land = crate::land::Land::empty();
            g.land.wrap = true;
            for t in &mut g.tanks {
                t.y = 0;
            }
        }
        assert!(!hits[0], "the wind should spoil the first shot");
        assert!(hits[2..].iter().filter(|h| **h).count() >= 3, "hits {hits:?}");
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use crate::sim::Config;
    use crate::sim::tests::game_with;

    /// A measurement, not a check: `cargo test --release ai::timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn demo_round_lengths() {
        let (mut total, mut rounds, mut stalemates, mut wins) = (0u64, 0, 0, 0);
        for seed in 0..12 {
            let mut g = game_with(0, |c| {
                *c = Config {
                    players: 0,
                    ai: vec![AiKind::Chooser, AiKind::Calculater, AiKind::Annihilater, AiKind::Insanity],
                    seed,
                    rounds: 3,
                    free: std::env::var("TORCHED_FREE").is_ok(),
                    ..Config::default()
                }
            });
            let mut ticks = 0u64;
            while g.phase != Phase::GameOver && ticks < 60 * 60 * 30 {
                let was = g.phase;
                g.tick();
                ticks += 1;
                if was != Phase::RoundOver && matches!(g.phase, Phase::RoundOver | Phase::GameOver) {
                    rounds += 1;
                    if g.message.starts_with("Stalemate") {
                        stalemates += 1;
                    } else if g.round_winner.is_some() {
                        wins += 1;
                    }
                }
            }
            total += ticks;
        }
        println!(
            "{rounds} rounds, {wins} won, {stalemates} stalemates, {:.1} minutes a round",
            total as f64 / 60.0 / 60.0 / rounds.max(1) as f64
        );
    }
}

#[cfg(test)]
mod accuracy {
    use super::*;
    use crate::sim::tests::{game_with, run_turn};
    use crate::sim::{Config, MAX_LIFE};

    /// A measurement: first-shot hit rate per kind on random ground.
    #[test]
    #[ignore]
    fn hit_rates() {
        for kind in [AiKind::Shooter, AiKind::Spreader, AiKind::Chooser, AiKind::Calculater, AiKind::Insanity] {
            let (mut hits, mut random, n) = (0, 0, 30);
            for seed in 0..n {
                let mut g = game_with(0, |c| {
                    *c = Config { players: 0, ai: vec![kind, AiKind::Moron], seed, free: true, ..Config::default() }
                });
                g.turn = 0;
                let plan = g.ai_think(0);
                if g.tanks[0].brain.unwrap().victim.is_none() {
                    random += 1;
                }
                g.tanks[0].angle = plan.angle;
                g.tanks[0].power = plan.power;
                run_turn(&mut g);
                if g.tanks[1].life < MAX_LIFE {
                    hits += 1;
                }
            }
            println!("{:<12} hit {hits}/{n}, no aim {random}", kind.name());
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;
    use crate::sim::Config;
    use crate::sim::tests::game_with;

    /// A search, not a check: seeds whose first 48 seconds have the most
    /// going on, for the catalog recording.
    #[test]
    #[ignore]
    fn lively_seeds() {
        let mut scored = Vec::new();
        for seed in 0..60u64 {
            let mut g = game_with(0, |c| {
                *c = Config {
                    players: 0,
                    ai: vec![AiKind::Chooser, AiKind::Calculater, AiKind::Annihilater, AiKind::Insanity],
                    seed,
                    cash: 300_000,
                    ..Config::default()
                }
            });
            let (mut big, mut deaths, mut seen) = (0, 0, std::collections::HashSet::new());
            for _ in 0..(48 * 60) {
                g.tick();
                for b in &g.blasts {
                    if b.age == 1 && b.radius >= 60.0 && seen.insert((b.x as i64, b.y as i64, b.radius as i64)) {
                        big += 1;
                    }
                }
                deaths = g.tanks.iter().filter(|t| !t.alive).count().max(deaths);
            }
            scored.push((big + 4 * deaths, seed, big, deaths));
        }
        scored.sort();
        for s in scored.iter().rev().take(8) {
            println!("seed {:>3}: big blasts {}, deaths {}", s.1, s.2, s.3);
        }
    }
}
