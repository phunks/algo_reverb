# Plugin installation

[Documentation index](README.md) · [Plugin guide](plugins.md)

## Formats and platforms

Both **Algo Reverb** and **Algo VN** are built as **VST3** and **CLAP**.
AU and AAX are not supported. Choose a format supported by your DAW.

| Archive platform | Architecture |
| --- | --- |
| Linux (built on Ubuntu 24.04) | x86_64 |
| macOS | Universal: Apple Silicon arm64 + Intel x86_64 |
| Windows | x86_64 |

The release workflow targets these platforms; a successful archive build alone
does not establish compatibility with every DAW or OS version. Linux builds are
not guaranteed to run on distributions older than the build environment.

## Install

Close your DAW, extract the archive, and copy each plugin you want to a location
scanned by your host. Keep `.vst3` and macOS `.clap` bundles intact; do not copy
only a binary from inside them.

| Platform | Typical VST3 location | Typical CLAP location |
| --- | --- | --- |
| macOS (user) | `~/Library/Audio/Plug-Ins/VST3/` | `~/Library/Audio/Plug-Ins/CLAP/` |
| Windows (system) | `C:\Program Files\Common Files\VST3\` | `C:\Program Files\Common Files\CLAP\` |
| Linux (user) | `~/.vst3/` | `~/.clap/` |

Windows system folders may require administrator permission. Hosts can support
additional/custom scan locations. Rescan plugins, insert a fresh instance and
start at low monitoring volume. Before replacing an installed build, keep a
backup and read the old-session warnings in the plugin guide.

macOS releases are **not Developer ID signed or notarized** by the workflow.
Gatekeeper or your host may block them. Follow your host's and macOS's security
procedures only for builds whose origin you trust; this guide does not recommend
disabling system-wide security protection.

## Archive contents and runtime requirements

Each archive contains the two plugins in both formats, a release README,
`LICENSE`, `THIRD_PARTY_NOTICES.md`, and these user guides. Source code, developer
tools, research scripts and research notes are not packaged.

No external IR WAVs are bundled. Algo Reverb's two room coefficient sets are
embedded in the plugin. Algo VN needs your own WAV IR; until one is loaded its
Wet path is silent. Source build tools and offline numerical packages are not
needed for normal plugin use or Algo Reverb's built-in IR-seed fitting.