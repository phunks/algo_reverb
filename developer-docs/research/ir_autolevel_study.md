# IR auto-level feasibility study

> **Historical study — 2026-11-09.** This records the offline investigation
> before plugin integration, not the current implementation status. Pink Auto
> Level is implemented in Algo VN and defaults OFF; gain is Wet-only, host-rate
> calibrated off the audio thread and bounded to ±24 dB. Algo Reverb's former
> convolution/IR panel has been removed; its live DiffFDN uses fixed room trims.
> See the [plugin guide](../../docs/plugins.md) for current behavior and limits.
> Numerical results below are not listening results.

[Research index](README.md) · [Offline IR tools](../ir_tools.md)

## Reproduce

Requires Python 3 and NumPy (already used by the local diagnostic scripts).
From the repository root, after generating a comparison export with `ir-compare`:

```sh
python3 -m unittest discover -s scripts -p 'test_compare_ir_autolevel.py' -v
python3 scripts/compare_ir_autolevel.py \
  /tmp/reference-comparison \
  --output /tmp/algo-autolevel-study-new
```

Output must not exist; inputs are read-only. Writes metrics.csv and
conditions.json, no rendered audio. Stereo float32 comparison exports only;
this prototype does not handle arbitrary PCM/mono WAVs or host resampling.

## Measurement definition

The mono probe has fixed PCG64 seed 1 and random phases, with magnitude
proportional to 1/sqrt(f) between 20 Hz and min(20 kHz, 0.45*sample rate).
Its period is a power of two, at least eight seconds and twice the IR length.
RMS is -24 dBFS (1.0 RMS = 0 dBFS). Each channel is convolved independently;
output power is the mean of L/R power, not power after summing L/R.
One common correction is applied to both channels; stereo balance is preserved.

FFT circular convolution measures the steady periodic response, equivalent to
the repeated probe after startup has settled. It is NOT a finite isolated noise
burst including its trailing silence. The entire period is measured without
gating. No limiter, Dry/Wet, Output, LUFS, peak normalization or gain cap enters
the calculation. Correction dB = input RMS dB - output RMS dB. Silence returns
null rather than infinite gain. The energy baseline is minus 10*log10(mean L/R
sum(h^2)), i.e. full-band stationary white-noise expected power gain.

With fixed spectral magnitudes and a full-period measurement, RMS is determined
by the spectrum-weighted IR magnitude response, not the random phases; phases
affect the probe's peaks. This is a repeatable spectral weighting, NOT an
established perceptual loudness model. A different bandwidth/weighting can give
different corrections. Probe duration/grid changes across rates or long IRs.

## Local results — 2026-11-09

All three references are 44.1 kHz; all use the same 524288-frame / 11.889-second
probe. Actual report: `/tmp/algo-autolevel-study-20261109/`.

| Reference | Energy correction dB | Pink RMS correction dB |
|---|---:|---:|
| Bottle Hall | -3.77 | -4.50 |
| Highly Damped Large Room | -12.76 | -13.73 |
| Parking Garage | -21.37 | -22.09 |

Individual pink correction yields -24 dBFS output RMS by construction. The
measured corrected sample peaks range from -10.41 to -7.97 dBFS across the
available reference/candidate files; this says nothing about other audio peaks.

Using each reference's pink correction unchanged for its candidates instead:
Bottle Hall output RMS ranges -24.30..-23.98 dBFS; Highly Damped Large Room
-24.21..-23.83 dBFS; Parking Garage -24.62..-23.93 dBFS. Parking Garage Hybrid
is -24.06 dBFS, versus reference -24.00. No Hybrid file was present for the first
two directories. These are probe-dependent numerical differences, not listening
equivalence. The baseline corrects all three references substantially differently,
consistent with their different convolution gains; pink adds 0.7..1.0 dB of
attenuation relative to energy matching in this sample, not proof it is better.

## Original recommendation / remaining work (historical)

The following recommendation predates implementation; the defaults and bounds
are now defined in the plugin guide. Listening validation remains separate.

Pink RMS is a viable transparent calibration definition. Before production,
audition on musical sources and compare with energy matching. Reference-shared
gain is appropriate for method comparisons; individual gain is a separate
convenience mode. A future implementation should measure the resampled kernel,
run off the audio thread, apply correction to Wet before mixing, bound gain for
quiet/near-silent IRs, retain the Safety Limiter, and persist calibration without
requiring the original reference directory at session restore. Old sessions must
retain their previous level unless explicitly opting in. Final gain limits,
default mode and host-rate policy remain undecided. No actual plugin-convolver
cross-validation or musical/DAW listening was performed in this study.

The study's local exports are not release assets. Redistribution of audio that
copies reference samples depends on the particular source license; this does not
establish a blanket restriction on fitted numerical coefficients.
