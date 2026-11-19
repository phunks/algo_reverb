# Offline IR tools

[Developer documentation](README.md) · [Build and test](development.md)

Run commands from the repository root after dependency setup. Output paths below
are examples; use a fresh file/directory each time. These tools do not run in the
plugin audio callback.

## Impulse response export

```sh
cargo run --release -- hybrid /tmp/algo-reverb.wav 2.5
```

Arguments: matrix (`hadamard` / `householder` / `hybrid`), output path, and T60 in seconds.
Exports a wet-only, 48 kHz stereo WAV. Existing files are never overwritten.

## Reference IR analysis

```sh
cargo run --release --features ir-analysis --bin ir-analyze -- /absolute/path/reference.wav /tmp/reference-analysis
```

The output directory must **not exist**. The source is never modified. Accepts
mono/stereo PCM or float WAV, 8–384 kHz, up to 60 seconds; keeps the native sample
rate and level, including float values above unity. No GUI/audio device required.

- `metadata.txt`: format, duration, peak, threshold onset and analysis caveats.
- `waveform.csv`: original samples (mono duplicated), with file-relative time.
- `edc.csv`: normalized Schroeder energy decay, approximately every 1 ms plus
  the final sample. Computed from L²+R², so opposite-polarity stereo cannot cancel.
- `decay.csv`: EDT / T20 / T30 **T60 extrapolations**, regression R² and fit times,
  band energy and tail-level proxy. Blank values mean undefined or rejected.
- `windows.csv`: nonoverlapping 10 ms energy, peak and zero-lag, mean-subtracted
  L/R correlation (not IACC); mono/silent correlation is blank.

Reports broadband plus approximate octave bands at 125–8000 Hz, omitting bands
whose upper edge reaches Nyquist. These are causal second-order Butterworth
HP+LP filters, **not IEC-compliant octave measurements**. Filter ringing can
bias short IRs; no artificial tail is appended. EDC fits use every sample, with
0..−10 dB (EDT), −5..−25 dB (T20), −5..−35 dB (T30) ranges.

All estimates are exploratory: no noise compensation or direct-sound removal.
The last 10% mean power is a conservative **tail-floor proxy**, not a measured
noise floor. Fits need at least 5 ms / 16 samples, 50 ms after the lower crossing,
and local power >10 dB above the tail proxy; otherwise T60 is suppressed.
`reference_only` additionally requires R² ≥ 0.98, but is not a confidence
guarantee. `nonlinear_decay` retains a descriptive slope with lower R².
Noise, truncation, multiple slopes and leading silence can bias these results.
Onset is simply the first sample above −60 dB relative to peak, not a direct-arrival
detector. Automatic mixing-time / echo-density detection is not implemented;
resynthesis is provided separately by `ir-compare`.
IR files are not bundled into outputs; verify their redistribution license separately.

## Resynthesis comparison (offline experiment)

```sh
cargo run --release --features ir-analysis --bin ir-compare -- /absolute/path/reference.wav /tmp/reference-comparison
```

Arguments: input WAV, **new** output directory, optional `early_ms` (80),
`fade_ms` (20), `seed` (1), `t60_seconds` override (or `auto`), `density` (2000
pulses/s/channel), and `density_rise_ms` (50; zero means constant). Without an
override, the first screened broadband T30 / T20 / EDT estimate is used; a
failed estimate is an error, not a silent fallback. Times are **file-relative**,
not an automatically detected mixing time. Keep >100 ms after the crossfade.

Writes native-rate, original-length float32 WAVs (preserving mono/stereo):

| File | Content |
| --- | --- |
| `reference.wav` | Reference copy |
| `early_fdn.wav` | Accepted temporal-fit FDN |
| `early_fdn_stereo.wav` | Experimental low-band stereo transform of the fitted FDN |
| `early_vn.wav` | Broadband velvet noise |
| `early_multiband_vn.wav` | Multiband velvet noise |
| `early_hybrid.wav` | Multiband VN → temporal FDN handoff |
| `early_dvn.wav` | Experimental routed-pulse Dark Velvet Noise |

