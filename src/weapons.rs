//! Weapons, read from a data file in Xscorch's `weapons.def` format.

use std::collections::HashMap;

use crate::defs;

/// The built-in arsenal.
pub const DEFAULT_DEF: &str = include_str!("../data/weapons.def");

/// How a weapon behaves, from `stateFlags` (`sweapon.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub null: bool,
    pub smoke: bool,
    pub plasma: bool,
    pub liquid: bool,
    pub dirt: bool,
    pub riot: bool,
    pub triple: bool,
    pub roller: bool,
    pub digger: bool,
    pub sapper: bool,
}

/// When and how a weapon changes in flight, from `phoenixFlags`
/// (`sphoenix.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Phoenix {
    pub at_rand: bool,
    pub at_tank: bool,
    pub at_apex: bool,
    pub at_land: bool,
    pub chain: bool,
    pub spider: bool,
    pub scatter: bool,
    pub convert: bool,
    pub detonate: bool,
}

// Price, bundle, arms level and the AI's "useless" are for the shop and
// the computer players (0.3.0, 0.4.0).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Weapon {
    pub ident: String,
    pub name: String,
    pub description: String,
    pub hidden: bool,
    pub arms_level: u32,
    pub price: u32,
    pub bundle: u32,
    pub radius: f64,
    pub force: f64,
    pub liquid: usize,
    pub scatter: f64,
    pub children: u32,
    /// Degrees; nonzero makes a wedge-shaped blast.
    pub angular_width: f64,
    pub useless: bool,
    /// Only ever made by another weapon, never fired.
    pub indirect: bool,
    pub state: State,
    pub phoenix: Phoenix,
    /// Index of the child weapon, resolved after loading.
    pub child: Option<usize>,
    child_name: Option<String>,
    /// Torched's own, for diggers and sandhogs.
    pub dig_length: f64,
    pub tunnel_radius: f64,
}

impl Weapon {
    /// Has a warhead that does something on impact.
    pub fn explodes(&self) -> bool {
        self.radius > 0.0
    }

    pub fn is_napalm(&self) -> bool {
        self.state.liquid && !self.state.dirt
    }

    /// Changes in flight: needs a child to do anything but detonate.
    pub fn splits(&self) -> bool {
        self.child.is_some() && self.children > 0
    }
}

#[derive(Debug, Default)]
pub struct Arsenal {
    pub weapons: Vec<Weapon>,
    by_name: HashMap<String, usize>,
}

