# Measured-IR validation — 2026-11-09

> **Research log, not a current plugin manual.** Sections record successive
> experiments; early tables and implementation-status statements apply to the
> revision being measured and can be superseded by later sections. Pink Auto
> Level and Safety Limiter are now implemented in Algo VN. Algo Reverb's former
> convolution engine and IR comparison shortcuts have been removed. Algo VN is
> convolution-only and loads individual WAVs via Browse
> and its folder dropdown, with no comparison buttons or manual Load WAV field.
> Hybrid remains an offline-rendered IR, not a live VN/FDN feedback engine.
> The current CLI additionally exports experimental `early_dvn.wav`, which is
> not evaluated by the original tables here. Local `/tmp` paths below identify
> historical artifacts and are not distributed fixtures.

[Research index](README.md) · [Current plugin guide](../../docs/plugins.md) ·
[Offline tools and reproduction](../ir_tools.md)

## Experimental Hybrid follow-up

The design goal is now a musical effect, not necessarily a faithful room copy.
Added `early_hybrid.wav` and a separate Algo VN loader shortcut as the first
offline integration prototype: energy-balanced Multiband VN leads until 100 ms
after the fully synthetic suffix begins, then hands off to accepted Temporal
FDN over 400 ms (smoothstep equal-power, shared stereo weights). The copied early
segment/crossfade and final shared suffix normalization use the existing path.
No feedback topology, accepted fitting objective, or existing candidate changed.
Report records transition constants and anchor. This boundary is not optimized
and can discard later VN events; short files may not complete the transition.
Energy matching does not ensure loudness, peak safety, or stereo correlation.
This is not yet a real-time VN engine or plugin unification. No listening win
is claimed; audition against both components before extending the architecture.

Algo VN has a separately switchable default-on sample-peak Safety Limiter.
The gain-check section below predates it and Pink Auto Level; exported WAVs
remain unlimited. For linear comparisons lower
Output before turning Safety Limiter off. Bypass is unprotected.

## Parking Garage listening feedback and gain check

User listening feedback: Parking Garage makes differences easier to identify.
FDN retains an open, recognizably FDN-like tail; Multiband VN expresses the
reference's repeated large-delay events more clearly. Overall reproduction is
closer to the original IR, though differences may be subtle without prompting.
The exact FDN comparison mode used was not specified. User reports no audible
distortion around -0.1 dB for Bottle Hall, suitable level around -9 dB for Highly
Damped Large Room and around -18 dB for Parking Garage, which is otherwise very
loud/distorted. These are reported listening settings, not calibrated safe trims;
input level, Dry/Wet, host routing and the clipping location are unverified.

Native-rate exported reference measurements:

| Reference | IR sample peak dBFS | Larger channel sum-of-squares dB | Sampled maximum frequency-response gain dB |
|---|---:|---:|---:|
| Bottle Hall | -0.04 | 5.58 | 15.37 |
| Highly Damped Large Room | 0.00 | 13.18 | 23.47 |
| Parking Garage | 0.00 | 21.47 | 32.55 |

Energy is 10*log10(sum(h²)), not playback RMS or LUFS. Response gain is the
largest L/R magnitude on a zero-padded FFT grid, not a transient peak bound.
Both depend on the stored discrete-time IR amplitude convention. Similar IR
sample peaks do NOT imply similar convolution output levels. The energy and
response-gain ordering is consistent with the user's report, but cannot locate
or prove its distortion cause. Parking Garage accepted FDN has sampled maximum
response gain 36.98 dB versus Multiband VN 32.30 dB; equal suffix energy does not
equalize narrow-band gain or perceived loudness.

Code inspection: convolution is floating-point linear FFT overlap-add, with no
intentional saturation; Output gain follows convolution and Dry/Wet mixing.
A synthetic hot-IR regression test verifies >full-scale output and linearity.
This does not reproduce the user's host session or exclude downstream clipping.
At the time of this gain check there were no automatic normalization, limiter,
source changes or default gain changes; current optional playback protection is
documented in the plugin guide above.
Use conservative Output settings and check the following host stages/meters;
maintain the SAME Output between methods within an IR comparison. A different
Output between reference IRs is appropriate for safe audition, not loudness matching.
Local measurements: `/tmp/algo-ir-gain-check-20261109.json`.

