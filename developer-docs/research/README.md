# Research archive (not a plugin manual)

[Developer documentation](../README.md) · [Current plugin guide](../../docs/plugins.md)

These notes preserve experiments, local measurements and earlier implementation
states. Historical test counts, temporary output paths and removed controls are
not current feature or performance claims. Reproduction requires the source
checkout, optional tools and, where indicated, separately licensed reference WAVs.
Local artifacts are not distributed fixtures. All commands run from the repository
root. This directory is excluded from plugin release archives.

- [Colorless FDN](colorless_fdn.md): isolated NumPy optimizer and static FDN.
- [Room-conditioned FDN](room_fdn.md): isolated room-profile fitter, not the
  current plugin's Rust IR-seed fitter.
- [DiffFDN / RIR2FDN port](difffdn.md): offline coefficient preparation, inference
  checks and history of the two embedded room fits.
- [Measured-IR validation](measured_ir_validation.md): successive offline
  FDN/VN/Hybrid/DVN comparisons and limits of the measurements.
- [IR auto-level study](ir_autolevel_study.md): original offline pink-RMS study;
  Pink Auto Level is now available in Algo VN only.
- [Hybrid preparation performance](hybrid_preparation_performance.md): offline
  fit optimization and historical live-Hybrid timings. Algo VN's live Hybrid,
  Generate/Rebuild controls and model cache have been removed.

Current Algo Reverb uses live DiffFDN, not the old Classic plugin engine; its
WAV seed fitting is implemented in Rust. Current Algo VN is convolution-only.
The Classic library/CLI and offline synthesis tools still exist. Do not confuse
the current Hybrid feedback-matrix control with the removed live Hybrid engine
or the offline VN-to-FDN WAV blend.