Each candidate
copies the early segment, equal-power crossfades to synthesis, then uses only
synthetic samples. One shared amplitude gain per method matches the reference's
energy **after** the crossfade. No peak normalization, clipping or limiting;
**listen quietly**, since peaks can exceed unity. No final fade is added.

- **Early + FDN:** Classic Hybrid, full diffusion, no modulation, threshold-onset
  excitation. Deterministic offline search renders native-rate FDN IRs and fits
  baseline T60, low-pass cutoff and passive Low/Mid attenuation to **post-EQ late
  temporal shape**. Screened 250 Hz / 2 kHz / 4 kHz T30 → T20 → EDT estimates
  still supply diagnostic targets and the longest-target baseline trial,
  then three coordinate-search scales (at most 26 renders, each at most 6 seconds).
  The objective averages bands (broadband and available 125..8000 Hz octaves),
  equally weighting squared normalized EDC and absolute 20 ms power error in dB.
  Reference-only masks require EDC >= -35 dB and window power above the greater
  of terminal-ten-window median +20 dB, band peak -50 dB and -240 dBFS. Bands need
  three valid windows and suffix energy above 1e-8 of broadband. Errors are clipped
  to +/-40 dB. This conservative floor heuristic does not establish physical noise.
  Trials include post-loop EQ, shared suffix normalization and mono duplication,
  retaining onset and causal filter history. A trial replaces initial settings only if it improves
  the objective. Baseline is bounded 0.01–60 s, cutoff 20 Hz–0.45 × rate, and
  attenuation 0–24 dB. The API takes **per-loop dB, not T60 multipliers**; shape
  remains 500/2000 Hz, Q=0.7. Raising the baseline permits longer band decay
  **without negative damping**. Explicit `t60_seconds` freezes that baseline;
  the other parameters still fit. No valid shape masks or insufficient search
  horizon skips optimization. Beyond six seconds the search's truncated EQ/EDC
  can differ from the final full-length render. Arbitrary curves
  are not necessarily attainable. Reports distinguish initial/baseline, target,
   and achieved decay; achieved fits use the final exported candidate's late region.
   After rendering, static shared-L/R gains in the synthesis LR4 bank match each
   component's reference suffix energy (1e-24 mean-square floor, +60 dB gain cap),
   including DC/Nyquist residuals, before summation and final shared normalization.
   This EQ leaves loop parameters unchanged and preserves isolated component
   normalized EDCs, but overlap/phase and crossover transients can change post-sum
   octave T60. Post-sum band energies are not guaranteed exact. Reports list all
   pre-sum gains and warn on cap hits; the final shared gain is separate.
- **Early + VN:** independently positioned signed sparse pulses, density rising
  exponentially from 25% to the requested density (10..sample_rate), anchored at
  **crossfade end** (first fully synthetic sample), not file start. Before that
  anchor density stays at 25%; zero rise time selects constant density. Per-channel
  20 ms RMS envelope gains are interpolated between centers. A 1e-24 mean-square
  floor and +60 dB coefficient/gain caps protect near-silence. Orthogonalized
  two-carrier mixing fits global suffix Pearson correlation, including negative
  values. Correlation fitting can smear per-channel envelope differences.
  Undefined/degenerate fits are skipped; no time-varying IACC fitting.
- **Early + multiband VN:** synthesis-only phase-compensated sequential LR4
  crossovers at octave edges, plus low-pass and high-pass residuals covering
  DC..Nyquist. The coherent unmodified sum has flat magnitude, not flat phase;
  independent noise bands do **not** have flat summed power at crossovers.
  Each band receives envelope, suffix correlation and energy fitting before
  summing, followed by shared normalization. Overlap, envelope modulation and
  mixing mean post-sum band energy/correlation is not exact. Analysis filters
  remain unchanged. This is **not a paper DVN implementation** or an optimized
  dictionary/density estimator; density controls are manual.

