//! Rust-only, bounded IR feature fitting. This is not a DecayFitNet reproduction.
//! No source samples are used during playback. Early reflections are not copied.
use super::{DiffFdn, Sos};
use crate::ir_analysis::{Impulse, correlation, edc, energy, fit_decay, octave};
use std::{fmt::Write, time::Instant};
mod structure;
use structure::Structure;

const CENTERS: [f64; 7] = [125., 250., 500., 1000., 2000., 4000., 8000.];

/// Versioned, compact coefficients; persisted instead of paths or source WAVs.
pub struct Seed {
    structure: Option<Structure>,
    decay: [f64; 7],
    tone: [f64; 7],
    left: [f64; 8],
    right: [f64; 8],
}

pub struct Report {
    pub seed: Seed,
    pub seconds: f64,
    pub reliable_bands: usize,
    pub target_correlation: f64,
}

fn peak(hz: f64, db: f64, rate: u32) -> Sos {
    let a = 10f64.powf(db / 40.);
    let w = std::f64::consts::TAU * hz / rate as f64;
    let alpha = w.sin() / (2. * std::f64::consts::FRAC_1_SQRT_2);
    let d = 1. + alpha / a;
    Sos {
        b: [
            (1. + alpha * a) / d,
            -2. * w.cos() / d,
            (1. - alpha * a) / d,
        ],
        a: [-2. * w.cos() / d, (1. - alpha / a) / d],
        state: [0.; 2],
    }
}

fn response(s: &Sos, hz: f64, rate: u32) -> f64 {
    let w = std::f64::consts::TAU * hz / rate as f64;
    let power = |c: [f64; 3]| {
        (c[0] + c[1] * w.cos() + c[2] * (2. * w).cos()).powi(2)
            + (c[1] * w.sin() + c[2] * (2. * w).sin()).powi(2)
    };
    10. * (power(s.b) / power([1., s.a[0], s.a[1]])).log10()
}

fn decay(frames: &[[f64; 2]], rate: u32) -> Option<f64> {
    let curve = edc(frames);
    [(-5., -35.), (-5., -25.), (-5., -15.)]
        .into_iter()
        .find_map(|(a, b)| {
            let f = fit_decay(frames, &curve, rate, a, b);
            (f.status == "reference_only")
                .then_some(f.t60)
                .flatten()
                .filter(|t| (0.1..=20.).contains(t))
        })
}

