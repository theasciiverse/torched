//! What players buy and carry: the shop, money between rounds, shields
//! and the other accessories (`sinventory.c`, `seconomy.c`, `sshield.c`,
//! `splayer.c`). Part of `Game`, kept apart from the flight and blasts.

use log::info;
use rand::Rng;

use crate::economy::{self, Accessory, Item, ShieldKind};
use crate::land;
use crate::physics::VELOCITY_SCALE;
use crate::sim::{Game, Order, Phase, Shield, TANK_RADIUS};

/// Strength a direct hit takes off a shield, double for a sapper.
const SHIELD_ABSORB_HIT: i32 = 700;
const SHIELD_SAPPER_RATE: i32 = 2;
/// Strength a solar panel restores each turn.
const SHIELD_RECHARGE_RATE: i32 = 500;
/// Magnetic shields (`sshield.h`).
const MAG_MAX_DIST: f64 = 100.0;
const MAG_ATTENUATION: f64 = 7.0;
const MAG_TO_POWER: f64 = 1.5;
const MAG_TO_COST: f64 = 0.75;
/// A battery restores this share of full life (`SC_BATTERY_RECHARGE_PERCT`).
const BATTERY_PERCENT: i32 = 1;
/// Highest step a tank can drive up (`SC_TANK_CLIMB_HEIGHT`).
const CLIMB_HEIGHT: i32 = 3;

/// The two lists in the shop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Aisle {
    #[default]
    Weapons,
    Accessories,
}

#[derive(Clone, Debug, Default)]
pub struct ShopState {
    pub player: usize,
    pub aisle: Aisle,
    pub cursor: usize,
}

/// One line in the shop.
pub struct ShopRow {
    pub name: String,
    pub description: String,
    pub bundle: u32,
    pub price: u32,
    pub have: u32,
    /// What buying would add and cost now (0 if it can't).
    pub buy: (u32, i64),
    pub affordable: bool,
}

impl Game {
    // ---- The shop -----------------------------------------------------

    pub(crate) fn begin_shopping(&mut self) {
        self.phase = Phase::Shop;
        self.shop = ShopState::default();
        self.message = format!("Round {} of {}: the shop", self.round + 1, self.cfg.rounds);
    }

    /// What's on sale in an aisle: within the arms level, not free.
    pub fn aisle(&self, aisle: Aisle) -> Vec<usize> {
        let lvl = self.cfg.arms_level;
        match aisle {
            Aisle::Weapons => self
                .arsenal
                .selectable()
                .into_iter()
                .filter(|&i| {
                    let w = &self.arsenal.weapons[i];
                    w.arms_level <= lvl && !w.infinite()
                })
                .collect(),
            Aisle::Accessories => self
                .accessories
                .listed()
                .into_iter()
                .filter(|&i| {
                    let a = &self.accessories.items[i];
                    a.arms_level <= lvl && !a.infinite()
                })
                .collect(),
        }
    }

    pub fn shop_rows(&self) -> Vec<ShopRow> {
        let p = self.shop.player;
        let t = &self.tanks[p];
        let row = |item: &dyn ItemRow, have: u32, can: bool| {
            let q = if can { economy::buy_quantity(&Priced(item.price(), item.bundle()), have) } else { 0 };
            let cost = economy::buy_price(&Priced(item.price(), item.bundle()), q);
            ShopRow {
                name: item.name().to_string(),
                description: item.description().to_string(),
                bundle: item.bundle(),
                price: item.price(),
                have,
                buy: (q, cost),
                affordable: q > 0 && cost <= t.money,
            }
        };
        match self.shop.aisle {
            Aisle::Weapons => self
                .aisle(Aisle::Weapons)
                .into_iter()
                .map(|i| row(&self.arsenal.weapons[i], t.stock[i], true))
                .collect(),
            Aisle::Accessories => self
                .aisle(Aisle::Accessories)
                .into_iter()
                .map(|i| {
                    let a = &self.accessories.items[i];
                    // A permanent accessory is bought once.
                    row(a, t.kit[i], !(a.permanent && t.kit[i] > 0))
                })
                .collect(),
        }
    }

