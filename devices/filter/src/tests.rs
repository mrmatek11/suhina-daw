// Copyright (C) 2026 The Suhina contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use suhina_device_api::conformance::test_signal;
use suhina_test_support::sine;

use super::*;

const SR: f64 = 48_000.0;

fn filter(settings: &[(usize, f64)]) -> Filter {
    let mut f = Filter::new();
    for &(index, value) in settings {
        f.set_param(index, value);
    }
    f.reset();
    f
}

fn stopped() -> ProcessContext {
    ProcessContext { tempo: 120.0, beat: 0.0, playing: false }
}

/// Peak of the last half of a second of a sine at `hz` (level 0.5) through `settings`,
/// relative to the input.
fn gain(settings: &[(usize, f64)], hz: f64) -> f32 {
    let mono = sine(hz, SR, SR as usize, 0.5);
    let mut buffer: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
    filter(settings).process(&stopped(), &mut buffer);
    buffer[buffer.len() / 2..].iter().fold(0.0f32, |m, s| m.max(s.abs())) / 0.5
}

fn db(gain: f32) -> f32 {
    20.0 * gain.log10()
}

suhina_device_api::conformance_test!(Filter::new());

#[test]
fn no_mix_passes_the_signal_through_exactly() {
    let input = test_signal(4_096);
    let mut output = input.clone();
    filter(&[(param::MIX, 0.0), (param::CUTOFF, 100.0)]).process(&stopped(), &mut output);
    assert_eq!(input, output);
}

#[test]
fn each_type_has_its_response() {
    let at = |kind: f64, hz: f64| gain(&[(param::TYPE, kind), (param::CUTOFF, 1_000.0)], hz);
    // Low pass: through below, -3 dB at the cutoff, about -24 dB two octaves up.
    assert!(at(0.0, 100.0) > 0.99);
    assert!((db(at(0.0, 1_000.0)) + 3.0).abs() < 0.2);
    assert!((db(at(0.0, 4_000.0)) + 24.0).abs() < 1.5);
    // High pass, mirrored.
    assert!(at(1.0, 10_000.0) > 0.99 && db(at(1.0, 250.0)) < -22.0);
    // Band pass: unity at the cutoff, down on both sides.
    assert!((at(2.0, 1_000.0) - 1.0).abs() < 0.01);
    assert!(at(2.0, 100.0) < 0.2 && at(2.0, 10_000.0) < 0.2);
    // Notch: nothing at the cutoff, through away from it.
    assert!(at(3.0, 1_000.0) < 0.02 && at(3.0, 100.0) > 0.97);
}

#[test]
fn twenty_four_db_falls_twice_as_fast() {
    let at = |hz: f64| gain(&[(param::SLOPE, 1.0), (param::CUTOFF, 1_000.0)], hz);
    assert!((db(at(4_000.0)) + 48.0).abs() < 3.0, "{}", db(at(4_000.0)));
    // Two Butterworth stages: -6 dB at the cutoff.
    assert!((db(at(1_000.0)) + 6.0).abs() < 0.3);
}

#[test]
fn resonance_raises_a_peak_at_the_cutoff() {
    let plain = gain(&[(param::CUTOFF, 1_000.0)], 1_000.0);
    let resonant = gain(&[(param::CUTOFF, 1_000.0), (param::RESONANCE, 1.0)], 1_000.0);
    // Q 25 against a Butterworth's 0.707.
    assert!((resonant / plain - 25.0 / std::f32::consts::SQRT_2.recip()).abs() < 1.5, "{plain} {resonant}");
}

#[test]
fn morph_moves_from_low_through_band_to_high() {
    let morph = |m: f64, hz: f64| gain(&[(param::TYPE, 4.0), (param::MORPH, m), (param::CUTOFF, 1_000.0)], hz);
    assert!((morph(0.0, 100.0) - gain(&[], 100.0)).abs() < 1e-4);
    assert!((morph(0.5, 1_000.0) - 1.0).abs() < 0.01);
    assert!(morph(1.0, 10_000.0) > 0.99 && morph(1.0, 100.0) < 0.02);
}

#[test]
fn drive_saturates_and_zero_drive_does_not() {
    // A low pass well above the tone, so only the drive changes it.
    let open = [(param::CUTOFF, 20_000.0)];
    let mut hot: Vec<f32> = sine(100.0, SR, 4_800, 0.9).iter().flat_map(|&s| [s, s]).collect();
    let clean = hot.clone();
    filter(&open).process(&stopped(), &mut hot);
    assert!(hot.iter().zip(&clean).all(|(a, b)| (a - b).abs() < 0.01));
    let mut driven = clean.clone();
    filter(&[open[0], (param::DRIVE, 24.0)]).process(&stopped(), &mut driven);
    assert!(driven.iter().all(|s| s.abs() <= 1.0) && driven.iter().any(|s| s.abs() > 0.99));
}

