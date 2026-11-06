//! The ground: a bitmap at Xscorch's scale, its seven terrain generators
//! (`slscape.c`, `slscapetools.c`) and loose dirt falling (`sland.c`).
//!
//! Coordinates are Xscorch's: x to the right, y up, (0, 0) bottom left.

use rand::Rng;

/// Field size in world units (Xscorch `SC_DEF_FIELD_WIDTH/HEIGHT`).
pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 600;
/// Highest the generators build (`SC_DEF_MAX_HEIGHT`).
pub const MAX_HEIGHT: f64 = 550.0;

const BUMP_HEIGHT_IN_PIXELS: f64 = 30.0;
const BUMP_WIDTH_IN_PIXELS: f64 = 5.0;
/// Default bumpiness, 0 to 100 (`SC_LAND_BUMPINESS_DEF`).
pub const BUMPINESS_DEF: f64 = 50.0;

const TABLE: usize = 4096;
const LOOKUP: usize = 4096;
const PARAMS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generator {
    None,
    Canyon,
    DoubleMountain,
    Hillside,
    Mountain,
    Plains,
    Traditional,
    Valley,
}

impl Generator {
    /// The seven Xscorch offers by name, plus Valley which its code has.
    pub const ALL: [Generator; 8] = [
        Generator::None,
        Generator::Canyon,
        Generator::DoubleMountain,
        Generator::Hillside,
        Generator::Mountain,
        Generator::Plains,
        Generator::Traditional,
        Generator::Valley,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Generator::None => "None",
            Generator::Canyon => "Canyon",
            Generator::DoubleMountain => "Double Mountain",
            Generator::Hillside => "Hillside",
            Generator::Mountain => "Mountain",
            Generator::Plains => "Plains",
            Generator::Traditional => "Traditional",
            Generator::Valley => "Valley",
        }
    }

    pub fn parse(s: &str) -> Option<Generator> {
        let s = s.to_ascii_lowercase().replace(['-', '_', ' '], "");
        Generator::ALL
            .into_iter()
            .find(|g| g.name().to_ascii_lowercase().replace(' ', "") == s)
    }

    /// Random pick over every generator but None, as Xscorch's Random does
    /// in practice (it draws `game_lrand(SC_LAND_GEN_RANDOM)`, which can
    /// give None; flat ground makes a dull round, so it is left out here).
    pub fn random(rng: &mut impl Rng) -> Generator {
        Generator::ALL[rng.random_range(1..Generator::ALL.len())]
    }
}

/// Hermite noise tables, `slscapetools.c`.
struct Noise {
    p: Vec<f64>,
    r: Vec<f64>,
    p1: Vec<f64>,
    p2: Vec<f64>,
    r1: Vec<f64>,
    r2: Vec<f64>,
}

impl Noise {
    fn new(rng: &mut impl Rng) -> Noise {
        let p = (0..TABLE).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
        let r = (0..TABLE).map(|_| rng.random::<f64>() * 8.0 - 4.0).collect();
        let (mut p1, mut p2, mut r1, mut r2) = (vec![0.0; LOOKUP], vec![0.0; LOOKUP], vec![0.0; LOOKUP], vec![0.0; LOOKUP]);
        for i in 0..LOOKUP {
            let t = i as f64 / (LOOKUP as f64 - 1.0);
            let (t2, t3) = (t * t, t * t * t);
            p1[i] = 2.0 * t3 - 3.0 * t2 + 1.0;
            p2[i] = -2.0 * t3 + 3.0 * t2;
            r1[i] = t3 - 2.0 * t2 + t;
            r2[i] = t3 - t2;
        }
        Noise { p, r, p1, p2, r1, r2 }
    }

    fn lookup(t: f64) -> usize {
        ((t * (LOOKUP as f64 - 1.0)) as usize).min(LOOKUP - 1)
    }

    fn indices(offset: u32, period: Option<u32>, x: f64) -> (usize, usize, f64) {
        let t = x - x.floor();
        let xi = x as i64;
        match period {
            Some(per) => {
                let lo = xi.rem_euclid(per as i64);
                let hi = (lo + 1) % per as i64;
                let o = offset as i64;
                (((lo + o) % TABLE as i64) as usize, ((hi + o) % TABLE as i64) as usize, t)
            }
            None => {
                let lo = ((xi + offset as i64).rem_euclid(TABLE as i64)) as usize;
                (lo, (lo + 1) % TABLE, t)
            }
        }
    }

    fn pnoise(&self, offset: u32, period: Option<u32>, x: f64) -> f64 {
        let (lo, hi, t) = Self::indices(offset, period, x);
        let i = Self::lookup(t);
        self.p[lo] * self.p1[i] + self.p[hi] * self.p2[i]
    }

