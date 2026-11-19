# Plugin guide

[Documentation index](README.md) · [Installation](installation.md)

## Algo Reverb: live DiffFDN replacement

**DiffFDN** replaces Classic. Algo Reverb is now DiffFDN-only; IR / Hybrid
playback is provided separately by Algo VN. The default is French Salon, Damping 0%, Dry / Wet 0.35,
Output 0 dB. The live wet path has fixed per-room reference trims of
**−25.4 dB (Salon) / −21.5 dB (Damped)**, followed by an equal-power Dry/Wet
crossfade and a
stereo-linked **−1 dBFS sample-peak safety limiter** after Dry/Wet and Output.
The trims approximately match input RMS on a 48 kHz log-spaced multitone with
default controls, not arbitrary music or perceptual loudness. Start at low listening
volume; the limiter is not a true-peak guarantee and can distort on overload.
Unity-dry Bypass is deliberately unprotected.

**Existing sessions become louder:** this replaces the quiet common −32 dB
Wet trim and linear crossfade. Wet rises by 6.6 / 10.5 dB respectively; the
equal-power crossfade also raises intermediate Dry/Wet levels. At 0% Wet,
Dry remains unity; at 100%, only Wet is present. Intermediate percentages now
represent a crossfade position, not linear amplitude fractions.
Bypass is unchanged. Lower Output before auditioning old sessions. Feedback
edits, sample rate, stereo-to-mono cancellation and source spectrum still affect
Wet loudness; no time-varying automatic gain is applied. Pre-limiter samples
can exceed full scale; the post-mix limiter remains enabled for DiffFDN.

The preset path is a pretrained eight-line delay/scattering network, per-line
absorption IIRs and fitted output EQ. It performs **no convolution**. The fixed output-EQ edge cuts are disabled in both bundled
coefficient sets: French 18th Century Salon and Highly Damped Large Room.
The diagnostic +2 dB direct/wet energy preference is **not** applied. There is
no direct impulse in Wet: direct sound comes only from the existing Dry / Wet
mix. Input to Wet is `(L+R)/2`; Wet is duplicated mono, while Dry stays stereo.

### Peak Space (experimental)

**Peak Space** is always expanded in the scrollable editor. Enable it after importing an IR and selecting **Use IR seed**.
It also works with the bundled room. It is **off by default** and does not change
seed fitting, topology, absorption or the sound of the disabled path.

- **Sensitivity**: onset novelty threshold, default −24 dBFS (lower = more sensitive).
  A fast/slow peak detector with 2:1 hysteresis and a 40 ms refractory interval
  rejects ordinary sustained-waveform ripple. Detection uses the wet mono input,
  not the ducking sidechain. Anti-phase stereo can cancel before detection.
- **Space Decay**: 150 ms by default, fades the new space's audible gain from
  unity to −18 dB. **Swell**: 500 ms back to unity, then holds there.
- **Space Release**: 300 ms by default, fades the previous space from its current
  envelope gain. All three timings cover 10–3000 ms.
- **Swell Steps**: **Smooth** (default) keeps the original continuous swell;
  **1–8** gives that many equal upward gain increments during one Swell. Try **4**
  with **Swell 800–1200 ms** for a staircase: low → hold → rise → hold → rise.
  The total Swell duration is unchanged; each rise uses a smooth 5 ms edge
  (at most one quarter of a short step), ending the final rise at unity.
  This shapes audible gain only, not pitch, delay lengths or feedback.

#### Slice Repeat

Enable both **Peak Space** and **Slice Repeat**. Repeat defaults to OFF, so old
sessions retain their sound. This is an evolving block-feedback effect, not a
freeze: each slice adds new source audio to the filtered previous slice.

- **Space Repeat** captures the wet output, including its reverberation, then
  repeats those stereo space fragments before ducking and Dry/Wet.
- **Input Repeat** captures the mono wet input and repeats fragments into Peak
  Space. The original stereo Dry path is untouched in both modes.