## Low-band stereo FDN comparison follow-up

Latest user feedback: the current result is substantially better than the first
VN version; Multiband VN is close to reference and further FDN improvement is
requested. The residual boom/spread is subtle, not an established causal diagnosis.

Added `early_fdn_stereo.wav` as a separate experimental candidate, preserving
`early_fdn.wav` and both VN exports. It uses the SAME temporal-fit parameters;
no feedback changes or gain trim. Synthesis bands ending at or below
500*sqrt(2) Hz (including the low residual) receive the existing bounded VN
zero-offset stereo transform before band-energy EQ. This fits reference suffix
Pearson correlation and channel energy, not centered stereo or time-varying IACC.
Mono bypasses the transform. Output EQ/shared suffix normalization and copied
early samples follow the existing path. Search still optimizes accepted FDN,
not this experimental transform; the transformed EDC can regress.

Bottle Hall late causal-octave metrics (reference / accepted / experimental):

| Band Hz | Pearson correlation | Accepted / experimental EDC RMSE dB | Accepted / experimental window RMSE dB |
|---|---|---|---|
| 125 | +0.159 / -0.222 / +0.167 | 2.176 / 2.525 | 2.978 / 2.931 |
| 250 | +0.067 / -0.339 / +0.100 | 1.231 / 1.357 | 2.125 / 1.804 |
| 500 | +0.028 / -0.258 / +0.073 | 0.699 / 0.721 | 1.576 / 1.359 |

Broadband EDC RMSE 0.512 -> 0.527 dB. Correlation improves while EDC slightly
regresses: not evidence of a perceptual win. No agent listening or DAW GUI test.
For current audition, load the WAV via Algo VN Browse at identical Output/Dry-Wet,
with transport stopped. The former Algo Reverb comparison shortcuts and Classic
plugin engine have since been removed. Historical local exports:
`/tmp/algo-fdn-stereo-validation-20261109/Bottle_Hall/`.
Reference-derived audio remains local evaluation only; do not redistribute.

## Source and reproduction

Francombe, Jon, 2017: **IoSR Listening Room Multichannel BRIR dataset**, Institute
of Sound Recording, University of Surrey, UK.
DOI: https://doi.org/10.15126/surreydata.00813511. **CC BY 4.0**.

Source revision: `d36af1bbd0f4083a9f9ee678ff8f499044e851a7` in
https://github.com/IoSR-Surrey/IoSR_ListeningRoom_BRIRs.
The source README and documentation explicitly license the dataset, not merely
the repository's software. Section 2.2 documents 48 kHz acquisition; section 3.2
specifies samples × L/R ears × loudspeakers. Head angle 0°, speaker indices
1 (FL, +60°), 3 (FC, 0°), 11 (SiL, +90°) were selected.

`scripts/fetch_measured_irs.py` verifies the source MAT SHA256, extracts those
three measurements without normalization/trimming/resampling and converts double
samples to float32 WAV. It saves the original license, source README, citation,
recording identities, transformation description and individual WAV hashes.
No third-party audio or generated reference-derived audio is bundled in Git.

```sh
python3 scripts/fetch_measured_irs.py /tmp/surrey-brirs
cargo run --release --features ir-analysis --bin ir-analyze -- /tmp/surrey-brirs/surrey_angle0_FC.wav /tmp/surrey-fc-analysis
cargo run --release --features ir-analysis --bin ir-compare -- /tmp/surrey-brirs/surrey_angle0_FC.wav /tmp/surrey-fc-comparison
```

Repeat for FL and SiL with fresh output directories. Default synthesis settings:
80 ms early, 20 ms fade, seed 1, auto baseline, 2000 pulses/s/channel, 50 ms
density rise anchored at crossfade end (100 ms), not file start.
Native length: 65536 frames (1.3653 s), stereo, 48 kHz.

## FDN results

Objective is mean squared log T60 ratio over accepted late-region targets, not
an audio-distance or perceptual score. All three searches used 26 evaluations.
Reported achieved values are re-analyzed from the final crossfaded candidate's
late suffix, not the isolated search render.