    fn rnoise(&self, offset: u32, period: Option<u32>, x: f64) -> f64 {
        let (lo, hi, t) = Self::indices(offset, period, x);
        let i = Self::lookup(t);
        self.r[lo] * self.r1[i] + self.r[hi] * self.r2[i]
    }

    fn smoothstep(&self, a: f64, b: f64, x: f64) -> f64 {
        if x > a && x > b {
            1.0
        } else if x < a && x < b {
            0.0
        } else {
            let i = Self::lookup((x - a) / (b - a));
            self.p2[i]
        }
    }
}

/// A landscape profile, `slscape.c`: maps 0..1 across to 0..1 up.
struct Profile {
    generator: Generator,
    noise: Noise,
    bump_height: f64,
    bump_freq: f64,
    ri: [u32; PARAMS],
    rd: [f64; PARAMS],
}

impl Profile {
    fn new(generator: Generator, bumpiness: f64, rng: &mut impl Rng) -> Profile {
        let noise = Noise::new(rng);
        let bump_height = BUMP_HEIGHT_IN_PIXELS / (2.0 * MAX_HEIGHT) * (bumpiness / 100.0).clamp(0.0, 1.0);
        let bump_freq = (WIDTH as f64 / BUMP_WIDTH_IN_PIXELS).floor();
        let mut ri = [0u32; PARAMS];
        let mut rd = [0.0; PARAMS];
        for i in 0..PARAMS {
            ri[i] = rng.random_range(0..TABLE as u32);
            rd[i] = rng.random();
        }
        Profile { generator, noise, bump_height, bump_freq, ri, rd }
    }

    fn eval(&self, x: f64) -> f64 {
        let n = &self.noise;
        let (ri, rd, bh, bf) = (&self.ri, &self.rd, self.bump_height, self.bump_freq);
        let bfp = Some(bf as u32);
        let x = x.clamp(0.0, 1.0);
        let y = match self.generator {
            Generator::None => 0.0,
            Generator::Canyon => {
                let delta = 1.0 - (0.2 + 2.0 * bh);
                let mut y = n.smoothstep(-0.25, 0.25, n.pnoise(ri[0], Some(7), 7.0 * x + rd[0])) * delta;
                y += n.pnoise(ri[1], Some(7), 7.0 * x + rd[1]) * 0.1;
                y += n.rnoise(ri[2], bfp, bf * x + rd[2]) * bh;
                y + bh + 0.1
            }
            Generator::Hillside => {
                let alpha = if rd[0] > 0.5 { 0.1 + bh } else { 0.9 - bh };
                let beta = 1.0 - 2.0 * alpha;
                let mut y = alpha + beta * x;
                y += n.pnoise(ri[0], None, 7.0 * x + rd[0]) * 0.1;
                y + n.rnoise(ri[1], None, bf * x + rd[1]) * bh
            }
            // Xscorch draws Double Mountain with the Mountain profile.
            Generator::Mountain | Generator::DoubleMountain => {
                let alpha = 4.0 * (2.0 * bh - 0.8);
                let gamma = 0.1 + bh;
                let mut y = alpha * x * x - alpha * x + gamma;
                y += n.pnoise(ri[0], Some(7), 7.0 * x + rd[0]) * 0.1;
                y + n.rnoise(ri[1], bfp, bf * x + rd[1]) * bh
            }
            Generator::Valley => {
                let alpha = 4.0 * (0.8 - 2.0 * bh);
                let gamma = 0.9 - bh;
                let mut y = alpha * x * x - alpha * x + gamma;
                y += n.pnoise(ri[0], Some(7), 7.0 * x + rd[0]) * 0.1;
                y + n.rnoise(ri[1], bfp, bf * x + rd[1]) * bh
            }
            Generator::Plains => {
                let y = n.pnoise(ri[0], Some(7), 7.0 * x + rd[0]);
                let y = (y + 1.0) / 10.0 + 0.3;
                y + n.rnoise(ri[1], bfp, bf * x + rd[1]) * bh
            }
            Generator::Traditional => {
                let mut y = n.pnoise(ri[0], Some(7), 7.0 * x + rd[0]) * 2.5;
                y += n.pnoise(ri[1], Some(17), 17.0 * x + rd[1]);
                y = (1.0 - bh) * y / 3.5;
                y += n.pnoise(ri[2], bfp, bf * x + rd[2]) * bh;
                (y + 1.0) / 2.0
            }
        };
        y.clamp(0.0, 1.0)
    }
}

/// The ground bitmap, column-major so a column falls with one copy.
#[derive(Clone)]
pub struct Land {
    cells: Vec<bool>,
    /// Wrap walls: x wraps around (`sc_land_translate_x`).
    pub wrap: bool,
}

