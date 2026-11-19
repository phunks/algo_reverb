# DiffFDN / RIR2FDN inference port

> **Engineering and research log.** Preparation commands require the source
> checkout, optional scripts and separately installed upstream tools. The plugin
> already embeds the two eight-line no-edge room fits; statements about pending
> integration below describe earlier experiments. User IR seeds now use a separate
> Rust-only fitter, not this offline Python/MATLAB/Octave pipeline.

[Research index](README.md) · [Current plugin guide](../../docs/plugins.md) ·
[Coefficient provenance and licensing](../../THIRD_PARTY_NOTICES.md)

This is a coefficient-compatible **transposed scattering inference path**, not
an end-to-end verified reproduction of the published listening examples yet.
The existing static/short-delay profiles and fitter are unchanged.

## Verified source

- RIR2FDN: <https://github.com/gdalsanto/rir2fdn>, commit
  `4840fafebb48b9140492dd6c4f688e881cb9fad0`.
- Its pinned diff-fdn-colorless: `2497bd0731d0d8f8959a0843bfb726c1ef801f7d`.
- Its pinned Two_stage_filter: `dcd149a1deef80bd5419bc42974de488e5f00ea0`.
- Its pinned fdnToolbox: `5f0c29b42e3682463cd0a0a75d32149949247731`.
- Its pinned DecayFitNet: `01daf3e7bbfd637aa1269bbca0cab7f445db0d5d`.

`inference.m` uses 48 kHz, third-octave TwoStage/acge3 filters and the
transposed path. `model.py` specifies four orthogonal scattering stages,
sparsity 3 and randomized left/right delays. Main delay lengths for the paper
are `[593, 743, 929, 1153, 1399, 1699]`.

The runtime implements the companion toolbox's `processTransposedFDN`:

1. Read main delay outputs, add input gains times the input.
2. Apply right delays, four matrices/interstage delays, then left delays.
3. Apply per-line absorption SOS.
4. Write these samples to the main delays and take the output gain sum here.
5. Apply the shared tone-correction SOS to that sum.

Linewise output filtering by the same tone filter commutes with summation.
The main delays are `delays-delayLeft-delayRight`. Filter design uses the
upstream convention `delays+size(feedbackMatrix,3)/2`, not those shortened
main delays. There are no explicit early reflections and no dry path.
Output is mono, duplicated in the stereo renderer; artificial stereo width
is deliberately not added.

## Preparing real coefficients

Use a recursively cloned copy of the upstream repository and its dependencies.
The repository contains SSH submodule URLs; a per-command HTTPS rewrite can
be needed. Keep upstream code separate from this project's source tree.

In the pinned `informed/diff-fdn-colorless` directory, run upstream `solver.py`
with `--scattering --alpha 1 --device cpu --delays 593 743 929 1153 1399 1699`
and a new `--train_dir`. Upstream defaults are 480000 frequency samples,
batch size 2000, 10 epochs and Adam learning rate .001. Record all settings.
Use upstream `rir_analysis.py --octave_bands 3` for DecayFitNet estimates.
Do **not** use `--reference_ir` as a substitute for the MATLAB TwoStage path:
the Python filter designer is a different path.

In MATLAB, call this project's `scripts/export_difffdn_filters.m` with the
upstream checkout, `scat_parameters.mat`, the room's EDC MAT file, original
RIR length in samples, and a new output MAT filename. It calls the actual
upstream `get_filters('TwoStage', ..., 3, 48000, ...)` function.
MATLAB with Signal Processing Toolbox is required.
This MATLAB wrapper has not been executed in the current environment.

Then run this project's `scripts/import_difffdn.py` with four positional paths:
`parameters.mat`, `scat_parameters.mat`, exported filter MAT, new `.fdn` output.
NumPy and SciPy are required, as in upstream.

The importer recovers the **realized** interstage delays from the saved
polynomial and raw learned stage parameters. It checks the reconstruction
instead of re-running the randomized upstream construction. It rejects
incompatible exports. This also catches stale `feedbackMatrix` exports.
The companion implementation caches this polynomial during forward passes;
when producing final exports, refresh the forward pass after the final
optimizer update before calling its parameter-saving function.

Render the profile with the existing `room-fdn` binary (`ir-analysis` feature).
The profile rate is explicit; there is no implicit resampling. The runtime
uses delay buffers, matrix products and SOS only, no FIR/FFT convolution.

## Validation and limits

Run `cargo test --lib difffdn` and Python unittest discovery for
`scripts/test_import_difffdn.py`. The Python integration test compares the
Rust WAV against an independent dense polynomial recursion with nonzero
left/right/interstage delays and stateful absorption/tone filters. The dense
convolution is **test-only**, never a runtime renderer.

