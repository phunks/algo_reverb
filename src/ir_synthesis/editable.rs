//! Offline editable Hybrid. Fixed reference gains avoid undoing damping edits.
use super::*;
use crate::ConfigError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TailControls {
    pub t60: f32,
    pub damping: f32,
    pub matrix: Matrix,
    pub diffusion: f32,
    pub loop_diffusion: f32,
    pub low: f32,
    pub mid: f32,
    pub low_hz: f32,
    pub mid_hz: f32,
    pub q: f32,
    pub delay: crate::DelayMode,
    pub depth: f32,
    pub rate: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_neutral_preserves_baseline_and_live_edits_preserve_prefix() {
        for rate in [8000, 16000] {
            for channels in [1, 2] {
                let original = super::super::tests::reference();
                let ir = Impulse {
                    sample_rate: rate,
                    channels,
                    ..original
                };
                let model = EditableHybrid::prepare(
                    &ir,
                    SynthesisOptions {
                        t60: Some(0.5),
                        ..Default::default()
                    },
                )
                .unwrap();
                let baseline = model.render(model.fitted).unwrap();
                let (correction, mut neutral) = model.streaming(model.fitted).unwrap();
                let (_, mut edited) = model.streaming(model.fitted).unwrap();
                let changed = TailControls {
                    t60: 0.12,
                    damping: 500.0,
                    low: 12.0,
                    mid: 10.0,
                    low_hz: 200.0,
                    mid_hz: 900.0,
                    q: 1.2,
                    ..model.fitted
                };
                edited.update(changed);
                let keep = model.late + (rate as f64 * HYBRID_HOLD_MS / 1000.0) as usize;
                let mut delta = 0.0;
                for i in 0..baseline.len() {
                    let input = if i == 0 { 1.0 } else { 0.0 };
                    let a = neutral.process(input);
                    let b = edited.process(input);
                    for ch in 0..2 {
                        assert!(
                            (correction[i][ch] + f64::from(a[ch]) - baseline[i][ch]).abs() < 1e-10
                        );
                        if i < keep {
                            assert_eq!(a[ch], b[ch]);
                        }
                        delta += f64::from((a[ch] - b[ch]).abs());
                    }
                }
                assert!(delta > 1e-6);
                neutral.reset();
                assert_eq!(neutral.process(0.0), [0.0; 2]);
            }
        }
    }

    #[test]
    fn live_tail_update_does_not_reset_history_and_extends_past_reference() {
        let ir = super::super::tests::reference();
        let model = EditableHybrid::prepare(
            &ir,
            SynthesisOptions {
                t60: Some(0.5),
                ..Default::default()
            },
        )
        .unwrap();
        let (_, mut tail) = model.streaming(model.fitted).unwrap();
        let mut energy_after_edit = 0.0_f64;
        for i in 0..ir.frames.len() * 2 {
            if i == ir.frames.len() / 2 {
                tail.update(TailControls {
                    t60: 2.0,
                    damping: 2000.0,
                    low: 1.0,
                    mid: 2.0,
                    ..model.fitted
                });
            }
            let frame = tail.process(if i == 0 { 1.0 } else { 0.0 });
            assert!(frame.iter().all(|v| v.is_finite() && v.abs() < 100.0));
            if i > ir.frames.len() {
                energy_after_edit += frame.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
            }
        }
        assert!(energy_after_edit > 1e-15);
        assert!(tail.tail_samples() >= ir.sample_rate * 6);
    }

    #[test]
    fn neutral_matches_baseline_and_edits_preserve_vn_onset() {
        for channels in [1, 2] {
            let ir = super::super::tests::reference();
            let ir = Impulse { channels, ..ir };
            let options = SynthesisOptions {
                t60: Some(0.5),
                ..Default::default()
            };
            let mut model = EditableHybrid::prepare(&ir, options).unwrap();
            let baseline = model.render(model.fitted).unwrap();
            let synthesis = synthesize(&ir, options).unwrap();
            assert_eq!(
                baseline,
                synthesis
                    .candidates
                    .iter()
                    .find(|c| c.name == "early_hybrid")
                    .unwrap()
                    .frames
            );
            // Force the general render path at the exact fitted settings.
            let fitted = model.fitted;
            model.fitted.t60 += 0.01;
            let neutral = model.render(fitted).unwrap();
            let error = baseline
                .iter()
                .zip(&neutral)
                .flat_map(|(a, b)| (0..2).map(move |ch| (a[ch] - b[ch]).abs()))
                .fold(0.0_f64, f64::max);
            assert!(error < 1e-10, "neutral reconstruction error {error}");
            let edited = model
                .render(TailControls {
                    t60: fitted.t60 * 0.5,
                    damping: fitted.damping * 0.5,
                    ..fitted
                })
                .unwrap();
            let keep = model.late + (model.rate as f64 * HYBRID_HOLD_MS / 1000.0) as usize;
            assert_eq!(baseline[..keep], edited[..keep]);
            assert_ne!(baseline[keep..], edited[keep..]);
            assert!(edited.iter().flatten().all(|v| v.is_finite()));
            assert!(
                model
                    .render(TailControls {
                        t60: f32::NAN,
                        ..fitted
                    })
                    .is_err()
            );
        }
    }
}
impl TailControls {
    pub fn apply(self, reverb: &mut Reverb) -> Result<(), String> {
        let apply = || -> Result<(), ConfigError> {
            reverb.set_parameters(self.t60, self.damping, self.matrix)?;
            reverb.set_diffusion(self.diffusion)?;
            reverb.set_loop_diffusion(self.loop_diffusion)?;
            reverb.set_band_damping(self.low, self.mid)?;
            reverb.set_damping_shape(self.low_hz, self.mid_hz, self.q)?;
            reverb.set_delay_modulation(self.delay, self.depth, self.rate)
        };
        let mut apply = apply;
        apply().map_err(|e| e.to_string())
    }
}