    pub fn shop_move(&mut self, delta: i32) {
        let n = self.aisle(self.shop.aisle).len() as i32;
        if n > 0 {
            self.shop.cursor = (self.shop.cursor as i32 + delta).clamp(0, n - 1) as usize;
        }
    }

    pub fn shop_switch(&mut self) {
        self.shop.aisle = match self.shop.aisle {
            Aisle::Weapons => Aisle::Accessories,
            Aisle::Accessories => Aisle::Weapons,
        };
        self.shop.cursor = 0;
    }

    /// Buy a bundle of the item under the cursor
    /// (`sc_inventory_buy_weapon`, `_accessory`).
    pub fn shop_buy(&mut self) {
        let p = self.shop.player;
        let rows = self.shop_rows();
        let Some(row) = rows.get(self.shop.cursor) else { return };
        if !row.affordable {
            return;
        }
        let i = self.aisle(self.shop.aisle)[self.shop.cursor];
        let (q, cost) = row.buy;
        let t = &mut self.tanks[p];
        match self.shop.aisle {
            Aisle::Weapons => t.stock[i] += q,
            Aisle::Accessories => t.kit[i] += q,
        }
        t.money -= cost;
        info!("{} buys {q} {} for {cost}", t.name, row.name);
    }

    /// Sell back up to a bundle at half price (`sc_inventory_sell_*`).
    pub fn shop_sell(&mut self) {
        let p = self.shop.player;
        let Some(&i) = self.aisle(self.shop.aisle).get(self.shop.cursor) else { return };
        let (have, item): (u32, Priced) = match self.shop.aisle {
            Aisle::Weapons => {
                let w = &self.arsenal.weapons[i];
                (self.tanks[p].stock[i], Priced(w.price, w.bundle))
            }
            Aisle::Accessories => {
                let a = &self.accessories.items[i];
                (self.tanks[p].kit[i], Priced(a.price, a.bundle))
            }
        };
        if have == 0 {
            return;
        }
        let (q, pay) = economy::sell(&item, have);
        let t = &mut self.tanks[p];
        match self.shop.aisle {
            Aisle::Weapons => t.stock[i] -= q,
            Aisle::Accessories => t.kit[i] -= q,
        }
        t.money += pay;
    }

    /// This player is done; the next shops, or the round starts.
    pub fn shop_done(&mut self) {
        let next = self.shop.player + 1;
        if next < self.tanks.len() {
            self.shop = ShopState { player: next, ..ShopState::default() };
        } else {
            self.start_round();
        }
    }

    /// Between rounds (`sc_economy_interest`), and Greedy's pay for
    /// unused stock (Torched's own).
    pub(crate) fn pay_interest(&mut self) {
        let rate = self.interest_rate;
        let stock_rate = self.scoring.stock_rate;
        for i in 0..self.tanks.len() {
            let worth = if stock_rate > 0.0 { self.stock_value(i) } else { 0 };
            let t = &mut self.tanks[i];
            if t.money > 0 {
                t.money = ((t.money as f64) * (1.0 + rate)) as i64;
            }
            t.money = (t.money + (worth as f64 * stock_rate) as i64).min(economy::MAX_CASH);
        }
        if self.cfg.dynamic_interest {
            let d = (self.rng.random_range(0..7) - 3) as f64 / 100.0;
            self.interest_rate = (self.interest_rate + d).clamp(0.0, economy::MAX_INTEREST);
        }
    }

    /// What a tank's stock would sell for.
    pub fn stock_value(&self, i: usize) -> i64 {
        let t = &self.tanks[i];
        let w: i64 = self
            .arsenal
            .weapons
            .iter()
            .zip(&t.stock)
            .filter(|(_, n)| **n > 0)
            .map(|(w, &n)| economy::sell(&Priced(w.price, w.bundle), n).1 * n.div_ceil(w.bundle.max(1)) as i64)
            .sum();
        let a: i64 = self
            .accessories
            .items
            .iter()
            .zip(&t.kit)
            .filter(|(_, n)| **n > 0)
            .map(|(a, &n)| economy::sell(&Priced(a.price, a.bundle), n).1 * n.div_ceil(a.bundle.max(1)) as i64)
            .sum();
        w + a
    }