| Source | Initial nominal s | Fitted nominal s | Initial objective | Fitted objective | Cutoff Hz |
|---|---:|---:|---:|---:|---:|
| FL | 0.212869 | 0.212869 | 0.014620 | 0.011941 | 4242.64 |
| FC | 0.228736 | 0.272015 | 0.034837 | 0.011785 | 2522.69 |
| SiL | 0.206214 | 0.245232 | 0.045892 | 0.014286 | 2522.69 |

| Source | 250 Hz target / achieved s | 2 kHz target / achieved s | 4 kHz target / achieved s |
|---|---:|---:|---:|
| FL | 0.313888 / 0.267167 | 0.242528 / 0.223005 | 0.199543 / 0.210286 |
| FC | 0.382205 / 0.338976 | 0.216056 / 0.249116 | 0.211650 / 0.217400 |
| SiL | 0.354594 / 0.307527 | 0.212463 / 0.239396 | 0.196769 / 0.215459 |

The 250 Hz achieved decay exceeds the old initial nominal baseline in all
three examples, with passive nonnegative band attenuation. Finite output alone
does not establish a perceptual benefit or an exact target match.

## Late broadband metrics

| Source | FDN EDC RMSE dB | VN EDC RMSE dB | Multiband VN EDC RMSE dB | Reference / VN / multiband correlation |
|---|---:|---:|---:|---|
| FL | 99.238 | 0.714 | 0.498 | 0.2212 / 0.2212 / 0.3512 |
| FC | 82.796 | 0.694 | 0.981 | 0.2545 / 0.2545 / 0.4366 |
| SiL | 83.541 | 0.699 | 0.462 | 0.0612 / 0.0612 / 0.2349 |

**FDN overall late EDC agreement remains poor.** The fit optimizes decay slopes
only, not the complete EDC/noise floor or stereo correlation. These recorded
suffixes retain noise whereas the passive FDN keeps decaying. No noise-floor
compensation was applied, so the table must not be presented as successful
whole-tail material reproduction. VN reconstructs reference noise/envelopes by
design. Multiband pre-sum correlations are fitted, but final summed correlation
is not constrained, as the table demonstrates. Multiband VN is not uniformly
better than broadband VN; no automatic winner is selected.

## Crossfade-anchored density follow-up

The tables above use the corrected density anchor. Density stays at 25% before
crossfade end, then rises exponentially; zero rise time remains constant density.
This corrects the carrier timing, not an automatically estimated room mixing time.
Audibility still requires listening validation, especially after band filtering.

`windows.csv` now appends reference/candidate L/R power columns. The following
late broadband per-channel 20 ms power RMSE uses each reference channel's own
peak-minus-60 dB gate (so its window selection differs from combined-power RMSE):

| Source | VN L / R RMSE dB | Multiband VN L / R RMSE dB |
|---|---:|---:|
| FL | 0.9775 / 0.5425 | 1.5757 / 1.5524 |
| FC | 0.6794 / 0.7550 | 1.4860 / 1.6992 |
| SiL | 0.9418 / 0.5075 | 1.3406 / 1.2547 |

These are total reconstruction errors, **not an isolated measurement of damage
from stereo matching**. Correlation mixing can smear L/R envelope differences;
interpolation, sparse carriers and band summation also contribute. No pre/post
stereo-transform ablation or listening test was performed.

Three local IMreverbs synthetic references were also compared with the same
defaults. Their license identifies Voxengo Impulse Modeler and permits use for
any purpose; redistribution restrictions still apply. No audio is added here.

| Synthetic source | 250 Hz target / achieved s | 2 kHz target / achieved s | 4 kHz target / achieved s |
|---|---:|---:|---:|
| Highly Damped Large Room | 0.651130 / 0.632830 | 0.617618 / 0.611450 | 0.569947 / 0.576898 |
| Bottle Hall | 0.521373 / 0.525285 | 0.492555 / 0.520002 | 0.487582 / 0.471554 |
| Parking Garage | 2.483080 / 2.489979 | 2.773641 / 2.881638 | 2.600631 / 2.630595 |

