//! Guitar Synth: listens to a guitar, tracks the note being played and plays a
//! synth on that note. Stage 1: monophonic tracking, snap-to-note, basic waveforms.

use nih_plug::prelude::*;
use std::f32::consts::TAU;
use std::sync::Arc;

mod tracker;
use tracker::PitchTracker;

/// How clearly pitched the signal must be before we trust a detection.
const MIN_CLARITY: f32 = 0.8;
/// How many agreeing detections (~2 ms apart) are needed to start or change a note.
const AGREE_COUNT: u32 = 2;

#[derive(Enum, PartialEq, Clone, Copy)]
enum Waveform {
    #[name = "Saw"]
    Saw,
    #[name = "Square"]
    Square,
    #[name = "Sine"]
    Sine,
}

#[derive(Params)]
struct GuitarSynthParams {
    #[id = "wave"]
    waveform: EnumParam<Waveform>,

    #[id = "octave"]
    octave: IntParam,

    #[id = "snap"]
    snap: BoolParam,

    #[id = "thresh"]
    threshold: FloatParam,

    #[id = "level"]
    level: FloatParam,

    #[id = "mix"]
    mix: FloatParam,
}

impl Default for GuitarSynthParams {
    fn default() -> Self {
        Self {
            waveform: EnumParam::new("Waveform", Waveform::Saw),

            octave: IntParam::new("Octave", 0, IntRange::Linear { min: -2, max: 2 }),

            snap: BoolParam::new("Snap to Note", true),

            threshold: FloatParam::new(
                "Input Threshold",
                -50.0,
                FloatRange::Linear { min: -80.0, max: -10.0 },
            )
            .with_unit(" dB")
            .with_step_size(0.5),

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

            mix: FloatParam::new("Dry/Wet", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
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

// PolyBLEP: smooths the sharp edges of saw/square so they don't alias harshly.
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

fn oscillator(wave: Waveform, phase: f32, dt: f32) -> f32 {
    match wave {
        Waveform::Sine => (phase * TAU).sin(),
        Waveform::Saw => (2.0 * phase - 1.0) - poly_blep(phase, dt),
        Waveform::Square => {
            let naive = if phase < 0.5 { 1.0 } else { -1.0 };
            naive + poly_blep(phase, dt) - poly_blep((phase + 0.5) % 1.0, dt)
        }
    }
}

struct GuitarSynth {
    params: Arc<GuitarSynthParams>,
    sample_rate: f32,
    tracker: PitchTracker,

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

    // Synth state
    phase: f32,
    pitch: f32, // smoothed pitch actually played (MIDI)
    amp: f32,
}

impl Default for GuitarSynth {
    fn default() -> Self {
        Self {
            params: Arc::new(GuitarSynthParams::default()),
            sample_rate: 48_000.0,
            tracker: PitchTracker::new(48_000.0),
            env: 0.0,
            fast_env: 0.0,
            slow_env: 0.0,
            env_att: 0.0,
            env_rel: 0.0,
            fast_rel: 0.0,
            slow_coef: 0.0,
            samples_since_onset: 0,
            active: false,
            note: 60.0,
            raw_pitch: 60.0,
            candidate: 0.0,
            candidate_count: 0,
            peak: 0.0,
            phase: 0.0,
            pitch: 60.0,
            amp: 0.0,
        }
    }
}

impl GuitarSynth {
    fn agree(&mut self, value: f32) -> bool {
        if self.candidate_count > 0 && (value - self.candidate).abs() < 0.5 {
            self.candidate_count += 1;
        } else {
            self.candidate = value;
            self.candidate_count = 1;
        }
        self.candidate_count >= AGREE_COUNT
    }

    fn on_detection(&mut self, freq: f32, clarity: f32, gate_on: f32) {
        if clarity < MIN_CLARITY || !freq.is_finite() {
            return;
        }
        let midi = freq_to_midi(freq);
        let snapped = midi.round();

        if !self.active {
            if self.env > gate_on && self.agree(snapped) {
                self.active = true;
                self.note = snapped;
                self.raw_pitch = midi;
                self.pitch = midi;
                self.peak = self.peak.max(self.env);
                self.candidate_count = 0;
            }
            return;
        }

        // Exact pitch for free mode: follow small, continuous movements (bends,
        // vibrato) but ignore sudden jumps, which are usually octave errors.
        if (midi - self.raw_pitch).abs() < 1.0 {
            self.raw_pitch = midi;
        }

        if snapped != self.note {
            if self.agree(snapped) {
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
        let sr = buffer_config.sample_rate;
        self.sample_rate = sr;
        self.tracker = PitchTracker::new(sr);
        self.env_att = coef(0.001, sr);
        self.env_rel = coef(0.060, sr);
        self.fast_rel = coef(0.015, sr);
        self.slow_coef = coef(0.040, sr);
        true
    }

    fn reset(&mut self) {
        self.tracker.reset();
        self.env = 0.0;
        self.fast_env = 0.0;
        self.slow_env = 0.0;
        self.active = false;
        self.candidate_count = 0;
        self.peak = 0.0;
        self.amp = 0.0;
        self.phase = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let sr = self.sample_rate;
        let wave = self.params.waveform.value();
        let octave = self.params.octave.value() as f32;
        let snap = self.params.snap.value();
        let gate_on = util::db_to_gain(self.params.threshold.value());
        let gate_off = gate_on * 0.5;

        let pitch_smooth = coef(0.008, sr);
        let amp_up = coef(0.002, sr);
        let amp_down = coef(0.030, sr);
        let refractory = (0.060 * sr) as u32;

        for mut channel_samples in buffer.iter_samples() {
            let level = self.params.level.smoothed.next();
            let mix = self.params.mix.smoothed.next();

            // Mono sum of the input
            let n = channel_samples.len().max(1) as f32;
            let mut input = 0.0f32;
            for s in channel_samples.iter_mut() {
                input += *s;
            }
            input /= n;
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
            }

            // Gate: note ends when the string dies down
            if self.env < gate_off {
                self.active = false;
                self.peak = 0.0;
            }

            if let Some(d) = self.tracker.push(input) {
                self.on_detection(d.freq, d.clarity, gate_on);
            }

            // Volume follows the string: full at the pick, fading as it decays
            self.peak = self.peak.max(self.env);
            let target_amp = if self.active && self.peak > 0.0 {
                (self.env / self.peak).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let c = if target_amp > self.amp { amp_up } else { amp_down };
            self.amp = target_amp + c * (self.amp - target_amp);

            // Pitch to play
            let target_pitch = if snap { self.note } else { self.raw_pitch } + 12.0 * octave;
            if snap {
                self.pitch = target_pitch;
            } else {
                self.pitch = target_pitch + pitch_smooth * (self.pitch - target_pitch);
            }

            let synth = if self.amp > 1e-5 {
                let dt = (midi_to_freq(self.pitch) / sr).min(0.45);
                let s = oscillator(wave, self.phase, dt);
                self.phase += dt;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }
                s * self.amp * level
            } else {
                0.0
            };

            for s in channel_samples.iter_mut() {
                *s = *s * (1.0 - mix) + synth * mix;
            }
        }

        ProcessStatus::Normal
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
