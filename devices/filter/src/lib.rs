// Copyright (C) 2026 The Suhina contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Filter: a state-variable filter (`suhina_dsp::svf`) with low-pass, high-pass, band-pass,
//! notch and morph (low-pass through band-pass to high-pass) responses, 12 or 24 dB per octave,
//! a drive stage before it, and an LFO and an envelope follower that move the cutoff.
//!
//! 24 dB is two stages in a row: a Butterworth stage, then the resonant one, so the resonance
//! peaks once. Resonance maps to the damping `√2 · (MIN_DAMPING / √2)^resonance`: a Butterworth
//! response at 0, Q 25 at 100 %.
//!
//! The cutoff moves in octaves: `cutoff · 2^(lfo · lfo amount + envelope · envelope amount)`. The
//! LFO runs as auto-pan's does (free in hertz, or on the song position in note lengths), and the
//! right side can be ahead of the left by the stereo phase. The envelope follows the louder side
//! of the input, before the drive, with its own attack and release; 1.0 is a full-scale peak.
//! The modulation goes through a 2 ms one-pole filter, so square and random shapes do not click.
//!
//! Drive is a gain into `tanh`, faded in over its first decibel so 0 dB passes the signal
//! untouched. Docs: `docs/spec/filter.md`.

use suhina_device_api::{
    DEFAULT_SAMPLE_RATE, Device, DeviceError, DeviceInfo, ParamBank, ParamInfo, ProcessContext, Role, Scale, Unit,
    check_sample_rate,
};
use suhina_dsp::lfo::{DEFAULT_SYNCED_RATE, SHAPE_NAMES, SYNCED_RATE_NAMES, SYNCED_RATES, Shape, wave};
use suhina_dsp::svf::BUTTERWORTH_DAMPING;
use suhina_dsp::{OnePole, Smoother, Svf, SvfCoefficients, SvfOutput, time_constant};

/// Ramp for changes of the cutoff, resonance, morph, drive and mix.
pub const RAMP_SECONDS: f64 = 0.02;
/// Time constant of the filter on the cutoff's modulation.
pub const EDGE_SECONDS: f64 = 0.002;
/// Damping at 100 % resonance (Q 25).
pub const MIN_DAMPING: f64 = 0.04;
/// The modulation never moves the cutoff further than this, in octaves.
const MAX_SWING: f64 = 12.0;
/// The envelope never reads above this, so a hot input does not throw the cutoff around.
const MAX_ENVELOPE: f64 = 4.0;

/// Parameter indices, in [`INFO`] order.
pub mod param {
    pub const TYPE: usize = 0;
    pub const SLOPE: usize = 1;
    pub const CUTOFF: usize = 2;
    pub const RESONANCE: usize = 3;
    pub const MORPH: usize = 4;
    pub const DRIVE: usize = 5;
    pub const LFO_AMOUNT: usize = 6;
    pub const SYNC: usize = 7;
    pub const RATE: usize = 8;
    pub const SYNCED_RATE: usize = 9;
    pub const SHAPE: usize = 10;
    pub const PHASE: usize = 11;
    pub const ENVELOPE_AMOUNT: usize = 12;
    pub const ATTACK: usize = 13;
    pub const RELEASE: usize = 14;
    pub const MIX: usize = 15;
    pub(crate) const COUNT: usize = 16;
}

const TYPE_NAMES: [&str; 5] = ["Low Pass", "High Pass", "Band Pass", "Notch", "Morph"];
const SLOPE_NAMES: [&str; 2] = ["12 dB", "24 dB"];

const PARAMS: [ParamInfo; param::COUNT] = [
    ParamInfo::choice("type", "Type", &TYPE_NAMES, 0),
    ParamInfo::choice("slope", "Slope", &SLOPE_NAMES, 0),
    ParamInfo::continuous("cutoff", "Cutoff", (20.0, 20_000.0), 1_000.0, Unit::Hertz, Scale::Logarithmic),
    ParamInfo::continuous("resonance", "Resonance", (0.0, 1.0), 0.0, Unit::Percent, Scale::Linear),
    // 0 low pass, 0.5 band pass, 1 high pass; only heard with the Morph type.
    ParamInfo::continuous("morph", "Morph", (0.0, 1.0), 0.0, Unit::Percent, Scale::Linear),
    ParamInfo::continuous("drive", "Drive", (0.0, 24.0), 0.0, Unit::Decibels, Scale::Linear),
    ParamInfo::continuous("lfoAmount", "LFO Amount", (0.0, 4.0), 0.0, Unit::Octaves, Scale::Linear).in_group("LFO"),
    ParamInfo::toggle("sync", "Sync", false).in_group("LFO"),
    ParamInfo::continuous("rate", "Rate", (0.01, 40.0), 1.0, Unit::Hertz, Scale::Logarithmic).in_group("LFO"),
    ParamInfo::choice("syncedRate", "Synced Rate", &SYNCED_RATE_NAMES, DEFAULT_SYNCED_RATE).in_group("LFO"),
    ParamInfo::choice("shape", "Shape", &SHAPE_NAMES, 0).in_group("LFO"),
    // In cycles: how far the right side's LFO is ahead of the left's.
    ParamInfo::continuous("phase", "Stereo Phase", (0.0, 1.0), 0.0, Unit::None, Scale::Linear).in_group("LFO"),
    ParamInfo::continuous("envelopeAmount", "Envelope Amount", (-6.0, 6.0), 0.0, Unit::Octaves, Scale::Linear)
        .in_group("Envelope"),
    ParamInfo::continuous("attack", "Attack", (0.0005, 0.5), 0.005, Unit::Seconds, Scale::Logarithmic)
        .in_group("Envelope"),
    ParamInfo::continuous("release", "Release", (0.005, 3.0), 0.2, Unit::Seconds, Scale::Logarithmic)
        .in_group("Envelope"),
    ParamInfo::continuous("mix", "Dry/Wet", (0.0, 1.0), 1.0, Unit::Percent, Scale::Linear),
];