- **Division** sets capture and repeat length: **1/4, 1/8 (default), 1/16, 1/32**.
  At 120 BPM, 1/8 is 250 ms. Tempo follows the host, limited to 20–400 BPM;
  missing/invalid tempo uses 120 BPM. While playing, available host beat position
  aligns the slice boundaries to the note grid. Without position the slices run
  from activation. Seeks, transport changes, tempo/division and mode edits fade
  out over 5 ms before discarding previous slices, then fade back in; realignment
  is applied at a subsequent host block, so transition time depends on block size.
  The first partial slice is zero padded. This is not a pitch-preserving tempo ramp.
- **Feedback**: 0–95%, default 60%. Zero plays one delayed copy; increasing it
  carries more of each filtered slice into the next. Internal samples are bounded
  at ±16 to prevent unbounded feedback accumulation, in addition to output safety.
- **Groove Offset**: fixed −50 to +50 ms shift of the repeat component, default 0.
  The note interval and feedback loop period stay unchanged: at 120 BPM / 1/8,
  −5 ms places repetitions at 245, 495, 745 ms rather than 250, 500, 750 ms.
  +5 ms places them at 255, 505, 755 ms. There is no accumulating drift.
  A separate playback tap shifts the repeats without moving Dry or the
  un-repeated source. In Input Repeat this shifts the repeated excitation before
  the reverb, not an already-existing reverb tail. Advance uses the existing
  one-note delay, not lookahead; the earliest possible repeat is 1 ms after its
  source, so very short notes limit the effective negative range. Offset changes
  fade and clear slices. Shift-drag gives fine adjustment; fractional sample
  positions use linear interpolation. The persisted `repeat_nudge` ID is retained,
  but nonzero values saved with the earlier per-loop Nudge now mean fixed offset.
- **Repeat Mix**: default 65%; 100% emphasizes isolated fragments, 0% retains the
  un-repeated source. It controls this stage only, not the plugin's Dry/Wet.
- **High Cut**: 500–20,000 Hz, default 6,000 Hz; a one-pole low-pass applied on
  every repeat makes later generations progressively darker.
- **High Pass**: 20–4,000 Hz, default 120 Hz; a one-pole high-pass removes bass
  from the repeat path on every pass. Neither filter affects Dry.

Slice boundaries have smooth 2 ms edges (at most one eighth of a short slice).
Use Repeat Mix 80–100%, 1/8, Feedback 60–75%, High Cut 3–6 kHz and High Pass
120–300 Hz as a starting point for separated, darkening space fragments.
Swell Steps remains independent; it still shapes the original Peak Space gain.
Turning Peak Space or Repeat off fades back to the original path. Capture buffers
are prepared at activation, not allocated while processing.

#### Separate Wet Out

The plugin exposes an additional stereo **Wet Out** bus alongside the unchanged
Main Out. Enable that additional output in a multi-output-capable VST3/CLAP host
and route it to another track for EQ, compression or recording. Exact routing
steps depend on the DAW. Updating the plugin changes its bus layout: close the
host before replacing the bundle, rescan, and test a fresh instance and copies
of old sessions. Existing instances may need to be recreated.

Wet Out is tapped after Peak Space / Slice Repeat and ducking, follows **Output**,
and has its own −1 dBFS stereo-linked safety limiter. It does **not** follow the
Dry/Wet knob, contains no Dry signal, and fades to silence on Bypass. It works
with both bundled rooms and imported seeds, even with Peak Space disabled.
Main Out's limiter and mix are unchanged. To use Main as Dry and the extra bus
as Wet, set Dry/Wet to 0%; otherwise monitoring both buses adds Wet twice.

Two prebuilt copies of the **same unchanged seed** alternate. A peak resets the
incoming voice with a 5 ms smooth output attack; only it receives input until the next accepted peak. The outgoing
voice receives silence and releases its existing tail. Overlap gains are normalized
when their sum exceeds unity (not a loudness guarantee). A 5 ms wet input predelay
preserves the attack while fading the older slot before reuse on rapid retriggers;
onsets inside 40 ms are ignored, not queued. Switching the effect on/off fades
through silence over 10 ms each way. Dry audio is neither delayed nor enveloped.