    /// This round's turn order (`sc_config` order options; the first
    /// round is always random).
    pub(crate) fn order_round(&mut self) {
        let n = self.tanks.len();
        if self.order.len() != n || self.round <= 1 || self.cfg.order != Order::RoundRobin {
            let mut o: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                o.swap(i, self.rng.random_range(0..=i));
            }
            if self.round > 1 {
                let score = |i: usize| self.tanks[i].kills as i64 - self.tanks[i].suicides as i64;
                match self.cfg.order {
                    Order::WinnersFirst => o.sort_by_key(|&i| std::cmp::Reverse(score(i))),
                    Order::LosersFirst => o.sort_by_key(|&i| score(i)),
                    Order::Random | Order::RoundRobin => {}
                }
            }
            self.order = o;
        } else {
            self.order.rotate_left(1);
        }
    }

    // ---- Holding things -------------------------------------------------

    /// Has at least one of a weapon a player can fire (or it's free and
    /// endless). Only-a-child weapons like the BMM Child never count.
    pub fn owns(&self, tank: usize, weapon: usize) -> bool {
        let w = &self.arsenal.weapons[weapon];
        if w.indirect || w.hidden || w.state.null {
            return false;
        }
        w.infinite() || self.tanks[tank].stock[weapon] > 0
    }

    /// Count to show for a weapon; None when endless.
    pub fn weapon_count(&self, tank: usize, weapon: usize) -> Option<u32> {
        if self.cfg.free || self.arsenal.weapons[weapon].infinite() {
            None
        } else {
            Some(self.tanks[tank].stock[weapon])
        }
    }

    /// One fired: used up, and back to the Baby Missile when gone.
    pub(crate) fn use_weapon(&mut self, tank: usize, weapon: usize) {
        if self.cfg.free || self.arsenal.weapons[weapon].infinite() {
            return;
        }
        let t = &mut self.tanks[tank];
        t.stock[weapon] = t.stock[weapon].saturating_sub(1);
        if t.stock[weapon] == 0 {
            t.weapon = self.arsenal.default_weapon();
        }
    }

    /// Holds an accessory matching `f`.
    pub fn has(&self, tank: usize, f: impl Fn(&Accessory) -> bool) -> bool {
        self.accessories.items.iter().zip(&self.tanks[tank].kit).any(|(a, &n)| n > 0 && f(a))
    }

    /// How many of the accessories matching `f` a tank holds.
    pub fn count(&self, tank: usize, f: impl Fn(&Accessory) -> bool) -> u32 {
        self.accessories.items.iter().zip(&self.tanks[tank].kit).filter(|(a, _)| f(a)).map(|(_, &n)| n).sum()
    }

    /// Use up one of the accessories matching `f`. False if none.
    fn consume(&mut self, tank: usize, f: impl Fn(&Accessory) -> bool) -> bool {
        let Some(i) = (0..self.accessories.items.len()).find(|&i| self.tanks[tank].kit[i] > 0 && f(&self.accessories.items[i])) else {
            return false;
        };
        self.tanks[tank].kit[i] -= 1;
        true
    }

    // ---- Shields --------------------------------------------------------

    /// The best shield a tank holds (`sc_shield_find_best`): strongest,
    /// then force over standard over magnetic, then more repulsion.
    fn best_shield(&self, tank: usize) -> Option<usize> {
        let rank = |a: &Accessory| {
            let kind = match a.shield {
                Some(ShieldKind::Force) => 2,
                Some(ShieldKind::Standard) => 1,
                _ => 0,
            };
            (a.shielding, kind, a.repulsion as i64)
        };
        (0..self.accessories.items.len())
            .filter(|&i| self.tanks[tank].kit[i] > 0 && self.accessories.items[i].shield.is_some())
            .max_by_key(|&i| rank(&self.accessories.items[i]))
    }

    /// Raise the best shield held, replacing any up now
    /// (`sc_player_activate_best_shield`).
    pub fn raise_shield(&mut self) -> bool {
        if self.phase != Phase::Aim {
            return false;
        }
        self.raise_best(self.turn)
    }

    fn raise_best(&mut self, tank: usize) -> bool {
        let Some(i) = self.best_shield(tank) else { return false };
        let t = &mut self.tanks[tank];
        t.kit[i] -= 1;
        t.shield = Some(Shield { acc: i, life: self.accessories.items[i].shielding });
        info!("{} raises {}", t.name, self.accessories.items[i].name);
        true
    }

    /// Auto Defense brings a shield up as soon as one is gone and the
    /// tank is hit (`sc_player_activate_auto_shield`).
    pub(crate) fn auto_defense(&mut self, tank: usize) {
        if self.tanks[tank].shield.is_none() && self.has(tank, |a| a.auto_defense) {
            self.raise_best(tank);
        }
    }

    /// A Solar Panel recharges the shield a little each turn.
    pub(crate) fn solar(&mut self, tank: usize) {
        if !self.has(tank, |a| a.recharge) {
            return;
        }
        let items = &self.accessories.items;
        if let Some(sh) = self.tanks[tank].shield.as_mut() {
            sh.life = (sh.life + SHIELD_RECHARGE_RATE).min(items[sh.acc].shielding);
        }
    }

    /// The first tank whose shield a shell crosses into between where it
    /// was and (x, y). Magnetic shields only stop sappers; nobody's
    /// shield stops their own shots.
    pub(crate) fn shield_crossed(&self, s: &crate::sim::Shell, x: f64, y: f64, r: f64, sapper: bool) -> Option<usize> {
        (0..self.tanks.len()).find(|&j| {
            let t = &self.tanks[j];
            let Some(sh) = t.shield else { return false };
            if !t.alive || j == s.owner {
                return false;
            }
            if self.accessories.items[sh.acc].shield == Some(ShieldKind::Magnetic) && !sapper {
                return false;
            }
            let d2 = |px: f64, py: f64| {
                let (dx, dy) = (self.land.dx(t.x as f64, px), py - t.y as f64);
                dx * dx + dy * dy
            };
            d2(s.x, s.y) >= r * r && d2(x, y) < r * r
        })
    }

    /// A shell stopped by a shield (`sc_shield_absorb_hit`).
    pub(crate) fn absorb_hit(&mut self, tank: usize, sapper: bool) {
        let t = &mut self.tanks[tank];
        if let Some(sh) = t.shield.as_mut() {
            sh.life -= SHIELD_ABSORB_HIT * if sapper { SHIELD_SAPPER_RATE } else { 1 };
            if sh.life <= 0 {
                t.shield = None;
            }
        }
    }

    /// A blast against a shield: what gets through
    /// (`sc_shield_absorb_explosion`; napalm never burns through).
    pub(crate) fn absorb_damage(&mut self, tank: usize, damage: i32, napalm: bool) -> i32 {
        let t = &mut self.tanks[tank];
        let Some(sh) = t.shield.as_mut() else { return damage };
        sh.life -= damage;
        if sh.life > 0 {
            return 0;
        }
        let through = -sh.life;
        t.shield = None;
        if napalm { 0 } else { through }
    }

    /// Magnetic shields' push on a shell at (x, y), and what each would
    /// spend making it (`sc_shield_get_deflection`). Changes nothing.
    pub(crate) fn magnetic_push(&self, owner: usize, x: f64, y: f64) -> ((f64, f64), Vec<(usize, i32)>) {
        let (mut dvx, mut dvy) = (0.0, 0.0);
        let mut costs = Vec::new();
        for j in 0..self.tanks.len() {
            let t = &self.tanks[j];
            let Some(sh) = t.shield else { continue };
            let a = &self.accessories.items[sh.acc];
            if j == owner || !t.alive || a.shield != Some(ShieldKind::Magnetic) {
                continue;
            }
            let (dx, dy) = (self.land.dx(t.x as f64, x), y - t.y as f64);
            let d2 = dx * dx + dy * dy;
            let r = TANK_RADIUS as f64;
            if d2 >= MAG_MAX_DIST * MAG_MAX_DIST || d2 <= r * r {
                continue;
            }
            let force = a.repulsion * MAG_ATTENUATION * MAG_ATTENUATION / d2 * MAG_TO_POWER * VELOCITY_SCALE;
            let d = d2.sqrt();
            dvx += force * dx / d;
            dvy += force * dy / d;
            costs.push((j, (force / (VELOCITY_SCALE * MAG_TO_COST)).round() as i32));
        }
        ((dvx, dvy), costs)
    }

    /// Magnetic shields push a shell away, spending strength to do it.
    /// Returns the new velocity, if changed.
    pub(crate) fn deflect(&mut self, owner: usize, x: f64, y: f64, (vx, vy): (f64, f64)) -> Option<(f64, f64)> {
        let ((dvx, dvy), costs) = self.magnetic_push(owner, x, y);
        if costs.is_empty() {
            return None;
        }
        for (j, cost) in costs {
            let t = &mut self.tanks[j];
            if let Some(sh) = t.shield.as_mut() {
                sh.life -= cost;
                if sh.life <= 0 {
                    t.shield = None;
                }
            }
        }
        Some((vx + dvx, vy + dvy))
    }

    // ---- Other things ---------------------------------------------------

    /// A battery: a little life back (`sc_player_activate_battery`).
    pub fn use_battery(&mut self) -> bool {
        let i = self.turn;
        if self.phase != Phase::Aim || self.tanks[i].life >= crate::sim::MAX_LIFE || !self.consume(i, |a| a.battery) {
            return false;
        }
        let t = &mut self.tanks[i];
        t.life = (t.life + BATTERY_PERCENT * crate::sim::MAX_LIFE / 100).min(crate::sim::MAX_LIFE);
        true
    }

    /// Contact triggers on or off; only with some in stock.
    pub fn toggle_triggers(&mut self) {
        let i = self.turn;
        if self.phase == Phase::Aim {
            let on = !self.tanks[i].triggers && self.has(i, |a| a.contact_trigger);
            self.tanks[i].triggers = on;
        }
    }

    /// Spend a trigger on a shot if they're on (`sc_player_use_contact_trigger`).
    pub(crate) fn use_contact_trigger(&mut self, tank: usize) -> bool {
        if !self.tanks[tank].triggers {
            return false;
        }
        let used = self.has(tank, |a| a.contact_trigger && a.permanent) || self.consume(tank, |a| a.contact_trigger);
        if !self.has(tank, |a| a.contact_trigger) {
            self.tanks[tank].triggers = false;
        }
        used
    }

    /// Fuel left, opened and unopened.
    pub fn fuel(&self, tank: usize) -> u32 {
        let unopened: u32 =
            self.accessories.items.iter().zip(&self.tanks[tank].kit).filter(|(a, _)| a.is_fuel).map(|(a, &n)| a.fuel * n).sum();
        self.tanks[tank].fuel + unopened
    }

    /// Drive one unit left or right on fuel (`sc_player_move`): no
    /// climbing more than a small step, no driving into another tank.
    pub fn drive(&mut self, delta: i32) -> bool {
        let i = self.turn;
        if self.phase != Phase::Aim {
            return false;
        }
        let t = &self.tanks[i];
        let nx = t.x + delta;
        if !self.land.wrap && (nx < TANK_RADIUS || nx >= land::WIDTH as i32 - TANK_RADIUS) {
            return false;
        }
        let y = (nx - TANK_RADIUS..=nx + TANK_RADIUS).map(|x| self.land.top(x)).max().unwrap_or(0);
        if y > t.y + CLIMB_HEIGHT {
            return false;
        }
        let blocked = self.tanks.iter().enumerate().any(|(j, o)| j != i && o.alive && self.land.dx(o.x as f64, nx as f64).abs() < (2 * TANK_RADIUS) as f64);
        if blocked {
            return false;
        }
        // Open a fuel tank if the tank's own is dry.
        if self.tanks[i].fuel == 0 {
            let Some(k) = (0..self.accessories.items.len()).find(|&k| self.tanks[i].kit[k] > 0 && self.accessories.items[k].is_fuel) else {
                return false;
            };
            self.tanks[i].kit[k] -= 1;
            self.tanks[i].fuel += self.accessories.items[k].fuel;
        }
        let t = &mut self.tanks[i];
        t.fuel -= 1;
        t.x = self.land.column(nx).map_or(nx, |c| c as i32);
        t.y = y;
        true
    }

    /// Teleport (Torched's own; Xscorch never finished it): to a random
    /// spot on the ground, clear of other tanks. The turn goes on.
    pub fn teleport(&mut self) -> bool {
        let i = self.turn;
        if self.phase != Phase::Aim || !self.has(i, |a| a.teleporter) {
            return false;
        }
        let margin = 3 * TANK_RADIUS;
        for _ in 0..200 {
            let x = self.rng.random_range(margin..land::WIDTH as i32 - margin);
            let clear = self.tanks.iter().enumerate().all(|(j, o)| j == i || !o.alive || (o.x - x).abs() > 3 * TANK_RADIUS);
            if clear {
                self.consume(i, |a| a.teleporter);
                let y = (x - TANK_RADIUS..=x + TANK_RADIUS).map(|xx| self.land.top(xx)).max().unwrap_or(0);
                let t = &mut self.tanks[i];
                (t.x, t.y) = (x, y);
                info!("{} teleports to {x}", t.name);
                return true;
            }
        }
        false
    }
}

