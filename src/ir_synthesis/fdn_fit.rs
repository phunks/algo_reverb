//! Bounded deterministic offline coordinate search; no changes to the live API.
use super::*;

#[derive(Clone, Debug)]
pub struct FdnFit {
    pub nominal_t60: f64,
    pub damping_hz: f64,
    pub attenuation_db: [f32; 2],
    pub targets: Vec<(f64, f64)>,
    /// Mean band score in dB²: equal EDC / 20 ms power weights after energy EQ.
    pub initial_error: Option<f64>,
    pub fitted_error: Option<f64>,
    /// Logical search evaluations, including scores reused from identical coordinates.
    pub evaluations: usize,
}

pub(super) fn render(
    rate: u32,
    len: usize,
    onset: usize,
    fit: &FdnFit,
) -> Result<Vec<[f64; 2]>, String> {
    let mut fdn = Reverb::new(Config {
        sample_rate: rate,
        t60_seconds: fit.nominal_t60 as f32,
        damping_hz: fit.damping_hz as f32,
        matrix: Matrix::Hybrid,
    })
    .map_err(|e| e.to_string())?;
    fdn.set_diffusion(1.0).map_err(|e| e.to_string())?;
    fdn.set_loop_diffusion(1.0).map_err(|e| e.to_string())?;
    fdn.set_band_damping(fit.attenuation_db[0], fit.attenuation_db[1])
        .map_err(|e| e.to_string())?;
    fdn.set_damping_shape(500.0, 2000.0, 0.7)
        .map_err(|e| e.to_string())?;
    fdn.reset();
    Ok((0..len)
        .map(|i| {
            if i < onset {
                [0.0; 2]
            } else {
                fdn.process_sample(if i == onset { 1.0 } else { 0.0 })
                    .map(f64::from)
            }
        })
        .collect())
}

/// Reference-only masks: never let a candidate select its own scoring interval.
struct ShapeBand {
    hz: Option<f64>,
    points: Vec<(usize, usize, f64, f64)>,
}

impl ShapeBand {
    fn new(hz: Option<f64>, frames: &[[f64; 2]], width: usize) -> Self {
        let curve = edc(frames);
        let powers: Vec<_> = frames
            .chunks_exact(width)
            .map(|w| w.iter().map(energy).sum::<f64>() / width as f64)
            .collect();
        let mut terminal: Vec<_> = powers.iter().rev().take(10).copied().collect();
        terminal.sort_by(f64::total_cmp);
        let floor = terminal.get(terminal.len() / 2).copied().unwrap_or(0.0);
        let peak = powers.iter().copied().fold(0.0_f64, f64::max);
        let threshold = (floor * 100.0).max(peak * 1e-5).max(1e-24);
        let points = powers
            .iter()
            .enumerate()
            .filter_map(|(i, &power)| {
                let start = i * width;
                let center = start + width / 2;
                (power > threshold && curve[center] >= -35.0).then_some((
                    start,
                    width,
                    curve[center],
                    10.0 * power.log10(),
                ))
            })
            .collect();
        Self { hz, points }
    }

    fn error(&self, frames: &[[f64; 2]]) -> f64 {
        let curve = edc(frames);
        self.points
            .iter()
            .map(|&(start, width, target_edc, target_power)| {
                let power =
                    frames[start..start + width].iter().map(energy).sum::<f64>() / width as f64;
                let edc_error = (curve[start + width / 2] - target_edc).clamp(-40.0, 40.0);
                let power_error =
                    (10.0 * power.max(1e-30).log10() - target_power).clamp(-40.0, 40.0);
                0.5 * (edc_error.powi(2) + power_error.powi(2))
            })
            .sum::<f64>()
            / self.points.len() as f64
    }
}

struct ShapeTarget {
    ir: Impulse,
    late: usize,
    onset: usize,
    bands: Vec<ShapeBand>,
    eq_energy: Vec<f64>,
}

impl ShapeTarget {
    fn new(ir: Impulse, late: usize, onset: usize, bands: Vec<ShapeBand>) -> Self {
        let eq_energy = (0..=bank::edges(ir.sample_rate).len())
            .map(|band| {
                bank::filter(&ir.frames, ir.sample_rate, band)[late..]
                    .iter()
                    .map(energy)
                    .sum()
            })
            .collect();
        Self {
            ir,
            late,
            onset,
            bands,
            eq_energy,
        }
    }

