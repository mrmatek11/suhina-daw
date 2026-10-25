---
description: The built-in filter - its responses and slopes, resonance, drive, the LFO and the envelope follower that move the cutoff, and how it stays stable while it is swept
read_when: filter, filter device, low pass, high pass, band pass, notch, morph, resonance, cutoff, envelope follower, auto filter, state-variable filter, svf
type: spec
status: current
last_verified: 2026-10-25
covers:
  - devices/filter/**
  - crates/dsp/src/svf.rs
---

# Filter

Suhina's `filter` (`devices/filter`): a state-variable filter whose cutoff an LFO and an envelope
follower move, with a drive stage in front. Own design; the filter structure is the
topology-preserving state-variable filter from Vadim Zavalishin's *The Art of VA Filter Design*
and Andrew Simper's published notes (`crates/dsp/src/svf.rs`).

## Signal flow

Per side: input → drive → one or two filter stages → mixed with the dry input by Dry/Wet. The
envelope follower listens to the input before the drive. No latency.

## Parameters

| Id | Name | Range | Default | What it does |
|---|---|---|---|---|
| `type` | Type | Low Pass, High Pass, Band Pass, Notch, Morph | Low Pass | The response. Band pass is unity at the cutoff. |
| `slope` | Slope | 12 dB, 24 dB | 12 dB | 24 dB puts a Butterworth stage before the resonant one, so the resonance peaks once and the cutoff is at -6 dB. |
| `cutoff` | Cutoff | 20 Hz … 20 kHz, logarithmic | 1 kHz | Ramped over 20 ms in octaves, so a jump sweeps evenly in pitch. |
| `resonance` | Resonance | 0 … 100 % | 0 % | Damping `√2 · (0.04 / √2)^resonance`: Butterworth (Q 0.707) at 0, Q 25 at 100 %. Never self-oscillates. |
| `morph` | Morph | 0 … 100 % | 0 % | With Morph: low pass at 0, band pass at 50 %, high pass at 100 %, crossfaded in between. |
| `drive` | Drive | 0 … 24 dB | 0 dB | A gain into `tanh` before the filter, faded in over the first decibel, so 0 dB leaves the signal untouched. |
| `lfoAmount` | LFO Amount | 0 … 4 octaves | 0 | How far the LFO moves the cutoff either way. |
| `sync` | Sync | off, on | off | Rate in hertz, or a note length on the song position. |
| `rate` | Rate | 0.01 … 40 Hz | 1 Hz | Free rate. |
| `syncedRate` | Synced Rate | 8 Bars … 1/32, dotted and triplet | 1/4 | The note lengths auto-pan offers (`suhina_dsp::lfo::SYNCED_RATES`). |
| `shape` | Shape | Sine, Triangle, Saw Up, Saw Down, Square, Random | Sine | `suhina_dsp::lfo::wave`. |
| `phase` | Stereo Phase | 0 … 1 cycle | 0 | How far the right side's LFO runs ahead of the left's. |
| `envelopeAmount` | Envelope Amount | -6 … +6 octaves | 0 | The cutoff moves by this many octaves for a full-scale input; negative closes it. |
| `attack` | Attack | 0.5 ms … 0.5 s | 5 ms | Time constant of the follower while the level rises. |
| `release` | Release | 5 ms … 3 s | 200 ms | Time constant while it falls. |
| `mix` | Dry/Wet | 0 … 100 % | 100 % | 0 % passes the input through exactly. |

## The modulated cutoff

The cutoff a sample is filtered at is `cutoff · 2^m`, where
`m = lfoAmount · lfo + envelopeAmount · envelope`, limited to ±12 octaves and to the frequencies
the sample rate allows. `m` goes through a 2 ms one-pole filter per side, so the square and random
shapes, and jumps of the amounts, glide instead of clicking. The LFO follows the song position
while the transport runs and Sync is on, and runs freely otherwise, as auto-pan's does. The
envelope follows the louder side's absolute value, read at most at 4.0, so a hot input cannot
throw the cutoff around.

The filter's coefficients are worked out every sample from the modulated cutoff. The
trapezoidal state-variable structure keeps its state meaningful while they change, so a sweep at
any speed and the highest resonance stay bounded (`crates/dsp/src/svf.rs` tests).

## Tests

`devices/filter/src/tests.rs` checks each response at and around the cutoff, the 24 dB slope,
the resonance peak, morph, drive, the LFO free and synced, the stereo phase, the envelope, click-free
square edges and the cutoff ramp; `golden_tests.rs` compares eight scenarios with the renders in
`devices/filter/tests/golden`.

## Import

Live's Auto Filter is meant to import into this device; the mapping is not written yet
([`device-import.md`](device-import.md)).
