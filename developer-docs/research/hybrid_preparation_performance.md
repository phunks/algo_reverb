# Hybrid preparation performance

> **Historical measurements.** Algo VN is now convolution-only: its live FDN,
> Generate/Rebuild controls, structural auto-rebuild and prepared-model cache
> described below have been removed. The offline FDN fit still uses cached
> reference-band energies, score memoization and bounded pairwise parallelism.
> IR Time Damping remains available, but does not rebuild a live Hybrid engine.
> Test counts and plugin timings below describe earlier revisions, not the
> current suite or available controls. See the [plugin guide](../../docs/plugins.md) and
> [development guide](../development.md) for current usage.

[Research index](README.md)

## Changes

- Cache the reference's crossover-band suffix energies once per temporal fit.
  Candidate filtering, gain bounds, scoring masks and summation order are unchanged.
  The experimental stereo EQ path still uses the reference samples.
- Memoize scores at exact floating-point search coordinates. Reported
  `fdn_fit_evaluations` remains the logical search count (normally 26), not the
  number of actual renders after cache hits.
- Evaluate each independent minus/plus pair with at most one additional worker
  when more than one CPU is available. Apply results minus-before-plus with the
  original strict improvement comparison. Coordinates still update serially.
  Thread creation failure falls back to serial evaluation. Each concurrent fit
  has its own two-thread limit, not a process-wide pool.
- Algo VN reuses the prepared model and pink calibration on structural
  Rebuild only if the full embedded reference (source rate, channel count and
  samples) and host rate match. Generate forces preparation and refreshes the cache.
  A shared in-memory LRU holds at most four prepared models while any Algo VN
  instance in the same loaded plugin module/process remains alive. Closing an
  editor does not clear it; deleting the last instance releases the shared cache.
  No disk cache or session serialization is added. Separate processes/modules,
  including CLAP versus VST3, do not share it. Editors may also retain their current
  model independently of LRU eviction. Cache misses prepare from the reference.
  This costs additional reference copies and concurrent candidate buffers during
  fitting. Cache access stays off the audio thread. Concurrent misses can prepare
  independently; they do not hold the cache lock during fitting.
- Structural parameter edits now automatically request a cached Rebuild after
  300 ms without further structural changes while the editor is open. Only one
  worker runs at a time; stale control/reference results are discarded. Live
  T60/damping edits alone do not request a rebuild. Manual Rebuild remains.
  Applying a structural result still clears history and may click; this is not
  clickless or sample-accurate automation. Edit structural controls while stopped.

No reduced-rate search, shortened horizon, fewer search steps, SIMD, new
dependencies, filter-bank rewrite or FFT algorithm changes are included.

## Validation

Release tests: core 52 unit tests plus 7 integration tests; Algo VN 32 unit
tests plus 18 real-time integration tests. One additional ignored manual timing
test runs explicitly below. Serial/parallel fits match exactly for mono/stereo
and free/fixed T60. Cached/noncached EQ samples and gains match exactly, including
silent candidates at 8 and 44.1 kHz. Plugin tests cover cache reuse after a
structural edit, invalidation for changed reference/rate, and missing-cache
preparation. Real-time allocation tests remain passing.
Automatic-rebuild tests cover debounce/coalescing, busy workers, no retry loop on
failure, ignoring live-only edits/WAV mode, cache reuse, and stale-control rejection.
Shared-cache tests cover cross-editor/instance reuse, reference/rate invalidation,
duplicate replacement, and releasing the shared cache after its last owner drops.
WAV IR damping now automatically prepares on a worker after 300 ms of stable
settings while the editor is open. Tests cover ON/OFF, debounce, preserving the
original IR, and rejecting stale settings/source/rate/mode results. Hybrid uses
its live FDN damping instead; its WAV-only controls show an explanation rather
than an unresolvable Apply warning. IR replacement still resets history and can click.
An additional output regression exercises the automatic damping worker, Pending
handoff and actual Plugin::process at 8/48 kHz, for each band with Auto Level
on/off. With a two-second probe and 24 dB/s, late output changes by approximately
16 dB (Low), 41 dB (Mid), and 16–22 dB (High); disabling restores the original
output exactly. These are synthetic band-specific probes, not DAW listening tests.
The ignored `imreverbs_damping_worker_changes_actual_convolution_output` test
loads all locally available IMreverbs WAVs, resamples to 48 kHz, and compares the
original and automatically prepared convolution impulse outputs. All 38 WAVs
changed with all decay amounts at 24 dB/s and default band frequencies. Total
wet energy changes ranged from -0.02 dB (Direct Cabinet N1) to -4.23 dB (On a
Star); Nice Drum Room was -0.41 dB and Masonic Lodge -0.77 dB. These broadband
energy measurements do not establish audibility or validate DAW GUI/host delivery.
The test is ignored by default because these local WAVs are not portable fixtures.

