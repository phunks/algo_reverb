# Algo Reverb / Algo VN

VST3 and CLAP reverb plugins:

- **Algo Reverb**: live DiffFDN, two embedded rooms, optional Rust-only stereo
  IR-seed fitting, Peak Space and Slice Repeat. No convolution.
- **Algo VN**: WAV IR convolution, IR Time Damping and Wet Tone Filter.

Read the [installation guide](docs/installation.md) and
[plugin guide](docs/plugins.md) before use. Start at low monitoring volume;
old sessions may sound different. No external IR WAVs are included.

These are plugin-only archives, not source/developer packages. macOS builds
are not Developer ID signed or notarized by the release workflow.

## License

Project code: [MIT](LICENSE). See [third-party notices](THIRD_PARTY_NOTICES.md)
for embedded coefficient provenance and upstream notices. User-supplied IRs
remain subject to their own licenses.