    fn error(&self, fit: &FdnFit) -> Result<f64, String> {
        let raw = render(self.ir.sample_rate, self.ir.frames.len(), self.onset, fit)?;
        let (mut frames, _) =
            fdn_output_eq_cached(&self.ir, &raw, self.late, false, Some(&self.eq_energy));
        if self.ir.channels == 1 {
            for f in &mut frames {
                f[1] = f[0];
            }
        }
        let wanted = self.ir.frames[self.late..].iter().map(energy).sum::<f64>();
        let actual = frames[self.late..].iter().map(energy).sum::<f64>();
        if actual <= 0.0 || !actual.is_finite() {
            return Ok(1600.0);
        }
        let gain = (wanted / actual).sqrt();
        for frame in &mut frames {
            for v in frame {
                *v *= gain;
            }
        }
        let mut error = 0.0;
        for band in &self.bands {
            let filtered = band.hz.map(|hz| octave(&frames, self.ir.sample_rate, hz));
            let samples = filtered.as_deref().unwrap_or(&frames);
            error += band.error(&samples[self.late..]);
        }
        Ok(error / self.bands.len() as f64)
    }
}

// Cache exact search coordinates, not rounded DSP parameters, so the optimizer's
// comparisons and tie-breaking remain identical to the serial search.
fn fit_key(fit: &FdnFit) -> (u64, u64, [u32; 2]) {
    (
        fit.nominal_t60.to_bits(),
        fit.damping_hz.to_bits(),
        fit.attenuation_db.map(f32::to_bits),
    )
}

struct Scores<'a> {
    target: &'a ShapeTarget,
    cache: Vec<((u64, u64, [u32; 2]), f64)>,
    parallel: bool,
}

impl Scores<'_> {
    fn error(&mut self, fit: &FdnFit) -> Result<f64, String> {
        let key = fit_key(fit);
        if let Some((_, score)) = self.cache.iter().find(|(k, _)| *k == key) {
            return Ok(*score);
        }
        let score = self.target.error(fit)?;
        self.cache.push((key, score));
        Ok(score)
    }

    fn pair(&mut self, trials: &[FdnFit; 2]) -> Result<[f64; 2], String> {
        if !self.parallel
            || trials
                .iter()
                .any(|trial| self.cache.iter().any(|(key, _)| *key == fit_key(trial)))
            || fit_key(&trials[0]) == fit_key(&trials[1])
        {
            return Ok([self.error(&trials[0])?, self.error(&trials[1])?]);
        }
        let target = self.target;
        let scores = std::thread::scope(|scope| -> Result<[f64; 2], String> {
            // At most one extra worker per fit. Fall back to serial execution
            // if the OS cannot create a thread; never involve the audio thread.
            match std::thread::Builder::new()
                .name("fdn-fit".into())
                .spawn_scoped(scope, || target.error(&trials[0]))
            {
                Ok(worker) => {
                    let second = target.error(&trials[1]);
                    let first = worker
                        .join()
                        .map_err(|_| "FDN fit worker panicked".to_string())?;
                    Ok([first?, second?])
                }
                Err(_) => Ok([target.error(&trials[0])?, target.error(&trials[1])?]),
            }
        })?;
        for (trial, score) in trials.iter().zip(scores) {
            self.cache.push((fit_key(trial), score));
        }
        Ok(scores)
    }
}

pub(super) fn fit(
    ir: &Impulse,
    t60: f64,
    late: usize,
    onset: usize,
    fixed: bool,
) -> Result<FdnFit, String> {
    fit_with_parallel(
        ir,
        t60,
        late,
        onset,
        fixed,
        std::thread::available_parallelism().is_ok_and(|n| n.get() > 1),
    )
}

