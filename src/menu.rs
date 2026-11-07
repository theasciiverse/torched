//! The title screen, with computers playing behind it, and the options
//! screen, whose settings are remembered between runs.

use std::{fs, path::PathBuf};

use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::ai::AiKind;
use crate::economy;
use crate::fx::Sky;
use crate::land::{self, Generator};
use crate::physics::{self, Walls};
use crate::sim::{Config, MAX_TANKS, Order};

/// Everything the options screen sets, as saved.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub people: usize,
    pub computers: Vec<String>,
    pub rounds: u32,
    pub terrain: String,
    pub bumpiness: f64,
    pub sky: String,
    pub walls: String,
    pub wind: f64,
    pub changing_wind: bool,
    pub air: f64,
    pub scoring: String,
    pub cash: i64,
    pub interest: f64,
    pub arms: u32,
    pub order: String,
    pub together: bool,
    pub tunneling: bool,
    pub free: bool,
    pub sound: bool,
}

impl Default for Options {
    fn default() -> Options {
        let c = Config::default();
        Options {
            people: 1,
            computers: vec!["Chooser".into()],
            rounds: c.rounds,
            terrain: "Random".into(),
            bumpiness: c.bumpiness,
            sky: c.sky.name().into(),
            walls: c.walls.name().into(),
            wind: c.max_wind,
            changing_wind: c.dynamic_wind,
            air: c.viscosity,
            scoring: c.scoring,
            cash: c.cash,
            interest: c.interest,
            arms: c.arms_level,
            order: c.order.name().into(),
            together: c.simultaneous,
            tunneling: c.tunneling,
            free: c.free,
            sound: true,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("torched").join("options.toml"))
}

impl Options {
    pub fn load() -> Options {
        let Some(p) = config_path() else { return Options::default() };
        match fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                warn!("{}: {e}; using defaults", p.display());
                Options::default()
            }),
            Err(_) => Options::default(),
        }
    }

    pub fn save(&self) {
        let Some(p) = config_path() else { return };
        let write = || -> Result<(), String> {
            if let Some(d) = p.parent() {
                fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            fs::write(&p, toml::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        };
        match write() {
            Ok(()) => info!("options saved to {}", p.display()),
            Err(e) => warn!("couldn't save options: {e}"),
        }
    }

    pub fn kinds(&self) -> Vec<AiKind> {
        self.computers.iter().filter_map(|n| AiKind::parse(n)).collect()
    }

    pub fn tanks(&self) -> usize {
        self.people + self.kinds().len()
    }

    pub fn to_config(&self, seed: u64) -> Config {
        let d = Config::default();
        Config {
            players: self.people,
            ai: self.kinds(),
            rounds: self.rounds.max(1),
            generator: Generator::parse(&self.terrain),
            bumpiness: self.bumpiness,
            seed,
            walls: Walls::parse(&self.walls).unwrap_or(d.walls),
            max_wind: self.wind.clamp(0.0, physics::WIND_MAX),
            dynamic_wind: self.changing_wind,
            viscosity: self.air.clamp(0.0, physics::VISCOSITY_MAX),
            free: self.free,
            cash: self.cash.clamp(0, economy::MAX_CASH),
            interest: self.interest.clamp(0.0, economy::MAX_INTEREST),
            dynamic_interest: false,
            scoring: self.scoring.clone(),
            arms_level: self.arms.min(economy::ARMS_LEVEL_MAX),
            order: Order::parse(&self.order).unwrap_or(d.order),
            simultaneous: self.together,
            tunneling: self.tunneling,
            sky: Sky::parse(&self.sky).unwrap_or(d.sky),
        }
    }

    /// A line saying what game Play would start.
    pub fn summary(&self) -> String {
        let people = match self.people {
            0 => "No people".to_string(),
            1 => "1 person".to_string(),
            n => format!("{n} people"),
        };
        let comps = match self.kinds().len() {
            0 => String::new(),
            1 => format!(", 1 computer ({})", self.computers[0]),
            n => format!(", {n} computers"),
        };
        format!(
            "{people}{comps} · {} rounds · {} terrain · {} walls{}",
            self.rounds,
            self.terrain,
            self.walls,
            if self.free { " · free arsenal" } else { "" }
        )
    }

    // ---- The options screen's rows --------------------------------------

    /// Rows: a label and the value as shown.
    pub fn rows(&self) -> Vec<(String, String)> {
        let yes = |b: bool| if b { "yes" } else { "no" }.to_string();
        let mut r = vec![
            ("People at this keyboard".to_string(), self.people.to_string()),
            ("Computer players".to_string(), self.computers.len().to_string()),
        ];
        for (i, c) in self.computers.iter().enumerate() {
            r.push((format!("  Computer {}", i + 1), c.clone()));
        }
        r.extend([
            ("Rounds".to_string(), self.rounds.to_string()),
            ("Terrain".to_string(), self.terrain.clone()),
            ("Bumpiness".to_string(), format!("{:.0}", self.bumpiness)),
            ("Sky".to_string(), self.sky.clone()),
            ("Walls".to_string(), self.walls.clone()),
            ("Strongest wind".to_string(), format!("{:.0}", self.wind * 1000.0)),
            ("Wind changes each turn".to_string(), yes(self.changing_wind)),
            ("Air drag".to_string(), format!("{:.3}", self.air)),
            ("Scoring".to_string(), self.scoring.clone()),
            ("Starting cash".to_string(), format!("${}", self.cash)),
            ("Interest per round".to_string(), format!("{:.0}%", self.interest * 100.0)),
            ("Arms level".to_string(), self.arms.to_string()),
            ("Turn order".to_string(), self.order.clone()),
            ("Everyone fires at once".to_string(), yes(self.together)),
            ("Tunnelling".to_string(), yes(self.tunneling)),
            ("Free arsenal, no shop".to_string(), yes(self.free)),
            ("Sound".to_string(), yes(self.sound)),
            ("Back".to_string(), String::new()),
        ]);
        r
    }

    /// Change row `i` by one step either way.
    pub fn change(&mut self, i: usize, dir: i32) {
        let n = self.computers.len();
        let cycle = |list: &[&str], cur: &str, dir: i32| -> String {
            let at = list.iter().position(|x| x.eq_ignore_ascii_case(cur)).unwrap_or(0) as i32;
            list[(at + dir).rem_euclid(list.len() as i32) as usize].to_string()
        };
        let room = MAX_TANKS;
        match i {
            0 => self.people = (self.people as i32 + dir).clamp(0, (room - n) as i32) as usize,
            1 => {
                let want = (n as i32 + dir).clamp(0, (room - self.people) as i32) as usize;
                self.computers.resize(want, "Chooser".into());
            }
            _ if i >= 2 && i < 2 + n => {
                let names: Vec<&str> = AiKind::ALL.iter().map(|k| k.name()).collect();
                self.computers[i - 2] = cycle(&names, &self.computers[i - 2], dir);
            }
            _ => {
                let j = i - 2 - n;
                let step = |v: f64, d: f64, lo: f64, hi: f64| (v + d * dir as f64).clamp(lo, hi);
                match j {
                    0 => self.rounds = (self.rounds as i32 + dir).clamp(1, 100) as u32,
                    1 => {
                        let mut names = vec!["Random"];
                        names.extend(Generator::ALL.iter().map(|g| g.name()));
                        self.terrain = cycle(&names, &self.terrain, dir);
                    }
                    2 => self.bumpiness = step(self.bumpiness, 10.0, 0.0, land::BUMPINESS_MAX),
                    3 => self.sky = cycle(&Sky::ALL.map(|s| s.name()), &self.sky, dir),
                    4 => self.walls = cycle(&Walls::ALL.map(|w| w.name()), &self.walls, dir),
                    5 => self.wind = (step(self.wind, 0.01, 0.0, 0.3) * 100.0).round() / 100.0,
                    6 => self.changing_wind = !self.changing_wind,
                    7 => self.air = (step(self.air, 0.005, 0.0, physics::VISCOSITY_MAX) * 1000.0).round() / 1000.0,
                    8 => self.scoring = cycle(&["Basic", "Standard", "Greedy"], &self.scoring, dir),
                    9 => self.cash = (self.cash + 10_000 * dir as i64).clamp(0, economy::MAX_CASH),
                    10 => self.interest = (step(self.interest, 0.01, 0.0, economy::MAX_INTEREST) * 100.0).round() / 100.0,
                    11 => self.arms = (self.arms as i32 + dir).clamp(0, economy::ARMS_LEVEL_MAX as i32) as u32,
                    12 => self.order = cycle(&Order::ALL.map(|o| o.name()), &self.order, dir),
                    13 => self.together = !self.together,
                    14 => self.tunneling = !self.tunneling,
                    15 => self.free = !self.free,
                    16 => self.sound = !self.sound,
                    _ => {}
                }
            }
        }
    }

    /// Whether row `i` is the Back row.
    pub fn is_back(&self, i: usize) -> bool {
        i + 1 == self.rows().len()
    }
}

/// The background game behind the title: sharp computers, no sound.
pub fn title_game_config(seed: u64) -> Config {
    Config {
        players: 0,
        ai: vec![AiKind::Calculater, AiKind::Annihilater, AiKind::Insanity, AiKind::Chooser],
        rounds: 1000,
        seed,
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_and_make_valid_games() {
        let o = Options { computers: vec!["Insanity".into(), "Moron".into()], walls: "Rubber".into(), ..Options::default() };
        let s = toml::to_string_pretty(&o).unwrap();
        let back: Options = toml::from_str(&s).unwrap();
        let c = back.to_config(1);
        assert_eq!(c.ai, vec![AiKind::Insanity, AiKind::Moron]);
        assert_eq!(c.walls, Walls::Rubber);
        // Old files missing fields still load.
        let partial: Options = toml::from_str("people = 3\n").unwrap();
        assert_eq!(partial.people, 3);
        assert_eq!(partial.rounds, 10);
    }

    #[test]
    fn every_row_changes_without_panicking() {
        let mut o = Options::default();
        for i in 0..o.rows().len() {
            for dir in [1, -1, 1, 1] {
                o.change(i, dir);
            }
        }
        assert!(o.tanks() <= MAX_TANKS);
        let _ = o.to_config(0);
    }
}
