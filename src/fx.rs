//! Look and sound cues that don't change the game: skies, flying dirt and
//! sparks, the flash and shake of big blasts, and the sounds to play.
//! Its own random numbers, so a seeded game plays the same with or
//! without it.

use rand::{Rng, SeedableRng, rngs::StdRng};

use crate::land::{self, Land};

/// Xscorch's skies (`sland.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sky {
    Night,
    Fire,
    Random,
}

impl Sky {
    pub const ALL: [Sky; 3] = [Sky::Night, Sky::Fire, Sky::Random];

    pub fn name(self) -> &'static str {
        match self {
            Sky::Night => "Night",
            Sky::Fire => "Fire",
            Sky::Random => "Random",
        }
    }

    pub fn parse(s: &str) -> Option<Sky> {
        Sky::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
    }

    pub fn resolve(self, rng: &mut impl Rng) -> Sky {
        match self {
            Sky::Random => [Sky::Night, Sky::Fire][rng.random_range(0..2)],
            s => s,
        }
    }

    /// Colour at the horizon and overhead, from Xscorch's gradients
    /// (`scolor-gtk.c`), toned down so dots stay readable on a terminal.
    pub fn gradient(self) -> ((u8, u8, u8), (u8, u8, u8)) {
        match self {
            // Xscorch: (0, 0, 0.05) to (0, 0, 0.45).
            Sky::Night | Sky::Random => ((3, 4, 14), (6, 10, 62)),
            // Xscorch: (1.0, 0.75, 0.25) to (0.37, 0, 0).
            Sky::Fire => ((150, 92, 34), (58, 6, 8)),
        }
    }
}

/// Something the player should hear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sound {
    /// A shot leaves the barrel; louder with power.
    Fire { power: f64 },
    /// A blast, by radius.
    Blast { radius: f64, plasma: bool },
    Napalm,
    Dirt,
    Riot,
    /// A shell stopped or bounced by a shield.
    Shield,
    /// A tank destroyed.
    Death,
    /// A purchase in the shop.
    Buy,
    /// A round ends.
    RoundOver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    /// A clod of earth, falling and stopping on the ground.
    Dirt,
    /// A spark from a blast, quick and bright.
    Spark,
    /// An ember rising off napalm.
    Ember,
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub kind: ParticleKind,
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    pub life: u32,
    pub max: u32,
}

impl Particle {
    /// 1 when new, 0 when gone.
    pub fn freshness(&self) -> f64 {
        self.life as f64 / self.max.max(1) as f64
    }
}

/// Most particles at once, so The Chaos^2 doesn't drown the frame.
const MAX_PARTICLES: usize = 1500;

pub struct Fx {
    pub particles: Vec<Particle>,
    /// 0 to 1: the sky lit by a big blast.
    pub flash: f64,
    /// Screen shake, in cells.
    pub shake: f64,
    /// Sounds raised since the last frame took them.
    pub sounds: Vec<Sound>,
    rng: StdRng,
}

impl Fx {
    pub fn new(seed: u64) -> Fx {
        Fx { particles: Vec::new(), flash: 0.0, shake: 0.0, sounds: Vec::new(), rng: StdRng::seed_from_u64(seed ^ 0x5eed_f00d) }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.flash = 0.0;
        self.shake = 0.0;
    }

    pub fn sound(&mut self, s: Sound) {
        // A volley of hundreds of blasts is one roar, not hundreds.
        if self.sounds.len() < 12 {
            self.sounds.push(s);
        }
    }

    /// A blast at full size: dirt thrown out of the crater, sparks, and
    /// for the big ones a flash and a shake.
    pub fn blast(&mut self, x: f64, y: f64, radius: f64, earth: bool, bright: bool) {
        let room = MAX_PARTICLES.saturating_sub(self.particles.len());
        let n = ((radius * 0.8) as usize).clamp(4, 60).min(room);
        for k in 0..n {
            let a = self.rng.random_range(0.0..std::f64::consts::TAU);
            let speed = self.rng.random_range(0.3..1.0) * (radius / 8.0).clamp(1.5, 9.0);
            let dirt = earth && k % 3 != 0;
            let life = if dirt { self.rng.random_range(30..70) } else { self.rng.random_range(10..25) };
            self.particles.push(Particle {
                kind: if dirt { ParticleKind::Dirt } else { ParticleKind::Spark },
                x: x + a.cos() * radius * 0.5,
                y: y + a.sin().abs() * radius * 0.4,
                vx: a.cos() * speed,
                vy: a.sin().abs() * speed + 1.0,
                life,
                max: life,
            });
        }
        if radius >= 50.0 {
            self.shake = self.shake.max((radius / 45.0).min(3.0));
        }
        if bright && radius >= 60.0 {
            self.flash = self.flash.max((radius / 160.0).min(0.9));
        }
    }

    /// Embers off burning napalm, a few each tick.
    pub fn embers(&mut self, cells: &[(i32, i32)]) {
        if cells.is_empty() || self.particles.len() >= MAX_PARTICLES {
            return;
        }
        for _ in 0..(cells.len() / 60).clamp(1, 4) {
            let (x, y) = cells[self.rng.random_range(0..cells.len())];
            let life = self.rng.random_range(15..40);
            self.particles.push(Particle {
                kind: ParticleKind::Ember,
                x: x as f64,
                y: y as f64 + 1.0,
                vx: self.rng.random_range(-0.4..0.4),
                vy: self.rng.random_range(0.6..1.6),
                life,
                max: life,
            });
        }
    }

    pub fn tick(&mut self, land: &Land, wind: f64) {
        for p in &mut self.particles {
            p.life = p.life.saturating_sub(1);
            match p.kind {
                ParticleKind::Ember => {
                    p.vx += wind * 4.0;
                    p.vy *= 0.97;
                }
                _ => p.vy -= 0.35,
            }
            p.x += p.vx;
            p.y += p.vy;
            // Dirt stops where it lands; sparks die on touching ground.
            if p.kind != ParticleKind::Ember && (p.y < 0.0 || land.get(p.x.round() as i32, p.y.round() as i32)) {
                p.life = 0;
            }
            if p.y > land::HEIGHT as f64 * 1.5 {
                p.life = 0;
            }
        }
        self.particles.retain(|p| p.life > 0);
        self.flash *= 0.82;
        if self.flash < 0.02 {
            self.flash = 0.0;
        }
        self.shake *= 0.86;
        if self.shake < 0.2 {
            self.shake = 0.0;
        }
    }
}
