//! Accessories, scorings, money and the shop's prices (`seconomy.c`,
//! `sinventory.c`, `saccessory.c`).

use crate::defs;

pub const DEFAULT_ACCESSORIES: &str = include_str!("../data/accessories.def");
pub const DEFAULT_SCORINGS: &str = include_str!("../data/scorings.def");

/// Most of one thing a player can hold (`SC_INVENTORY_MAX_ITEMS`).
pub const MAX_ITEMS: u32 = 99;
/// Starting cash (`SC_ECONOMY_DEF_CASH`), interest per round.
pub const DEF_CASH: i64 = 100_000;
pub const MAX_CASH: i64 = 1_000_000;
pub const DEF_INTEREST: f64 = 0.05;
pub const MAX_INTEREST: f64 = 0.30;
/// A broken bundle costs this much more per item; selling pays this share.
const SPLIT_MARKUP: f64 = 1.30;
const SELL_MARKUP: f64 = 0.50;
/// Arms levels (`SC_ARMS_LEVEL_DEF`, `_MAX`). Weapons above the level
/// aren't sold.
pub const ARMS_LEVEL_DEF: u32 = 4;
pub const ARMS_LEVEL_MAX: u32 = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShieldKind {
    #[default]
    Standard,
    Magnetic,
    Force,
}

#[derive(Clone, Debug, Default)]
pub struct Accessory {
    pub name: String,
    pub description: String,
    pub hidden: bool,
    pub arms_level: u32,
    pub price: u32,
    pub bundle: u32,
    /// Shield strength, in damage points (a tank has 1000 life).
    pub shielding: i32,
    pub repulsion: f64,
    pub fuel: u32,
    pub null: bool,
    pub consumable: bool,
    pub permanent: bool,
    pub shield: Option<ShieldKind>,
    pub is_fuel: bool,
    pub triple_turret: bool,
    pub auto_defense: bool,
    pub recharge: bool,
    pub contact_trigger: bool,
    pub battery: bool,
    pub teleporter: bool,
    pub parachute: bool,
}

#[derive(Debug)]
pub struct Accessories {
    pub items: Vec<Accessory>,
}

impl Accessories {
    pub fn parse(src: &str) -> Result<Accessories, String> {
        let mut items = Vec::new();
        for b in defs::parse(src, "accessories_class")? {
            items.push(build_accessory(&b).map_err(|e| format!("line {}: {e}", b.line))?);
        }
        Ok(Accessories { items })
    }

    #[cfg(test)]
    pub fn find(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|a| a.name == name)
    }

    /// Those for sale, and so worth listing.
    pub fn listed(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| !self.items[i].hidden && !self.items[i].null).collect()
    }
}

fn build_accessory(b: &defs::Block) -> Result<Accessory, String> {
    let (name, hidden) = b.name("accessoryName")?;
    let mut a = Accessory {
        name,
        hidden,
        description: b.text("description").unwrap_or_default().to_string(),
        arms_level: b.num("armsLevel")? as u32,
        price: b.num("price")? as u32,
        bundle: b.num("bundle")? as u32,
        shielding: b.num("shielding")? as i32,
        repulsion: b.num("repulsion")?,
        fuel: b.num("fuel")? as u32,
        ..Accessory::default()
    };
    let mut shield = false;
    let mut kind = ShieldKind::Standard;
    for f in b.list("stateFlags") {
        match f.as_str() {
            "null" => a.null = true,
            "consumable" => a.consumable = true,
            "permanent" => a.permanent = true,
            "shield" => shield = true,
            "standard" => kind = ShieldKind::Standard,
            "magnetic" => kind = ShieldKind::Magnetic,
            "force" => kind = ShieldKind::Force,
            "fuel" => a.is_fuel = true,
            "triple_turret" => a.triple_turret = true,
            "auto_defense" => a.auto_defense = true,
            "recharge" => a.recharge = true,
            "contact_trigger" => a.contact_trigger = true,
            "battery" => a.battery = true,
            "teleporter" => a.teleporter = true,
            "parachute" => a.parachute = true,
            other => return Err(format!("{}: unknown stateFlag '{other}'", b.ident)),
        }
    }
    if shield {
        a.shield = Some(kind);
    }
    Ok(a)
}

/// How money is won and lost (`scorings.def`).
#[derive(Clone, Debug, Default)]
pub struct Scoring {
    pub name: String,
    pub description: String,
    pub survival_bonus: i64,
    pub damage_bonus: i64,
    pub kill_bonus: i64,
    pub damage_loss: i64,
    pub death_loss: i64,
    pub suicide_loss: i64,
    /// Not scaled by the starting cash.
    pub fixed: bool,
    /// Torched's own, for Greedy: share of unused stock's resale value
    /// paid each round.
    pub stock_rate: f64,
}

