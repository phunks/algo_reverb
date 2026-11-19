# IR-free colorless FDN experiment

> **Research prototype, not the plugin architecture.** Commands require a source
> checkout and its optional research scripts. The live plugin uses DiffFDN, not
> this optimizer. Measurements below are historical, not current performance claims.

[Research index](README.md) · [Current plugin guide](../../docs/plugins.md)

This is an isolated experiment inspired by
[Optimizing tiny colorless feedback delay networks (2025)](https://doi.org/10.1186/s13636-025-00401-w).
It does not change Classic, plugin controls or existing sessions. No measured IR
is used for training, and WAV rendering runs a live recursive FDN, not convolution.

## Scope and departures from the paper

- NumPy is the only Python dependency. There is no PyTorch requirement.
- Optimize an orthogonal feedback matrix and mono input / stereo output gains.
  Analytic frequency-domain gradients and a Cayley retraction preserve matrix
  orthogonality. This is stochastic gradient descent with batch backtracking,
  **not** the paper's matrix-exponential/Adam implementation or a reproduction of
  its measured results.
- The loss is stereo-output plus mono-mid magnitude MSE toward unity, and the
  paper's matrix-density penalty (weight 1). There is no individual delay-channel
  spectral loss. Mid is an experimental extension; it does not guarantee mono
  compatibility or stereo decorrelation. Inspect the exported stereo metrics.
- Training is at 48 kHz, homogeneous T60 = 1.44 s. Delays stay fixed.
  No attenuation EQ, scattering matrix, input/loop diffusion or modulation.
- At eight lines, initialization uses Classic's nominal delays, Hadamard matrix
  and signed input/output weights. The baseline intentionally omits Classic's
  lowpass damping: it is a matched static baseline, **not** the complete plugin.
- Line count is configurable from 2 to 32 at training/construction time, not
  automated during playback. Other counts start from Householder, deterministic
  signed gains and logarithmic prime delays with approximately the same total
  delay budget as eight lines. This is not the paper's exact delay table.
- A held-out frequency set selects the best checkpoint; it is validation data,
  not an independent final test. Multiple seeds and fresh frequency sets are
  needed before making a general performance claim.

## Run

From the repository root (all output directories must be new):

```sh
python3 scripts/optimize_fdn.py target/colorless-8 --lines 8 --steps 600
cargo run --release --features ir-analysis --bin fdn-compare -- \
  target/colorless-8/initial.fdn target/colorless-8/optimized.fdn target/colorless-8-impulse 2

# Compare other line counts independently, without overwriting the previous run.
python3 scripts/optimize_fdn.py target/colorless-6 --lines 6 --steps 600

# Optional instrument WAV, native 48 kHz; stereo input is averaged to mono.
cargo run --release --features ir-analysis --bin fdn-compare -- \
  target/colorless-8/initial.fdn target/colorless-8/optimized.fdn \
  target/colorless-8-instrument 2 /absolute/path/to/instrument.wav
```

`initial.fdn` / `optimized.fdn` store native rate, count, integer delays,
row-major feedback matrix, input vector and two output vectors. The Rust loader
rejects nonfinite coefficients and nonorthogonal feedback. Changing rate or
delays requires retraining; profiles are not implicitly resampled.

Training writes validation total/spectral/density losses and orthogonality error
to `training.json`. Rendering writes two wet-only float WAVs, `metrics.csv` and
`report.txt`. The optional WAV is processed directly through each FDN with a
T60+1 s trailing silence; both complete renders are held in memory.

WAVs are matched by **total stereo energy**, then attenuated together if needed
to keep peak below 0.8. This is not LUFS matching and can retain perceived
loudness differences. Do not peak-normalize each WAV separately. Start at low
volume, audition blind, check mono, and compare the same dry signal and wet mix.
Use several instruments, T60 values, line counts and optimizer seeds. A lower
training loss alone is not proof of better sound or better instrument integration.
This homogeneous comparison has no band-dependent damping, so this deliberately
bright test is not intended as a final reverb preset. The separate
[room-conditioned experiment](room_fdn.md)
adds RIR2FDN-inspired loop attenuation and output tone fitting.

```sh
python3 -m unittest discover -s scripts -p 'test_optimize_fdn.py' -v
cargo test --features ir-analysis colorless
```

## Initial smoke experiment

One seed (1), 200 steps, batch 512, 4096 held-out frequency points:

| Lines | Validation spectral MSE before | After | L/R cross-energy before / after (T60 2 s) |
| --- | --- | --- | --- |
| 8 | 0.40253 | 0.25595 | -0.289 / 0.182 |
| 6 | 0.32967 | 0.20448 | -0.035 / 0.540 |

These are separate baselines, not a controlled ranking of line counts. The
stereo change, especially at six lines, is substantial: the current mid-inclusive
objective can narrow the image. This must be assessed before adopting any
coefficients. No listening preference is claimed. Native-rate impulse FFT spot
checks agree with the Python transfer function within 2.4e-5 absolute complex
error (finite tail truncation and float32 WAV export included). Rendered pair
energies agree to better than 1e-10 relative error.