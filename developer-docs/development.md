# Build, test, and WAV audition

[Developer documentation](README.md) · [Plugin guide](../docs/plugins.md)

Run the commands below from the repository root.

## Requirements and setup

- Rust / Cargo: the root package declares Rust 1.88; Algo VN declares 1.95.
  Use Rust 1.95+ for the full workspace and plugin builds, whose dependency
  requirements can exceed the root package's declared minimum.
- `just`, Git and network access for initial dependency setup.
- A VST3 / CLAP host to use the plugins; AU is not supported.
- Python 3 and NumPy only for the optional numerical diagnostic scripts/tests.
  The measured-IR downloader uses the Python standard library only.

```sh
just setup-ext
```

This fetches pinned revisions of `nice-plug` and `egui_knob` into `ext/` and
applies the patches in `patches/`. Cargo uses these local paths, so set them up
before the first build, including core-only builds. Re-running setup skips
already-applied patches. Do not use the destructive `reset-target` recipe to
update a clone containing work you want to keep.

## Plugin bundles

```sh
cargo xtask bundle algo_reverb --release --locked --lib --features plugin
cargo xtask bundle algo_vn --release --locked --lib
```

Bundles are written to `target/bundled/`. Close the host before replacing an
installed bundle, retain a backup, then rescan and verify a fresh instance.
Build each plugin separately to avoid export-symbol collisions. See the
[plugin guide](../docs/plugins.md) before opening old sessions or loading IRs.

Algo Reverb replaces the Classic plugin engine with live DiffFDN. Embedded
coefficient provenance and applicable upstream MIT texts are recorded in
[third-party notices](../THIRD_PARTY_NOTICES.md). Focused integration checks:

```sh
cargo test --release --features plugin --lib difffdn
cargo test --release --features plugin --lib plugin::
```

These exercise the actual plugin buffer path, control/reset allocation counts,
parameter snapshot restore, UI layout and 12-second impulse stability at
8/44.1/48/88.2/96/192/384 kHz. They do not replace a DAW save/reload, latency
rescan or listening test. Only the selected preset network runs in steady state;
both run during finite 150 ms transitions. With the optional **Peak Space** effect,
each selected room/seed instead runs two preallocated voices; rapid onsets reuse
the older slot after a 5 ms fade. Its tests include exact disabled-path equivalence,
dip/swell bounds, onset/sustain/retrigger, outgoing-tail release, seed restore and
replacement, block independence and zero allocation/deallocation during processing,
mode changes and reset at 8/48/192 kHz. Benchmark the actual host at its
buffer size before relying on real-time deadlines.

Slice Repeat focused checks:

```sh
cargo test --release --features plugin --lib slice_repeat
```

These cover musical lengths and fractional timing at 8–384 kHz, impulse repeat
and decay, progressive high-cut loss, high-pass bass rejection, seek/mode reset,
disabled equivalence, both routing modes, host-block independence, output safety
and allocation-free control/reset. Host tempo/position integration and subjective
slice character still require a DAW audition; automation transitions are faded
and applied at a subsequent host block rather than being sample-accurate.

Groove Offset tests check that five generations have the same signed fixed shift
without changing interval, minimum causal playback delay and buffer capacity.
Wet Out regressions check independence
from Dry/Wet, Output gain, silent Bypass, unchanged Main output and the advertised
stereo bus. Run `cargo test --release --features plugin --lib plugin::` for these
and the existing layout, safety and processing regressions. DAW multi-output
routing and old-session bus-layout compatibility still require host validation.

The Classic control layout is restored with active controls in the live
scattering loop. Its body scrolls in the fixed window; the peak meter stays
visible. New regression tests check every restored control for an audible-path
change, pre-limiter wet headroom for impulses/DC/tones/sweeps at 8–384 kHz,
and the post-mix safety ceiling. Fixed Wet reference trims are −25.4 dB (Salon)
and −21.5 dB (Damped), with an equal-power DiffFDN Dry/Wet crossfade. A 48 kHz
log-spaced multitone regression checks bypass-relative RMS within 1 dB at
0/35/70/100% Wet and Output 0/+6 dB below limiting. This is not a perceptual
loudness guarantee; pre-limiter Wet can exceed full scale. Fitted absorption remains, so
the T60 control adds loss rather than specifying the final measured T60.

Live DiffFDN caches unchanged Classic parameter targets and modulation-rate
coefficients, uses branch-wrapped delay writes and row-slice scattering dot
products. The host now stops the unselected room and clears it at fade completion;
the destination tail is cold on the next switch. Reversals retain the current
blend and both running states. No allocation is needed for transitions/reset,
but buffer clearing at fade completion has a bounded callback cost. IR / Hybrid
host processing/UI/state have been removed; Algo VN and offline tools remain.
Wet leveling, equal-power Dry/Wet mixing and the safety limiter are preserved.
For a manual CPU comparison (48 kHz, 128-frame blocks, 10 seconds of audio), run
`cargo test --release --features plugin --lib difffdn_cpu_measurement -- --ignored --nocapture`.
One local before/after pair measured 881/883 ms → 782/782 ms for Static/Random
Allpass with identical printed output checksums. This is about 11% less DSP
time, not an 11-point reduction in the DAW CPU meter; scheduling, host buffer
size, sample rate, UI and other plugins affect actual results.

