# Documentation

[Project overview](../README.md)

## Plugin user guides

- [Installation](installation.md): supported formats, platform-specific locations,
  archive contents and unsigned macOS builds.
- [Plugin guide](plugins.md): controls, IR loading, latency, protection and
  session compatibility.
- [Third-party notices](../THIRD_PARTY_NOTICES.md): embedded coefficient provenance
  and applicable upstream license texts.

**Algo Reverb** is a live DiffFDN with two embedded rooms and optional Rust-only
WAV-to-stereo-seed fitting. It does not convolve IRs. **Algo VN** plays WAV IRs
through convolution, with IR Time Damping and Wet Tone Filter; it does not run
a live Velvet Noise / Hybrid generator.

These guides are self-contained in plugin release archives. Developer tools,
source build instructions and historical research notes are not included in
those archives. No Python, MATLAB or Octave installation is required to use
either plugin, including Algo Reverb's IR-seed importer.
