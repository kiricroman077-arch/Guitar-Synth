//! Sound generation: oscillators, sub oscillator and filter.

use nih_plug::prelude::Enum;
use std::f32::consts::{PI, TAU};

#[derive(Enum, PartialEq, Clone, Copy, Debug)]
pub enum Waveform {
    #[name = "Saw"]
    Saw,
    #[name = "Square"]
    Square,
    #[name = "Pulse (PWM)"]
    Pulse,
    #[name = "Triangle"]
    Triangle,
    #[name = "Sine"]
    Sine,
    #[name = "Supersaw"]
    Supersaw,
    #[name = "FM"]
    Fm,
    #[name = "Hard Sync"]
    Sync,
}

const SUPERSAW_VOICES: usize = 7;
const SUPERSAW_SPREAD: [f32; SUPERSAW_VOICES] = [-1.0, -0.62, -0.28, 0.0, 0.28, 0.62, 1.0];

/// PolyBLEP: smooths the sharp edges of saw/square so they don't alias harshly.
#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

#[inline]
fn saw(phase: f32, dt: f32) -> f32 {
    (2.0 * phase - 1.0) - poly_blep(phase, dt)
}

#[inline]
fn pulse(phase: f32, dt: f32, width: f32) -> f32 {
    let naive = if phase < width { 1.0 } else { -1.0 };
    let mut p2 = phase + 1.0 - width;
    if p2 >= 1.0 {
        p2 -= 1.0;
    }
    // Keep the average at zero regardless of pulse width
    naive + poly_blep(phase, dt) - poly_blep(p2, dt) - (2.0 * width - 1.0)
}

/// Wavefolder: pushes the wave into itself for a brighter, metallic tone.
#[inline]
fn fold(x: f32, amount: f32) -> f32 {
    if amount <= 0.001 {
        x
    } else {
        (x * (1.0 + 4.0 * amount) * PI * 0.5).sin()
    }
}

#[inline]
fn wrap(p: &mut f32) {
    if *p >= 1.0 {
        *p -= p.floor();
    }
}

pub struct Oscillator {
    phase: f32,
    sub_phase: f32,
    mod_phase: f32,
    sync_phase: f32,
    super_phases: [f32; SUPERSAW_VOICES],
    rng: u32,
}

impl Default for Oscillator {
    fn default() -> Self {
        let mut o = Self {
            phase: 0.0,
            sub_phase: 0.0,
            mod_phase: 0.0,
            sync_phase: 0.0,
            super_phases: [0.0; SUPERSAW_VOICES],
            rng: 0x1234_5678,
        };
        o.randomize_supersaw();
        o
    }
}

impl Oscillator {
    fn rand(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Random supersaw phases, so every note has the classic "spread" right away.
    pub fn randomize_supersaw(&mut self) {
        for i in 0..SUPERSAW_VOICES {
            self.super_phases[i] = self.rand();
        }
    }

    /// One sample. `dt` = frequency / sample rate. `character` is 0..1 and does
    /// something different for each waveform (see README).
    #[inline]
    pub fn next(
        &mut self,
        wave: Waveform,
        dt: f32,
        character: f32,
        fm_ratio: f32,
        sub_level: f32,
    ) -> f32 {
        let main = match wave {
            Waveform::Saw => saw(self.phase, dt),
            Waveform::Square => pulse(self.phase, dt, 0.5),
            Waveform::Pulse => pulse(self.phase, dt, 0.5 - 0.45 * character),
            Waveform::Triangle => {
                let tri = 1.0 - 4.0 * (self.phase - 0.5).abs();
                fold(tri, character)
            }
            Waveform::Sine => fold((self.phase * TAU).sin(), character),
            Waveform::Supersaw => {
                let detune = character * character * 0.6; // up to ±0.6 semitones
                let mut sum = 0.0;
                for i in 0..SUPERSAW_VOICES {
                    let ratio = 2f32.powf(SUPERSAW_SPREAD[i] * detune / 12.0);
                    let vdt = dt * ratio;
                    sum += saw(self.super_phases[i], vdt);
                    self.super_phases[i] += vdt;
                    wrap(&mut self.super_phases[i]);
                }
                sum * 0.36
            }
            Waveform::Fm => {
                let index = character * 6.0;
                let m = (self.mod_phase * TAU).sin();
                self.mod_phase += dt * fm_ratio;
                wrap(&mut self.mod_phase);
                (self.phase * TAU + index * m).sin()
            }
            Waveform::Sync => {
                // A saw that restarts every time the main oscillator does.
                let ratio = 1.0 + 4.0 * character;
                let sdt = dt * ratio;
                let s = 2.0 * self.sync_phase - 1.0;
                self.sync_phase += sdt;
                wrap(&mut self.sync_phase);
                s
            }
        };

        let sub = if sub_level > 0.0 {
            let half = dt * 0.5;
            let s = pulse(self.sub_phase, half, 0.5);
            self.sub_phase += half;
            wrap(&mut self.sub_phase);
            s * sub_level
        } else {
            0.0
        };

        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            if wave == Waveform::Sync {
                self.sync_phase = self.phase * (1.0 + 4.0 * character);
                wrap(&mut self.sync_phase);
            }
        }

        main + sub
    }
}

/// Resonant low-pass filter (zero-delay-feedback state variable filter) with a
/// bit of drive for an analog-style character.
#[derive(Default)]
pub struct Filter {
    ic1: f32,
    ic2: f32,
}

impl Filter {
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32, cutoff: f32, resonance: f32, sample_rate: f32) -> f32 {
        let fc = cutoff.clamp(20.0, sample_rate * 0.45);
        let g = (PI * fc / sample_rate).tan();
        let k = 2.0 - 1.96 * resonance.clamp(0.0, 1.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        let v0 = (x * 1.2).tanh();
        let v3 = v0 - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}
