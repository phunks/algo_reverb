# Room-conditioned recursive FDN experiment

> **Research prototype, not the plugin's IR-seed fitter.** Commands require a
> source checkout and optional numerical tools. Measurements and implementation
> status below describe this isolated experiment, not current plugin behavior.

[Research index](README.md) · [Current plugin guide](../../docs/plugins.md)

This isolated prototype adds the **architecture** of
[RIR2FDN](https://github.com/gdalsanto/rir2fdn): a colorless foundation,
delay-dependent frequency-dependent loop attenuation, and output tone correction.
It does not alter Classic, plugin controls, or existing sessions. Runtime is
recursive delay buffers and IIR filters, with **no convolution or retained RIR**.
`StaticFdn::from_room_profile()` prepares the tank outside the audio thread;
processing and reset do not allocate. The original homogeneous constructor remains.

## Scope: not the published implementation

Upstream `get_filters.m` / `inference.m` use estimated EDC parameters to design
per-line absorption and output EQ. Upstream offers GEQ or TwoStage designs;
the paper uses a scattering FDN, and explicitly targets late reverberation rather
than early reflections. This prototype does **not** reproduce DecayFitNet,
scattering matrices, TwoStage, graphic-EQ design, multi-slope/noise fitting, or the
paper's results. No upstream code or neural-network weights were vendored.

Here, NumPy analyzes smooth log-frequency FFT bands (not IEC octave filters),
fits single-slope Schroeder decays after a user-selected late start, screens
noise-floor/range/linearity, and requires at least three accepted bands. Rejected
bands are recorded and omitted; the first-order shelving design spans accepted
centers, and extrapolates outside them. It fits the usual per-line attenuation
`-60 * delay_samples / (sample_rate * T60_band)` dB. Filter group delay is not
compensated, so actual rendered T60 must be measured, especially at low frequency.

Real first-order shelf cascades have stable poles. A conservative interval bound
on the product of section magnitudes certifies strict loop passivity over the
**continuous** frequency interval, not just frequency samples. Rust checks this
again on load. Very lightly damped or highly irregular curves may be attenuated
further to enforce passivity, compromising the target. Design reports retain
both target and actual attenuation. An orthogonal matrix plus passive diagonal
loop filters gives a stable feedback loop.

Tone correction estimates average band energy from densely sampled FDN transfer
functions and targets the fitted late-field initial power/decay. Common stereo
output EQ is capped at +/-36 dB before shelf fitting; this is approximate and
does not fit stereo correlation, time-dependent spectral balance, or early attacks.
The frequency grid can miss narrow modes. Reference onset is detected for analysis
but is **not** copied as runtime predelay. FDN onset is determined by its delays.
No automatic resampling or live source/harmonic tracking is done.

## Run

The Python fitter requires NumPy 2.x and 16/24/32-bit integer PCM WAV. Use a
reference at the base profile's native rate. Keep resampling explicit, e.g. on macOS:

```sh
afconvert -f WAVE -d LEI16@48000 \
  'resources/IMreverbs/Nice Drum Room.wav' \
  target/nice-drum-room-48k.wav

python3 scripts/fit_room_fdn.py \
  target/colorless-8-trial/optimized.fdn \
  target/nice-drum-room-48k.wav \
  target/room-8.fdn --late-ms 80

cargo run --release --features ir-analysis --bin room-fdn -- \
  target/room-audition 3 target/room-8.fdn
```

Output profiles/JSON sidecars and render directories must be new. A room profile
is the complete base profile followed by `room-filters-v1`, then section counts
and `b0 b1 a1` triples for each loop and the two output channels. Coefficient
preparation, parsing, and validation are offline operations, not callback work.

## Initial Nice Drum Room smoke comparison

### Propagation-time / tightness experiment

The fitter also accepts `--delay-scale .5` (allowed range `.125..1`). This
scales and rounds the base delay lengths, keeping the orthogonal matrix and I/O
gains unchanged, then **refits loop attenuation and output tone to the same room**.
It is not a T60 multiplier, extra treble damping, a time-domain gate, or a copied
early-reflection kernel. Colliding scaled lengths are rejected. The scale and
actual lengths are recorded in the JSON sidecar. Decay remains approximate;
shorter lengths change modes and can increase the relative importance of IIR
group delay. The colorless optimization is not rerun, so coloration is possible.
Runtime code and per-sample operation count are unchanged; delay storage decreases.
CPU timings are not guaranteed to decrease (including denormal/tail effects).

The reproducible comparison uses the current fitter for **all** scales, including
the unity-scale control, rather than mixing historical profile revisions:

- `/Users/pnk/RustroverProjects/algo_reverb/target/room-fdn-tightness/salon-controlled/`
- `/Users/pnk/RustroverProjects/algo_reverb/target/room-fdn-tightness/damped-controlled/`

In each directory, `room_0` / `room_1` / `room_2` are 8 lines at scales
1 / .5 / .25; `room_3` / `room_4` / `room_5` are the corresponding 16-line
variants. Each profile was rendered through the Rust recursive DSP for four
seconds at 48 kHz. No reference direct sound or reflections were added.

`scripts/analyze_room_tightness.py`
reads PCM/float WAVs, measures onset, both input-relative and onset-relative
early energy, energy quantile times, and the same late-band decay estimator.
It writes `audition/` copies matched in integrated mean-channel energy to the
first input, followed by one common peak-headroom gain. This is **not LUFS or
perceptual loudness matching**; no onset trimming is applied. `analysis.json`
retains source paths, matching gains, and decay results. Reference direct sound
can dominate its early-energy metrics. These metrics diagnose temporal changes,
not whether a listener finds the result tighter. Listen for metallic modes,
stereo-width changes, and whether room character survives, not only earlier onset.

Historical local artifacts under `target/` (not distributed):

- `room-fdn-reference/`: explicitly resampled reference and 8/16-line profiles
  with analysis/design JSON. This is a provisional room, not the user's agreed target.
- `room-fdn-16-base/`: 200-step seed-1 colorless optimization, batch 512,
  4096 validation frequencies. Uses approximately the eight-line total delay budget.
- `room-fdn-nice-drum-8-vs-16/room_0_wet.wav`: eight-line optimized room FDN.
- `room-fdn-nice-drum-8-vs-16/room_1_wet.wav`: sixteen-line optimized room FDN.
- `room-fdn-nice-drum-8-vs-16/decay-validation.json`: reanalysis of exported WAVs.

Both use the same room analysis and 3 s render length. WAVs receive **one common
peak-headroom attenuation**, not independent peak normalization or energy/LUFS
matching; fitted level differences remain. The timing CSV reports raw energy
before that common gain. Disable convolution auto-leveling, keep dry/wet, EQ,
decay and routing identical, and begin at low volume. These WAVs are LTI FDN
impulses, so convolution audition is valid for the finite exported tail. They
contain no copied dry signal or early RIR; do not judge whole-room attacks as
though they were complete room reconstructions. Avoid serial room -> FDN unless
deliberately evaluating extension: that double-colors/double-decays the input.

Measured T60 examples (seconds; exploratory estimator, not ISO measurement):

| Band | Reference | 8 lines | 16 lines |
| --- | ---: | ---: | ---: |
| 63 Hz | .592 | .639 | .367 |
| 125 Hz | .595 | .584 | .646 |
| 1 kHz | .629 | .630 | .623 |
| 4 kHz | .633 | .635 | .633 |
| 16 kHz | .325 | .446 | .422 |

The endpoints are **not well matched**. Filter group delay, modes, and band
overlap need further work; more lines are not automatically better. Stereo
cross-energy was .307 / .217 (8/16 lines), without reference-width fitting.
Raw total energies were 13.126 / 14.972, so this is not a loudness-controlled test.

One release-build wall-clock pass (48 kHz, 144000 samples, output stores included,
construction excluded) measured 169 / 383 ns per sample, or .81% / 1.84% of
realtime (8/16 lines). Other tests were running: these are noisy smoke timings,
not host/plugin CPU guarantees. No listening preference is claimed. A dry
instrument and confirmed algo_vn routing/settings are still needed to assess
source continuity and whether the extra lines justify their cost.

Validation: Python shelf/passivity/synthetic-decay/recursive-transfer tests;
Rust malformed-profile/passivity/routing/stateful-reset tests; integration test
for zero allocations/deallocations during room processing/reset. Strict Clippy
currently hits an existing `type_complexity` warning in the unrelated FDN fitter;
the scoped run allowing that lint passes.
Exported Rust impulse FFTs also match the Python room transfer functions at
1024 bins to within 5.9e-7 absolute complex error (finite tails and float32 WAV
quantization included). This checks coefficient convention/routing, not perceptual
room similarity.