All nine synthetic-reference late-band T30s were `reference_only`; Surrey targets
still use screened T30/T20/EDT fallback, so do not equate every target with T30.
Achieved decays vary across these inputs rather than collapsing to one profile.
This verifies an input-dependent slope-fitting path, **not material identity or
perceptual equivalence**. Similar achieved ratios alone could not establish a
source-IR fault: model limits and search bias must also be ruled out.

All six fits reached the 0 dB attenuation boundary in at least one band (none
reached 24 dB). Reports now flag this for review of passive limits, search bounds
and the frequency-dependent all-pass group delay hidden by nominal conversion.
The analytic initialization calculation is retained; report labels distinguish
full-reference screened T60 from post-search applied attenuation.

The six CLI runs exported 24 WAVs; all 3,447,400 float samples were finite.
The release suite passed 44 tests; offline release Clippy with warnings denied,
format check, IDE build and `git diff --check` passed. Raw follow-up results are
local temporary artifacts, not committed fixtures.

## Post-loop FDN energy EQ experiment

The earlier tables are the **pre-EQ baseline**, not the current FDN output.
The offline `early_fdn` renderer now applies static shared-L/R gains in the same
phase-compensated LR4 synthesis bank as multiband VN, including low/high residuals.
Gains fit suffix component energy with a 1e-24 mean-square floor and +60 dB cap;
summation is followed by the existing shared suffix normalization. Mono fits the
retained left channel before duplication. Reports list pre-sum gains, cap hits
and the limitations of overlapping-band reconstruction. Neither the feedback
engine nor its log-T60 search objective was changed.

Before implementation, late curves in all analysis bands were inspected. Surrey
has large bandwise errors too, not merely a broadband composition error. Its
reference broadband local equivalent T60 changes from 0.31–0.45 s in -5..-15 dB
to 3.85–5.20 s in -20..-30 dB, with `tail_floor_risk` T30s. The final ten complete
20 ms reference windows have median broadband power around -137 dBFS. These
observations suggest a floor contribution but do not independently establish
noise or a physical double slope. Parking Garage's reference straight-line
residual is only 0.22 dB RMS in broadband (-5..-35 dB): no strong double-slope
case was established. Highly Damped Large Room is the clearest weight-mismatch
case: small 250..4000 Hz EDC errors coexist with roughly -4..-6 dB low/mid
energy errors and +4 dB at 8 kHz.

Same inputs, default timing, seed 1, and original-length exports were rerun:

| Source | Late broadband EDC RMSE before / after EQ dB |
|---|---:|
| Highly Damped Large Room | 2.4005 / 0.4810 |
| Bottle Hall | 1.2217 / 2.2140 |
| Parking Garage | 0.8533 / 0.6789 |
| Surrey FL | 99.2382 / 86.8276 |
| Surrey FC | 82.7957 / 69.2741 |
| Surrey SiL | 83.5413 / 72.3868 |

For Highly Damped Large Room, analysis-band energy errors improve from -6.25
to -0.18 dB at 250 Hz, -3.68 to -0.56 dB at 2 kHz and +4.04 to +0.94 dB at
8 kHz. This supports spectral-weight mismatch as an important contributor in
that input, **not a universal cure**. Bottle Hall worsens despite improved
midband energy balance: spectral and temporal errors can previously compensate.
Surrey's large residual remains unsuitable as evidence of material reproduction.

Constant scaling preserves each isolated component's normalized EDC, not every
analysis octave after recombination. Highly Damped Large Room's achieved late
T30 changes from 0.63283 to 0.63441 s at 250 Hz, 0.61145 to 0.61702 s at 2 kHz,
0.57690 to 0.59156 s at 4 kHz and 0.51395 to 0.54540 s at 8 kHz. Its 8 kHz EDC
RMSE actually worsens from 1.73 to 2.91 dB. Post-sum band checking is essential.

All six CLI runs succeeded; no EQ gain cap was hit. Fitted nominal T60, cutoff,
objectives and evaluation counts exactly matched the old reports. Both VN WAVs
were byte-identical to baseline for every input, and all new FDN float samples
were finite. The release suite passed 46 tests, including EQ floor/cap/silence,
colored-reference band-energy improvement, isolated-component EDC invariance
and CLI report assertions. Release Clippy with warnings denied and IDE build
passed. These checks are objective only; no listening validation was performed.

## Bottle Hall residual and local listening preparation