### Stronger convolution damping revision

The figures above describe the former 0–24 dB/s branch-subtraction renderer.
IR Decay now spans 0–120 dB/s and uses time-varying RBJ low/high cut shelves
(slope 1) and a mid peaking cut, with depth limited to 120 dB. Preparation still
runs off-thread; Wet Tone and live FDN are unchanged. Enabled legacy settings
sound different, and normalized host automation may need adjustment for the
expanded range. OFF/zero remain exact passthrough. Deep cuts widen the shelf
transitions/mid bandwidth; this is not a fixed-band brick-wall decomposition.

At 8/48/192 kHz, each band's synthetic 250 ms probe has at least 8 dB less
energy during 100–200 ms at 120 versus 24 dB/s. Tests also check requested
steady-state band gains through 96 dB, shelf passbands, extreme frequencies/Q,
stereo isolation and finite output through the 120 dB depth cap. The existing
worker → actual Plugin::process regression still passes at 24 dB/s with Auto
Level on/off and exact restoration after disabling.

The local fixture test now uses 120 dB/s for all bands. All 38 fixtures change:
total wet energy ranges from -0.09 dB (Direct Cabinet N1) to -11.00 dB (Deep
Space); Masonic Lodge measures -3.27 dB, Nice Drum Room -1.73 dB. Masonic Lodge
must remain below -2.5 dB in this optional regression. These measurements include
engine handoff/convolution but not GUI/host delivery or listening validation.

The pre-change release `ir-compare` executable was retained externally. Both
Masonic Lodge and Parking Garage generated byte-identical WAVs and identical
reports before/after the optimization on this machine. That is a regression
check on these inputs, not a cross-platform bit-identity guarantee.

## Measurements

Local Apple Silicon release runs, not DAW timings or universal performance
guarantees. Repeated CLI A/B wall times, including generation, analysis and output:

| IR (44.1 kHz stereo) | Before | After |
| --- | ---: | ---: |
| Masonic Lodge, 1.21 s | 3.64 s | 1.48 s |
| Parking Garage, 3.73 s | 11.25 s | 4.54 s |

These single A/B pairs were not fully isolated from background activity. Earlier
runs were 3.46 → 3.46 s and 10.21 → 5.46 s respectively, so results vary; these
are not benchmark-distribution statistics. In the repeated pairs CPU time for
Masonic Lodge fell from 3.44 s to 1.82 s; Parking Garage from 10.69 s to 6.11 s.

The plugin worker path (including engine handoff, but no DAW or audio playback)
was timed directly on Parking Garage at 44.1 kHz: cold preparation **4.33 s**,
cached structural Rebuild after changing Diffusion **0.50 s**. These are both
post-change measurements, not a before/after plugin benchmark. Cold preparation
uses the embedded-reference Rebuild path with the current controls, as after
opening an editor without its cache. Generate uses the same preparation work
but resets controls to fitted values. At a 96 kHz host rate, the same post-change
test measured **9.89 s** cold and **1.00 s** for cached structural Rebuild.

## Historical reproduction commands

The CLI commands remain applicable with separately supplied local WAVs and fresh
output paths. The `profile_hybrid_preparation_and_rebuild` test was removed with
the live-Hybrid plugin path; its invocation below is archival, not a current
benchmark command. For current convolution timing use the development guide.
Commands were run from the project root:

```sh
cargo test --release -p algo_reverb --features ir-analysis
cargo test --release -p algo_vn
cargo build --release --features ir-analysis --bin ir-compare
/usr/bin/time -p target/release/ir-compare \
  'resources/IMreverbs/Parking Garage.wav' /tmp/algo-ir-speed-compare
ALGO_PROFILE_IR="$PWD/resources/IMreverbs/Parking Garage.wav" \
ALGO_PROFILE_RATE=44100 cargo test --release -p algo_vn --lib \
  profile_hybrid_preparation_and_rebuild -- --ignored --nocapture
cargo xtask bundle algo_vn --release
```

Use a fresh CLI output directory. Set `ALGO_PROFILE_RATE=96000` to exercise
resampling and host-rate-dependent preparation. Timing does not assert a fixed
threshold in automated tests. DAW listening, UI responsiveness under load and
the original reported 16–20-second operation still require host-side validation.
