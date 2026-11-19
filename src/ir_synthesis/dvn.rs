//! Experimental routed-pulse Dark Velvet Noise, with no feedback/FDN.
//! One sparse carrier per channel; each pulse excites exactly one fixed filter.
//! Reference-derived, interpolated powers control routing and pulse amplitude.
use super::{Random, SynthesisOptions, bank, stereo_match, velvet};
use crate::ir_analysis::Impulse;

fn powers_at(powers: &[[f64; 2]], sample: usize, block: usize) -> [f64; 2] {
    let x = ((sample as f64 + 0.5) / block as f64 - 0.5).max(0.0);
    let left = (x.floor() as usize).min(powers.len() - 1);
    let right = (left + 1).min(powers.len() - 1);
    std::array::from_fn(|ch| powers[left][ch] * (1.0 - x.fract()) + powers[right][ch] * x.fract())
}

fn probabilities(powers: &[f64]) -> Vec<f64> {
    let total = powers.iter().sum::<f64>();
    powers
        .iter()
        .map(|p| {
            // Keep quiet filters reachable, avoiding very large inverse-probability gains.
            if total > 0.0 {
                0.95 * p / total + 0.05 / powers.len() as f64
            } else {
                1.0 / powers.len() as f64
            }
        })
        .collect()
}

pub(super) fn render(ir: &Impulse, options: SynthesisOptions, late: usize) -> Vec<[f64; 2]> {
    let count = bank::edges(ir.sample_rate).len() + 1;
    let block = (ir.sample_rate as usize / 50).max(1);
    let mut powers = Vec::with_capacity(count);
    let mut filter_energy = Vec::with_capacity(count);
    let mut impulse = vec![[0.0; 2]; ir.sample_rate as usize];
    impulse[0] = [1.0; 2];
    for band in 0..count {
        powers.push(
            bank::filter(&ir.frames, ir.sample_rate, band)
                .chunks(block)
                .map(|cell| {
                    std::array::from_fn(|ch| {
                        cell.iter().map(|f| f[ch].powi(2)).sum::<f64>() / cell.len() as f64
                    })
                })
                .collect::<Vec<_>>(),
        );
        filter_energy.push(
            bank::filter(&impulse, ir.sample_rate, band)
                .iter()
                .map(|f| f[0].powi(2))
                .sum::<f64>()
                .max(1e-24),
        );
    }
    // Dedicated RNGs leave all pre-existing VN candidate sequences unchanged.
    let carrier = velvet(
        ir.frames.len(),
        ir.sample_rate,
        &mut Random(options.seed ^ 0x44564e),
        options,
        late,
    );
    let mut random = Random(options.seed ^ 0x726f757465);
    let mut routed = vec![vec![[0.0; 2]; ir.frames.len()]; count];
    for (i, pulse) in carrier.iter().enumerate() {
        if *pulse == [0.0; 2] {
            continue;
        }
        let local: Vec<_> = powers.iter().map(|p| powers_at(p, i, block)).collect();
        let rise = if options.density_rise_ms == 0.0 {
            1.0
        } else {
            1.0 - (-(i.saturating_sub(late) as f64)
                / (f64::from(ir.sample_rate) * options.density_rise_ms * 0.001))
                .exp()
        };
        let width = (f64::from(ir.sample_rate) / (options.density * (0.25 + 0.75 * rise)))
            .round()
            .max(1.0);
        for ch in 0..2 {
            if pulse[ch] == 0.0 {
                continue;
            }
            let p: Vec<_> = local.iter().map(|p| p[ch]).collect();
            let probabilities = probabilities(&p);
            let draw = (random.next() >> 11) as f64 / ((1_u64 << 53) as f64);
            let mut cumulative = 0.0;
            let selected = probabilities
                .iter()
                .position(|probability| {
                    cumulative += probability;
                    draw < cumulative
                })
                .unwrap_or(count - 1);
            let gain = (p[selected] * width / (probabilities[selected] * filter_energy[selected]))
                .sqrt()
                .min(1000.0);
            routed[selected][i][ch] = pulse[ch] * gain;
        }
    }
    let mut out = vec![[0.0; 2]; ir.frames.len()];
    for (band, input) in routed.into_iter().enumerate() {
        let mut component = bank::filter(&input, ir.sample_rate, band);
        if ir.channels == 2 {
            let target = bank::filter(&ir.frames, ir.sample_rate, band);
            stereo_match(&mut component, &target, late);
        }
        for (sum, frame) in out.iter_mut().zip(component) {
            for ch in 0..2 {
                sum[ch] += frame[ch];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir_analysis::energy;

    #[test]
    fn routing_tracks_time_varying_spectral_power() {
        let bright = probabilities(&[1.0, 9.0]);
        let dark = probabilities(&[9.0, 1.0]);
        assert!(bright[1] > 0.85 && dark[1] < 0.15);
        assert_eq!(probabilities(&[0.0, 0.0]), vec![0.5, 0.5]);
        for (actual, expected) in powers_at(&[[1.0, 3.0], [3.0, 1.0]], 19, 20)
            .into_iter()
            .zip([1.95, 2.05])
        {
            assert!((actual - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn deterministic_finite_decaying_and_stereo() {
        let ir = super::super::tests::reference();
        let options = SynthesisOptions::default();
        let a = render(&ir, options, 800);
        assert_eq!(a, render(&ir, options, 800));
        assert_ne!(a, render(&ir, SynthesisOptions { seed: 2, ..options }, 800));
        assert!(a.iter().flatten().all(|v| v.is_finite()));
        assert!(a.iter().any(|f| f[0] != f[1]));
        assert!(
            a[8000..].iter().map(energy).sum::<f64>()
                < a[800..4000].iter().map(energy).sum::<f64>() * 0.1
        );
    }

    #[test]
    fn rendered_spectrum_darkens_with_reference() {
        let rate = 8000;
        let mut random = Random(19);
        let raw: Vec<_> = (0..16000)
            .map(|_| {
                std::array::from_fn(|_| (random.next() >> 11) as f64 / ((1_u64 << 52) as f64) - 1.0)
            })
            .collect();
        let low = bank::filter(&raw, rate, 0);
        let high = bank::filter(&raw, rate, bank::edges(rate).len());
        let ir = Impulse {
            sample_rate: rate,
            channels: 2,
            bits: 32,
            frames: low
                .iter()
                .zip(high)
                .enumerate()
                .map(|(i, (l, h))| {
                    let t = i as f64 / f64::from(rate);
                    std::array::from_fn(|ch| l[ch] * (-t).exp() + h[ch] * (-5.0 * t).exp())
                })
                .collect(),
        };
        let out = render(
            &ir,
            SynthesisOptions {
                density_rise_ms: 0.0,
                ..Default::default()
            },
            800,
        );
        let l = bank::filter(&out, rate, 0);
        let h = bank::filter(&out, rate, bank::edges(rate).len());
        let ratio = |range: std::ops::Range<usize>| {
            h[range.clone()].iter().map(energy).sum::<f64>()
                / l[range].iter().map(energy).sum::<f64>()
        };
        assert!(ratio(10000..15000) < ratio(1000..5000) * 0.1);
    }
}
