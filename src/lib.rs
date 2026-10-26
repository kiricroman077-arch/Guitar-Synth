//! Guitar Synth: listens to a guitar, tracks the note being played and plays a
//! synth on that note.

use nih_plug::prelude::*;
use std::sync::Arc;

mod synth;
mod tracker;
use synth::{Filter, Oscillator, Waveform};
use tracker::PitchTracker;

/// Below this clarity a detection counts as "noise, not a note".
const NOISE_CLARITY: f32 = 0.6;
/// A ringing note ends if it has been unclear (noise only) for this long.
const UNCLEAR_TIMEOUT: f32 = 0.10;
/// A new note may only start this soon after a pick attack.
const ONSET_WINDOW: f32 = 0.12;

#[derive(Params)]
struct GuitarSynthParams {
    // --- Tracking ---
    #[id = "thresh"]
    threshold: FloatParam,
    #[id = "noiserej"]
    noise_rejection: FloatParam,
    #[id = "snap"]
    snap: BoolParam,

    // --- Oscillator ---
    #[id = "wave"]
    waveform: EnumParam<Waveform>,
    #[id = "charac"]
    character: FloatParam,
    #[id = "fmratio"]
    fm_ratio: IntParam,
    #[id = "octave"]
    octave: IntParam,
    #[id = "sub"]
    sub_level: FloatParam,

    // --- Filter ---
    #[id = "cutoff"]
    cutoff: FloatParam,
    #[id = "reso"]
    resonance: FloatParam,

    // --- Dynamics ---
    #[id = "sustain"]
    sustain: FloatParam,
    #[id = "accent"]
    accent: FloatParam,
    #[id = "release"]
    release: FloatParam,

    // --- Output ---
    #[id = "level"]
    level: FloatParam,
    #[id = "mix"]
    mix: FloatParam,
}

