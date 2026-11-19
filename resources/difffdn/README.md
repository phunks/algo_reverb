# Embedded live DiffFDN coefficients

These are text coefficients, **not impulse-response WAVs**. Both are eight-line
SCAT/N8D1A fits with the fixed output EQ edge-correction cuts disabled; fitted
per-line absorption and the remaining output EQ are retained unchanged.

Copied verbatim from these local validated artifacts:

- `target/difffdn-no-edge-v1/French 18th Century Salon/validation/room.fdn`
  → `salon.fdn`, SHA-256
  `1911471b21b6cead46f2360e32d4dd2aed6880db0620d8f518bfd5cf9dc695cf`
- `target/difffdn-no-edge-v1/Highly Damped Large Room/validation/room.fdn`
  → `damped.fdn`, SHA-256
  `eb197dde11b4e8ed361f3dec3300f84de133238a43bc0b23d368efd046bac417`

See the [source-only preparation log](../../developer-docs/research/difffdn.md)
for pinned upstream model/filter dependencies, the explicit scattering-delay
override and export/validation commands. See [third-party notices](../../THIRD_PARTY_NOTICES.md)
for the licensing findings and full upstream MIT texts. The pretrained structure
repository is MIT; using GPL calculation tools does not by itself make coefficient
outputs GPL. The source IR terms restrict IR-file redistribution; no express
prohibition on fitted coefficient distribution was found, which is not explicit
permission or legal clearance. The earlier blanket "local research only" warning
has been replaced by these qualified findings. Reference WAVs are not packaged.

No diagnostic direct impulse or +2 dB direct/wet preference is embedded.
Coefficient files retain fitted gains unchanged. The plugin applies fixed
per-room reference trims of −25.4 dB (Salon) / −21.5 dB (Damped), an equal-power
Dry/Wet crossfade and a −1 dBFS post-mix sample-peak safety limiter. This replaces
the quiet common −32 dB trim; existing sessions become louder. The reference
is a 48 kHz multitone RMS measurement, not perceptual matching or AGC.
Classic controls add editable diffusion, feedback loss and
modulation; the extra 0–100% damping defaults to bypass. Both
networks are prepared at activation, never parsed/allocated by processing.