//! Bounded analysis-by-synthesis of early energy, density and echo periodicity.
//! No measured/pretrained profile or source sample is used by the new topology.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Structure {
    seconds: [f64; 8],
    scatter: f64,
    angle: f64,
}

impl Default for Structure {
    fn default() -> Self {
        Self::candidate(0.025, 0.002, 0.65)
    }
}

impl Structure {
    fn candidate(period: f64, scatter: f64, angle: f64) -> Self {
        Self {
            seconds: [0.71, 0.83, 0.97, 1.07, 1.19, 1.31, 1.43, 1.61].map(|x| x * period),
            scatter,
            angle,
        }
    }

    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.seconds
            .iter()
            .copied()
            .chain([self.scatter, self.angle])
    }

    pub fn decode(v: &[f64]) -> Result<Self, String> {
        if v.len() != 10
            || v.iter().any(|x| !x.is_finite())
            || v[..8].iter().any(|x| !(0.001..=0.3).contains(x))
            || !(0.0001..=0.02).contains(&v[8])
            || !(0.08..=1.4).contains(&v[9])
        {
            return Err("Invalid structural seed coefficients".into());
        }
        Ok(Self {
            seconds: v[..8].try_into().unwrap(),
            scatter: v[8],
            angle: v[9],
        })
    }

    pub fn build(&self, rate: u32) -> Result<DiffFdn, String> {
        if !(8000..=384000).contains(&rate) {
            return Err("unsupported sample rate".into());
        }
        // Construct a trusted profile from analytic orthogonal butterfly rotations.
        // Four small paraunitary stages preserve feedback passivity. Parsing also
        // validates orthogonality and bounds, independently of the generator.
        let mut p = String::from("difffdn-scattering-v1 48000 8 4");
        for t in self.seconds {
            write!(p, " {}", (t * 48000.).round().max(1.) as usize).unwrap();
        }
        for _ in 0..16 {
            p.push_str(" 0");
        }
        for i in 0..8 {
            write!(
                p,
                " {}",
                if i % 3 == 0 {
                    -0.3535533905932738
                } else {
                    0.3535533905932738
                }
            )
            .unwrap();
        }
        for _ in 0..8 {
            p.push_str(" 0.3535533905932738");
        }
        for stage in 0..4 {
            let bit = 1 << (stage % 3);
            let (s, c) = (self.angle * [0.8, 1., 1.2, 0.9][stage]).sin_cos();
            for row in 0..8 {
                for col in 0..8 {
                    let x = if row == col {
                        c
                    } else if row ^ col == bit {
                        if row & bit == 0 { -s } else { s }
                    } else {
                        0.
                    };
                    write!(p, " {x:.17e}").unwrap();
                }
            }
            for line in 0..8 {
                let factor = 0.15 + ((line * 3 + stage * 5) % 11) as f64 / 10.;
                write!(p, " {}", (48000. * self.scatter * factor).round() as usize).unwrap();
            }
        }
        for _ in 0..9 {
            p.push_str(" 1 1 0 0 1 0 0");
        }
        DiffFdn::parse(&p)?.at_sample_rate(rate)
    }
}

// 240 ms captures buildup without letting late-tail duration dominate the loss.
// Moment density is amplitude/decay independent within each short window. Echo
// correlation uses 1 ms energy bins, not aliased audio-rate resampling.
struct Features {
    envelope: [f64; 24],
    density: [f64; 12],
    echoes: [f64; 16],
}