The envelope is **outside** the feedback loop: no extra feedback gain or nonlinear
processing. T60/IR absorption still determine available tail energy; a short or
heavily damped IR can die before the swell. Increase T60 or shorten Swell if needed.
Swell has fixed timing, not tempo prediction or anticipation of an unknown next
note. Continuous input can mask the dip because it keeps exciting the active space.
The wet output remains subject to the existing ducking and safety limiter.

Both voices are allocated during activation/background import, never at a peak.
Peak Space processes two FDNs instead of one; reset clears preallocated buffers
and thus still has a bounded CPU cost. Audition in a DAW at low listening volume;
automated tests do not establish perceptual click-free transitions for every IR.

### Import IR seed (experimental, Rust-only)

Activate the plugin, click **Browse…**, choose a mono/stereo WAV in the same
in-editor browser used by Algo VN, then
enable **Use IR seed** after the status reports completion. Disable it to return
to the embedded room (Salon for new instances). The folder WAV dropdown,
**◀ / ▶** (no wrapping) and **Refresh** support auditioning adjacent files.
The last successfully imported path/folder is remembered; failed imports keep
the previous seed and selection. Restoring sound never requires that WAV.
Room and Matrix selectors are hidden; their saved parameter IDs and DSP remain
for existing sessions. New instances use **Householder -> Hadamard**.
Delay Modulation defaults to **Static** (off): select **Random / Linear** or
**Random / Allpass** to activate Depth and Rate.
Existing room parameter IDs and trims are unchanged;
the new enable parameter is appended and defaults off.

The worker decodes WAVs (0.1–60 s, 8–384 kHz), estimates seven octave-band decay
times, and builds an independent eight-line scattering FDN, without loading Salon
coefficients. It compares 36 analytic topology candidates and 16 delay refinements
using capped 240 ms / 12 kHz renders. The structural loss measures early energy
distribution, local moment density, and energy-cluster autocorrelation over the
first 240 ms. Main delays, scattering delay scale, and orthogonal butterfly
rotation strength are fitted; feedback remains passive. It then fits passive
per-loop peaking cuts with bounded coordinate descent, and
calibrates output tone, late L/R energy ratio and zero-lag correlation using
three capped 24 kHz trial renders. At least two reliable bands and a reliable
broadband decay are required; unreliable bands use the broadband estimate.
Invalid/noisy/short/non-decaying sources can be rejected without replacing the
current room. Mono seeds target identical outputs. Stereo seeds use distinct
output weights from **one shared network**, not two running reverbs.

Cabinet/early-reflection-only IRs are not the intended input: files under 100 ms
are rejected, and padding them does not provide a reliable reverberant decay.
Use Algo VN convolution for faithful cabinet tone or early-reflection playback.

This is feature matching, **not** the pretrained Python/DecayFitNet pipeline or
waveform reconstruction. New imports fit delays/scattering as well as
absorption/output EQ, but do not copy early reflections,
predelay, per-channel decay, frequency-dependent spatial cues or true-stereo input
responses. Input remains `(L+R)/2`. Zero-lag correlation is not IACC. Output tone
is an approximate capped correction, not a jointly optimized multiband solution.
The fit does not identify wall materials, individual room modes, or narrow-band
resonances. The bounded structural loss is a proxy, not a perceptual similarity
guarantee; octave decay still approximates spectral evolution. Compact v2 state
stores the selected structure; v1 seeds restore the old Salon-based sound unchanged.
Re-import the source WAV to upgrade an existing seed. Embedded preset/v1
coefficient provenance is recorded in [third-party notices](../THIRD_PARTY_NOTICES.md);
new v2 topology generation does not use the pretrained room coefficients.

Custom Wet is normalized to unit impulse energy at the calibration rate, not
matched to source WAV amplitude or perceptual loudness. Existing controls add
loss/modulation and therefore change the fitted decay and spatial statistics;
even default live loss changes the offline calibration. Equal-power Dry/Wet and
the −1 dBFS limiter remain. Start with low monitoring volume.