impl Arsenal {
    pub fn parse(src: &str) -> Result<Arsenal, String> {
        let mut weapons = Vec::new();
        for b in defs::parse(src, "weapons_class")? {
            weapons.push(build(&b).map_err(|e| format!("line {}: {e}", b.line))?);
        }
        let by_name: HashMap<String, usize> = weapons.iter().enumerate().map(|(i, w)| (w.name.clone(), i)).collect();
        for w in &mut weapons {
            if let Some(c) = &w.child_name {
                let j = *by_name.get(c).ok_or_else(|| format!("{}: unknown phoenixChild '{c}'", w.name))?;
                w.child = Some(j);
            }
        }
        let a = Arsenal { weapons, by_name };
        if a.find("Baby Missile").is_none() {
            return Err("the arsenal needs a Baby Missile".into());
        }
        a.check_cycles()?;
        Ok(a)
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    pub fn default_weapon(&self) -> usize {
        self.find("Baby Missile").expect("checked at load")
    }

    /// Weapons a player can pick: not hidden, not only a child, not empty.
    pub fn selectable(&self) -> Vec<usize> {
        (0..self.weapons.len())
            .filter(|&i| {
                let w = &self.weapons[i];
                !w.hidden && !w.indirect && !w.state.null
            })
            .collect()
    }

    /// A weapon whose children lead back to itself through anything but a
    /// chain or convert would never end (`sc_phoenix_verify`).
    fn check_cycles(&self) -> Result<(), String> {
        for start in 0..self.weapons.len() {
            let mut seen = vec![start];
            let mut cur = start;
            while let Some(c) = self.weapons[cur].child {
                let w = &self.weapons[cur];
                if w.phoenix.chain || w.phoenix.convert || !w.splits() {
                    break;
                }
                if seen.contains(&c) {
                    return Err(format!("{} splits into itself", self.weapons[start].name));
                }
                seen.push(c);
                cur = c;
            }
        }
        Ok(())
    }
}

fn build(b: &defs::Block) -> Result<Weapon, String> {
    let ident = b.ident.clone();
    let (name, hidden) = b.name("weaponName")?;
    let num = |k: &str| b.num(k);
    let flag = |k: &str| b.flag(k);
    let list = |k: &str| b.list(k);
    let mut state = State::default();
    for s in list("stateFlags") {
        match s.as_str() {
            "null" => state.null = true,
            "smoke" => state.smoke = true,
            "plasma" => state.plasma = true,
            "liquid" => state.liquid = true,
            "dirt" => state.dirt = true,
            "riot" => state.riot = true,
            "triple" => state.triple = true,
            "roller" => state.roller = true,
            "digger" => state.digger = true,
            "sapper" => state.sapper = true,
            "beam" | "defer" => {}
            other => return Err(format!("{ident}: unknown stateFlag '{other}'")),
        }
    }
    let mut phoenix = Phoenix::default();
    for s in list("phoenixFlags") {
        match s.as_str() {
            "at_rand" => phoenix.at_rand = true,
            "at_tank" => phoenix.at_tank = true,
            "at_apex" => phoenix.at_apex = true,
            "at_land" => phoenix.at_land = true,
            "chain" => phoenix.chain = true,
            "spider" => phoenix.spider = true,
            "scatter" => phoenix.scatter = true,
            "convert" => phoenix.convert = true,
            "detonate" => phoenix.detonate = true,
            other => return Err(format!("{ident}: unknown phoenixFlag '{other}'")),
        }
    }
    Ok(Weapon {
        description: b.text("description").unwrap_or_default().to_string(),
        hidden,
        arms_level: num("armsLevel")? as u32,
        price: num("price")? as u32,
        bundle: num("bundle")? as u32,
        radius: num("radius")?,
        force: num("force")?,
        liquid: num("liquid")? as usize,
        scatter: num("scatter")?,
        children: num("children")? as u32,
        angular_width: num("angularWidth")?,
        useless: flag("useless"),
        indirect: flag("indirect"),
        state,
        phoenix,
        child: None,
        child_name: b.text("phoenixChild").filter(|s| !s.is_empty()).map(str::to_string),
        dig_length: num("digLength")?,
        tunnel_radius: num("tunnelRadius")?,
        name,
        ident,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_arsenal_loads() {
        let a = Arsenal::parse(DEFAULT_DEF).unwrap();
        assert_eq!(a.weapons.len(), 39);
        let mirv = &a.weapons[a.find("MIRV").unwrap()];
        assert!(mirv.phoenix.scatter && mirv.phoenix.at_apex);
        assert_eq!(a.weapons[mirv.child.unwrap()].name, "Missile");
        assert_eq!(mirv.children, 5);
        let riot = &a.weapons[a.find("Riot Charge").unwrap()];
        assert_eq!(riot.angular_width, 60.0);
        // No Weapon and the BMM Child are never picked.
        let sel = a.selectable();
        assert!(!sel.contains(&a.find("BMM Child").unwrap()));
        assert!(!sel.contains(&a.find("No Weapon").unwrap()));
        assert!(sel.contains(&a.find("Heavy Sandhog").unwrap()));
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(Arsenal::parse("X = weapons_class {\n weaponName = \"X\";\n").is_err());
        let loop_def = "A = weapons_class {\n weaponName = \"Baby Missile\";\n children = 2;\n phoenixFlags = \"scatter, at_apex\";\n phoenixChild = \"Baby Missile\";\n};\n";
        assert!(Arsenal::parse(loop_def).unwrap_err().contains("itself"));
    }
}