An additional manual check on upstream's supplied `SCAT/N6D1A` learned
`parameters.mat` / `param_fdnToolbox.mat` pair recovered all three interstage
delay vectors, with maximum polynomial reconstruction error `1.69e-7`.
This checks an actual learned scattering matrix, not room-filter parity.

Parsing validates dimensions, bounded delay allocations, finite coefficients,
orthogonal stages and stable SOS. It does not certify closed-loop passivity:
an imported stable SOS cascade can still have gain above one. Only import
trusted offline fits and check decay/stability over long renders before use.
The plugin now uses the eight-line no-edge fits (see the integration section). Published-room
coefficients, full-budget optimization, MATLAB parity and listening comparisons
still need validation; synthetic integration tests do not establish perceptual
equivalence to the published DiffFDN.

## Room validation tools

`scripts/prepare_difffdn.py` accepts the pinned upstream checkout, raw learned
parameters, scattering MAT, a **new** output directory and reference WAVs.
It explicitly resamples 44.1 kHz sources with SciPy `resample_poly` to 48 kHz,
uses channel zero (as upstream does), and runs the pinned single-slope
DecayFitNet/ONNX model at the 25 third-octave frequencies. Its JSON records
source hashes, original rates/lengths, realized scattering delays and estimates.
Required offline packages: torch, torchaudio, onnx, onnxruntime, soundfile,
matplotlib, h5py, NumPy and SciPy. These are upstream dependencies, not Rust
runtime dependencies.

`scripts/export_difffdn_octave.m` is a separately labelled Octave adapter for
the pinned `twoFilters` / `acge3` code (requires `signal`). It duplicates
`get_filters`' fixed third-octave padding, correction factors and 8 kHz shelf
crossover. Band energy is calculated in Python from the same fifth-order
Butterworth design and 48001-sample impulse as MATLAB's helper. This avoids
Octave's different SOS padding; it is **not evidence of MATLAB parity**.
The MATLAB exporter remains the authoritative next check when licensed MATLAB
is available. The only scalar-string compatibility helper is isolated under
`scripts/octave_compat`; it is not added to the MATLAB path.

`scripts/validate_difffdn.py` imports the filters, renders Rust, and compares
against an independent block recursion using the original saved polynomial
(not its factorization). FFT convolution is used **only by this offline oracle**.
The oracle is itself checked against the existing direct samplewise test.
It records WAV headroom gain, relative RMS/max error, last-second tail energy
and densely sampled absorption gains. A 12-second decaying render and sampled
gains below one are useful diagnostics, not a continuous-frequency proof.

The bundled upstream `SCAT/N6D1A` uses main nominal delays
`[997, 1153, 1327, 1559, 1801, 2099]`, **not** the paper's six-delay vector
above. Auditions using it must be labelled supplied pretrained scattering;
they do not establish completion of new paper-budget optimization.

### Executed room results

Artifacts are in `target/difffdn-rooms-v2/` (generated files, not committed).
Both rooms have `filters.mat` with absorption shape `(6,1,32,6)` and tone
shape `(31,6)`, `validation-v2/room.fdn`, a 12-second Rust impulse render,
and `validation-v2/validation.json`. Octave 11.3.0 + signal 1.4.8 executed
the upstream filter routines. MATLAB itself has still not been executed.

| Check | Salon | Damped |
|---|---:|---:|
| Relative RMS error vs original polynomial | 1.1823e-6 | 1.0624e-6 |
| Maximum absolute WAV error | 6.13e-7 | 6.20e-7 |
| Last-second energy / total (12 s) | 9.59e-39 | 6.06e-98 |
| Maximum sampled absorption gain | 0.9020 | 0.7506 |
| Release render ns/sample (one pass) | 549 | 548 |

Both renders are finite and duplicated mono. These are full-filter renders,
not new training runs. Learned polynomial reconstruction error was freshly
confirmed as `1.689134e-7`. The supplied historical `parameters.mat` stores
noninteger latent `m`, whereas `param_fdnToolbox.mat` stores the realized
integer delays. Validation therefore explicitly used `--scattering-delays`;
the normal importer still rejects delay mismatches. I/O and polynomial
cross-checks remain active. Never apply this override silently to new exports.

Each room's `audition/` contains: `00_reference_energy_matched.wav`,
`01_03_8_quarter_delays_energy_matched.wav` (preserved 8-line short-delay
prototype), and `02_room_0_wet_energy_matched.wav` (this DiffFDN).
Whole-IR mean-channel energy is matched, with one common headroom gain;
no trimming or envelope changes. These are IR auditions, not convolved music,
and are not perceptual/LUFS matching or an actual listening assessment.