The UI reports elapsed load/fit/build time. Local release measurements after
structural fitting were 0.165–0.220 s for three 0.95–3.73 s / 44.1 kHz stereo
room IRs (concurrent timing tests; not isolated benchmarks). These are
not a DAW deadline guarantee. Decode/analysis/build run on a worker. Import
replacement fades to the embedded room, swaps at zero custom blend, then fades
back if enabled (150 ms per leg). Only the selected network runs at steady state.
The callback try-locks prepared handoffs and does not destroy retired networks;
the UI/worker reclaims them. Reset clears existing storage without constructing
a network. Activation rebuilds compact persisted coefficients, without needing
the original WAV or repeating analysis. Sample-rate changes invalidate in-flight
imports; import again. DAW save/reload and in-editor browser behavior still need a
manual host check.

**Damping (0–100%)** adds passive HF feedback loss, interpolating the original
loop signal with a 2.5 kHz one-pole lowpass (20 ms control smoothing). At 0%
the original fitted filters are unchanged (Classic editing controls still apply). It can shorten/darken the tail,
not lengthen it; it is not a calibrated T60 knob. Room changes use a finite **150 ms linear crossfade**. Only the selected
network runs in steady state; both run during transitions. The destination starts
with an empty tail, and the outgoing network is cleared at fade completion.
Rapid reversals continue from the current blend position without a jump. Both
networks remain preallocated and receive control updates; clearing uses existing
buffers, without allocating. Reset clears both and selects the current room.

Integer rates **8–384 kHz** are accepted. 48 kHz is the original fit; other
rates scale/round all delays and bilinear-transform every SOS off the audio
thread. This preserves stable poles and passive filter magnitude bounds but
is an approximation, **not a new fit**: frequency warping, delay rounding and
wet gain differences are especially pronounced at low rates. Prefer 48 kHz
for the auditioned response. Fractional/out-of-range rates fail activation.

**Old sessions sound different.** Plugin IDs are retained. The obsolete `engine_mode`, `ir_auto_level`, and
`ir_safety_limiter` parameters and embedded IR state have been removed.
Old IR-mode sessions cannot reproduce their sound in this build; retain the
previous plugin and migrate them to Algo VN before upgrading. DiffFDN control
IDs remain stable; verify old sessions in your host. The Classic interface and its
Matrix, Input/Loop Diffusion, T60, delay modulation, and Low/Mid/High damping
controls are active again. They edit the learned network without convolution:
T60 adds loop attenuation on top of fitted absorption (not a calibrated final
decay time); Low/Mid/High damping adds feedback loss, and input/loop allpasses
and random delay modulation use the original Classic control laws. Hybrid
keeps the learned scattering; Householder/Hadamard add orthogonal transforms
to it, rather than replacing its four stages. Thus these controls do not
reproduce the old Classic sound. New automation IDs
are `difffdn_room` and `difffdn_damping`. Room and Damping save as ordinary host
parameters; no external coefficient/IR files are needed to restore them.

Algo Reverb reports zero latency. WAV-to-FDN seed import is available as described
above; it extracts features and builds a live network rather than playing the WAV.
For convolution playback of the actual IR, use Algo VN as described below.

No reference WAV is bundled. Embedded coefficient provenance and upstream license
notices are recorded in [third-party notices](../THIRD_PARTY_NOTICES.md).

## Algo VN: IR Time Damping / Wet Tone Filter

Algo VN is now convolution-only: the live FDN, reference fitting, editable Hybrid
engine, structural rebuilds and `fdn_*` host parameters have been removed. The
offline synthesis tools are unchanged; Algo Reverb now uses DiffFDN as described above.

The compact editor reuses Algo Reverb's ring dials and JetBrains Mono font:

- **Impulse Response**: choose **Browse…**, navigate folders, then double-click a
  WAV or choose **Open & Load** to load it automatically. This picker is drawn
  inside the plugin, avoiding the native AppKit dialog that previously crashed
  Live. Cancel/Escape retains the current IR. WAV selection uses only **Browse…**
  and the dropdown; there is no manual path field or separate load button.
  Successful loads remember the folder and selected file in this plugin instance's
  project/preset state. The dropdown beside **Browse…** lists only that folder's
  WAVs (case-insensitive extension, no subfolder recursion); choosing one loads
  it automatically. **Refresh** rescans after adding/removing files. Reopening
  the editor or restoring the project restores the folder; a missing folder
  reports an error but never replaces the embedded IR. This is per-instance state,
  not a machine-wide default shared by newly inserted plugin instances.
  Development-only **Comparison WAVs** shortcuts have been removed; no comparison
  folder, `reference.wav` companion file, or offline generation is required.
  Bring your own IR; no factory IRs are bundled.
- **IR Time Damping**: Low / Mid / High attenuation (0–120 dB/s), their frequencies,
  and Mid Q. Left-drag a dual ring for decay, right-drag for frequency.
  Preparation messages occupy a reserved line so the controls stay in place.
  Browse and WAV selection remain temporarily disabled during IR preparation.
  With IR Time Damping OFF, changing its knobs does not rebuild the IR. Switching
  ON to OFF still restores the untouched IR once; switching back ON applies the
  latest knob positions. Loading a different WAV still prepares its convolution.
- **Wet Tone Filter**: Low / Mid cuts (0–24 dB), frequencies and Mid Q, plus a High
  low-pass cutoff. This filters Wet only, with 20 ms smoothing.
- **Dry / Wet**, **Output**, **Bypass**, **Pink Auto Level**, **Safety Limiter**
  retain their IDs and defaults (Output −12 dB; Pink OFF; safety ON).

Shift-drag gives fine adjustment. Double-click or Ctrl/Cmd-click resets the
selected ring. Both filter stages default OFF. IR damping increases cut with
IR age (up to 120 dB), using cut-only shelves and a mid peaking filter; it is not
feedback-loop damping. Its background worker applies changes after 300 ms while
the editor is open and discards stale source/rate/settings results. **Stop
transport before loading or changing IR damping**: applying clears convolution
history and can click. Wet Tone changes are live and leave Dry / Bypass alone.

The original IR, calibration reference and last applied IR damping are embedded
in sessions. Repeated damping edits always start from the original. Pink gain
uses the untouched IR / shared Reference, so calibration does not undo damping.
Zero added latency and allocation-free convolution processing are retained;
FFT work is synchronous, so test CPU deadlines at the actual host buffer size.
Safety is sample-peak protection, not true peak; Bypass is unprotected.

**Compatibility:** plugin IDs are unchanged. Older generated Hybrid sessions now
play their embedded rendered fallback IR only; the live FDN tail and its
automation are no longer restored, so these sessions may sound different.

## Algo VN loading and convolution

Load **Algo VN** in the DAW and stop transport. Click **Browse…** and select an IR
WAV, then **Open & Load** (or double-click the WAV). The selection loads automatically.
Folders open on double-click; **Up** goes to the parent and the location field plus
**Go** accepts an absolute folder path. Folder listings are read in the background.
This is a fixed-height in-editor picker, not a native macOS dialog; this layout
has not yet been verified in Live. The main dropdown remembers the WAV folder;
**Refresh** rescans it and **◀ / ▶** load the previous/next WAV in sorted order
(no wrapping at the ends). Long names are truncated in the dropdown and can be
read using hover text; the Browse list supports horizontal scrolling.
Comparison-folder shortcuts have been removed; the plugin does not analyze or generate IRs.
Existing offline exports can still be loaded individually as ordinary WAV IRs.
Do not load the music/listening probe WAVs as impulse responses.

In macOS Finder, select the WAV and press **Option–Command–C** to copy its pathname.
Spaces and enclosing quotes are accepted in the browser location field. The
in-editor picker does not open the native modal dialog removed after Live crashes. No IR is bundled; without
one, Wet output is silent. Stop transport before each load: switching clears the
tail and is not seamless or host-automatable. Start with low Output.