pub struct EditableHybrid {
    pub fitted: TailControls,
    rate: u32,
    channels: u16,
    late: usize,
    onset: usize,
    baseline: Vec<[f64; 2]>,
    vn: Vec<[f64; 2]>,
    eq: Vec<FdnEqBand>,
    fdn_gain: f64,
    hybrid_gain: f64,
}

/// Delayed, reference-equalized live tail. All buffers are allocated at preparation.
/// The correction FIR plus this branch reproduces the rendered baseline at the
/// prepared settings (within floating-point error, over the reference duration).
/// Edits use a delayed FDN delta, NOT the offline impulse-age crossfade.
pub struct StreamingTail {
    reverb: Reverb,
    eq: bank::StreamingBank,
    delay: Vec<f32>,
    cursor: usize,
    mono: bool,
    controls: TailControls,
    longest_t60: f32,
    rate: u32,
}

impl StreamingTail {
    /// Only continuous controls update live. Structural controls remain prepared.
    pub fn update(&mut self, controls: TailControls) {
        let next = TailControls {
            t60: controls.t60,
            damping: controls.damping.min(self.rate as f32 * 0.5),
            low: controls.low,
            mid: controls.mid,
            low_hz: controls.low_hz,
            mid_hz: controls.mid_hz,
            q: controls.q,
            ..self.controls
        };
        if next != self.controls && next.apply(&mut self.reverb).is_ok() {
            self.controls = next;
            self.longest_t60 = self.longest_t60.max(next.t60);
        }
    }

    pub fn tail_samples(&self) -> u32 {
        ((2.0 * self.longest_t60 + 2.0) * self.rate as f32) as u32 + self.delay.len() as u32
    }

    pub fn reset(&mut self) {
        self.reverb.reset();
        self.eq.reset();
        self.delay.fill(0.0);
        self.cursor = 0;
        self.longest_t60 = self.controls.t60;
    }

    pub fn process(&mut self, input: f32) -> [f32; 2] {
        let delayed = self.delay[self.cursor];
        self.delay[self.cursor] = input;
        self.cursor = (self.cursor + 1) % self.delay.len();
        let raw = self.reverb.process_sample(delayed);
        if raw.iter().any(|v| !v.is_finite()) {
            self.reset();
            return [0.0; 2];
        }
        let output = self.eq.process(raw.map(f64::from));
        if self.mono {
            [output[0] as f32; 2]
        } else {
            output.map(|v| v as f32)
        }
    }
}

impl EditableHybrid {
    /// Prepare a correction IR and live tail at this model's sample rate.
    /// Delaying excitation until after the VN hold preserves that entire prefix
    /// for every control setting. A fixed correction retains the old transition
    /// at neutral; edits and extension beyond the reference are explicitly new DSP.
    pub fn streaming(
        &self,
        controls: TailControls,
    ) -> Result<(Vec<[f64; 2]>, StreamingTail), String> {
        let mut reverb = Reverb::new(Config {
            sample_rate: self.rate,
            t60_seconds: controls.t60,
            damping_hz: controls.damping,
            matrix: controls.matrix,
        })
        .map_err(|e| e.to_string())?;
        controls.apply(&mut reverb)?;
        reverb.reset();
        let gains: Vec<_> = self
            .eq
            .iter()
            .map(|eq| eq.late_gain * self.fdn_gain * self.hybrid_gain)
            .collect();
        let delay = self
            .onset
            .max(self.late + (self.rate as f64 * HYBRID_HOLD_MS / 1000.0) as usize)
            .max(1);
        let mut tail = StreamingTail {
            reverb,
            eq: bank::StreamingBank::new(self.rate, &gains),
            delay: vec![0.0; delay],
            cursor: 0,
            mono: self.channels == 1,
            controls,
            longest_t60: controls.t60,
            rate: self.rate,
        };
        let mut correction = self.render(controls)?;
        for (i, frame) in correction.iter_mut().enumerate() {
            let live = tail.process(if i == 0 { 1.0 } else { 0.0 });
            for ch in 0..2 {
                frame[ch] -= f64::from(live[ch]);
            }
        }
        tail.reset();
        Ok((correction, tail))
    }