fn fit_with_parallel(
    ir: &Impulse,
    t60: f64,
    late: usize,
    onset: usize,
    fixed: bool,
    parallel: bool,
) -> Result<FdnFit, String> {
    let bands = band_damping(ir, t60);
    let total = ir.frames[late..].iter().map(energy).sum::<f64>();
    let mut targets = Vec::new();
    for hz in [250.0, 2000.0, 4000.0] {
        if hz * 2.0_f64.sqrt() >= f64::from(ir.sample_rate) * 0.5 {
            continue;
        }
        let filtered = octave(&ir.frames, ir.sample_rate, hz);
        if filtered[late..].iter().map(energy).sum::<f64>() > total * 1e-8
            && let Some(t) = screened_t60(&filtered[late..], ir.sample_rate)
        {
            targets.push((hz, t));
        }
    }
    let mut best = FdnFit {
        nominal_t60: t60,
        damping_hz: (f64::from(ir.sample_rate) * 0.45).min(6000.0),
        attenuation_db: bands.map(|b| b.attenuation_db),
        targets,
        initial_error: None,
        fitted_error: None,
        evaluations: 0,
    };
    // Fit a bounded native-rate render; score only the synthetic region. Long
    // decays may remain unidentifiable inside this six-second search horizon.
    let len = ir.frames.len().min(onset + ir.sample_rate as usize * 6);
    if late + ir.sample_rate as usize / 10 >= len {
        return Ok(best);
    }
    let reference = Impulse {
        frames: ir.frames[..len].to_vec(),
        sample_rate: ir.sample_rate,
        channels: ir.channels,
        bits: ir.bits,
    };
    let mut shape_bands = Vec::new();
    for hz in std::iter::once(None).chain(band_centers(ir.sample_rate).into_iter().map(Some)) {
        let filtered = hz.map(|hz| octave(&reference.frames, ir.sample_rate, hz));
        let frames = filtered.as_deref().unwrap_or(&reference.frames);
        if frames[late..].iter().map(energy).sum::<f64>() < total * 1e-8 {
            continue;
        }
        let band = ShapeBand::new(hz, &frames[late..], ir.sample_rate as usize / 50);
        if band.points.len() >= 3 {
            shape_bands.push(band);
        }
    }
    if shape_bands.is_empty() {
        return Ok(best);
    }
    let target = ShapeTarget::new(reference, late, onset, shape_bands);
    let mut scores = Scores {
        target: &target,
        cache: Vec::new(),
        parallel,
    };
    let mut score = scores.error(&best)?;
    best.initial_error = Some(score);
    best.evaluations = 1;
    if !fixed {
        let mut trial = best.clone();
        trial.nominal_t60 = best
            .targets
            .iter()
            .map(|(_, t)| *t)
            .fold(t60, f64::max)
            .clamp(0.01, 60.0);
        let trial_score = scores.error(&trial)?;
        if trial_score < score {
            score = trial_score;
            best = trial;
        }
        best.evaluations = 2;
    }
    for step in [1.0, 0.5, 0.25] {
        for parameter in 0..4 {
            if fixed && parameter == 0 {
                continue;
            }
            let base = best.clone();
            let trials = [-1.0, 1.0].map(|sign| {
                let mut trial = base.clone();
                match parameter {
                    0 => {
                        trial.nominal_t60 =
                            (base.nominal_t60 * 2.0_f64.powf(sign * step)).clamp(0.01, 60.0)
                    }
                    1 => {
                        trial.damping_hz = (base.damping_hz * 2.0_f64.powf(sign * step))
                            .clamp(20.0, f64::from(ir.sample_rate) * 0.45)
                    }
                    p => {
                        trial.attenuation_db[p - 2] =
                            (base.attenuation_db[p - 2] + (sign * step) as f32).clamp(0.0, 24.0)
                    }
                }
                trial
            });
            let trial_scores = scores.pair(&trials)?;
            // Preserve the serial search's minus-before-plus tie-breaking.
            for (trial, trial_score) in trials.into_iter().zip(trial_scores) {
                let evaluations = best.evaluations + 1;
                if trial_score < score {
                    score = trial_score;
                    best = trial;
                }
                best.evaluations = evaluations;
            }
        }
    }
    best.fitted_error = Some(score);
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ir(rate: u32, channels: u16) -> Impulse {
        let frames = (0..rate as usize / 2)
            .map(|i| {
                let amplitude = (-10.0 * i as f64 / rate as f64).exp();
                let left = ((i * 7919 % 1009) as f64 / 504.5 - 1.0) * amplitude;
                let right = if channels == 1 {
                    left
                } else {
                    ((i * 3571 % 1013) as f64 / 506.5 - 1.0) * amplitude
                };
                [left, right]
            })
            .collect();
        Impulse {
            frames,
            sample_rate: rate,
            channels,
            bits: 32,
        }
    }

    #[test]
    fn cached_eq_is_bit_identical_to_reference_filtering() {
        for rate in [8000, 44100] {
            for channels in [1, 2] {
                let ir = test_ir(rate, channels);
                let late = rate as usize / 10;
                let target = ShapeTarget::new(ir, late, 0, Vec::new());
                for raw in [
                    target.ir.frames.clone(),
                    vec![[0.0; 2]; target.ir.frames.len()],
                ] {
                    let (expected, eq) = fdn_energy_eq(&target.ir, &raw, late);
                    let (actual, cached_eq) = fdn_output_eq_cached(
                        &target.ir,
                        &raw,
                        late,
                        false,
                        Some(&target.eq_energy),
                    );
                    assert_eq!(expected, actual);
                    for (a, b) in eq.iter().zip(cached_eq) {
                        assert_eq!(a.late_gain.to_bits(), b.late_gain.to_bits());
                        assert_eq!(a.gain_capped, b.gain_capped);
                    }
                }
            }
        }
    }

    #[test]
    fn parallel_and_serial_fits_preserve_coordinates_scores_and_counts() {
        for channels in [1, 2] {
            let ir = test_ir(8000, channels);
            for fixed in [false, true] {
                let serial = fit_with_parallel(&ir, 0.6, 800, 0, fixed, false).unwrap();
                let parallel = fit_with_parallel(&ir, 0.6, 800, 0, fixed, true).unwrap();
                assert_eq!(fit_key(&serial), fit_key(&parallel));
                assert_eq!(serial.initial_error, parallel.initial_error);
                assert_eq!(serial.fitted_error, parallel.fitted_error);
                assert_eq!(serial.evaluations, if fixed { 19 } else { 26 });
                assert_eq!(serial.evaluations, parallel.evaluations);
            }
        }
    }

    #[test]
    fn repeated_coordinates_reuse_score_without_rendering() {
        let ir = test_ir(8000, 2);
        let fit = fit_with_parallel(&ir, 0.6, 800, 0, false, false).unwrap();
        let band = ShapeBand::new(None, &ir.frames[800..], 160);
        assert!(!band.points.is_empty());
        let target = ShapeTarget::new(ir, 800, 0, vec![band]);
        let mut scores = Scores {
            target: &target,
            cache: Vec::new(),
            parallel: true,
        };
        let expected = scores.error(&fit).unwrap();
        assert!(expected.is_finite());
        assert_eq!(
            scores.pair(&[fit.clone(), fit.clone()]).unwrap(),
            [expected; 2]
        );
        assert_eq!(scores.cache.len(), 1);
    }

    fn decay() -> Vec<[f64; 2]> {
        (0..8000)
            .map(|i| [10.0_f64.powf(-3.0 * i as f64 / 8000.0); 2])
            .collect()
    }

    #[test]
    fn shape_score_detects_gain_and_temporal_errors() {
        let reference = decay();
        let band = ShapeBand::new(None, &reference, 160);
        assert!(band.points.len() >= 3);
        assert!(band.error(&reference) < 1e-20);
        let louder: Vec<_> = reference.iter().map(|f| f.map(|v| v * 2.0)).collect();
        // EDC is gain-invariant, absolute power is deliberately not.
        assert!((band.error(&louder) - 0.5 * (20.0 * 2.0_f64.log10()).powi(2)).abs() < 1e-9);
        let altered: Vec<_> = reference
            .iter()
            .enumerate()
            .map(|(i, f)| f.map(|v| v * if (1600..3200).contains(&i) { 2.0 } else { 1.0 }))
            .collect();
        assert!(band.error(&altered) > 0.5);
        assert!(band.error(&vec![[0.0; 2]; reference.len()]).is_finite());
    }

    #[test]
    fn shape_mask_omits_silence_and_terminal_floor() {
        assert!(
            ShapeBand::new(None, &vec![[0.0; 2]; 8000], 160)
                .points
                .is_empty()
        );
        assert!(
            ShapeBand::new(None, &vec![[0.001; 2]; 8000], 160)
                .points
                .is_empty()
        );
        let mut reference = decay();
        for f in &mut reference[4000..] {
            *f = [0.01; 2];
        }
        let band = ShapeBand::new(None, &reference, 160);
        assert!(!band.points.is_empty());
        assert!(band.points.iter().all(|p| p.0 < 4000));
        // Reference masks do not change when the candidate is silent or slower.
        assert!(band.error(&vec![[0.0; 2]; 8000]) > band.error(&reference));
    }

    #[test]
    fn post_eq_score_is_zero_for_identical_onset_aligned_fdn() {
        let fit = FdnFit {
            nominal_t60: 1.0,
            damping_hz: 3000.0,
            attenuation_db: [0.25, 0.5],
            targets: Vec::new(),
            initial_error: None,
            fitted_error: None,
            evaluations: 0,
        };
        for channels in [1, 2] {
            let mut frames = render(8000, 16000, 137, &fit).unwrap();
            if channels == 1 {
                for f in &mut frames {
                    f[1] = f[0];
                }
            }
            let ir = Impulse {
                sample_rate: 8000,
                channels,
                bits: 32,
                frames,
            };
            // EQ is not a bypass, so compare against the actual EQ/normalize path.
            let (mut eq, _) = fdn_energy_eq(&ir, &ir.frames, 800);
            if channels == 1 {
                for f in &mut eq {
                    f[1] = f[0];
                }
            }
            let gain = (ir.frames[800..].iter().map(energy).sum::<f64>()
                / eq[800..].iter().map(energy).sum::<f64>())
            .sqrt();
            for f in &mut eq {
                for v in f {
                    *v *= gain;
                }
            }
            let band = ShapeBand::new(None, &eq[800..], 160);
            let target = ShapeTarget::new(ir, 800, 137, vec![band]);
            assert!(target.error(&fit).unwrap() < 1e-20);
        }
    }
}