Follow-up analysis used the approximately 1 ms samples in `curves.csv`, selecting
the same reference -5..-35 dB late interval before/after EQ. For each band, the
error was decomposed into its mean, a centered time-linear term and the remaining
residual. These terms are orthogonal on that sampled interval, so their mean
squares sum to total squared error. This is a descriptive decomposition, **not
a causal separation** of model mechanisms. Sampled results approximate, rather
than replace, the all-sample `metrics.csv` values.

| Bottle Hall broadband | Before EQ | After EQ |
|---|---:|---:|
| Mean EDC error dB | -1.138 | -2.170 |
| Time-linear error term RMS dB | 0.110 | 0.109 |
| Nonlinear error residual RMS dB | 0.434 | 0.423 |
| Mean-offset fraction of squared error | 86.6% | 96.1% |
| Reference local equivalent T60 s | 0.4840 | 0.4840 |
| Candidate local equivalent T60 s | 0.4903 | 0.4778 |

The local slopes above fit both curves over the **same reference-selected time
interval**; they are not the separately screened T30 reported by the CLI. After
EQ, reference/candidate crossings relative to late start are 44.9/31.9 ms at
-5 dB, 115.7/103.8 ms at -15 dB and 282.4/268.4 ms at -35 dB. The candidate
loses too much normalized remaining energy before the scored interval, then
runs nearly parallel to the reference. A scalar gain cannot repair this: each
suffix EDC is independently normalized to 0 dB. A global delay is not established
as a solution either; other bands have opposite signed offsets.

The 8 kHz residual also contains a genuine slope mismatch: local equivalent T60
is 0.487 s for reference versus 0.400 s after EQ, with mean error -6.20 dB and
time-linear error RMS 1.84 dB. Meanwhile 250 Hz and 1 kHz retain positive mean
errors of +1.90 and +2.00 dB. Reference straight-line residuals in these analysis
bands are approximately 0.36..0.64 dB RMS: this is not compelling evidence that
a strong physical double slope requires two FDNs. Nor can one nominal-T60 change
resolve the mixed signs. Next optimization experiments should include bandwise
EDC/early suffix shape and the final EQ path, with floor-aware screening for
Surrey, rather than only three pre-EQ T60 estimates. No such optimizer or engine
change was made in this follow-up.

A local temporary listening pack was generated for Bottle Hall and Highly Damped
Large Room, each with reference, pre-EQ FDN and post-EQ FDN wet-only WAVs. The
identical deterministic synthetic probe contains a click at 0.25 s, a 300 ms
noise burst at 1 s and a 180 ms harmonic pluck at 2 s. FFT convolution uses full
linear-convolution length. One common gain per source sets the largest peak of
its three renders to -18 dBFS; files are not individually peak/RMS normalized.
Native-rate stereo PCM16 export was checked for frame counts, finite input and
unclipped peaks. The pack includes source license text and a local-evaluation-only
notice; reference-derived audio is not committed or authorized for redistribution.
Copied early content remains included, so this is not an isolated late-tail
listening test. No audio was played and no subjective improvement is claimed.

## Post-EQ temporal-shape search (follow-up)

Listening feedback preferred the energy-EQ output, but described both Bottle Hall
FDNs as slightly swollen compared with the tighter reference. The original
listening probe was right-heavy already in the reference (+5.76 dB integrated
R/L versus +5.37 dB for post-EQ FDN). A fixed channel-energy-balanced diagnostic
version was provided; that deliberately changes stereo balance, not the engine.
These are user observations, not agent listening results.

The current offline coordinate search replaces the old log-T60 objective with
post-EQ temporal shape: equal squared-dB EDC and complete 20 ms absolute window
power errors, then equal averaging over broadband and valid 125..8000 Hz octaves.
Both terms use fixed reference-only masks: center EDC >= -35 dB and window power
above max(last-ten-complete-window median +20 dB, band peak -50 dB, -240 dBFS).
At least three windows and relative suffix band energy >=1e-8 are required.
Individual errors are clipped to +/-40 dB. This floor heuristic intentionally
excludes weak terminal energy; it is neither a noise detector nor proof of a
second physical decay. Bands with no valid mask are omitted, not zero-scored.

