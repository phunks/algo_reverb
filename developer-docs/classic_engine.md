# Classic reverb engine

[Developer documentation](README.md) · [Plugin guide](../docs/plugins.md)

This guide describes the **Classic library / CLI / WAV audition engine**, not
the current Algo Reverb plugin. The plugin uses live DiffFDN; its similarly named
controls edit a learned network and do not reproduce Classic's response.

The Classic engine runs **one eight-line FDN**, with the original 31–63 ms
delay distribution, original diffusion, and no added early reflections.
The experimental Room / Chamber / Plate banks and Character crossfade have
been removed: no inactive tanks are allocated or processed.

T60, Matrix, Damping, Input / Loop Diffusion, modulation, mix, output and ducking
controls remain available as before. T60 is nominal: diffusion, damping and
modulation also affect actual decay. Reset clears the single tank and immediately
applies current target parameters. There are no independent Size or Pre-delay controls.

The Character selector, its plugin parameter (`character`), and the fourth CLI
argument have been removed. Old Room / Chamber / Plate settings no longer
reproduce those voicings. The Classic library DSP response is preserved, but the
current plugin has a different engine, an Output default of **0 dB**, and a
post-mix safety limiter. Classic CLI exports are not protected by that plugin
limiter. Start with low listening volume.

## Damping

All damping is inside the feedback loop, shortening the affected bands' tails
rather than simply EQing the output.

- **High Damping / Hz**: the original one-pole low-pass. Lower values darken the tail faster.
- **Low Damping / dB**: additional feedback attenuation, 0–24 dB, with a low shelf (20–2000 Hz, default 250 Hz).
- **Mid Damping / dB**: additional feedback attenuation, 0–24 dB, with adjustable center (100–12000 Hz, default 1 kHz) and Q (0.2–8, default 0.707).

In the current plugin's Classic-style UI, Low and Mid are concentric knobs: **left-drag** adjusts the outer
attenuation ring; **right-drag** adjusts the inner frequency ring. Both values are
always displayed. Hold Shift for fine adjustment. Double-click with the respective
button (or Ctrl/Cmd-click) to reset that parameter. Mid's compact Q field supports
dragging and keyboard entry. High remains a single frequency knob.
Each value is a separate host-automatable parameter. At low sample rates, Low/Mid
frequencies are internally capped at 45% of the sample rate.
The audition app exposes the same shape parameters under **Damping bands**.

Low and Mid default to 0 dB (off), preserving the original sound. Their dB values
describe attenuation per feedback pass at DC (Low) or the center frequency (Mid),
not output gain or an exact decay-time reduction. Their broad responses overlap.
Start with small values. Changes are smoothed over 20 ms without clearing the tail.
The existing High parameter retains its host automation ID (`damping`).

## Ducking (current DiffFDN plugin, not the Classic CLI)

Ducking lowers only the wet output, leaving dry audio and the reverb's internal
tail intact. **Source** selects the internal stereo input or the DAW's external
sidechain; a disconnected external sidechain is treated as silence.

The **Level / Transient** button selects the detector (also host-automatable):

- **Level** (default): the original full-band level detector. Loud sustained input
  keeps the wet signal ducked. Existing parameter IDs and defaults are unchanged.
- **Transient**: detects rises above a slower peak envelope, then releases even
  while the input sustains. A new onset can trigger again. Threshold sets the
  absolute onset sensitivity, with a relative floor to reject waveform ripple.
  This is an onset detector, not a frequency filter.

**Amount** caps the reduction. **Attack** controls how quickly it increases;
**Release** controls recovery. In Transient mode, each trigger holds its target
for the longer of 20 ms or three Attack time constants so slow attacks still work
(up to 600 ms at maximum Attack). Release follows that hold. A slower Attack still
lets the earliest part of the onset through; there is no lookahead.