impl Seed {
    pub fn encode(&self) -> String {
        let mut text = String::from(if self.structure.is_some() {
            "rust-stereo-seed-v2"
        } else {
            "rust-stereo-seed-v1"
        });
        for x in self
            .decay
            .iter()
            .chain(&self.tone)
            .chain(&self.left)
            .chain(&self.right)
        {
            write!(text, " {x:.17e}").unwrap();
        }
        if let Some(s) = &self.structure {
            for x in s.values() {
                write!(text, " {x:.17e}").unwrap();
            }
        }
        text
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        if text.len() > 4096 {
            return Err("Seed state too large".into());
        }
        let mut words = text.split_whitespace();
        let version = match words.next() {
            Some("rust-stereo-seed-v1") => 1,
            Some("rust-stereo-seed-v2") => 2,
            _ => return Err("Unsupported seed version".into()),
        };
        let values = words
            .map(|s| {
                s.parse::<f64>()
                    .map_err(|_| "Invalid seed number".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if values.len() != if version == 1 { 30 } else { 40 }
            || values.iter().any(|x| !x.is_finite())
            || values[..7].iter().any(|x| !(0.1..=20.).contains(x))
            || values[7..14].iter().any(|x| x.abs() > 12.)
            || values[14..30].iter().any(|x| x.abs() > 16.)
        {
            return Err("Invalid seed coefficients".into());
        }
        Ok(Self {
            structure: if version == 2 {
                Some(Structure::decode(&values[30..])?)
            } else {
                None
            },
            decay: values[..7].try_into().unwrap(),
            tone: values[7..14].try_into().unwrap(),
            left: values[14..22].try_into().unwrap(),
            right: values[22..30].try_into().unwrap(),
        })
    }

    /// Bounded coefficient fitting and network construction, never on the callback.
    pub fn build(&self, rate: u32) -> Result<DiffFdn, String> {
        let mut f = if let Some(s) = &self.structure {
            s.build(rate)?
        } else {
            // Keep old session sound unchanged; new imports never enter this branch.
            DiffFdn::parse(include_str!("../../resources/difffdn/salon.fdn"))?
                .at_sample_rate(rate)?
        };
        let centers: Vec<_> = CENTERS
            .into_iter()
            .filter(|hz| *hz < rate as f64 * 0.45)
            .collect();
        let longest = self.decay.iter().copied().fold(0.1, f64::max);
        for line in 0..8 {
            let samples =
                f.main[line].data.len() + f.left[line].data.len() + f.right[line].data.len();
            let scattering: f64 = f
                .stages
                .iter()
                .map(|(_, d)| d.iter().map(|d| d.data.len() as f64).sum::<f64>() / 8.)
                .sum();
            let seconds = (samples as f64 + scattering) / rate as f64;
            let base = -60. * seconds / longest;
            let targets: Vec<_> = centers
                .iter()
                .enumerate()
                .map(|(b, _)| -60. * seconds / self.decay[b] - base)
                .collect();
            let mut gains = vec![0.; centers.len()];
            // Coordinate descent only changes passive peaking cuts. Each pass is
            // finite and every feedback filter has magnitude <= 1 at all frequencies.
            for step in [4., 2., 1., 0.5, 0.25] {
                for _ in 0..3 {
                    for b in 0..centers.len() {
                        let score = |g: &[f64]| -> f64 {
                            centers
                                .iter()
                                .zip(&targets)
                                .map(|(&hz, &target)| {
                                    let actual: f64 = centers
                                        .iter()
                                        .zip(g)
                                        .map(|(&fc, &db)| response(&peak(fc, db, rate), hz, rate))
                                        .sum();
                                    (actual - target).powi(2)
                                })
                                .sum()
                        };
                        let old = gains[b];
                        let mut best = score(&gains);
                        let mut choice = old;
                        for candidate in [(old - step).max(-24.), (old + step).min(0.)] {
                            gains[b] = candidate;
                            let error = score(&gains);
                            if error < best {
                                best = error;
                                choice = candidate;
                            }
                        }
                        gains[b] = choice;
                    }
                }
            }
            let mut filters = vec![Sos {
                b: [10f64.powf(base / 20.), 0., 0.],
                a: [0.; 2],
                state: [0.; 2],
            }];
            filters.extend(
                centers
                    .iter()
                    .zip(gains)
                    .map(|(&hz, db)| peak(hz, db, rate)),
            );
            f.absorption[line] = filters;
        }
        f.output = self.left.to_vec();
        f.tone = centers
            .iter()
            .zip(self.tone)
            .map(|(&hz, db)| peak(hz, db, rate))
            .collect();
        f.stereo = Some((
            self.right.to_vec(),
            centers
                .iter()
                .zip(self.tone)
                .map(|(&hz, db)| peak(hz, db, rate))
                .collect(),
        ));
        Ok(f)
    }
}

/// Full source-rate analysis; calibration renders are capped at 6 seconds/24 kHz.
pub fn analyze(ir: &Impulse) -> Result<Report, String> {
    let start = Instant::now();
    if !(8000..=384000).contains(&ir.sample_rate)
        || !(1..=2).contains(&ir.channels)
        || ir.frames.len() < ir.sample_rate as usize / 10
        || ir.frames.len() > ir.sample_rate as usize * 60
        || ir.frames.iter().flatten().any(|x| !x.is_finite())
    {
        return Err("Expected finite mono/stereo IR, 0.1–60 seconds, 8–384 kHz".into());
    }
    // Normalize for numerical robustness; absolute IR gain is not a room feature.
    let peak = ir.frames.iter().flatten().fold(0f64, |p, x| p.max(x.abs()));
    if peak <= 1e-15 {
        return Err("Silent IR".into());
    }
    let frames: Vec<_> = ir.frames.iter().map(|f| f.map(|x| x / peak)).collect();
    let nominal = decay(&frames, ir.sample_rate)
        .ok_or("No reliable decay found (noise, short or non-decaying IR)")?;
    let mut seed = Seed {
        structure: Some(Structure::default()),
        decay: [nominal; 7],
        tone: [0.; 7],
        left: [0.3535533905932738; 8],
        right: std::array::from_fn(|i| {
            if i % 2 == 0 {
                0.3535533905932738
            } else {
                -0.3535533905932738
            }
        }),
    };
    let mut reliable_bands = 0;
    let mut target_energy = [0.; 7];
    for (b, hz) in CENTERS.into_iter().enumerate() {
        if hz >= ir.sample_rate as f64 * 0.45 {
            continue;
        }
        let filtered = octave(&frames, ir.sample_rate, hz);
        target_energy[b] = filtered.iter().map(energy).sum();
        if let Some(t) = decay(&filtered, ir.sample_rate) {
            seed.decay[b] = t;
            reliable_bands += 1;
        }
    }
    if reliable_bands < 2 {
        return Err("Fewer than two reliable decay bands".into());
    }
    structure::fit(&mut seed, &frames, ir.sample_rate)?;
    let late = &frames[(ir.sample_rate as usize * 80 / 1000).min(frames.len() - 1)..];
    let target_correlation = if ir.channels == 1 {
        1.
    } else {
        correlation(late).unwrap_or(0.)
    };
    let lr: [f64; 2] = std::array::from_fn(|c| late.iter().map(|f| f[c] * f[c]).sum());
    let rate = 24000;
    let length = ((nominal * 1.2).clamp(1., 6.) * rate as f64) as usize;
    let render = |seed: &Seed| -> Result<Vec<[f64; 2]>, String> {
        let mut f = seed.build(rate)?;
        Ok((0..length)
            .map(|i| f.process_stereo(if i == 0 { 1. } else { 0. }))
            .collect())
    };
    let trial = render(&seed)?;
    let source_total: f64 = target_energy.iter().sum();
    let rendered: Vec<_> = CENTERS
        .into_iter()
        .map(|hz| octave(&trial, rate, hz).iter().map(energy).sum::<f64>())
        .collect();
    let rendered_total: f64 = rendered.iter().sum();
    for b in 0..7 {
        if target_energy[b] > 0. && rendered[b] > 0. {
            seed.tone[b] = (10.
                * ((target_energy[b] / source_total) / (rendered[b] / rendered_total)).log10())
            .clamp(-12., 12.);
        }
    }
    let trial = render(&seed)?;
    let late_trial = &trial[(rate as usize * 80 / 1000)..];
    let a: f64 = late_trial.iter().map(|f| f[0] * f[0]).sum();
    let b: f64 = late_trial.iter().map(|f| f[1] * f[1]).sum();
    let cross: f64 = late_trial.iter().map(|f| f[0] * f[1]).sum();
    let orthogonal = (b - cross * cross / a).max(1e-20).sqrt();
    let original = seed.right;
    let balance = (lr[1] / lr[0].max(1e-20)).sqrt().clamp(0., 16.);
    for (i, &r) in original.iter().enumerate() {
        seed.right[i] = balance
            * (target_correlation * seed.left[i]
                + (1. - target_correlation.powi(2)).max(0.).sqrt() * a.sqrt() / orthogonal
                    * (r - cross / a * seed.left[i]));
    }
    if lr[0] < lr[1] * 1e-12 {
        seed.left.fill(0.);
        seed.right = original;
    }
    let trial = render(&seed)?;
    let e: f64 = trial.iter().map(energy).sum();
    if !e.is_finite() || e < 1e-20 {
        return Err("Invalid calibration render".into());
    }
    let gain = 1. / e.sqrt();
    for x in seed.left.iter_mut().chain(&mut seed.right) {
        *x *= gain;
    }
    let seed = Seed::decode(&seed.encode())?;
    Ok(Report {
        seed,
        seconds: start.elapsed().as_secs_f64(),
        reliable_bands,
        target_correlation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(seconds: u32, rate: u32, mono: bool) -> Impulse {
        let mut random = 31u64;
        let frames = (0..rate * seconds)
            .map(|i| {
                let envelope = 10f64.powf(-3. * i as f64 / rate as f64);
                let mut noise = || {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    random as i64 as f64 / i64::MAX as f64
                };
                let l = noise() * envelope;
                [l, if mono { l } else { noise() * envelope * 0.6 }]
            })
            .collect();
        Impulse {
            sample_rate: rate,
            channels: if mono { 1 } else { 2 },
            bits: 32,
            frames,
        }
    }
    #[test]
    fn stereo_seed_roundtrip_decay_correlation_and_safety() {
        for mono in [false, true] {
            let report = analyze(&fixture(3, 24000, mono)).unwrap();
            assert!(report.reliable_bands >= 5);
            assert!(report.seed.decay.iter().all(|t| (t - 1.).abs() < 0.2));
            let text = report.seed.encode();
            let seed = Seed::decode(&text).unwrap();
            assert_eq!(text, seed.encode());
            for rate in [8000, 48000, 192000, 384000] {
                let mut f = seed.build(rate).unwrap();
                let out: Vec<_> = (0..rate * 3)
                    .map(|i| f.process_stereo(if i == 0 { 1. } else { 0. }))
                    .collect();
                assert!(out.iter().flatten().all(|x| x.is_finite() && x.abs() < 4.));
                let corr = correlation(&out[(rate as usize * 80 / 1000)..]).unwrap();
                assert!((corr - report.target_correlation).abs() < 0.18, "{corr}");
                assert!(out[(rate as usize * 2)..].iter().map(energy).sum::<f64>() < 0.001);
                f.reset();
                assert!((0..100).all(|_| f.process_stereo(0.) == [0.; 2]));
            }
        }
    }
    #[test]
    fn invalid_sources_and_state_are_rejected() {
        let mut ir = fixture(1, 8000, false);
        ir.frames.fill([0.; 2]);
        assert!(analyze(&ir).is_err());
        ir.frames.fill([0.5; 2]);
        assert!(analyze(&ir).is_err());
        ir.frames[0][0] = f64::NAN;
        assert!(analyze(&ir).is_err());
        assert!(Seed::decode("rust-stereo-seed-v1 NaN").is_err());
    }
    #[test]
    #[ignore = "manual release-mode import timing"]
    fn seed_import_timing() {
        for seconds in [3, 10, 30] {
            let report = analyze(&fixture(seconds, 48000, false)).unwrap();
            eprintln!(
                "{seconds}s/48k stereo IR: {:.3}s, {} reliable bands",
                report.seconds, report.reliable_bands
            );
        }
    }

    #[test]
    #[ignore = "manual timing against separately supplied local IRs"]
    fn local_seed_import_timing() {
        for name in [
            "French 18th Century Salon",
            "Highly Damped Large Room",
            "Parking Garage",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/IMreverbs")
                .join(format!("{name}.wav"));
            let started = Instant::now();
            let ir = Impulse::load(&path).unwrap();
            match analyze(&ir) {
                Ok(report) => {
                    let mut f = report.seed.build(48000).unwrap();
                    f.enable_live_controls();
                    eprintln!(
                        "{name}: {:.2}s IR/{}Hz, {:.3}s load+fit+build, bands={}, corr={:.3}",
                        ir.frames.len() as f64 / ir.sample_rate as f64,
                        ir.sample_rate,
                        started.elapsed().as_secs_f64(),
                        report.reliable_bands,
                        report.target_correlation
                    );
                }
                Err(e) => panic!("{name}: {e}"),
            }
        }
    }
}
