//! Monophonic pitch tracker (YIN algorithm).
//!
//! The guitar signal is low-pass filtered, downsampled to roughly 12 kHz to keep
//! the CPU cost low, and analysed every ~2 ms. Each analysis returns the detected
//! frequency and a "clarity" value between 0 and 1 (how periodic, i.e. how
//! clearly pitched, the signal is right now).

/// Lowest frequency we try to detect. Low E is 82 Hz, drop-D 73 Hz, drop-C 65 Hz.
pub const MIN_FREQ: f32 = 60.0;
/// Highest frequency we try to detect (high E string, 24th fret is ~1319 Hz).
pub const MAX_FREQ: f32 = 1500.0;
/// YIN threshold: lower = stricter about what counts as a pitched note.
const YIN_THRESHOLD: f32 = 0.15;
/// Time between analyses.
const HOP_SECONDS: f32 = 0.002;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(sample_rate: f32, cutoff: f32, q: f32) -> Self {
        let w0 = std::f32::consts::TAU * cutoff / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        // Transposed direct form II
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Detection {
    pub freq: f32,
    pub clarity: f32,
}

pub struct PitchTracker {
    lp1: Biquad,
    lp2: Biquad,
    decimation: usize,
    dec_counter: usize,
    dec_rate: f32,

    ring: Vec<f32>,
    write_pos: usize,
    frame: Vec<f32>,
    diff: Vec<f32>,

    window: usize,
    tau_min: usize,
    tau_max: usize,

    hop: usize,
    hop_counter: usize,
}

impl PitchTracker {
    pub fn new(sample_rate: f32) -> Self {
        let decimation = ((sample_rate / 12_000.0).round() as usize).max(1);
        let dec_rate = sample_rate / decimation as f32;

        let tau_max = (dec_rate / MIN_FREQ).ceil() as usize;
        let tau_min = ((dec_rate / MAX_FREQ).floor() as usize).max(2);
        let window = tau_max / 3;
        let len = window + tau_max + 2;

        let cutoff = (1_800.0f32).min(dec_rate * 0.4);

        Self {
            lp1: Biquad::lowpass(sample_rate, cutoff, 0.54),
            lp2: Biquad::lowpass(sample_rate, cutoff, 1.31),
            decimation,
            dec_counter: 0,
            dec_rate,
            ring: vec![0.0; len],
            write_pos: 0,
            frame: vec![0.0; len],
            diff: vec![0.0; tau_max + 2],
            window,
            tau_min,
            tau_max,
            hop: ((HOP_SECONDS * dec_rate).round() as usize).max(1),
            hop_counter: 0,
        }
    }

    pub fn reset(&mut self) {
        self.lp1.reset();
        self.lp2.reset();
        self.ring.iter_mut().for_each(|x| *x = 0.0);
        self.write_pos = 0;
        self.dec_counter = 0;
        self.hop_counter = 0;
    }

    /// Feed one input sample (at the host sample rate). Returns a detection
    /// whenever a new analysis has been run.
    #[inline]
    pub fn push(&mut self, x: f32) -> Option<Detection> {
        let y = self.lp2.process(self.lp1.process(x));

        self.dec_counter += 1;
        if self.dec_counter < self.decimation {
            return None;
        }
        self.dec_counter = 0;

        self.ring[self.write_pos] = y;
        self.write_pos = (self.write_pos + 1) % self.ring.len();

        self.hop_counter += 1;
        if self.hop_counter < self.hop {
            return None;
        }
        self.hop_counter = 0;

        Some(self.analyse())
    }

    fn analyse(&mut self) -> Detection {
        // Unroll the ring buffer, oldest sample first
        let len = self.ring.len();
        for i in 0..len {
            self.frame[i] = self.ring[(self.write_pos + i) % len];
        }

        let w = self.window;
        let x = &self.frame;

        // Difference function
        // Difference function, anchored at the newest samples so that short
        // periods (high notes) only need the most recent audio.
        let end = len - 1;
        for tau in 1..=self.tau_max + 1 {
            let mut sum = 0.0f32;
            for j in 0..w {
                let d = x[end - j] - x[end - j - tau];
                sum += d * d;
            }
            self.diff[tau] = sum;
        }

        // Cumulative mean normalised difference
        self.diff[0] = 1.0;
        let mut running = 0.0f32;
        for tau in 1..=self.tau_max + 1 {
            running += self.diff[tau];
            self.diff[tau] = if running > 0.0 {
                self.diff[tau] * tau as f32 / running
            } else {
                1.0
            };
        }

        // First dip below the threshold, then walk down to its local minimum
        let mut best: Option<usize> = None;
        let mut tau = self.tau_min;
        while tau <= self.tau_max {
            if self.diff[tau] < YIN_THRESHOLD {
                while tau < self.tau_max && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }

        // Nothing under the threshold: fall back to the global minimum (low clarity)
        let tau = best.unwrap_or_else(|| {
            let mut min_tau = self.tau_min;
            for t in self.tau_min..=self.tau_max {
                if self.diff[t] < self.diff[min_tau] {
                    min_tau = t;
                }
            }
            min_tau
        });

        // Parabolic interpolation for sub-sample accuracy
        let refined = if tau > 1 && tau <= self.tau_max {
            let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 {
                tau as f32 + 0.5 * (a - c) / denom
            } else {
                tau as f32
            }
        } else {
            tau as f32
        };

        Detection {
            freq: self.dec_rate / refined,
            clarity: (1.0 - self.diff[tau]).clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(freq: f32, sr: f32) -> Detection {
        let mut t = PitchTracker::new(sr);
        let mut last = None;
        // Guitar-ish tone: fundamental plus strong harmonics
        for i in 0..(sr * 0.15) as usize {
            let ph = std::f32::consts::TAU * freq * i as f32 / sr;
            let x = 0.5 * ph.sin() + 0.4 * (2.0 * ph).sin() + 0.3 * (3.0 * ph).sin()
                + 0.2 * (4.0 * ph).sin();
            if let Some(d) = t.push(x) {
                last = Some(d);
            }
        }
        last.unwrap()
    }

    #[test]
    fn detects_guitar_range() {
        for &sr in &[44_100.0, 48_000.0, 96_000.0] {
            for &f in &[73.42, 82.41, 110.0, 146.83, 196.0, 246.94, 329.63, 659.25, 1046.5] {
                let d = detect(f, sr);
                let cents = 1200.0 * (d.freq / f).log2();
                assert!(cents.abs() < 10.0, "sr {sr} f {f}: got {} ({cents} cents)", d.freq);
                assert!(d.clarity > 0.85, "sr {sr} f {f}: clarity {}", d.clarity);
            }
        }
    }
}

#[cfg(test)]
mod pluck_tests {
    use super::*;

    /// Karplus-Strong plucked string: a decent stand-in for a real guitar note.
    fn pluck(freq: f32, sr: f32, secs: f32, seed: u32) -> Vec<f32> {
        let n = (sr / freq).round() as usize;
        let mut s = seed;
        let mut buf: Vec<f32> = (0..n)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            })
            .collect();
        let mut out = Vec::new();
        let mut i = 0;
        for _ in 0..(sr * secs) as usize {
            let a = buf[i];
            let b = buf[(i + 1) % n];
            buf[i] = 0.996 * 0.5 * (a + b);
            out.push(a * 0.3);
            i = (i + 1) % n;
        }
        out
    }

    #[test]
    fn pluck_latency_and_octaves() {
        let sr = 48_000.0;
        for &(name, f) in &[("low E", 82.41f32), ("A", 110.0), ("D", 146.83), ("G", 196.0), ("B", 246.94), ("high E", 329.63), ("12th fr high E", 659.25)] {
            let mut t = PitchTracker::new(sr);
            // 50 ms of silence, then the pluck
            let mut sig = vec![0.0f32; (sr * 0.05) as usize];
            sig.extend(pluck(f, sr, 1.5, 12345));
            let start = (sr * 0.05) as usize;
            let mut first_ok: Option<f32> = None;
            let mut wrong = 0;
            let mut total = 0;
            for (i, &x) in sig.iter().enumerate() {
                if let Some(d) = t.push(x) {
                    if i < start || d.clarity < 0.8 { continue; }
                    let semis = 12.0 * (d.freq / f).log2();
                    total += 1;
                    if semis.abs() < 0.5 {
                        if first_ok.is_none() { first_ok = Some((i - start) as f32 / sr * 1000.0); }
                    } else {
                        wrong += 1;
                    }
                }
            }
            println!("{name:>16}: first correct after {:>5.1} ms, wrong {wrong}/{total}", first_ok.unwrap_or(-1.0));
        }
    }
}