After removing continuous dual-room processing, the same default-Salon benchmark
measured **758/752 ms → 398/398 ms** (Static/Random Allpass), approximately
47% less processing time with identical printed output checksums. A separate
regression compares both room choices against continuously running references.
This is steady-state timing: during room fades both networks still run, and
clearing the outgoing buffers adds a bounded cost at fade completion. It does
not predict a particular DAW CPU percentage.

The `plugin` feature enables Rust IR analysis for experimental stereo seed
import. It shares Algo VN's in-editor WAV browser and folder navigation, without
enabling `rfd` (still used by the standalone `gui` feature). No Python runtime is needed.
See the plugin guide for fidelity limits and handoff behavior. Focused checks:

```sh
cargo test --release --features plugin --lib seed
cargo test --release --features ir-analysis --lib seed_import_timing -- --ignored --nocapture
```

The second command includes an ignored local-WAV timing check which requires
the separately supplied room files under `resources/IMreverbs/`.

New IR imports use independent v2 topology fitting; persisted v1 seeds keep the
previous Salon-based path. Re-import to upgrade them. Structural regressions:

```sh
cargo test --release --features plugin --lib matched_decay_spaces -- --nocapture
cargo test --release --features ir-analysis --lib local_structural_comparison -- --ignored --nocapture
cargo test --release --features plugin --test reverb custom_stereo_seed
```

The portable fixtures have approximately matched 1 s decay but different echo
cluster spacing/density buildup. They check source-versus-render structural loss
against both Salon and an independent fixed topology, plus swapped-topology
specificity. This is not waveform or listening validation. The optional measured
room check requires local WAVs. Current local losses (lower is better) were
0.118/0.462 fitted versus 0.316/0.625 Salon for the procedural spaces, and
0.022/0.135/0.098 versus 0.105/0.328/0.252 for Salon/Damped/Garage.
These comparisons hold fitted decay/EQ/readouts fixed while changing topology;
they are not a separately recalibrated old-versus-new end-to-end benchmark.

## GitHub Actions plugin builds

The **Release plugins** workflow can be started with **Run workflow** in GitHub
Actions. Pushing a `v*` tag also builds all platforms and attaches their archives
to a GitHub Release after every build succeeds.

| Platform | Architecture | Archive |
| --- | --- | --- |
| Ubuntu 24.04 | x86_64 | `.tar.gz` |
| macOS | Universal (Intel x86_64 + Apple Silicon arm64) | `.zip` |
| Windows | x86_64 (MSVC) | `.zip` |

Each archive contains `Algo Reverb.vst3`, `Algo Reverb.clap`, `Algo VN.vst3`,
`Algo VN.clap`, the license, user guides and `THIRD_PARTY_NOTICES.md`. Manual runs
upload downloadable Actions artifacts without creating a GitHub Release. These
are plugin-only builds: the WAV audition app, offline tools, `scripts`, and
`developer-docs` (including research notes) are not included. Packaging uses an
explicit allowlist of user guides and a dedicated release README, so it does not
copy source-only links into binary archives. Keep that allowlist synchronized
with the user documentation index when adding a guide.
The workflow does not configure Apple Developer ID signing or notarization.

**Fresh-checkout requirement:** ensure the coefficient files,
documentation, patches and dependency lockfile are committed along with the
workflow: CI builds from a fresh checkout, not local untracked files. Preserve
the notices when redistributing. The licensing findings distinguish upstream
MIT parameters, GPL calculation tools and source IR terms; they are not legal
clearance for copied/ported third-party code or arbitrary user-supplied content.

## WAV audition app

```sh
cargo run --release --features gui --bin reverb-lab
```

Click **Open WAV…**, then **Play**. Start at low listening volume. The app
auditions the Classic reverb; it is separate from the plugin IR-loading UI.
The `gui` feature uses eframe, a file dialog and audio-device playback.

The `bundle-macos` just recipe currently references a missing
`scripts/package-macos.sh`; use `cargo run` above rather than that recipe.

## Offline tools

```sh
cargo run --release --bin reverb-cli -- --help
cargo run --release --features ir-analysis --bin ir-analyze -- --help
cargo run --release --features ir-analysis --bin ir-compare -- --help
```

See [Offline IR tools](ir_tools.md) for arguments, output formats and limitations.

## Tests and checks

```sh
cargo test
cargo test --features ir-analysis
cargo test --features plugin
cargo test -p algo_vn
python3 -m unittest discover -s scripts -p 'test_*.py' -v
```

The default features are empty: plain `cargo test` does not exercise plugin DSP
or the optional IR-analysis tools. Use the feature/package commands above for
those paths.

Core integration tests in `tests/reverb.rs` cover the current Classic engine's
allocation-free controls, impulse onset, stereo decay, exact reset and bounded
modulation at extreme sample rates. The obsolete Character-switching tests have
been removed with that API. To run a focused CLI, IR-analysis and core check:

```sh
cargo test --features ir-analysis --bin reverb-cli --test reverb --test ir_analysis
```

Headless GUI tests inspect egui shapes and explicitly clear texture deltas,
since no renderer consumes their texture uploads. Run `cargo test -p algo_vn`
to exercise the Algo VN layout, file-browser and DSP tests without a plugin host.

For broader checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features plugin,gui -- -D warnings
cargo run -p algo_vn --release --example convolution_bench
```

`--offline` can be added after the required Cargo dependencies have been cached.
The benchmark is synthetic DSP timing, not a DAW deadline guarantee. Ignored
local-IR regressions require separately supplied WAVs and are not portable test
fixtures. Historical test counts and measurements in research notes are not
the current suite totals.