impl Features {
    fn measure(frames: &[[f64; 2]], rate: u32) -> Self {
        let mut bins = [0.; 240];
        let mut fourth = [0.; 12];
        let mut sums = [0.; 12];
        let mut count = [0.; 12];
        for (i, f) in frames.iter().take(rate as usize * 240 / 1000).enumerate() {
            let ms = i * 1000 / rate as usize;
            let e = energy(f);
            bins[ms] += e;
            sums[ms / 20] += e;
            fourth[ms / 20] += e * e;
            count[ms / 20] += 1.;
        }
        let total = bins.iter().sum::<f64>().max(1e-30);
        let envelope =
            std::array::from_fn(|i| (bins[i * 10..i * 10 + 10].iter().sum::<f64>() / total).sqrt());
        let density = std::array::from_fn(|i| {
            (2. * sums[i].powi(2) / (count[i] * fourth[i]).max(1e-30)).min(1.)
        });
        // Remove smooth decay/buildup before measuring repeated energy clusters.
        let residual: Vec<_> = (0usize..240)
            .map(|i| {
                let lo = i.saturating_sub(8);
                let hi = (i + 9).min(240);
                let mean = bins[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
                (bins[i] - mean) / (mean + total * 1e-5)
            })
            .collect();
        let echoes = std::array::from_fn(|i| {
            let lag = (i + 1) * 4;
            let mut cross = 0.;
            let mut a = 0.;
            let mut b = 0.;
            for t in lag..240 {
                cross += residual[t] * residual[t - lag];
                a += residual[t].powi(2);
                b += residual[t - lag].powi(2);
            }
            cross / (a * b).sqrt().max(1e-30)
        });
        Self {
            envelope,
            density,
            echoes,
        }
    }

    fn loss(&self, other: &Self) -> f64 {
        let error = |a: &[f64], b: &[f64]| {
            a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum::<f64>() / a.len() as f64
        };
        12. * error(&self.envelope, &other.envelope)
            + error(&self.density, &other.density)
            + 0.25 * error(&self.echoes, &other.echoes)
    }
}

pub(super) fn fit(seed: &mut Seed, frames: &[[f64; 2]], rate: u32) -> Result<(), String> {
    let target = Features::measure(frames, rate);
    let evaluate = |seed: &Seed| -> Result<f64, String> {
        let rate = 12000;
        let mut f = seed.build(rate)?;
        let out: Vec<_> = (0..rate * 240 / 1000)
            .map(|i| f.process_stereo(if i == 0 { 1. } else { 0. }))
            .collect();
        Ok(target.loss(&Features::measure(&out, rate)))
    };
    let mut best = f64::INFINITY;
    let mut choice = Structure::default();
    // Fixed work budget: 36 coarse trials, then 16 single-line refinements.
    for period in [0.008, 0.02, 0.045, 0.09] {
        for scatter in [0.0002, 0.002, 0.008] {
            for angle in [0.18, 0.65, 1.1] {
                seed.structure = Some(Structure::candidate(period, scatter, angle));
                let loss = evaluate(seed)?;
                if loss < best {
                    best = loss;
                    choice = seed.structure.clone().unwrap();
                }
            }
        }
    }
    for line in 0..8 {
        let old = choice.seconds[line];
        for factor in [0.85, 1.15] {
            let mut trial = choice.clone();
            trial.seconds[line] = old * factor;
            seed.structure = Some(trial.clone());
            let loss = evaluate(seed)?;
            if loss < best {
                best = loss;
                choice = trial;
            }
        }
    }
    seed.structure = Some(choice);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent procedural fixtures: equal exponential decay, but different
    // echo cluster spacings and density buildup. Not generated by the fitted FDN.
    fn space(spacing_ms: usize, rise_ms: f64) -> Impulse {
        let rate = 24000;
        let mut random = 73u64;
        let frames = (0..rate * 3)
            .map(|i| {
                let ms = i as f64 * 1000. / rate as f64;
                let t = i as f64 / rate as f64;
                let envelope = 10f64.powf(-3. * t);
                let density = 0.02 + 0.98 * (1. - (-ms / rise_ms).exp());
                let mut noise = || {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (random >> 32) as u32 as f64 / u32::MAX as f64 * 2. - 1.
                };
                let cluster = if (ms as usize) % spacing_ms < 2 {
                    1.
                } else {
                    0.07
                };
                let gate = if noise().abs() < density { 1. } else { 0. };
                [
                    noise() * envelope * cluster * gate,
                    noise() * envelope * cluster * gate,
                ]
            })
            .collect();
        Impulse {
            sample_rate: rate,
            channels: 2,
            bits: 32,
            frames,
        }
    }

    fn render(seed: &Seed) -> Vec<[f64; 2]> {
        let mut f = seed.build(24000).unwrap();
        (0..5760)
            .map(|i| f.process_stereo(if i == 0 { 1. } else { 0. }))
            .collect()
    }

    #[test]
    fn matched_decay_spaces_improve_over_salon_and_fixed_topology() {
        let mut fitted = Vec::new();
        for (spacing, rise) in [(7, 8.), (43, 180.)] {
            let ir = space(spacing, rise);
            let report = analyze(&ir).unwrap();
            let seed = report.seed;
            let target = Features::measure(&ir.frames, ir.sample_rate);
            let actual = target.loss(&Features::measure(&render(&seed), 24000));
            let mut fixed = Seed::decode(&seed.encode()).unwrap();
            fixed.structure = Some(Structure::default());
            let default_loss = target.loss(&Features::measure(&render(&fixed), 24000));
            fixed.structure = None;
            let salon = target.loss(&Features::measure(&render(&fixed), 24000));
            eprintln!(
                "spacing={spacing}ms: fitted={actual:.5}, fixed={default_loss:.5}, Salon={salon:.5}, delays={:?}, decay={:?}",
                seed.structure, seed.decay
            );
            assert!(
                actual < default_loss * 0.95,
                "no structural improvement over independent fixed topology"
            );
            assert!(
                actual < salon * 0.95,
                "no structural improvement over Salon"
            );
            assert!((seed.decay[3] - 1.).abs() < 0.2);
            fitted.push(seed);
        }
        assert_ne!(
            fitted[0].structure.as_ref().unwrap().seconds,
            fitted[1].structure.as_ref().unwrap().seconds
        );
        for (i, (spacing, rise)) in [(7, 8.), (43, 180.)].into_iter().enumerate() {
            let ir = space(spacing, rise);
            let target = Features::measure(&ir.frames, ir.sample_rate);
            let own = target.loss(&Features::measure(&render(&fitted[i]), 24000));
            let mut swapped = Seed::decode(&fitted[i].encode()).unwrap();
            swapped.structure = fitted[1 - i].structure.clone();
            let other = target.loss(&Features::measure(&render(&swapped), 24000));
            eprintln!("spacing={spacing}ms: own={own:.5}, swapped topology={other:.5}");
            assert!(
                own < other,
                "topology is not specific to the measured space"
            );
        }
    }

    #[test]
    fn versions_and_structural_bounds() {
        let seed = analyze(&space(7, 8.)).unwrap().seed;
        let text = seed.encode();
        assert!(text.starts_with("rust-stereo-seed-v2 "));
        assert_eq!(Seed::decode(&text).unwrap().encode(), text);
        let mut legacy = Seed::decode(&text).unwrap();
        legacy.structure = None;
        let text = legacy.encode();
        assert!(text.starts_with("rust-stereo-seed-v1 "));
        let restored = Seed::decode(&text).unwrap();
        assert_eq!(render(&legacy), render(&restored));
        assert_eq!(text, restored.encode());
        for index in 30..40 {
            for bad in ["NaN", "inf", "-1", "0", "999999"] {
                let mut words: Vec<_> = seed
                    .encode()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                words[index + 1] = bad.into();
                assert!(Seed::decode(&words.join(" ")).is_err());
            }
        }
        assert!(Seed::decode(&(seed.encode() + " 1")).is_err());
    }

    #[test]
    #[ignore = "requires separately supplied room WAVs"]
    fn local_structural_comparison() {
        for name in [
            "French 18th Century Salon",
            "Highly Damped Large Room",
            "Parking Garage",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/IMreverbs")
                .join(format!("{name}.wav"));
            let ir = Impulse::load(&path).unwrap();
            let mut seed = analyze(&ir).unwrap().seed;
            let target = Features::measure(&ir.frames, ir.sample_rate);
            let fitted = target.loss(&Features::measure(&render(&seed), 24000));
            let structure = seed.structure.take();
            let salon = target.loss(&Features::measure(&render(&seed), 24000));
            eprintln!(
                "{name}: structural loss fitted={fitted:.5}, Salon={salon:.5}, {structure:?}"
            );
            assert!(fitted < salon, "{name}: structural fit regressed");
        }
    }
}