impl Land {
    pub fn generate(generator: Generator, bumpiness: f64, rng: &mut impl Rng) -> Land {
        let profile = Profile::new(generator, bumpiness, rng);
        let mut land = Land::empty();
        for x in 0..WIDTH {
            // Xscorch fills columns from the right, evaluating at x/width.
            let h = (profile.eval((WIDTH - x) as f64 / WIDTH as f64) * MAX_HEIGHT) as usize;
            for y in 0..h.min(HEIGHT) {
                land.cells[x * HEIGHT + y] = true;
            }
        }
        land
    }

    pub fn empty() -> Land {
        Land { cells: vec![false; WIDTH * HEIGHT], wrap: false }
    }

    /// A column inside the field, wrapping if the walls wrap.
    #[inline]
    pub fn column(&self, x: i32) -> Option<usize> {
        if self.wrap {
            Some(x.rem_euclid(WIDTH as i32) as usize)
        } else if x < 0 || x >= WIDTH as i32 {
            None
        } else {
            Some(x as usize)
        }
    }

    /// Horizontal distance from a to b, the short way round if wrapping.
    pub fn dx(&self, a: f64, b: f64) -> f64 {
        let d = b - a;
        if self.wrap {
            let w = WIDTH as f64;
            d - w * (d / w).round()
        } else {
            d
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> bool {
        match self.column(x) {
            Some(cx) if y >= 0 && y < HEIGHT as i32 => self.cells[cx * HEIGHT + y as usize],
            _ => false,
        }
    }

    pub fn set(&mut self, x: i32, y: i32, v: bool) {
        if let Some(cx) = self.column(x)
            && y >= 0
            && y < HEIGHT as i32
        {
            self.cells[cx * HEIGHT + y as usize] = v;
        }
    }

    /// Height of the topmost ground in column x (0 if none).
    pub fn top(&self, x: i32) -> i32 {
        let Some(x) = self.column(x) else { return 0 };
        let col = &self.cells[x * HEIGHT..(x + 1) * HEIGHT];
        col.iter().rposition(|&c| c).map_or(0, |y| y as i32 + 1)
    }

    /// Clear a disc, the crater of a normal explosion.
    pub fn carve(&mut self, cx: f64, cy: f64, r: f64) {
        self.disc(cx, cy, r, None, false);
    }

    /// Fill the sky in a disc with dirt (`SC_EXPLOSION_DIRT`).
    pub fn fill(&mut self, cx: f64, cy: f64, r: f64) {
        self.disc(cx, cy, r, None, true);
    }

    /// Clear or fill a disc, or only the wedge of it within `width/2`
    /// radians either side of `dir` (riot charges).
    pub fn disc(&mut self, cx: f64, cy: f64, r: f64, wedge: Option<(f64, f64)>, fill: bool) {
        let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
        let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
        for x in x0..=x1 {
            for y in y0..=y1 {
                let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                if let Some((dir, width)) = wedge {
                    let off = (dy.atan2(dx) - dir + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                    if off.abs() > width / 2.0 && dx * dx + dy * dy > 4.0 {
                        continue;
                    }
                }
                self.set(x, y, fill);
            }
        }
    }

    /// A liquid's flood from an impact, as `sc_spill_new`: clear the
    /// impact cell, then fill sky cells depth first, down before sideways,
    /// sideways before up, until `size` cells are wet or nowhere is left.
    /// Returns the cells in the order they fill.
    pub fn spill(&mut self, cx: i32, cy: i32, size: usize, rng: &mut impl Rng) -> Vec<(i32, i32)> {
        let mut sp = Spill { cells: Vec::new(), seen: std::collections::HashSet::new(), size };
        let Some(cx) = self.column(cx) else { return sp.cells };
        if cy < 0 || cy >= HEIGHT as i32 {
            return sp.cells;
        }
        let cx = cx as i32;
        self.set(cx, cy, false);
        sp.fill(self, cx, cy - 1, true, rng);
        sp.cells
    }

    /// Turn cells into dirt (liquid dirt setting).
    pub fn add_dirt(&mut self, cells: &[(i32, i32)]) {
        for &(x, y) in cells {
            self.set(x, y, true);
        }
    }

    /// Level [x-w, x+w] to height ht: a platform for a tank
    /// (`sc_land_level_around`).
    pub fn level(&mut self, x: i32, w: i32, ht: i32) {
        for cx in x - w..=x + w {
            for y in 0..HEIGHT as i32 {
                self.set(cx, y, y < ht);
            }
        }
    }

    /// One cycle of loose dirt falling, as `_sc_land_drop_column`: in each
    /// column the ground above the lowest gap falls by up to
    /// `max_drop` (Xscorch: 48). Returns true while anything is still falling.
    pub fn drop_cycle(&mut self, max_drop: usize) -> bool {
        let mut moving = false;
        for x in 0..WIDTH {
            let col = &mut self.cells[x * HEIGHT..(x + 1) * HEIGHT];
            let Some(gap) = col.iter().position(|&c| !c) else { continue };
            let Some(above) = col[gap..].iter().position(|&c| c).map(|p| p + gap) else { continue };
            let d = (above - gap).min(max_drop);
            col.copy_within(above.., above - d);
            for c in &mut col[HEIGHT - d..] {
                *c = false;
            }
            moving = true;
        }
        moving
    }
}

struct Spill {
    cells: Vec<(i32, i32)>,
    seen: std::collections::HashSet<(i32, i32)>,
    size: usize,
}

impl Spill {
    fn fill(&mut self, land: &Land, x: i32, y: i32, allow_up: bool, rng: &mut impl Rng) {
        let r = if rng.random::<bool>() { 1 } else { -1 };
        self.try_fill(land, x, y - 1, false, rng);
        self.try_fill(land, x + r, y - 1, false, rng);
        self.try_fill(land, x - r, y - 1, false, rng);
        self.try_fill(land, x + r, y, false, rng);
        self.try_fill(land, x - r, y, false, rng);
        // Only the centre column may climb, when trapped in a hollow.
        if allow_up {
            self.try_fill(land, x, y + 1, true, rng);
            self.try_fill(land, x + r, y + 1, true, rng);
            self.try_fill(land, x - r, y + 1, true, rng);
        }
    }

    fn try_fill(&mut self, land: &Land, x: i32, y: i32, allow_up: bool, rng: &mut impl Rng) {
        if self.cells.len() >= self.size || y < 0 || y >= HEIGHT as i32 {
            return;
        }
        let Some(x) = land.column(x) else { return };
        let x = x as i32;
        if land.get(x, y) || !self.seen.insert((x, y)) {
            return;
        }
        self.cells.push((x, y));
        self.fill(land, x, y, allow_up, rng);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn every_generator_stays_in_the_field() {
        for g in Generator::ALL {
            let mut rng = StdRng::seed_from_u64(7);
            let land = Land::generate(g, BUMPINESS_DEF, &mut rng);
            for x in 0..WIDTH as i32 {
                assert!(land.top(x) <= MAX_HEIGHT as i32 + 1, "{g:?} too tall at {x}");
            }
        }
    }

    #[test]
    fn mountain_peaks_in_the_middle_and_valley_dips() {
        let mut rng = StdRng::seed_from_u64(3);
        let m = Land::generate(Generator::Mountain, BUMPINESS_DEF, &mut rng);
        assert!(m.top(400) > m.top(10) + 150);
        let v = Land::generate(Generator::Valley, BUMPINESS_DEF, &mut rng);
        assert!(v.top(400) + 150 < v.top(10));
    }

    #[test]
    fn dirt_falls_into_a_cave() {
        let mut land = Land::empty();
        land.level(100, 0, 200);
        land.carve(100.0, 100.0, 20.0);
        assert_eq!(land.top(100), 200);
        while land.drop_cycle(48) {}
        // 41 cells cleared under the column, so it settles 41 lower.
        assert_eq!(land.top(100), 159);
        assert!((0..159).all(|y| land.get(100, y)));
    }

    #[test]
    fn a_spill_pools_in_a_hollow() {
        let mut land = Land::empty();
        land.level(400, 400, 100);
        land.carve(400.0, 100.0, 20.0);
        let mut rng = StdRng::seed_from_u64(1);
        let cells = land.spill(400, 99, 200, &mut rng);
        assert_eq!(cells.len(), 200);
        // It runs to the bottom of the crater first and stays in it.
        assert!(cells.iter().all(|&(x, y)| (x - 400).abs() <= 21 && y <= 100 + 3));
        assert!(cells.iter().any(|&(_, y)| y <= 81));
    }

    #[test]
    fn wrap_joins_the_edges() {
        let mut land = Land::empty();
        land.wrap = true;
        land.carve(0.0, 0.0, 0.0);
        land.set(-1, 5, true);
        assert!(land.get(WIDTH as i32 - 1, 5));
        assert_eq!(land.dx(790.0, 10.0), 20.0);
    }

    #[test]
    fn parse_names() {
        assert_eq!(Generator::parse("double-mountain"), Some(Generator::DoubleMountain));
        assert_eq!(Generator::parse("Plains"), Some(Generator::Plains));
        assert_eq!(Generator::parse("lava"), None);
    }
}
