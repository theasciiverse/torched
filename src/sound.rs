//! Sound, synthesised: no sample files. Every cue is a few hundred
//! milliseconds of generated samples handed to rodio. With no sound
//! device the game is simply silent.

use log::{info, warn};
use rodio::{OutputStream, OutputStreamHandle, Source, buffer::SamplesBuffer};

use crate::fx::Sound;

const RATE: u32 = 44_100;
const VOLUME: f32 = 0.35;

pub struct Audio {
    // Kept alive for as long as sound plays.
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    pub muted: bool,
    noise: u32,
}

impl Audio {
    /// Open the default device; silent if there is none or `muted`.
    pub fn new(muted: bool) -> Audio {
        let (stream, handle) = if muted {
            (None, None)
        } else {
            match OutputStream::try_default() {
                Ok((s, h)) => (Some(s), Some(h)),
                Err(e) => {
                    warn!("no sound: {e}");
                    (None, None)
                }
            }
        };
        if handle.is_some() {
            info!("sound on");
        }
        Audio { _stream: stream, handle, muted, noise: 0x1234_5678 }
    }

    pub fn toggle(&mut self) {
        self.muted = !self.muted;
    }

    pub fn play(&mut self, s: Sound) {
        if self.muted {
            return;
        }
        if self.handle.is_none() {
            return;
        }
        let samples = self.synth(s);
        let Some(h) = &self.handle else { return };
        if samples.is_empty() {
            return;
        }
        let buf = SamplesBuffer::new(1, RATE, samples);
        if let Err(e) = h.play_raw(buf.convert_samples()) {
            warn!("sound: {e}");
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32, plenty for noise.
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn synth(&mut self, s: Sound) -> Vec<f32> {
        match s {
            Sound::Fire { power } => {
                let loud = (0.4 + power / 1000.0 * 0.6) as f32;
                self.sweep(0.16, 220.0, 55.0, loud, 0.25)
            }
            Sound::Blast { radius, plasma } => {
                let r = radius as f32;
                let dur = (0.25 + r / 160.0).min(1.6);
                let mut v = self.noise_burst(dur, (0.25 + r / 120.0).min(1.0), (1200.0 - r * 4.0).max(250.0));
                if plasma {
                    let rumble = self.sweep(dur * 1.2, 70.0, 30.0, 0.8, 0.0);
                    mix(&mut v, &rumble);
                }
                v
            }
            Sound::Napalm => self.crackle(0.9),
            Sound::Dirt => {
                let mut v = self.sweep(0.22, 90.0, 40.0, 0.7, 0.4);
                let n = self.noise_burst(0.2, 0.3, 500.0);
                mix(&mut v, &n);
                v
            }
            Sound::Riot => self.whoosh(0.45),
            Sound::Shield => self.zap(0.14),
            Sound::Death => {
                let mut v = self.noise_burst(0.9, 0.9, 400.0);
                let fall = self.sweep(0.9, 300.0, 40.0, 0.5, 0.0);
                mix(&mut v, &fall);
                v
            }
            Sound::Buy => {
                let mut v = tone(0.05, 880.0, 0.35);
                v.extend(tone(0.07, 1320.0, 0.35));
                v
            }
            Sound::RoundOver => {
                let mut v = Vec::new();
                for f in [392.0, 523.3, 659.3, 784.0] {
                    v.extend(tone(0.11, f, 0.3));
                }
                v
            }
        }
    }

    /// A falling (or rising) sine with an exponential tail and a dash
    /// of noise at the start.
    fn sweep(&mut self, dur: f32, f0: f32, f1: f32, loud: f32, click: f32) -> Vec<f32> {
        let n = (dur * RATE as f32) as usize;
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let f = f0 + (f1 - f0) * t;
                phase += f / RATE as f32 * std::f32::consts::TAU;
                let env = (-4.0 * t).exp();
                let attack = (i as f32 / 200.0).min(1.0);
                let noise = if t < 0.05 { self.rand() * click * (1.0 - t / 0.05) } else { 0.0 };
                (phase.sin() * env * attack + noise) * loud * VOLUME
            })
            .collect()
    }

    /// Low-passed noise with a fast attack and a long decay: a bang.
    fn noise_burst(&mut self, dur: f32, loud: f32, cutoff: f32) -> Vec<f32> {
        let n = (dur * RATE as f32) as usize;
        let a = 1.0 - (-std::f32::consts::TAU * cutoff / RATE as f32).exp();
        let mut y = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                y += a * (self.rand() - y);
                let env = (-5.0 * t).exp() * (i as f32 / 100.0).min(1.0);
                y * env * loud * VOLUME * 2.5
            })
            .collect()
    }

    /// Sparse pops: fire crackling.
    fn crackle(&mut self, dur: f32) -> Vec<f32> {
        let n = (dur * RATE as f32) as usize;
        let mut pop = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                if self.rand() > 0.997 {
                    pop = 1.0;
                }
                pop *= 0.985;
                let hiss = self.rand() * 0.08;
                (self.rand() * pop + hiss) * (1.0 - t) * VOLUME
            })
            .collect()
    }

    /// Noise swelling and fading: a rush of air.
    fn whoosh(&mut self, dur: f32) -> Vec<f32> {
        let n = (dur * RATE as f32) as usize;
        let mut y = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let cutoff = 300.0 + 2500.0 * t;
                let a = 1.0 - (-std::f32::consts::TAU * cutoff / RATE as f32).exp();
                y += a * (self.rand() - y);
                y * (std::f32::consts::PI * t).sin() * VOLUME * 1.6
            })
            .collect()
    }

    /// A falling square wave.
    fn zap(&mut self, dur: f32) -> Vec<f32> {
        let n = (dur * RATE as f32) as usize;
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                phase += (900.0 - 600.0 * t) / RATE as f32;
                let sq = if phase.fract() < 0.5 { 1.0 } else { -1.0 };
                sq * (1.0 - t) * 0.25 * VOLUME
            })
            .collect()
    }
}

/// A plain sine note with soft ends.
fn tone(dur: f32, f: f32, loud: f32) -> Vec<f32> {
    let n = (dur * RATE as f32) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let env = (t * 12.0).min(1.0) * ((1.0 - t) * 6.0).min(1.0);
            (i as f32 / RATE as f32 * f * std::f32::consts::TAU).sin() * env * loud * VOLUME
        })
        .collect()
}

/// Add b into a, growing a if b is longer.
fn mix(a: &mut Vec<f32>, b: &[f32]) {
    if b.len() > a.len() {
        a.resize(b.len(), 0.0);
    }
    for (x, y) in a.iter_mut().zip(b) {
        *x = (*x + y).clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_makes_bounded_samples() {
        let mut a = Audio { _stream: None, handle: None, muted: false, noise: 7 };
        for s in [
            Sound::Fire { power: 1000.0 },
            Sound::Blast { radius: 200.0, plasma: true },
            Sound::Blast { radius: 20.0, plasma: false },
            Sound::Napalm,
            Sound::Dirt,
            Sound::Riot,
            Sound::Shield,
            Sound::Death,
            Sound::Buy,
            Sound::RoundOver,
        ] {
            let v = a.synth(s);
            assert!(!v.is_empty(), "{s:?}");
            assert!(v.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "{s:?} out of range");
        }
    }
}