Trials now include onset-aligned excitation, causal filter history, the actual
static energy EQ, mono retention/duplication and shared suffix normalization.
The fully synthetic suffix is scored, not the crossfade. The same four passive
parameters, three coordinate scales and maximum 26 evaluations remain. The
six-second horizon bounds cost but can bias long-source EQ and EDC relative to
the full export. Screened T60s remain diagnostic and seed the longest-target
trial; an explicit nominal-T60 override still freezes only that parameter.
Report fields now say `shape_error_db2`, not `log_error`; their values are not
comparable to old scores. No dual FDN, dynamic output envelope or live/plugin
processing change was introduced.

Same six sources, defaults and seed were rerun:

| Source | Late broadband EDC RMSE, previous EQ / temporal fit dB |
|---|---:|
| Bottle Hall | 2.2140 / 0.5119 |
| Highly Damped Large Room | 0.4810 / 0.3098 |
| Parking Garage | 0.6789 / 0.6789 |
| Surrey FL | 86.8276 / 72.2734 |
| Surrey FC | 69.2741 / 69.2779 |
| Surrey SiL | 72.3868 / 72.3264 |

Surrey's all-sample EDC metric still includes floor-risk regions intentionally
excluded from optimization. FC slightly worsens on that metric; these results
do not establish a universal improvement or satisfactory Surrey reproduction.

Bottle Hall chooses nominal T60 0.49417 s, cutoff 14270.5 Hz, Low/Mid attenuation
1.75/1.75 dB, versus 6000 Hz and 0.75/0 dB previously. For the 240..320 ms
FILE-relative interval, candidate/reference integrated window-power errors are:

| Hz | Previous EQ dB | Temporal fit dB |
|---|---:|---:|
| 125 | +3.00 | +1.19 |
| 250 | +4.26 | +2.69 |
| 500 | +3.25 | +1.82 |
| 1000 | +2.91 | +0.78 |
| 2000 | +2.36 | +0.33 |
| 8000 | -5.29 | +0.48 |

The sampled broadband mean EDC error improves from -2.170 to -0.186 dB, but
nonlinear residual RMS after removing mean/slope changes from 0.423 to 0.466 dB.
Thus offset/spectral-temporal balance improves; curvature is not solved, and
250/500 Hz still have excess mid-tail power. This is consistent with a useful
candidate for listening, not proof that the audible swelling has disappeared.

Release tests passed 49 tests, including new temporal-error, gain sensitivity,
silence/floor-mask and mono/stereo onset/post-EQ path checks. Release Clippy with
warnings denied and IDE build passed. All six comparison runs succeeded and VN
exports were byte-identical to the previous EQ run. New synthetic-probe listening
files compare `reference`, `previous_eq` (the old after), and `temporal_fit`, with
the same shared -18 dBFS peak ceiling and an additional channel-energy-balanced
Bottle Hall set. No agent listening was performed. Temporary result directories
are recorded by `/tmp/algo-fdn-shape-validation-latest.txt` and
`/tmp/algo-fdn-shape-listening-latest.txt`; they are local, not repository assets.

Follow-up user listening feedback reported that `temporal_fit` was substantially
better, with only a possible slight excess in perceived level noted. In the
channel-energy-balanced Bottle Hall probe, whole-file RMS is -48.818 dBFS for
reference and -48.961 dBFS for temporal fit; peaks are -19.989 and -19.928 dBFS.
These measurements do not establish equal perceived loudness or explain the
remaining impression. The user accepted the current temporal-fit approach and
deferred gain adjustment. Keep the existing suffix-energy normalization unchanged;
no proposed -0.5 dB output trim was applied. This is informal user feedback on
diagnostic probes, not a controlled listening study or an agent listening result.

## Bottle Hall low-frequency weight and stereo follow-up

User feedback reports less low-frequency fullness than the reference and an
audible right bias. No gain/EQ/FDN or plugin DSP change was made in response.
`scripts/diagnose_ir_weight.py` measures existing exports and writes twelve local
wet-only 55..440 Hz harmonic-burst probes: reference/temporal FDN/multiband VN,
each in original stereo, fixed IR-channel-energy-balanced stereo, summed mono,
and probe-specific energy-centered stereo.
One shared gain limits the entire pack to a -18 dBFS peak ceiling; no per-file
loudness match. Balancing is per full IR, preserving its total two-channel energy,
not its image, spectral balance or perceived loudness. Mono is diagnostic and
can cancel anti-correlated content. Do not load these rendered probes as IRs.

