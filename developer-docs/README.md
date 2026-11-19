# Developer documentation (source checkout only)

[Project overview](../README.md) · [Plugin user guides](../docs/README.md)

- [Build, test and WAV audition](development.md)
- [Classic library / CLI engine](classic_engine.md)
- [Offline IR analysis and resynthesis](ir_tools.md)
- [Research archive](research/README.md)

This directory and the optional `scripts` directory are not included in plugin
release archives. Commands run from the repository root, not this directory.
Optional Python/MATLAB/Octave tools are for offline development only; neither
plugin needs them at runtime. Source distribution must preserve licenses for
any third-party code it includes, independently of binary archive contents.