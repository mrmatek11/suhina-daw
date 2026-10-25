# Golden renders of the filter

`renders_like_the_golden_files` (in `src/golden_tests.rs`) renders every scenario listed there and
compares it, sample for sample (within 1e-5), with the file of the same name here. The other tests
check what the filter must do; these catch any change in how it sounds.

Each scenario sets some controls and feeds one second of noise at 16 kHz, in bursts (0.5 for
100 ms, then 0.05 for 150 ms, different on each side), in blocks of 256 frames at 120 BPM with the
transport running. Each file is little-endian 32-bit floats, left and right interleaved, every
fourth frame.

## Making the renders again

Only when a change to the sound is intended, and the spec says so:

```sh
SUHINA_BLESS=1 cargo test -p suhina-device-filter renders_like_the_golden_files
```
