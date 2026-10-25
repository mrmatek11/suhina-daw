// Copyright (C) 2026 The Suhina contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A state-variable filter in the topology-preserving (trapezoidal) form, with low-pass,
//! band-pass and high-pass outputs from one pass. The structure follows Vadim Zavalishin's *The
//! Art of VA Filter Design* and Andrew Simper's published "Linear trap integrated SVF" notes; the
//! code is our own. Unlike a biquad, its state stays meaningful when the cutoff moves every
//! sample, so it can be swept by an LFO or an envelope without zipper noise or blowing up.

use crate::{flush, usable_frequency, usable_sample_rate};

/// Damping of a Butterworth response (`1 / Q` with Q = 1/√2).
pub const BUTTERWORTH_DAMPING: f64 = std::f64::consts::SQRT_2;

/// The filter's coefficients for one cutoff and damping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfCoefficients {
    /// `tan(π · cutoff / sample rate)`, the integrators' gain.
    g: f64,
    /// Damping, `1 / Q`: 2 is no resonance, towards 0 a sharper peak at the cutoff.
    k: f64,
    a1: f64,
    a2: f64,
    a3: f64,
}

impl SvfCoefficients {
    /// Cutoff `frequency` in Hz (kept below Nyquist) and `damping` (`1 / Q`, kept within
    /// 0.01 … 2; a non-finite value is a Butterworth response).
    pub fn new(sample_rate: f64, frequency: f64, damping: f64) -> Self {
        let sample_rate = usable_sample_rate(sample_rate);
        let g = (std::f64::consts::PI * usable_frequency(frequency, sample_rate) / sample_rate).tan();
        Self::from_gain(g, damping)
    }

    /// The same cutoff with another damping, without computing the cutoff again.
    pub fn with_damping(&self, damping: f64) -> Self {
        Self::from_gain(self.g, damping)
    }

    fn from_gain(g: f64, damping: f64) -> Self {
        let k = if damping.is_finite() { damping.clamp(0.01, 2.0) } else { BUTTERWORTH_DAMPING };
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { g, k, a1, a2, a3: g * a2 }
    }

    /// The damping these coefficients were made with.
    pub fn damping(&self) -> f64 {
        self.k
    }
}

/// The three outputs of one sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfOutput {
    pub low: f64,
    /// Band-pass with its peak at `1 / damping`; times the damping it peaks at unity.
    pub band: f64,
    pub high: f64,
}

impl SvfOutput {
    /// Band-pass scaled to unity gain at the cutoff.
    #[inline]
    pub fn band_normalised(&self, coefficients: &SvfCoefficients) -> f64 {
        self.band * coefficients.k
    }

    /// Notch: everything but the cutoff.
    #[inline]
    pub fn notch(&self) -> f64 {
        self.low + self.high
    }
}

/// One channel of the filter: two integrator states.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Svf {
    ic1: f64,
    ic2: f64,
}

impl Svf {
    #[inline]
    pub fn process(&mut self, c: &SvfCoefficients, x: f64) -> SvfOutput {
        let x = if x.is_finite() { x } else { 0.0 };
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        SvfOutput { low: v2, band: v1, high: x - c.k * v1 - v2 }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// Steady-state gain of `pick` for a sine at `frequency`, after a second to settle.
    fn gain(c: SvfCoefficients, frequency: f64, pick: fn(&SvfOutput, &SvfCoefficients) -> f64) -> f64 {
        let mut f = Svf::default();
        let w = std::f64::consts::TAU * frequency / SR;
        let mut peak = 0.0f64;
        for n in 0..SR as usize * 2 {
            let y = pick(&f.process(&c, (w * n as f64).sin()), &c);
            if n > SR as usize {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn outputs_have_the_expected_responses() {
        let c = SvfCoefficients::new(SR, 1_000.0, BUTTERWORTH_DAMPING);
        let low = |o: &SvfOutput, _: &SvfCoefficients| o.low;
        let high = |o: &SvfOutput, _: &SvfCoefficients| o.high;
        let band = |o: &SvfOutput, c: &SvfCoefficients| o.band_normalised(c);
        let notch = |o: &SvfOutput, _: &SvfCoefficients| o.notch();
        // -3 dB at the cutoff for the Butterworth low and high pass, 12 dB/octave beyond.
        assert!((gain(c, 1_000.0, low) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert!((gain(c, 1_000.0, high) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert!(gain(c, 100.0, low) > 0.99 && gain(c, 4_000.0, low) < 0.07);
        assert!(gain(c, 10_000.0, high) > 0.99 && gain(c, 250.0, high) < 0.07);
        assert!((gain(c, 1_000.0, band) - 1.0).abs() < 0.01);
        assert!(gain(c, 1_000.0, notch) < 0.01 && gain(c, 100.0, notch) > 0.98);
    }

    #[test]
    fn low_damping_peaks_at_the_cutoff() {
        let c = SvfCoefficients::new(SR, 1_000.0, 0.1);
        // Q = 10: the low pass is ten times louder at the cutoff.
        assert!((gain(c, 1_000.0, |o, _| o.low) - 10.0).abs() < 0.3);
        let same = SvfCoefficients::new(SR, 1_000.0, BUTTERWORTH_DAMPING).with_damping(0.1);
        assert_eq!(same, c);
    }

    #[test]
    fn a_cutoff_swept_every_sample_stays_bounded() {
        let mut f = Svf::default();
        let mut largest = 0.0f64;
        for n in 0..SR as usize {
            let frequency = 20.0 * 1_000.0f64.powf(0.5 + 0.5 * (n as f64 * 0.01).sin());
            let c = SvfCoefficients::new(SR, frequency, 0.01);
            let x = if n % 2 == 0 { 1.0 } else { -1.0 };
            largest = largest.max(f.process(&c, x).low.abs());
        }
        assert!(largest.is_finite() && largest < 1_000.0, "{largest}");
    }

    #[test]
    fn hostile_values_are_kept_usable() {
        for (frequency, damping) in [(f64::NAN, f64::NAN), (1e9, -1.0), (-5.0, 1e9)] {
            let c = SvfCoefficients::new(SR, frequency, damping);
            let mut f = Svf::default();
            let o = f.process(&c, f64::INFINITY);
            assert!(o.low.is_finite() && o.band.is_finite() && o.high.is_finite());
        }
    }
}