- Mono excitation `(L + R) / 2` into a stereo IR, matching Classic's input convention;
  this is **not true-stereo** convolution and anti-phase input cancels in the wet path.
- Algo VN uses **zero added latency**: a 128-sample direct
  FIR head plus FFT stages starting at IR offsets 128, 256, 512, 1024 and 2048.
  Dry and internal bypass are undelayed; host latency is reported as zero.
  IR onset/pre-delay and audio-interface latency remain. Algo Reverb's former
  integrated convolution engine has been removed.
- Source WAVs: 8–384 kHz, mono/stereo, **maximum 30 seconds**. Host: integer rates
  8–192 kHz. Rate conversion uses windowed-sinc filtering with kernel gain correction;
  native-rate loading preserves the IR samples. Optional Wet calibration and
  default-on Safety Limiter do not rewrite the source IR; no tail fade.
- File loading, FFT preparation and allocations happen in the GUI/activation,
  not processing. The GUI may pause while loading large files. Processing uses
  a nonblocking handoff; retired buffers are reclaimed outside the audio thread.
  Loading **clears the previous tail and may click**: stop transport first.
- Dry/Wet, Output and Bypass are smoothed and host-automatable. Output defaults to
  **−12 dB**, versus Algo Reverb's 0 dB. Start quietly: source IR gain is unaltered.
- The source samples are embedded in project/preset state, not referenced by a
  temporary path. This increases state size; verify the reference-derived content's
  license before sharing projects. Invalid loads retain the previous IR.

## Pink Auto Level

Enable **Pink Auto Level** to calibrate Wet RMS using a 20 Hz–min(20 kHz,
0.45 × host rate) pink spectrum. Algo VN defaults to OFF and uses individual
WAV calibration. Historical comparison
buttons used a shared `reference.wav` calibration and required the Reference file
at load time. Source and any legacy calibration Reference are embedded
in the session, so neither external WAV is needed on restore. Source samples
remain unchanged. Host-rate resampling precedes calibration; changing rate may
slightly change correction. Loading/calibration can pause the GUI.

Correction is smoothed, Wet-only, before Dry/Wet, Output and Safety Limiter.
It is limited to ±24 dB; transfer RMS at or below -120 dB receives unity gain.
This is spectral RMS matching, not LUFS or peak protection. Keep Safety Limiter
enabled for auditioning. Stop transport before loading. Algo Reverb does not
offer Pink Auto Level; its embedded rooms use the fixed trims described above.

## Safety Limiter

The IR engines use a linked stereo, zero-lookahead sample-peak limiter after
Dry/Wet and Output, before Bypass. Its ceiling is −1 dBFS, with instantaneous
attack and 100 ms release. Lower Output if reduction is frequent. It can distort
transients and is not true-peak protection, a loudness matcher, or a guarantee
of safe listening volume. Bypass is unprotected; live DiffFDN is also protected.
For strict linear comparisons, lower Output before disabling the limiter.

## Real-time limitations

**Experimental scheduling limitation:** FFT work is currently synchronous and
clusters at stage boundaries. Initial single-instance synthetic DSP measurements
at 88.2 kHz showed deadline overruns for short host buffers, and for a 30-second
IR even at 256 samples. This is not ready to claim reliable low-buffer live use.
Try short IRs and 256+ samples first; measure in the DAW. Future work should spread
FFT/multiply work across callbacks or use deadline-aware background processing.
The benchmark reports mean cost, p99, maximum callback time and over-budget count;
OS scheduling noise and concurrent system load affect results. It excludes the
plugin UI, smoothing, limiter, and the rest of the DAW.

## Session compatibility

Plugin IDs are unchanged. Test copies of old projects before replacing installed
bundles. Older Algo VN sessions from the 1024-sample engine now use zero added
latency, so the host must refresh delay compensation. Sessions without the
Safety Limiter parameter use its enabled default; overloaded playback may differ.
See the Algo VN compatibility warning above for removed live Hybrid parameters.