The remaining discrepancy is important: Damped reference onset-relative
50%-energy time is **2.31 ms**, versus **18.96 ms** for DiffFDN (short-delay
prototype: 18.60 ms). Salon is 22.38 ms versus 20.35 ms. DiffFDN improves
algorithmic onset here to 1.02 ms, but **does not reproduce Damped's early
energy concentration**. There is no explicit direct/early-reflection model,
and the two source rooms are stereo while the published path is mono.
Do not claim matching perceptual tightness from decay/parity alone.

Remaining: licensed MATLAB cross-check, new optimization at the paper's
delays/budget, actual listening, longer stress tests and plugin UI integration.

### Eight-line preset audition

Historical local artifacts: `target/difffdn-rooms-8line-v1/` (not distributed).
This uses supplied pretrained `SCAT/N8D1A`, not padding the six-line matrices
or a new optimization. Nominal delays are
`[809,877,937,1049,1151,1249,1373,1499]`. Both room exports contain eight
absorption cascades and use the same Rust runtime without code changes.
The explicit historical latent-delay override is still required.

Each room's `audition/` contains energy-matched `00_reference`,
`01_room_0_wet` (six-line DiffFDN), and `02_room_0_wet` (eight-line DiffFDN)
WAVs, with full filenames and metrics recorded in `analysis.json`.
These preserve original durations and remain duplicated mono on the FDN path.
Parity relative RMS errors were `8.44e-7` (Salon), `7.39e-7` (Damped).
Both 12-second renders were finite, with final-second energy fractions
`3.99e-37` and `1.13e-89`. These checks are not a general stability proof.

Measured locally with dependencies already installed:

- Full two-room preparation rerun, including Python/model startup, resampling,
  DecayFitNet and MAT/provenance output: **2.04 s total**.
- Eight-line Octave filter design: **2.27 s Salon / 2.40 s Damped**.
- Rust rendering plus independent polynomial validation: **2.41 / 2.48 s**.
- Filter design, validation and audition creation: **5.92 / 6.13 s** per room
  (reusing prior EDC analysis). Timings are individual wall-clock observations,
  not performance guarantees; see `timings.json` and `analysis-timing.json`.

Room analysis was rerun separately into `analysis-timing/` to measure its
cost; the actual audition reused the existing EDC files. New scattering
training is a separate, unmeasured cost and is not needed per room when
reusing a pretrained structure. Reproduce exports with the documented Octave
adapter, then `validate_difffdn.py ... --seconds 12 --scattering-delays`.

Eight lines alone do not guarantee tighter or more faithful sound. In this
comparison onset-relative 50%-energy times increased from 20.35 to 31.98 ms
for Salon and 18.96 to 30.48 ms for Damped. Different pretrained delays,
matrices and I/O gains changed together, so this does not isolate line count.
Actual listening and plugin integration remain outstanding.

### Output edge-correction-only comparison

`export_difffdn_octave(..., outputPath, false)` disables only the fixed output
level correction `[15,5,0,...,0,5,15]` dB. The optional argument defaults to
true, preserving upstream-compatible behavior. The T60 correction, padding,
scattering delays, gains, matrices and per-loop absorption are unchanged.
This is an intentional sound-design deviation, not a claim of upstream parity.
The MAT export records the applied vector as `edgeCorrectionDb`.

Historical local artifacts are in `target/difffdn-no-edge-v1/` (not distributed).
Each room's `audition/` has `reference_mono.wav`, `current_edge_cut.wav`, and
`no_edge_cut.wav`. All are channel-zero/duplicated-mono, whole-IR energy
matched with common headroom, not LUFS matched. Previous renders are retained.
Each `comparison.json` records coefficient checks and relative band energies;
`validation/validation.json` records the independent polynomial comparison.
An explicit default-argument re-export was checked bit-for-bit against the
prior filter export; disabled-correction absorption and target T60 were also
checked bit-for-bit. Only the output EQ changes. Listening acceptance is pending.

## Live plugin integration

Classic in Algo Reverb is replaced by the eight-line no-edge DiffFDN. See
[Plugin guide](../../docs/plugins.md) for controls, old-session compatibility, dual-mono
Wet, sample-rate conversion and output-level cautions. Historical statements
above saying plugin integration is outstanding describe those earlier stages.
The embedded coefficient copies and their hashes/provenance are in
[coefficient README](../../resources/difffdn/README.md); runtime never reads
`target/` or source WAVs for the embedded rooms.
48 kHz preserves original coefficients; other integer rates use rounded delay
scaling and bilinear SOS substitution, not unchanged 48 kHz coefficients.
Damping adds a passive one-pole blend in each loop, smoothed over 20 ms.