For the accepted Bottle Hall export, full-length broadband R/L energy is
**+4.959 dB reference, +4.759 dB temporal FDN, +5.087 dB multiband VN**.
After the 100 ms early/crossfade boundary it is +1.542/-0.134/+1.655 dB.
The reference itself is therefore right-heavy, particularly in the early part;
aggregate FDN bias is not greater than reference. These IR statistics differ
from earlier synthetic-probe statistics because excitation and region differ.
There is no single universal +5.76 dB balance correction for all probes/regions.

Independent zero-phase FFT-bin diagnostics (NOT the causal analysis octave bank)
give the following for the suffix after 100 ms:

| Quantity | Reference | Temporal FDN | Multiband VN |
|---|---:|---:|---:|
| 20..100 Hz energy vs reference, dB | 0 | +0.493 | +2.856 |
| 100..250 Hz energy vs reference, dB | 0 | +1.696 | -0.406 |
| 250..500 Hz energy vs reference, dB | 0 | +0.747 | -0.402 |
| 100..250 Hz Side/Mid energy, dB | -2.706 | +3.165 | +0.570 |
| 100..250 Hz normalized L/R cross-energy | +0.302 | -0.352 | -0.066 |
| 100..250 Hz energy median time after boundary, ms | 16.87 | 36.85 | 28.66 |

Whole-IR 20..500 Hz energy differences are only +0.13 dB FDN / -0.18 dB VN.
The existing causal octave CSV independently confirms excess, not deficient,
FDN suffix energy at 125/250/500 Hz (+1.048/+1.091/+0.803 dB) and negative
FDN correlations (-0.222/-0.339/-0.258), versus positive reference correlations
(+0.159/+0.067/+0.028). This points to stereo coherence and temporal distribution
as plausible contributors to reduced fullness, not simply low-band level loss
or universally faster decay. It does not establish the perceptual cause; VN
has different residuals and cannot be explained solely by the FDN's correlation.
FFT band timing includes noncausal ringing and no noise mask; treat it as
descriptive, not a replacement objective or production octave metric.

The colored low-register probe has R/L +10.208/+10.757/+9.200 dB for
reference/FDN/VN. Full-IR balancing leaves +5.249/+5.998/+4.113 dB: imbalance
is frequency/input dependent. The additional `probe_centered` files remove
this probe's aggregate channel-energy difference without changing its total
energy; they are not general-purpose IR corrections or loudness matches.

Local diagnostics: `/tmp/algo-bottle-hall-weight-20261109-final/` (summary JSON, window
CSV, twelve PCM16 probes, README). Reference-derived audio remains local-only and
subject to source-license restrictions. No agent listening was performed.
Next listening comparison should check original vs balanced for image bias,
and original vs mono for coherence/cancellation, at identical playback settings.
Any eventual image correction should be optional, not destructive source
normalization. The accepted fitting and gain remain unchanged.

## Checks and remaining limits

- Both CLIs completed on all three recordings. CSV schemas and finite numeric
  exports were checked; undefined fits remain blank.
- Synthetic tests cover longer-than-initial-baseline FDN decay, long-tail
  finiteness/decay, passive attenuation bounds, fixed T60 override, repeatability,
  mono/native-rate operation, density rise, positive/negative correlation and
  coherent LR4 summation including low/high residual coverage.
- These are **three directions in one treated room**, not a material-diverse
  room survey. Binaural HATS, speaker EQ/bass management and recording chain are
  part of each measured IR. More spaces and controlled listening remain open.
- No agent listening or controlled listening session was performed; informal
  user feedback on the diagnostic probes is recorded above, not generalized.
- EchoThief's October 2025 archive was inspected for licensing only. Its current
  license limits algorithm training and software/product embedding and directs
  such uses to SDSURF commercial licensing. It was not used for fitting,
  regression fixtures or redistribution. MIT/OpenAIR recording-specific rights
  were not established here, so their audio was not added.
