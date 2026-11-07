//! Flight, wind, air and walls (`sphysics.c`).

use rand::Rng;

/// Gravity in units per step per step (`SC_PHYSICS_GRAVITY_DEF`).
pub const GRAVITY: f64 = 0.5;
/// Units per step per point of power (`SC_PHYSICS_VELOCITY_SCL`).
pub const VELOCITY_SCALE: f64 = 0.027;
/// Default and largest wind, units per step per step.
pub const WIND_DEF: f64 = 0.05;
pub const WIND_MAX: f64 = 10.0;
/// Most the wind changes in a turn, as a share of the maximum.
const DELTA_WIND_MAX: f64 = 0.2;
pub const VISCOSITY_MAX: f64 = 0.2;
/// How far off either side a shot is followed with no walls.
pub const BORDERS_EXTEND: f64 = 2500.0;
/// Steps before a shot gives up (`SC_TRAJ_TIMEOUT`).
pub const TIMEOUT: f64 = 240.0;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walls {
    Concrete,
    Padded,
    Rubber,
    Spring,
    Wrap,
    None,
    Random,
}

impl Walls {
    pub const ALL: [Walls; 7] =
        [Walls::Concrete, Walls::Padded, Walls::Rubber, Walls::Spring, Walls::Wrap, Walls::None, Walls::Random];

    pub fn name(self) -> &'static str {
        match self {
            Walls::Concrete => "Concrete",
            Walls::Padded => "Padded",
            Walls::Rubber => "Rubber",
            Walls::Spring => "Spring",
            Walls::Wrap => "Wrap",
            Walls::None => "None",
            Walls::Random => "Random",
        }
    }

    pub fn parse(s: &str) -> Option<Walls> {
        Walls::ALL.into_iter().find(|w| w.name().eq_ignore_ascii_case(s))
    }

    /// A real kind for the round.
    pub fn resolve(self, rng: &mut impl Rng) -> Walls {
        match self {
            Walls::Random => Walls::ALL[rng.random_range(0..6)],
            w => w,
        }
    }

    /// Share of speed kept on a bounce, for the elastic walls.
    pub fn elasticity(self) -> Option<f64> {
        match self {
            Walls::Padded => Some(0.5),
            Walls::Rubber => Some(1.0),
            Walls::Spring => Some(1.5),
            _ => None,
        }
    }
}

/// Wind, which Xscorch changes a little between turns.
pub fn initial_wind(max: f64, rng: &mut impl Rng) -> f64 {
    (rng.random::<f64>() * 2.0 - 1.0) * max
}

/// `sc_physics_update_wind`.
pub fn update_wind(wind: f64, max: f64, rng: &mut impl Rng) -> f64 {
    if max <= 0.0 {
        return 0.0;
    }
    let mut w = wind;
    loop {
        if w < -max {
            w += rng.random::<f64>() * max * DELTA_WIND_MAX;
        } else if w > max {
            w -= rng.random::<f64>() * max * DELTA_WIND_MAX;
        } else {
            w += (rng.random::<f64>() * 2.0 - 1.0) * max * DELTA_WIND_MAX * 2.0;
        }
        if (-max..=max).contains(&w) {
            return w;
        }
    }
}

/// A flight path: start, starting velocity, constant acceleration (wind
/// and gravity) and linear air drag.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
pub struct Path {
    pub ctr: (f64, f64),
    pub vel: (f64, f64),
    pub acc: (f64, f64),
    /// Drag per step. Xscorch has the setting but never applied it ("N/A"
    /// in its dialog); this is Torched's: dv/dt = acc - drag * v.
    pub drag: f64,
}

impl Path {
    /// Position and velocity t steps along. Without drag this is
    /// `sc_traj_get_current_x/y`, `ctr + v t + a t^2 / 2`.
    pub fn at(&self, t: f64) -> ((f64, f64), (f64, f64)) {
        let k = self.drag;
        let one = |c: f64, v: f64, a: f64| {
            if k <= 1e-9 {
                (c + t * (v + 0.5 * t * a), v + a * t)
            } else {
                let e = (-k * t).exp();
                let term = v - a / k;
                (c + a * t / k + term * (1.0 - e) / k, a / k + term * e)
            }
        };
        let (x, vx) = one(self.ctr.0, self.vel.0, self.acc.0);
        let (y, vy) = one(self.ctr.1, self.vel.1, self.acc.1);
        ((x, y), (vx, vy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn drag_slows_and_matches_no_drag_at_the_limit() {
        let p = Path { ctr: (0.0, 0.0), vel: (5.0, 5.0), acc: (0.0, -GRAVITY), drag: 0.0 };
        let q = Path { drag: 1e-7, ..p };
        let ((x0, y0), _) = p.at(10.0);
        let ((x1, y1), _) = q.at(10.0);
        assert!((x0 - x1).abs() < 1e-3 && (y0 - y1).abs() < 1e-3);
        let r = Path { drag: 0.05, ..p };
        assert!(r.at(10.0).0.0 < x0);
    }

    #[test]
    fn wind_stays_in_range() {
        let mut rng = StdRng::seed_from_u64(9);
        let mut w = initial_wind(WIND_DEF, &mut rng);
        for _ in 0..1000 {
            w = update_wind(w, WIND_DEF, &mut rng);
            assert!(w.abs() <= WIND_DEF);
        }
    }
}