#[test]
fn the_lfo_sweeps_the_cutoff() {
    // A 4 kHz tone through a 1 kHz low pass, the LFO swinging two octaves at 2 Hz: it opens up
    // to the tone and closes again twice a second.
    let settings = [(param::CUTOFF, 1_000.0), (param::LFO_AMOUNT, 2.0), (param::RATE, 2.0)];
    let mono = sine(4_000.0, SR, SR as usize, 0.5);
    let mut buffer: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
    filter(&settings).process(&stopped(), &mut buffer);
    let window = |start: usize| buffer[start * 2..(start + 480) * 2].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // A quarter cycle in (125 ms) the cutoff is at 4 kHz, three quarters in at 250 Hz.
    assert!(window(6_000 - 240) > 0.3, "{}", window(6_000 - 240));
    assert!(window(18_000 - 240) < 0.01, "{}", window(18_000 - 240));
}

#[test]
fn stereo_phase_moves_the_sides_apart() {
    let settings = [(param::CUTOFF, 1_000.0), (param::LFO_AMOUNT, 2.0), (param::RATE, 2.0), (param::PHASE, 0.5)];
    let mono = sine(4_000.0, SR, SR as usize / 4, 0.5);
    let mut buffer: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
    filter(&settings).process(&stopped(), &mut buffer);
    let frames = &buffer.as_chunks::<2>().0[5_000..7_000];
    let left = frames.iter().fold(0.0f32, |m, f| m.max(f[0].abs()));
    let right = frames.iter().fold(0.0f32, |m, f| m.max(f[1].abs()));
    assert!(left > 0.3 && right < 0.01, "{left} {right}");
}

#[test]
fn the_envelope_opens_the_filter_with_the_input() {
    // A loud tone opens a closed low pass by up to six octaves; a quiet one barely moves it.
    let settings = [(param::CUTOFF, 100.0), (param::ENVELOPE_AMOUNT, 6.0)];
    let at = |level: f64| {
        let mono = sine(2_000.0, SR, SR as usize / 2, level);
        let mut buffer: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        filter(&settings).process(&stopped(), &mut buffer);
        buffer[buffer.len() / 2..].iter().fold(0.0f32, |m, s| m.max(s.abs())) / level as f32
    };
    assert!(at(0.9) > 0.5, "{}", at(0.9));
    assert!(at(0.01) < 0.01, "{}", at(0.01));
}

#[test]
fn a_synced_lfo_follows_the_song_position() {
    let settings = [(param::CUTOFF, 1_000.0), (param::LFO_AMOUNT, 2.0), (param::SYNC, 1.0), (param::SYNCED_RATE, 7.0)];
    let render = |beat: f64| {
        // 20 ms, a twenty-fifth of the cycle, the last 5 ms measured.
        let mono = sine(4_000.0, SR, 960, 0.5);
        let mut buffer: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        let context = ProcessContext { tempo: 120.0, beat, playing: true };
        filter(&settings).process(&context, &mut buffer);
        buffer[720 * 2..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    };
    // Quarter-note cycles: a quarter in, the cutoff is two octaves up; three quarters in, down.
    assert!(render(0.25) > 0.3 && render(0.75) < 0.01, "{} {}", render(0.25), render(0.75));
}

#[test]
fn square_lfo_edges_do_not_click() {
    let settings = [(param::CUTOFF, 1_000.0), (param::LFO_AMOUNT, 4.0), (param::RATE, 10.0), (param::SHAPE, 4.0)];
    let mono = sine(200.0, SR, SR as usize, 0.5);
    let mut buffer: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
    filter(&settings).process(&stopped(), &mut buffer);
    let largest_step = buffer.as_chunks::<2>().0.windows(2).map(|w| (w[0][0] - w[1][0]).abs()).fold(0.0, f32::max);
    // A 200 Hz sine at 0.5 moves at most about 0.013 per frame.
    assert!(largest_step < 0.03, "{largest_step}");
}

#[test]
fn a_cutoff_change_glides() {
    let mut f = filter(&[(param::CUTOFF, 20_000.0)]);
    let mut buffer: Vec<f32> = sine(5_000.0, SR, 4_800, 0.5).iter().flat_map(|&s| [s, s]).collect();
    f.set_param(param::CUTOFF, 100.0);
    f.process(&stopped(), &mut buffer);
    let level = |range: std::ops::Range<usize>| buffer[range].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // Still open in the first millisecond, closed after the 20 ms ramp.
    assert!(level(0..96) > 0.3 && level(4_000..9_600) < 0.01);
}
