// Copyright (C) 2026 The Suhina contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Golden renders: every scenario below is rendered and compared with its file in
//! `tests/golden` (see the README there).

use std::path::{Path, PathBuf};

use super::*;

/// Largest difference from a golden file: above the last bits of an f32, so another platform's
/// libm (tan, exp2, tanh) does not fail it.
const TOLERANCE: f32 = 1e-5;
const BLOCK: usize = 256;
const TEMPO: f64 = 120.0;
const SAMPLE_RATE: f64 = 16_000.0;
/// Every n-th frame is kept in the file.
const KEEP: usize = 4;

struct Scenario {
    name: &'static str,
    settings: &'static [(usize, f64)],
}

const SCENARIOS: &[Scenario] = &[
    Scenario { name: "defaults", settings: &[] },
    Scenario { name: "high-pass-24", settings: &[(param::TYPE, 1.0), (param::SLOPE, 1.0), (param::CUTOFF, 2_000.0)] },
    Scenario {
        name: "band-resonant",
        settings: &[(param::TYPE, 2.0), (param::CUTOFF, 800.0), (param::RESONANCE, 0.8)],
    },
    Scenario { name: "notch", settings: &[(param::TYPE, 3.0), (param::CUTOFF, 1_500.0), (param::RESONANCE, 0.3)] },
    Scenario {
        name: "morph-drive",
        settings: &[(param::TYPE, 4.0), (param::MORPH, 0.3), (param::DRIVE, 12.0), (param::MIX, 0.7)],
    },
    Scenario {
        name: "lfo-stereo",
        settings: &[(param::LFO_AMOUNT, 3.0), (param::RATE, 3.0), (param::SHAPE, 1.0), (param::PHASE, 0.25)],
    },
    Scenario {
        name: "lfo-synced",
        settings: &[(param::LFO_AMOUNT, 2.0), (param::SYNC, 1.0), (param::SYNCED_RATE, 10.0), (param::SHAPE, 5.0)],
    },
    Scenario {
        name: "envelope",
        settings: &[
            (param::CUTOFF, 200.0),
            (param::ENVELOPE_AMOUNT, 5.0),
            (param::RESONANCE, 0.5),
            (param::SLOPE, 1.0),
        ],
    },
];

/// Repeatable noise from -1.0 to 1.0, different on each side, the same on every platform.
fn noise(frame: usize, side: u64) -> f32 {
    let mut x = (frame as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ side.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 29;
    ((x >> 40) as f32 / (1u64 << 23) as f32) - 1.0
}

/// Noise in bursts of 100 ms every quarter second, so the envelope has something to follow.
fn input(frame: usize) -> [f32; 2] {
    let level = if frame % 4_000 < 1_600 { 0.5 } else { 0.05 };
    [noise(frame, 1) * level, noise(frame, 2) * level]
}

fn render(s: &Scenario) -> Vec<f32> {
    let mut f = Filter::new();
    for &(index, value) in s.settings {
        f.set_param(index, value);
    }
    f.prepare(SAMPLE_RATE).unwrap();
    let frames = SAMPLE_RATE as usize;
    let mut buffer: Vec<f32> = (0..frames).flat_map(input).collect();
    for (n, block) in buffer.chunks_mut(BLOCK * 2).enumerate() {
        let beat = (n * BLOCK) as f64 / SAMPLE_RATE * TEMPO / 60.0;
        f.process(&ProcessContext { tempo: TEMPO, beat, playing: true }, block);
    }
    buffer.as_chunks::<2>().0.iter().step_by(KEEP).flatten().copied().collect()
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn renders_like_the_golden_files() {
    for s in SCENARIOS {
        let ours = render(s);
        assert!(ours.iter().all(|v| v.is_finite()), "{}: not finite", s.name);
        assert!(ours.iter().any(|v| v.abs() > 0.01), "{} is silent", s.name);
        suhina_test_support::assert_golden_f32(&golden_dir().join(format!("{}.f32", s.name)), &ours, TOLERANCE);
    }
}

#[test]
fn every_scenario_has_its_own_file() {
    let mut names: Vec<String> = SCENARIOS.iter().map(|s| s.name.to_owned()).collect();
    names.sort_unstable();
    let mut files: Vec<String> = std::fs::read_dir(golden_dir())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".f32").map(str::to_owned))
        .collect();
    files.sort_unstable();
    assert_eq!(names, files);
}