impl Scoring {
    pub fn parse_all(src: &str) -> Result<Vec<Scoring>, String> {
        defs::parse(src, "scorings_class")?
            .iter()
            .map(|b| {
                Ok(Scoring {
                    name: b.name("scoringName")?.0,
                    description: b.text("description").unwrap_or_default().to_string(),
                    survival_bonus: b.num("survivalBonus")? as i64,
                    damage_bonus: b.num("damageBonus")? as i64,
                    kill_bonus: b.num("killBonus")? as i64,
                    damage_loss: b.num("damageLoss")? as i64,
                    death_loss: b.num("deathLoss")? as i64,
                    suicide_loss: b.num("suicideLoss")? as i64,
                    fixed: b.flag("fixed"),
                    stock_rate: b.num("stockRate")?,
                })
            })
            .collect()
    }

    /// Scaled to the starting cash, as `sc_economy_init` does.
    pub fn scaled(&self, initial_cash: i64) -> Scoring {
        if self.fixed {
            return self.clone();
        }
        let r = initial_cash as f64 / DEF_CASH as f64;
        let k = |v: i64| (v as f64 * r) as i64;
        Scoring {
            survival_bonus: k(self.survival_bonus),
            damage_bonus: k(self.damage_bonus),
            kill_bonus: k(self.kill_bonus),
            damage_loss: k(self.damage_loss),
            death_loss: k(self.death_loss),
            suicide_loss: k(self.suicide_loss),
            ..self.clone()
        }
    }
}

/// Something with a price and a bundle size.
pub trait Item {
    fn price(&self) -> u32;
    fn bundle(&self) -> u32;
    /// Free and endless, like the Baby Missile (`SC_WEAPON_IS_INFINITE`).
    fn infinite(&self) -> bool {
        self.bundle() == 0 || self.price() == 0
    }
}

impl Item for Accessory {
    fn price(&self) -> u32 {
        self.price
    }
    fn bundle(&self) -> u32 {
        self.bundle
    }
}

impl Item for crate::weapons::Weapon {
    fn price(&self) -> u32 {
        self.price
    }
    fn bundle(&self) -> u32 {
        self.bundle
    }
}

/// How many a purchase adds: a bundle, or what fits under the limit.
pub fn buy_quantity(item: &impl Item, have: u32) -> u32 {
    item.bundle().min(MAX_ITEMS.saturating_sub(have))
}

/// What a purchase of `quantity` costs; a broken bundle costs more.
pub fn buy_price(item: &impl Item, quantity: u32) -> i64 {
    if quantity < item.bundle() {
        let unit = item.price() / item.bundle();
        (unit as f64 * quantity as f64 * SPLIT_MARKUP) as i64
    } else {
        item.price() as i64
    }
}

/// What selling up to a bundle of what you have pays.
pub fn sell(item: &impl Item, have: u32) -> (u32, i64) {
    let quantity = have.min(item.bundle());
    let pay = if quantity < item.bundle() {
        let unit = item.price() / item.bundle();
        (unit as f64 * quantity as f64 * SELL_MARKUP) as i64
    } else {
        (item.price() as f64 * SELL_MARKUP) as i64
    };
    (quantity, pay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_files_load() {
        let a = Accessories::parse(DEFAULT_ACCESSORIES).unwrap();
        assert_eq!(a.listed().len(), 17);
        let force = &a.items[a.find("Heavy Force").unwrap()];
        assert_eq!(force.shield, Some(ShieldKind::Force));
        assert_eq!(force.shielding, 16000);
        let s = Scoring::parse_all(DEFAULT_SCORINGS).unwrap();
        assert_eq!(s.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Basic", "Standard", "Greedy"]);
        assert_eq!(s[1].scaled(200_000).kill_bonus, 60_000);
    }

    #[test]
    fn bundles_and_markups() {
        let a = Accessories::parse(DEFAULT_ACCESSORIES).unwrap();
        let fuel = &a.items[a.find("Fuel Tank").unwrap()]; // 1000 for 10
        assert_eq!(buy_quantity(fuel, 0), 10);
        assert_eq!(buy_price(fuel, 10), 1000);
        assert_eq!(buy_quantity(fuel, 95), 4);
        assert_eq!(buy_price(fuel, 4), 520);
        assert_eq!(sell(fuel, 10), (10, 500));
        assert_eq!(sell(fuel, 3), (3, 150));
    }
}