- **Experimental stereo FDN:** uses the accepted FDN parameters, then fits suffix
  Pearson correlation and channel energies in synthesis bands ending at or below
  500 × sqrt(2) Hz, including the low residual. Applied outside feedback, before
  band EQ and shared normalization; mono bypasses it. Not included in the search,
  not a centered-image correction, and may worsen temporal metrics.
- **Hybrid VN → FDN:** after the copied early segment/crossfade, Multiband VN
  leads for 100 ms, then hands off to accepted Temporal FDN over 400 ms using
  smoothstep equal-power weights shared by L/R. Component suffix energies are
  matched first (+60 dB cap), followed by final shared suffix normalization.
  This is a fixed offline blend, not a live feedback hybrid or an optimized
  transition. Short files may end before the handoff completes.
- **Dark Velvet Noise (experimental):** one sparse pulse train per channel;
  each pulse is routed to exactly one fixed LR4 synthesis filter. Interpolated
  20 ms reference band powers set routing probabilities (95% energy-proportional,
  5% uniform) and inverse-probability/impulse-energy-normalized pulse weights
  (+60 dB cap). Global per-band stereo fitting follows filtering. No FDN and no
  independent pulse generator per band. This is not a specific paper reproduction
  or compact decay extrapolator; routing, overlap and fitting can bias matching.

`metrics.csv` compares energy, EDC shape RMSE (reference −5..−35 dB), 20 ms
power-envelope RMSE, L/R correlation and decay estimates, per band. Compare
**late** rows separately: full rows include the copied early IR. `curves.csv`
and `windows.csv` expose the underlying comparisons, including the reference
self-comparison. `windows.csv` includes reference/candidate left/right power in
addition to combined power. Undefined comparisons are blank, not zero. `report.txt`
records options, fit source, gains, peaks and methodological limitations.

VN methods use the original time-varying envelope (including noise); they are
descriptive reconstructions, not compact parametric models or extrapolations.
Metrics do not establish perceptual quality;
there is no automatic winner or real-time CPU comparison. Generation is offline;
plugins load the resulting WAVs rather than running this search live.
Generated files retain reference-derived content: check licensing before sharing.

For material/decay validation, compare several **measured** spaces as well as
synthetic IRs. The local IMreverbs `license.txt` identifies its source as Voxengo
Impulse Modeler; it is not evidence of measured rooms or sampled hardware.
No external WAVs are bundled. A stdlib-only downloader extracts three native
48 kHz binaural measurements (FL/FC/SiL, head angle 0°) from Jon Francombe's
**IoSR Listening Room Multichannel BRIR dataset (2017), CC BY 4.0**:
https://doi.org/10.15126/surreydata.00813511.

```sh
python3 scripts/fetch_measured_irs.py /tmp/surrey-brirs
cargo run --release --features ir-analysis --bin ir-compare -- /tmp/surrey-brirs/surrey_angle0_FC.wav /tmp/surrey-fc-comparison
```

The fetcher pins a source revision and SHA256, refuses overwrites, retains the
license and recording-specific attribution/transform manifest, and only converts
the selected float64 samples to float32 WAV. These are three source directions
in **one room**, not three materials/spaces; HATS and speaker coloration are
included. See [measured-IR validation](research/measured_ir_validation.md) for validation scope and results.
The currently distributed EchoThief license restricts algorithm training and
software embedding; it is not used as a fitting fixture. More independently
licensed spaces and listening tests remain necessary before perceptual claims.

## Python diagnostics

`compare_ir_autolevel.py` and `diagnose_ir_weight.py` require Python 3 and NumPy.
They read comparison exports without modifying them and require new output
folders. Diagnostic audio is not an impulse response: do not load listening
probes into a convolver.

```sh
python3 scripts/compare_ir_autolevel.py /tmp/reference-comparison --output /tmp/autolevel-report
python3 scripts/diagnose_ir_weight.py /tmp/reference-comparison /tmp/ir-weight-report
```

The first writes numerical calibration reports; the second writes stereo/low-band
measurements and listening probes. See the [auto-level study](research/ir_autolevel_study.md)
and [measured-IR notes](research/measured_ir_validation.md) for definitions and limitations.