pub const INFO: DeviceInfo = DeviceInfo { id: "filter", name: "Filter", role: Role::Effect, params: &PARAMS };

/// The response a stage outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Response {
    Low,
    High,
    Band,
    Notch,
    Morph,
}

impl Response {
    fn pick(self, o: &SvfOutput, c: &SvfCoefficients, morph: f64) -> f64 {
        match self {
            Self::Low => o.low,
            Self::High => o.high,
            Self::Band => o.band_normalised(c),
            Self::Notch => o.notch(),
            Self::Morph => {
                let band = o.band_normalised(c);
                if morph < 0.5 {
                    o.low + (band - o.low) * 2.0 * morph
                } else {
                    band + (o.high - band) * (2.0 * morph - 1.0)
                }
            }
        }
    }
}

pub struct Filter {
    sample_rate: f64,
    params: ParamBank<{ param::COUNT }>,
    /// The cutoff in octaves above 1 Hz (`log2`), so a ramp sweeps evenly in pitch.
    cutoff: Smoother,
    resonance: Smoother,
    morph: Smoother,
    drive: Smoother,
    mix: Smoother,
    /// Per side: the Butterworth stage of 24 dB and the resonant stage.
    stages: [[Svf; 2]; 2],
    /// Per side: the cutoff's modulation in octaves, softened.
    edges: [OnePole; 2],
    /// Whether the edge filters still have to start at the first modulation after a reset.
    fresh: bool,
    envelope: f64,
    attack: f64,
    release: f64,
    /// Free-running LFO position: whole cycles and the fraction of the current one.
    free_cycles: u64,
    free_phase: f64,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        let sr = DEFAULT_SAMPLE_RATE;
        let params = ParamBank::new(&PARAMS);
        let ramp = Smoother::ramp_frames(RAMP_SECONDS, sr);
        let smoother = |value: f64| Smoother::new(value as f32, ramp);
        let mut filter = Self {
            sample_rate: sr,
            cutoff: smoother(params.get(param::CUTOFF).log2()),
            resonance: smoother(params.get(param::RESONANCE)),
            morph: smoother(params.get(param::MORPH)),
            drive: smoother(params.get(param::DRIVE)),
            mix: smoother(params.get(param::MIX)),
            params,
            stages: [[Svf::default(); 2]; 2],
            edges: [OnePole::new(EDGE_SECONDS, sr); 2],
            fresh: true,
            envelope: 0.0,
            attack: 0.0,
            release: 0.0,
            free_cycles: 0,
            free_phase: 0.0,
        };
        filter.update_times();
        filter.reset();
        filter
    }

    fn value(&self, index: usize) -> f64 {
        self.params.get(index)
    }

    fn smoothers(&mut self) -> [&mut Smoother; 5] {
        [&mut self.cutoff, &mut self.resonance, &mut self.morph, &mut self.drive, &mut self.mix]
    }

    fn update_times(&mut self) {
        self.attack = time_constant(self.value(param::ATTACK), self.sample_rate);
        self.release = time_constant(self.value(param::RELEASE), self.sample_rate);
    }

    fn response(&self) -> Response {
        [Response::Low, Response::High, Response::Band, Response::Notch, Response::Morph]
            .get(self.value(param::TYPE) as usize)
            .copied()
            .unwrap_or(Response::Low)
    }

    fn shape(&self) -> Shape {
        Shape::ALL.get(self.value(param::SHAPE) as usize).copied().unwrap_or(Shape::Sine)
    }

    /// Length of one synced cycle in beats.
    fn synced_beats(&self) -> f64 {
        SYNCED_RATES.get(self.value(param::SYNCED_RATE) as usize).map_or(1.0, |r| r.1)
    }
}