/// A bare price and bundle, for the pricing rules.
struct Priced(u32, u32);

impl Item for Priced {
    fn price(&self) -> u32 {
        self.0
    }
    fn bundle(&self) -> u32 {
        self.1
    }
}

/// Weapons and accessories both, for shop rows.
trait ItemRow {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn price(&self) -> u32;
    fn bundle(&self) -> u32;
}

impl ItemRow for crate::weapons::Weapon {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn price(&self) -> u32 {
        self.price
    }
    fn bundle(&self) -> u32 {
        self.bundle
    }
}

impl ItemRow for Accessory {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn price(&self) -> u32 {
        self.price
    }
    fn bundle(&self) -> u32 {
        self.bundle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::tests::{game_with, lob, run_turn, select};
    use crate::sim::{Config, MAX_LIFE};

    fn shop_game(players: usize) -> Game {
        game_with(players, |c| c.free = false)
    }

    fn kit(g: &mut Game, tank: usize, name: &str, n: u32) {
        let i = g.accessories.find(name).unwrap();
        g.tanks[tank].kit[i] = n;
    }

    /// Where a tracer with the shooter's aim comes down.
    fn landing(g: &Game) -> f64 {
        let mut probe = game_with(g.tanks.len(), |c| *c = Config { seed: g.cfg.seed, ..g.cfg.clone() });
        probe.cfg.free = true;
        probe.land = g.land.clone();
        probe.tanks = g.tanks.clone();
        probe.turn = g.turn;
        for t in &mut probe.tanks {
            t.shield = None;
        }
        select(&mut probe, "Tracer");
        run_turn(&mut probe);
        probe.tanks[g.turn].last_trail.last().unwrap().0
    }

    /// Shooter at 200 lobbing, the other tank moved to where it lands.
    fn duel(g: &mut Game, weapon: &str) -> usize {
        lob(g, "Tracer");
        let me = g.turn;
        let other = 1 - me;
        g.tanks[other].x = 800;
        let x = landing(g);
        g.tanks[other].x = x.round() as i32;
        select(g, weapon);
        other
    }

    #[test]
    fn the_shop_sells_by_the_bundle_within_the_arms_level() {
        let mut g = shop_game(2);
        assert_eq!(g.phase, Phase::Shop);
        let missile = g.aisle(Aisle::Weapons).iter().position(|&i| g.arsenal.weapons[i].name == "Missile").unwrap();
        g.shop.cursor = missile;
        g.shop_buy();
        let mi = g.arsenal.find("Missile").unwrap();
        assert_eq!(g.tanks[0].stock[mi], 5);
        assert_eq!(g.tanks[0].money, 100_000 - 1875);
        g.shop_sell();
        assert_eq!(g.tanks[0].stock[mi], 0);
        assert_eq!(g.tanks[0].money, 100_000 - 1875 + 937);

        let low = game_with(2, |c| {
            c.free = false;
            c.arms_level = 0;
        });
        let names: Vec<&str> = low.aisle(Aisle::Weapons).iter().map(|&i| low.arsenal.weapons[i].name.as_str()).collect();
        assert!(names.contains(&"Missile") && !names.contains(&"MIRV") && !names.contains(&"Baby Missile"));

        g.shop_done();
        assert_eq!(g.phase, Phase::Shop);
        g.shop_done();
        assert_eq!(g.phase, Phase::Aim);
    }

    #[test]
    fn weapons_run_out() {
        let mut g = shop_game(2);
        let mi = g.arsenal.find("Missile").unwrap();
        g.tanks.iter_mut().for_each(|t| t.stock[mi] = 1);
        g.shop_done();
        g.shop_done();
        let me = g.turn;
        g.tanks[me].weapon = mi;
        run_turn(&mut g);
        assert_eq!(g.tanks[me].stock[mi], 0);
        assert_eq!(g.tanks[me].weapon, g.arsenal.default_weapon());
    }

    #[test]
    fn a_standard_shield_stops_a_shell() {
        let mut g = game_with(2, |_| {});
        let other = duel(&mut g, "Missile");
        kit(&mut g, other, "Shield", 1);
        g.turn = other;
        assert!(g.raise_shield());
        g.turn = 1 - other;
        run_turn(&mut g);
        assert_eq!(g.tanks[other].life, MAX_LIFE);
        assert_eq!(g.tanks[other].shield.unwrap().life, 8000 - 700);
    }

    #[test]
    fn a_sapper_drains_twice_as_much() {
        let mut g = game_with(2, |_| {});
        let other = duel(&mut g, "Shield Sapper");
        kit(&mut g, other, "Baby Shield", 1);
        g.turn = other;
        g.raise_shield();
        g.turn = 1 - other;
        run_turn(&mut g);
        assert_eq!(g.tanks[other].shield.unwrap().life, 4000 - 1400);
    }

    #[test]
    fn a_force_shield_bounces_a_shell_away() {
        let mut g = game_with(2, |_| {});
        let other = duel(&mut g, "Tracer");
        kit(&mut g, other, "Force", 1);
        g.turn = other;
        g.raise_shield();
        g.turn = 1 - other;
        run_turn(&mut g);
        assert_eq!(g.tanks[other].shield.unwrap().life, 8000 - 700);
        // The tracer came down somewhere else.
        let end = g.tanks[1 - other].last_trail.last().unwrap().0;
        assert!((end - g.tanks[other].x as f64).abs() > 12.0, "landed at {end}");
    }

    #[test]
    fn a_magnetic_shield_pushes_a_shell_aside() {
        let mut g = game_with(2, |_| {});
        let other = duel(&mut g, "Tracer");
        kit(&mut g, other, "Heavy Magnetic", 1);
        g.turn = other;
        g.raise_shield();
        g.turn = 1 - other;
        run_turn(&mut g);
        let end = g.tanks[1 - other].last_trail.last().unwrap().0;
        assert!((end - g.tanks[other].x as f64).abs() > 10.0, "landed at {end}");
        assert!(g.tanks[other].shield.map_or(0, |s| s.life) < 16000);
    }

    #[test]
    fn shields_soak_up_blasts_but_napalm_never_burns_through() {
        let mut g = game_with(2, |_| {});
        g.tanks[1].shield = Some(Shield { acc: g.accessories.find("Baby Shield").unwrap(), life: 100 });
        assert_eq!(g.absorb_damage(1, 300, false), 200);
        assert!(g.tanks[1].shield.is_none());
        g.tanks[1].shield = Some(Shield { acc: 0, life: 100 });
        assert_eq!(g.absorb_damage(1, 300, true), 0);
    }

    #[test]
    fn a_triple_turret_fires_three() {
        let mut g = game_with(2, |_| {});
        lob(&mut g, "Missile");
        let me = g.turn;
        kit(&mut g, me, "Triple Turret", 1);
        g.fire();
        assert_eq!(g.shells.len(), 3);
        // MIRVs aren't triple weapons.
        let mut g = game_with(2, |_| {});
        lob(&mut g, "MIRV");
        let me = g.turn;
        kit(&mut g, me, "Triple Turret", 1);
        g.fire();
        assert_eq!(g.shells.len(), 1);
    }

    #[test]
    fn tunnelling_goes_deep_unless_triggered() {
        let blast_y = |trigger: bool| {
            let mut g = game_with(2, |c| c.tunneling = true);
            lob(&mut g, "Missile");
            let me = g.turn;
            g.tanks[me].angle = 70;
            g.tanks[me].power = 700;
            if trigger {
                kit(&mut g, me, "Contact Trigger", 5);
                g.toggle_triggers();
                assert!(g.tanks[me].triggers);
            }
            g.fire();
            while g.blasts.is_empty() {
                g.tick();
            }
            g.blasts[0].y
        };
        let (deep, shallow) = (blast_y(false), blast_y(true));
        assert!(shallow > 97.0, "triggered went off at {shallow}");
        assert!(deep < shallow - 5.0, "tunnelled to {deep}");
    }

    #[test]
    fn a_parachute_saves_a_fall() {
        for chute in [false, true] {
            let mut g = game_with(2, |_| {});
            lob(&mut g, "Tracer");
            let other = 1 - g.turn;
            if chute {
                kit(&mut g, other, "Parachute", 3);
            }
            // Dig the ground away under the other tank.
            let x = g.tanks[other].x;
            for cx in x - 30..x + 30 {
                for y in 40..100 {
                    g.land.set(cx, y, false);
                }
            }
            g.phase = Phase::Settle;
            while g.phase == Phase::Settle {
                g.tick();
            }
            let pi = g.accessories.find("Parachute").unwrap();
            if chute {
                assert_eq!(g.tanks[other].life, MAX_LIFE);
                assert_eq!(g.tanks[other].kit[pi], 2);
            } else {
                assert!(g.tanks[other].life < MAX_LIFE);
            }
        }
    }

    #[test]
    fn money_for_hits_kills_and_surviving() {
        let mut g = game_with(2, |_| {});
        let other = duel(&mut g, "Nuke");
        let me = 1 - other;
        let (m0, o0) = (g.tanks[me].money, g.tanks[other].money);
        run_turn(&mut g);
        assert!(!g.tanks[other].alive);
        let sc = g.scoring.clone();
        // One hit and the kill, the round, and maybe the death blast's
        // splash on the shooter.
        let got = g.tanks[me].money - m0;
        assert!(got >= sc.damage_bonus + sc.kill_bonus + sc.survival_bonus - sc.damage_loss, "shooter got {got}");
        assert_eq!(g.tanks[other].money - o0, -(sc.damage_loss + sc.death_loss));
    }

    #[test]
    fn everyone_aims_then_all_fire() {
        let mut g = game_with(3, |c| c.simultaneous = true);
        let first = g.turn;
        g.fire();
        assert_eq!(g.phase, Phase::Aim);
        assert_ne!(g.turn, first);
        g.fire();
        assert_eq!(g.phase, Phase::Aim);
        g.fire();
        assert_eq!(g.phase, Phase::Action);
        assert_eq!(g.shells.len(), 3);
    }

    #[test]
    fn round_robin_rotates_and_interest_pays() {
        let mut g = game_with(3, |c| {
            c.order = Order::RoundRobin;
            c.rounds = 5;
        });
        g.round = 2;
        let before = g.order.clone();
        g.order_round();
        assert_eq!(g.order, [before[1], before[2], before[0]]);
        let m = g.tanks[0].money;
        g.pay_interest();
        assert_eq!(g.tanks[0].money, (m as f64 * 1.05) as i64);
    }

    #[test]
    fn drive_on_fuel_and_teleport() {
        let mut g = game_with(2, |_| {});
        lob(&mut g, "Tracer");
        let me = g.turn;
        let x = g.tanks[me].x;
        assert!(!g.drive(1), "no fuel");
        kit(&mut g, me, "Fuel Tank", 1);
        for _ in 0..10 {
            assert!(g.drive(1));
        }
        assert!(!g.drive(1), "ten units to a tank");
        assert_eq!(g.tanks[me].x, x + 10);
        kit(&mut g, me, "Teleporter", 1);
        assert!(g.teleport());
        assert!(!g.teleport());
    }

    #[test]
    fn a_shopping_game_plays_through() {
        let mut g = game_with(2, |c| {
            c.free = false;
            c.rounds = 2;
        });
        for _ in 0..500 {
            match g.phase {
                Phase::Shop => {
                    // Buy the first thing affordable, then move on.
                    g.shop_buy();
                    g.shop_done();
                }
                Phase::Aim => {
                    let me = g.turn;
                    g.tanks[me].angle = 90;
                    g.tanks[me].power = 300;
                    run_turn(&mut g);
                }
                Phase::RoundOver => g.advance(),
                Phase::GameOver => return,
                p => panic!("stuck in {p:?}"),
            }
        }
        panic!("never ended");
    }
}