fn percent(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

impl Default for GuitarSynthParams {
    fn default() -> Self {
        Self {
            threshold: FloatParam::new(
                "Input Threshold",
                -50.0,
                FloatRange::Linear { min: -80.0, max: -10.0 },
            )
            .with_unit(" dB")
            .with_step_size(0.5),
            noise_rejection: percent("Noise Rejection", 0.5),
            snap: BoolParam::new("Snap to Note", true),

            waveform: EnumParam::new("Waveform", Waveform::Saw),
            character: percent("Character", 0.4).with_smoother(SmoothingStyle::Linear(20.0)),
            fm_ratio: IntParam::new("FM Ratio", 2, IntRange::Linear { min: 1, max: 8 }),
            octave: IntParam::new("Octave", 0, IntRange::Linear { min: -2, max: 2 }),
            sub_level: percent("Sub Osc", 0.0).with_smoother(SmoothingStyle::Linear(20.0)),

            cutoff: FloatParam::new(
                "Filter Cutoff",
                4_000.0,
                FloatRange::Skewed {
                    min: 40.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            resonance: percent("Filter Resonance", 0.2).with_smoother(SmoothingStyle::Linear(20.0)),

            sustain: percent("Sustain", 0.7),
            accent: percent("Pick Accent", 0.4),
            release: FloatParam::new(
                "Release",
                0.25,
                FloatRange::Skewed { min: 0.01, max: 3.0, factor: FloatRange::skew_factor(-2.0) },
            )
            .with_unit(" s")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            level: FloatParam::new(
                "Synth Level",
                util::db_to_gain(-6.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-40.0),
                    max: util::db_to_gain(6.0),
                    factor: FloatRange::gain_skew_factor(-40.0, 6.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            mix: percent("Dry/Wet", 1.0).with_smoother(SmoothingStyle::Linear(20.0)),
        }
    }
}

/// One-pole smoothing coefficient for a given time constant.
fn coef(seconds: f32, sr: f32) -> f32 {
    (-1.0 / (seconds * sr).max(1.0)).exp()
}

fn freq_to_midi(f: f32) -> f32 {
    69.0 + 12.0 * (f / 440.0).log2()
}

fn midi_to_freq(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

/// Everything the per-sample engine needs from the parameters.
#[derive(Clone, Copy)]
struct Settings {
    wave: Waveform,
    octave: f32,
    snap: bool,
    fm_ratio: f32,
    gate_on: f32,
    noise_rejection: f32,
    accent: f32,
    sustain: f32,
    release: f32,
    character: f32,
    sub_level: f32,
    cutoff: f32,
    resonance: f32,
}

struct GuitarSynth {
    params: Arc<GuitarSynthParams>,
    sample_rate: f32,
    tracker: PitchTracker,
    osc: Oscillator,
    filter: Filter,

    // Input level followers
    env: f32,
    fast_env: f32,
    slow_env: f32,
    env_att: f32,
    env_rel: f32,
    fast_rel: f32,
    slow_coef: f32,
    samples_since_onset: u32,

    // Note state
    active: bool,
    note: f32,      // snapped note (MIDI number)
    raw_pitch: f32, // exact detected pitch (MIDI, fractional)
    candidate: f32,
    candidate_count: u32,
    peak: f32,
    unclear_time: f32,

    // Synth state
    pitch: f32, // pitch actually played (MIDI)
    amp: f32,
    accent_env: f32,
}

impl Default for GuitarSynth {
    fn default() -> Self {
        Self {
            params: Arc::new(GuitarSynthParams::default()),
            sample_rate: 48_000.0,
            tracker: PitchTracker::new(48_000.0),
            osc: Oscillator::default(),
            filter: Filter::default(),
            env: 0.0,
            fast_env: 0.0,
            slow_env: 0.0,
            env_att: 0.0,
            env_rel: 0.0,
            fast_rel: 0.0,
            slow_coef: 0.0,
            samples_since_onset: u32::MAX,
            active: false,
            note: 60.0,
            raw_pitch: 60.0,
            candidate: 0.0,
            candidate_count: 0,
            peak: 0.0,
            unclear_time: 0.0,
            pitch: 60.0,
            amp: 0.0,
            accent_env: 0.0,
        }
    }
}

impl GuitarSynth {
    fn agree(&mut self, value: f32, needed: u32) -> bool {
        if self.candidate_count > 0 && (value - self.candidate).abs() < 0.5 {
            self.candidate_count += 1;
        } else {
            self.candidate = value;
            self.candidate_count = 1;
        }
        self.candidate_count >= needed
    }

    fn on_detection(&mut self, freq: f32, clarity: f32, gate_on: f32, nr: f32) {
        let hop_seconds = self.tracker.hop_seconds();
        // Stricter = needs a clearer pitch, held for longer, before it counts as a note
        let min_clarity = 0.78 + 0.17 * nr;
        let needed = 2 + (3.0 * nr).round() as u32;
        let need_pick = nr > 0.25;

        if !freq.is_finite() || clarity < min_clarity {
            if clarity < NOISE_CLARITY {
                self.unclear_time += hop_seconds;
                if self.active && self.unclear_time > UNCLEAR_TIMEOUT {
                    // Only noise left (muted or dying string): end the note
                    self.active = false;
                }
            }
            return;
        }
        self.unclear_time = 0.0;

        let midi = freq_to_midi(freq);
        let snapped = midi.round();

        if !self.active {
            let picked =
                (self.samples_since_onset as f32) < ONSET_WINDOW * self.sample_rate;
            // A new note must start clearly louder than the threshold, so light
            // touches and string noise don't trigger it (0..+20 dB).
            let start_level = gate_on * util::db_to_gain(20.0 * nr);
            if self.env > start_level && (picked || !need_pick) && self.agree(snapped, needed) {
                self.active = true;
                self.note = snapped;
                self.raw_pitch = midi;
                self.pitch = midi;
                self.peak = self.peak.max(self.env);
                self.candidate_count = 0;
                self.osc.randomize_supersaw();
            }
            return;
        }

        // Exact pitch for free mode: follow small, continuous movements (bends,
        // vibrato) but ignore sudden jumps, which are usually octave errors.
        if (midi - self.raw_pitch).abs() < 1.0 {
            self.raw_pitch = midi;
        }

        if snapped != self.note {
            if self.agree(snapped, needed) {
                self.note = snapped;
                self.raw_pitch = midi;
                self.candidate_count = 0;
            }
        } else {
            self.candidate_count = 0;
        }
    }
}

impl Plugin for GuitarSynth {
    const NAME: &'static str = "Guitar Synth";
    const VENDOR: &'static str = "kiricroman077";
    const URL: &'static str = "https://github.com/kiricroman077-arch/guitar-synth";
    const EMAIL: &'static str = "none@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.setup(buffer_config.sample_rate);
        true
    }

    fn reset(&mut self) {
        self.tracker.reset();
        self.filter.reset();
        self.env = 0.0;
        self.fast_env = 0.0;
        self.slow_env = 0.0;
        self.samples_since_onset = u32::MAX;
        self.active = false;
        self.candidate_count = 0;
        self.peak = 0.0;
        self.unclear_time = 0.0;
        self.amp = 0.0;
        self.accent_env = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let p = self.params.clone();
        let mut st = Settings {
            wave: p.waveform.value(),
            octave: p.octave.value() as f32,
            snap: p.snap.value(),
            fm_ratio: p.fm_ratio.value() as f32,
            gate_on: util::db_to_gain(p.threshold.value()),
            noise_rejection: p.noise_rejection.value(),
            accent: p.accent.value(),
            sustain: p.sustain.value(),
            release: p.release.value(),
            character: 0.0,
            sub_level: 0.0,
            cutoff: 0.0,
            resonance: 0.0,
        };

        for mut channel_samples in buffer.iter_samples() {
            let level = p.level.smoothed.next();
            let mix = p.mix.smoothed.next();
            st.character = p.character.smoothed.next();
            st.sub_level = p.sub_level.smoothed.next();
            st.cutoff = p.cutoff.smoothed.next();
            st.resonance = p.resonance.smoothed.next();

            // Mono sum of the input
            let n = channel_samples.len().max(1) as f32;
            let mut input = 0.0f32;
            for s in channel_samples.iter_mut() {
                input += *s;
            }
            input /= n;

            let synth = self.tick(input, &st) * level;

            for s in channel_samples.iter_mut() {
                *s = *s * (1.0 - mix) + synth * mix;
            }
        }

        ProcessStatus::Normal
    }
}

impl GuitarSynth {
    fn setup(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.tracker = PitchTracker::new(sr);
        self.env_att = coef(0.001, sr);
        self.env_rel = coef(0.060, sr);
        self.fast_rel = coef(0.015, sr);
        self.slow_coef = coef(0.040, sr);
    }

    /// Process one input sample, return one synth sample (before level / mix).
    fn tick(&mut self, input: f32, st: &Settings) -> f32 {
        let sr = self.sample_rate;
        let gate_on = st.gate_on;
        let gate_off = gate_on * 0.4;
        // Sustain flattens the string's natural decay: 0% = follows the string,
        // 100% = stays at full volume until the note ends.
        let decay_curve = 1.0 - 0.95 * st.sustain;
        // (These are cheap enough to compute per sample.)
        let amp_up = coef(0.002, sr);
        let amp_follow = coef(0.030, sr);
        let amp_release = coef(st.release, sr);
        let accent_decay = coef(0.09, sr);
        let pitch_smooth = coef(0.008, sr);
        let refractory = (0.060 * sr) as u32;

        let a = input.abs();

        // Level followers
        let c = if a > self.env { self.env_att } else { self.env_rel };
        self.env = a + c * (self.env - a);
        if a > self.fast_env {
            self.fast_env = a;
        } else {
            self.fast_env *= self.fast_rel;
        }
        self.slow_env = self.fast_env + self.slow_coef * (self.slow_env - self.fast_env);

        // Pick attack (onset): sudden jump in level
        self.samples_since_onset = self.samples_since_onset.saturating_add(1);
        if self.fast_env > gate_on
            && self.fast_env > 2.0 * self.slow_env
            && self.samples_since_onset > refractory
        {
            self.samples_since_onset = 0;
            self.peak = self.env.max(1e-6);
            self.accent_env = 1.0;
        }

        // Gate: note ends when the string dies down
        if self.env < gate_off {
            self.active = false;
        }

        if let Some(d) = self.tracker.push(input) {
            self.on_detection(d.freq, d.clarity, gate_on, st.noise_rejection);
        }

        // Volume follows the string (shaped by Sustain)
        self.peak = self.peak.max(self.env);
        let target_amp = if self.active && self.peak > 0.0 {
            (self.env / self.peak).clamp(0.0, 1.0).powf(decay_curve)
        } else {
            0.0
        };
        let c = if target_amp > self.amp {
            amp_up
        } else if self.active {
            amp_follow
        } else {
            amp_release
        };
        self.amp = target_amp + c * (self.amp - target_amp);
        if !self.active && self.amp < 1e-4 {
            self.peak = 0.0;
        }

        // Pick accent: a short boost in volume and brightness on every attack
        let accent = st.accent * self.accent_env;
        self.accent_env *= accent_decay;

        // Pitch to play
        let target_pitch = if st.snap { self.note } else { self.raw_pitch } + 12.0 * st.octave;
        if st.snap {
            self.pitch = target_pitch;
        } else {
            self.pitch = target_pitch + pitch_smooth * (self.pitch - target_pitch);
        }

        if self.amp > 1e-5 {
            let dt = (midi_to_freq(self.pitch) / sr).min(0.45);
            let raw = self.osc.next(st.wave, dt, st.character, st.fm_ratio, st.sub_level);
            let fc = st.cutoff * 2f32.powf(3.0 * accent);
            let filtered = self.filter.process(raw, fc, st.resonance, sr);
            filtered * self.amp * (1.0 + 0.8 * accent)
        } else {
            self.filter.reset();
            0.0
        }
    }
}

impl ClapPlugin for GuitarSynth {
    const CLAP_ID: &'static str = "com.kiricroman077.guitar-synth";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Guitar-to-synth pitch tracker");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

impl Vst3Plugin for GuitarSynth {
    const VST3_CLASS_ID: [u8; 16] = *b"GuitarSynthKRv01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Synth];
}

nih_export_clap!(GuitarSynth);
nih_export_vst3!(GuitarSynth);

#[cfg(test)]
mod tests {
    use super::*;

    fn pluck(freq: f32, sr: f32, secs: f32, gain: f32, seed: u32) -> Vec<f32> {
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
            buf[i] = 0.998 * 0.5 * (a + b);
            out.push(a * 0.3 * gain);
            i = (i + 1) % n;
        }
        out
    }

    fn settings(wave: Waveform) -> Settings {
        let p = GuitarSynthParams::default();
        Settings {
            wave,
            octave: 0.0,
            snap: true,
            fm_ratio: 2.0,
            gate_on: util::db_to_gain(p.threshold.default_plain_value()),
            noise_rejection: p.noise_rejection.default_plain_value(),
            accent: p.accent.default_plain_value(),
            sustain: p.sustain.default_plain_value(),
            release: p.release.default_plain_value(),
            character: 0.5,
            sub_level: 0.3,
            cutoff: p.cutoff.default_plain_value(),
            resonance: p.resonance.default_plain_value(),
        }
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn rejects_touches_plays_picks() {
        let sr = 48_000.0;
        let mut g = GuitarSynth::default();
        g.setup(sr);
        let st = settings(Waveform::Saw);

        let mut input = vec![0.0f32; (sr * 0.1) as usize];
        // Light string touch: a quiet ringing A plus a short scratch of noise
        let touch_start = input.len();
        let mut rng = 99u32;
        let mut touch = pluck(110.0, sr, 0.3, 0.02, 7);
        for (i, t) in touch.iter_mut().enumerate().take((sr * 0.02) as usize) {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *t += ((rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.01 * (1.0 - i as f32 / (sr * 0.02));
        }
        input.extend(touch);
        input.extend(vec![0.0f32; (sr * 0.2) as usize]);
        // Real picked note: low E, held 2 s
        let pick_start = input.len();
        input.extend(pluck(82.41, sr, 2.0, 1.0, 3));

        let out: Vec<f32> = input.iter().map(|&x| g.tick(x, &st)).collect();
        for &v in &out {
            assert!(v.is_finite());
        }

        let touch_rms = rms(&out[touch_start..pick_start]);
        let first = out[pick_start..].iter().position(|v| v.abs() > 0.01).unwrap();
        let ms = |s: f32| (s * sr) as usize;
        let early = rms(&out[pick_start + ms(0.03)..pick_start + ms(0.13)]);
        let late = rms(&out[pick_start + ms(1.5)..pick_start + ms(1.6)]);
        let input_early = rms(&input[pick_start + ms(0.03)..pick_start + ms(0.13)]);
        let input_late = rms(&input[pick_start + ms(1.5)..pick_start + ms(1.6)]);
        println!(
            "touch output rms {touch_rms:.5}; pick heard after {:.1} ms; synth level 1.5s vs start {:.0}% (string itself: {:.0}%)",
            first as f32 / sr * 1000.0,
            100.0 * late / early,
            100.0 * input_late / input_early
        );
        assert!(touch_rms < 0.001, "touch triggered the synth");
        assert!((first as f32 / sr) < 0.04, "pick too slow");
    }

    #[test]
    fn all_waveforms_are_sane() {
        let sr = 48_000.0;
        for i in 0..Waveform::variants().len() {
            let wave = Waveform::from_index(i);
            let mut g = GuitarSynth::default();
            g.setup(sr);
            let st = settings(wave);
            let input = pluck(196.0, sr, 0.6, 1.0, 5);
            let out: Vec<f32> = input.iter().map(|&x| g.tick(x, &st)).collect();
            let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let level = rms(&out[(sr * 0.1) as usize..(sr * 0.4) as usize]);
            println!("{:>12}: peak {peak:.2}, rms {level:.3}", Waveform::variants()[i]);
            assert!(out.iter().all(|v| v.is_finite()));
            assert!(peak < 3.0 && level > 0.02);
        }
    }
}