/// The drive stage: `tanh` of the gained signal, faded in over the first decibel.
#[inline]
fn drive(x: f64, drive_db: f64) -> f64 {
    if drive_db <= 0.0 {
        return x;
    }
    let shaped = (x * 10.0f64.powf(drive_db / 20.0)).tanh();
    x + (shaped - x) * drive_db.min(1.0)
}

impl Device for Filter {
    fn info(&self) -> &'static DeviceInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f64) -> Result<(), DeviceError> {
        let sr = check_sample_rate(sample_rate)?;
        self.sample_rate = sr;
        let ramp = Smoother::ramp_frames(RAMP_SECONDS, sr);
        for smoother in self.smoothers() {
            smoother.set_ramp(ramp);
        }
        self.edges = [OnePole::new(EDGE_SECONDS, sr); 2];
        self.update_times();
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        for smoother in self.smoothers() {
            smoother.reset(smoother.target());
        }
        self.stages = [[Svf::default(); 2]; 2];
        self.fresh = true;
        self.envelope = 0.0;
        self.free_cycles = 0;
        self.free_phase = 0.0;
    }

    fn set_param(&mut self, index: usize, value: f64) {
        let Some(value) = self.params.set(index, value) else { return };
        match index {
            param::CUTOFF => self.cutoff.set_target(value.log2() as f32),
            param::RESONANCE => self.resonance.set_target(value as f32),
            param::MORPH => self.morph.set_target(value as f32),
            param::DRIVE => self.drive.set_target(value as f32),
            param::MIX => self.mix.set_target(value as f32),
            param::ATTACK | param::RELEASE => self.update_times(),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Option<f64> {
        self.params.param(index)
    }

    fn process(&mut self, context: &ProcessContext, buffer: &mut [f32]) {
        let tempo = context.tempo_or_default();
        let beats_per_frame = tempo / 60.0 / self.sample_rate;
        let synced = self.value(param::SYNC) >= 0.5;
        let follows_song = synced && context.playing && context.beat.is_finite();
        let cycles_per_frame =
            if synced { beats_per_frame / self.synced_beats() } else { self.value(param::RATE) / self.sample_rate };
        let (response, shape) = (self.response(), self.shape());
        let two_stages = self.value(param::SLOPE) >= 0.5;
        let lfo_amount = self.value(param::LFO_AMOUNT);
        let envelope_amount = self.value(param::ENVELOPE_AMOUNT);
        let phase = self.value(param::PHASE);

        for (n, frame) in buffer.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let position = if follows_song {
                (context.beat + n as f64 * beats_per_frame) / self.synced_beats()
            } else {
                self.free_cycles as f64 + self.free_phase
            };
            let dry = [f64::from(frame[0]), f64::from(frame[1])];
            let level = dry[0].abs().max(dry[1].abs());
            let level = if level.is_finite() { level.min(MAX_ENVELOPE) } else { 0.0 };
            let coefficient = if level > self.envelope { self.attack } else { self.release };
            self.envelope = level + (self.envelope - level) * coefficient;

            let cutoff = f64::from(self.cutoff.next_value());
            let resonance = f64::from(self.resonance.next_value());
            let morph = f64::from(self.morph.next_value());
            let drive_db = f64::from(self.drive.next_value());
            let mix = f64::from(self.mix.next_value());
            let damping = BUTTERWORTH_DAMPING * (MIN_DAMPING / BUTTERWORTH_DAMPING).powf(resonance);
            let swing = |offset: f64| {
                (lfo_amount * wave(shape, position + offset) + envelope_amount * self.envelope)
                    .clamp(-MAX_SWING, MAX_SWING)
            };
            let swings = [swing(0.0), swing(phase)];
            if self.fresh {
                for (edge, target) in self.edges.iter_mut().zip(swings) {
                    edge.reset(target);
                }
                self.fresh = false;
            }

            for (side, sample) in frame.iter_mut().enumerate() {
                let octaves = cutoff + self.edges[side].process(swings[side]);
                let resonant = SvfCoefficients::new(self.sample_rate, octaves.exp2(), damping);
                let [first, second] = &mut self.stages[side];
                let mut wet = drive(dry[side], drive_db);
                if two_stages {
                    let flat = resonant.with_damping(BUTTERWORTH_DAMPING);
                    wet = response.pick(&first.process(&flat, wet), &flat, morph);
                }
                wet = response.pick(&second.process(&resonant, wet), &resonant, morph);
                let out = dry[side] + (wet - dry[side]) * mix;
                *sample = if out.is_finite() { out as f32 } else { 0.0 };
            }

            if !follows_song {
                self.free_phase += cycles_per_frame;
                if self.free_phase >= 1.0 {
                    self.free_cycles = self.free_cycles.wrapping_add(self.free_phase as u64) % (1 << 32);
                    self.free_phase = self.free_phase.fract();
                }
            }
        }
    }
}

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod tests;
