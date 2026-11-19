# Embedded DiffFDN provenance and third-party notices

## Embedded room coefficients

Algo Reverb embeds two text coefficient sets, **not IR WAVs**: French 18th Century
Salon (`salon.fdn`) and Highly Damped Large Room (`damped.fdn`). They use the
eight-line `SCAT/N8D1A` pretrained structure from diff-fdn-colorless, room-specific
absorption and output EQ, with the fixed output-EQ edge cuts disabled.
No diagnostic direct impulse or +2 dB direct/wet preference is embedded.

SHA-256 of the embedded coefficient files:

| File | SHA-256 |
| --- | --- |
| `salon.fdn` | `1911471b21b6cead46f2360e32d4dd2aed6880db0620d8f518bfd5cf9dc695cf` |
| `damped.fdn` | `eb197dde11b4e8ed361f3dec3300f84de133238a43bc0b23d368efd046bac417` |

Upstream preparation references:

- [RIR2FDN](https://github.com/gdalsanto/rir2fdn),
  `4840fafebb48b9140492dd6c4f688e881cb9fad0`.
- [diff-fdn-colorless](https://github.com/gdalsanto/diff-fdn-colorless),
  `2497bd0731d0d8f8959a0843bfb726c1ef801f7d`: MIT, including the repository
  containing the reused `SCAT/N8D1A` parameters. No separate parameter license
  was found. Its copyright and permission notice are reproduced below.
- [DecayFitNet](https://github.com/georg-goetz/DecayFitNet),
  `01daf3e7bbfd637aa1269bbca0cab7f445db0d5d`: MIT. The pretrained model was used
  for offline decay estimation; it is not a plugin runtime dependency. Its
  notice is preserved below for the preparation provenance.
- Two_stage_filter, `dcd149a1deef80bd5419bc42974de488e5f00ea0`, and fdnToolbox,
  `5f0c29b42e3682463cd0a0a75d32149949247731`: GPLv3 offline design/reference
  tools. Using a GPL program to calculate coefficients does not by itself make
  those numerical outputs GPL. This is not clearance for copying or porting GPL
  implementation code; any such code requires separate license review.

The room reference WAVs were supplied locally from the Voxengo Impulse Modeler
IR collection, whose notice identifies Aleksey Vaneev as the copyright owner.
Its terms permit royalty-free use, including commercial use, but restrict
redistribution of the IR files themselves. No express prohibition on distributing
fitted FDN coefficients was found; that is not an explicit grant for coefficients
or legal clearance. Do not infer that the WAV files can be bundled with these
plugins. No reference WAVs, Python scripts, upstream training models or
MATLAB/Octave tools are included in plugin archives.

These findings replace the earlier blanket description of the coefficients as
"local research only". They document provenance and the identified terms rather
than asserting that all reference-derived outputs are either prohibited or
unrestricted. User-supplied IRs, session-embedded audio and exports that copy
reference samples must be assessed under their own source licenses.

This notice covers the embedded DiffFDN material and its preparation provenance;
it is not a complete inventory of Rust dependency or bundled font licenses.
Other third-party components retain their respective licenses.

## diff-fdn-colorless — MIT License

Copyright (c) 2023 Gloria Dal Santo

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## DecayFitNet — MIT License

Copyright (c) 2021 Georg Götz

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.