    pub fn prepare(ir: &Impulse, options: SynthesisOptions) -> Result<Self, String> {
        let mut synthesis = synthesize(ir, options)?;
        let peak = ir
            .frames
            .iter()
            .flatten()
            .fold(0.0_f64, |a, v| a.max(v.abs()));
        let onset = ir
            .frames
            .iter()
            .position(|f| f.iter().any(|v| v.abs() > peak * 0.001))
            .unwrap_or(0);
        let fit = &synthesis.fdn_fit;
        let fitted = TailControls {
            t60: fit.nominal_t60 as f32,
            damping: fit.damping_hz as f32,
            matrix: Matrix::Hybrid,
            diffusion: 1.0,
            loop_diffusion: 1.0,
            low: fit.attenuation_db[0],
            mid: fit.attenuation_db[1],
            low_hz: 500.0,
            mid_hz: 2000.0,
            q: 0.7,
            delay: crate::DelayMode::Static,
            depth: 0.0,
            rate: 0.3,
        };
        let fdn_candidate = synthesis
            .candidates
            .iter()
            .find(|c| c.name == "early_fdn")
            .unwrap();
        let late = synthesis.early_samples + synthesis.fade_samples;
        let power = |frames: &[[f64; 2]]| {
            frames[late..]
                .iter()
                .map(|f| {
                    if ir.channels == 1 {
                        f[0] * f[0]
                    } else {
                        energy(f)
                    }
                })
                .sum::<f64>()
        };
        let wanted = power(&ir.frames);
        let fdn_gain = suffix_energy_gain(
            wanted,
            power(&fdn_candidate.frames) / fdn_candidate.late_gain.powi(2),
            ir.frames.len() - late,
        )
        .0;
        let hybrid = synthesis.candidates.swap_remove(
            synthesis
                .candidates
                .iter()
                .position(|c| c.name == "early_hybrid")
                .unwrap(),
        );
        let vn = synthesis.candidates.swap_remove(
            synthesis
                .candidates
                .iter()
                .position(|c| c.name == "early_multiband_vn")
                .unwrap(),
        );
        let vn_gain = suffix_energy_gain(
            wanted,
            power(&vn.frames) / vn.late_gain.powi(2),
            ir.frames.len() - late,
        )
        .0 / vn.late_gain;
        let mut vn_frames = vn.frames;
        for frame in &mut vn_frames[late..] {
            for sample in frame {
                *sample *= vn_gain;
            }
        }
        Ok(Self {
            fitted,
            rate: ir.sample_rate,
            channels: ir.channels,
            onset,
            late: synthesis.early_samples + synthesis.fade_samples,
            baseline: hybrid.frames,
            vn: vn_frames,
            eq: synthesis.fdn_eq_bands,
            fdn_gain,
            hybrid_gain: hybrid.late_gain,
        })
    }

    pub fn render(&self, controls: TailControls) -> Result<Vec<[f64; 2]>, String> {
        if controls == self.fitted {
            return Ok(self.baseline.clone());
        }
        let mut reverb = Reverb::new(Config {
            sample_rate: self.rate,
            t60_seconds: controls.t60,
            damping_hz: controls.damping,
            matrix: controls.matrix,
        })
        .map_err(|e| e.to_string())?;
        controls.apply(&mut reverb)?;
        reverb.reset();
        let raw: Vec<_> = (0..self.baseline.len())
            .map(|i| {
                if i < self.onset {
                    [0.0; 2]
                } else {
                    reverb
                        .process_sample(if i == self.onset { 1.0 } else { 0.0 })
                        .map(f64::from)
                }
            })
            .collect();
        let mut fdn = vec![[0.0; 2]; raw.len()];
        for (band, eq) in self.eq.iter().enumerate() {
            for (sum, frame) in fdn.iter_mut().zip(bank::filter(&raw, self.rate, band)) {
                for ch in 0..2 {
                    sum[ch] += frame[ch] * eq.late_gain * self.fdn_gain;
                }
            }
        }
        let mut output = self.baseline.clone();
        for i in self.late..output.len() {
            let ms = (i - self.late) as f64 * 1000.0 / self.rate as f64;
            let x = ((ms - HYBRID_HOLD_MS) / HYBRID_TRANSITION_MS).clamp(0.0, 1.0);
            if x == 0.0 {
                continue;
            }
            let phase = x * x * (3.0 - 2.0 * x) * std::f64::consts::FRAC_PI_2;
            let (a, b) = if x == 1.0 {
                (0.0, 1.0)
            } else {
                (phase.cos(), phase.sin())
            };
            for ch in 0..2 {
                let f = fdn[i][if self.channels == 1 { 0 } else { ch }];
                output[i][ch] = (a * self.vn[i][ch] + b * f) * self.hybrid_gain;
            }
        }
        if output
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > f32::MAX as f64)
        {
            return Err("Tail render exceeded finite audio range; previous IR retained".into());
        }
        Ok(output)
    }
}
