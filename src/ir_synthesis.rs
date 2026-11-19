//! Offline comparison baselines, including an experimental routed-pulse DVN.
use crate::ir_analysis::{Impulse, edc, energy, fit_decay, octave};
use crate::{Config, Matrix, Reverb};
mod bank;
mod dvn;
mod fdn_fit;
pub use fdn_fit::FdnFit;
mod editable;
pub use editable::{EditableHybrid, StreamingTail, TailControls};

#[derive(Clone, Copy, Debug)]
pub struct SynthesisOptions {
    /// Absolute file-relative end of the verbatim early segment.
    pub early_ms: f64,
    pub fade_ms: f64,
    pub seed: u64,
    pub t60: Option<f64>,
    /// Final pulse density per second/channel, limited to the sample rate.
    pub density: f64,
    /// Exponential density rise time from crossfade end in ms; zero is constant.
    pub density_rise_ms: f64,
}

impl Default for SynthesisOptions {
    fn default() -> Self {
        Self {
            early_ms: 80.0,
            fade_ms: 20.0,
            seed: 1,
            t60: None,
            density: 2000.0,
            density_rise_ms: 50.0,
        }
    }
}

pub struct Synthesis {
    pub early_samples: usize,
    pub fade_samples: usize,
    pub t60: f64,
    pub t60_source: &'static str,
    pub fdn_band_damping: [BandDamping; 2],
    pub fdn_fit: FdnFit,
    pub fdn_eq_bands: Vec<FdnEqBand>,
    pub vn_bands: Vec<VnBand>,
    pub candidates: Vec<Candidate>,
}

/// Static post-loop gains before summation and final shared suffix normalization.
#[derive(Clone, Debug)]
pub struct FdnEqBand {
    pub lower_hz: f64,
    pub upper_hz: f64,
    pub late_gain: f64,
    pub gain_capped: bool,
}

#[derive(Clone, Debug)]
pub struct VnBand {
    pub lower_hz: f64,
    pub upper_hz: f64,
    pub target_correlation: Option<f64>,
    pub actual_correlation: Option<f64>,
    pub late_gain: f64,
}

/// Full-reference estimate and fitted additional passive per-loop attenuation.
/// The fitter can raise nominal decay; it never introduces negative attenuation.
#[derive(Clone, Copy, Debug)]
pub struct BandDamping {
    pub center_hz: f64,
    pub t60: Option<f64>,
    pub attenuation_db: f32,
}

fn screened_t60(frames: &[[f64; 2]], rate: u32) -> Option<f64> {
    let curve = edc(frames);
    [(-5.0, -35.0), (-5.0, -25.0), (0.0, -10.0)]
        .into_iter()
        .find_map(|(upper, lower)| {
            let fit = fit_decay(frames, &curve, rate, upper, lower);
            (fit.status == "reference_only")
                .then_some(fit.t60)
                .flatten()
                .filter(|t| t.is_finite() && *t > 0.0)
        })
}

fn band_damping(ir: &Impulse, nominal_t60: f64) -> [BandDamping; 2] {
    // Mean Classic delay plus the full loop all-pass delay. This is a nominal
    // conversion, not an inverse fit of the overlapping damping filters or LP.
    // The loop all-pass has frequency-dependent group delay, not a pure buffer delay.
    let loop_seconds = [
        1493.0, 1601.0, 1867.0, 2053.0, 2251.0, 2399.0, 2791.0, 3011.0, 127.0, 179.0, 233.0, 283.0,
        337.0, 389.0, 443.0, 499.0,
    ]
    .iter()
    .sum::<f64>()
        / (8.0 * 48000.0);
    [250.0, 2000.0].map(|center_hz| {
        let t60 = screened_t60(
            &octave(&ir.frames, ir.sample_rate, center_hz),
            ir.sample_rate,
        );
        let attenuation_db = t60.map_or(0.0, |t| {
            (60.0 * loop_seconds * (1.0 / t - 1.0 / nominal_t60)).clamp(0.0, 24.0) as f32
        });
        BandDamping {
            center_hz,
            t60,
            attenuation_db,
        }
    })
}

pub struct Candidate {
    pub name: &'static str,
    pub frames: Vec<[f64; 2]>,
    /// One shared amplitude gain, fitted on the fully synthetic suffix only.
    pub late_gain: f64,
}

pub fn band_centers(rate: u32) -> Vec<f64> {
    [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0]
        .into_iter()
        .filter(|hz| hz * 2.0_f64.sqrt() < f64::from(rate) * 0.5)
        .collect()
}

// SplitMix64: deterministic local RNG, no extra dependency or global state.
struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
}

/// One randomly located ±1 pulse per variable-length cell and channel.
/// Rise is anchored at the first fully synthetic sample, not file start.
fn velvet(
    len: usize,
    rate: u32,
    random: &mut Random,
    options: SynthesisOptions,
    anchor: usize,
) -> Vec<[f64; 2]> {
    let mut out = vec![[0.0; 2]; len];
    let mut start = 0;
    while start < len {
        let rise = if options.density_rise_ms == 0.0 {
            1.0
        } else {
            let t = start.saturating_sub(anchor) as f64 / f64::from(rate);
            1.0 - (-t / (options.density_rise_ms * 0.001)).exp()
        };
        let density = options.density * (0.25 + 0.75 * rise);
        let width = ((f64::from(rate) / density).round() as usize)
            .max(1)
            .min(len - start);
        for ch in [0, 1] {
            let pos = start + (random.next() % width as u64) as usize;
            out[pos][ch] = if random.next() & 1 == 0 { -1.0 } else { 1.0 };
        }
        start += width;
    }
    out
}

/// Fit suffix Pearson correlation using two centered orthonormal carriers.
/// This is equivalent to mixing common/differential components, with independent
/// L/R energy retained. Global per-band fitting does not fit time-varying IACC.
/// Mixing can smear previously fitted per-channel envelope differences.
fn stereo_match(carrier: &mut [[f64; 2]], target: &[[f64; 2]], late: usize) {
    let Some(rho) = crate::ir_analysis::correlation(&target[late..]) else {
        return;
    };
    let count = (carrier.len() - late) as f64;
    let means = [0, 1].map(|ch| carrier[late..].iter().map(|f| f[ch]).sum::<f64>() / count);
    let variances = [0, 1].map(|ch| {
        carrier[late..]
            .iter()
            .map(|f| (f[ch] - means[ch]).powi(2))
            .sum::<f64>()
    });
    if variances.iter().any(|v| *v < 1e-24 * count) {
        return;
    }
    let covariance = carrier[late..]
        .iter()
        .map(|f| (f[0] - means[0]) * (f[1] - means[1]))
        .sum::<f64>();
    let projection = covariance / variances[0];
    let residual = variances[1] - covariance * projection;
    if residual < 1e-24 * count {
        return;
    }
    let powers = [0, 1].map(|ch| target[late..].iter().map(|f| f[ch].powi(2)).sum::<f64>());
    let left = (powers[0] / variances[0]).sqrt().min(1000.0);
    let b = ((1.0 - rho * rho).max(0.0) * powers[1] / residual).sqrt();
    let a = rho * (powers[1] / variances[0]).sqrt() - b * projection;
    let cap = (1000.0 / (a.abs() + b.abs()).max(1000.0)).min(1.0);
    for f in carrier {
        // Apply a zero-offset linear transform: centering the emitted waveform
        // would introduce a constant tail/DC floor. Pearson itself is centered.
        *f = [left * f[0], cap * (a * f[0] + b * f[1])];
    }
}

/// Match per-channel 20 ms mean-square envelopes by interpolating RMS gains
/// between block centers. No source waveform/phase or L/R correlation is copied.
fn envelope_match(carrier: &mut [[f64; 2]], target: &[[f64; 2]], rate: u32) {
    let block = (rate as usize / 50).max(1);
    let gains: Vec<[f64; 2]> = target
        .chunks(block)
        .zip(carrier.chunks(block))
        .map(|(t, c)| {
            std::array::from_fn(|ch| {
                let wanted = t.iter().map(|f| f[ch] * f[ch]).sum::<f64>();
                let actual = c.iter().map(|f| f[ch] * f[ch]).sum::<f64>();
                // Bound gain at +60 dB, including almost-silent filtered cells.
                (wanted / actual.max(1e-24 * c.len() as f64))
                    .sqrt()
                    .min(1000.0)
            })
        })
        .collect();
    for (i, f) in carrier.iter_mut().enumerate() {
        let x = (i as f64 + 0.5) / block as f64 - 0.5;
        let left = x.floor().max(0.0) as usize;
        let right = (left + 1).min(gains.len() - 1);
        let fraction = if x < 0.0 { 0.0 } else { x.fract() };
        for ch in 0..2 {
            f[ch] *= gains[left][ch] * (1.0 - fraction) + gains[right][ch] * fraction;
        }
    }
}

fn suffix_energy_gain(wanted: f64, actual: f64, samples: usize) -> (f64, bool) {
    let gain = (wanted / actual.max(1e-24 * samples as f64)).sqrt();
    (gain.min(1000.0), gain >= 1000.0)
}

/// Audition baseline: VN-led synthetic onset, then a smooth equal-power handoff
/// to the accepted temporal FDN. Not a feedback hybrid or a fitted transition.
pub const HYBRID_HOLD_MS: f64 = 100.0;
pub const HYBRID_TRANSITION_MS: f64 = 400.0;

fn hybrid_frames(ir: &Impulse, vn: &[[f64; 2]], fdn: &[[f64; 2]], late: usize) -> Vec<[f64; 2]> {
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
    let gains = [vn, fdn].map(|f| suffix_energy_gain(wanted, power(f), f.len() - late).0);
    vn.iter()
        .zip(fdn)
        .enumerate()
        .map(|(i, (v, f))| {
            let ms = i.saturating_sub(late) as f64 * 1000.0 / f64::from(ir.sample_rate);
            let x = ((ms - HYBRID_HOLD_MS) / HYBRID_TRANSITION_MS).clamp(0.0, 1.0);
            // Smoothstep also makes the derivative zero at both transition endpoints.
            let phase = x * x * (3.0 - 2.0 * x) * std::f64::consts::FRAC_PI_2;
            let (a, b) = if x == 0.0 {
                (1.0, 0.0)
            } else if x == 1.0 {
                (0.0, 1.0)
            } else {
                (phase.cos(), phase.sin())
            };
            std::array::from_fn(|ch| a * gains[0] * v[ch] + b * gains[1] * f[ch])
        })
        .collect()
}

fn fdn_energy_eq(ir: &Impulse, raw: &[[f64; 2]], late: usize) -> (Vec<[f64; 2]>, Vec<FdnEqBand>) {
    fdn_output_eq_cached(ir, raw, late, false, None)
}

/// Experimental static low-band stereo transform, outside the feedback loop.
/// Keep the accepted temporal-fit path separate until comparative listening.
fn fdn_output_eq(
    ir: &Impulse,
    raw: &[[f64; 2]],
    late: usize,
    low_stereo: bool,
) -> (Vec<[f64; 2]>, Vec<FdnEqBand>) {
    fdn_output_eq_cached(ir, raw, late, low_stereo, None)
}

// Only the temporal path can use energy-only targets: stereo matching needs
// the reference samples themselves. Preserve filtering and summation order.
fn fdn_output_eq_cached(
    ir: &Impulse,
    raw: &[[f64; 2]],
    late: usize,
    low_stereo: bool,
    target_energy: Option<&[f64]>,
) -> (Vec<[f64; 2]>, Vec<FdnEqBand>) {
    debug_assert!(!low_stereo || target_energy.is_none());
    let edges = bank::edges(ir.sample_rate);
    let mut frames = vec![[0.0; 2]; raw.len()];
    let mut bands = Vec::new();
    for band in 0..=edges.len() {
        let target = target_energy
            .is_none()
            .then(|| bank::filter(&ir.frames, ir.sample_rate, band));
        let mut component = bank::filter(raw, ir.sample_rate, band);
        if low_stereo
            && ir.channels == 2
            && edges
                .get(band)
                .is_some_and(|&upper| upper <= 500.0 * 2.0_f64.sqrt())
        {
            stereo_match(&mut component, target.as_ref().unwrap(), late);
        }
        let wanted = target_energy.map_or_else(
            || {
                target.as_ref().unwrap()[late..]
                    .iter()
                    .map(energy)
                    .sum::<f64>()
            },
            |energies| energies[band],
        );
        // Mono export keeps only L; fit that channel before it is duplicated.
        let actual = component[late..]
            .iter()
            .map(|f| {
                if ir.channels == 1 {
                    f[0] * f[0]
                } else {
                    energy(f)
                }
            })
            .sum::<f64>();
        let (gain, gain_capped) = suffix_energy_gain(wanted, actual, raw.len() - late);
        bands.push(FdnEqBand {
            lower_hz: if band == 0 { 0.0 } else { edges[band - 1] },
            upper_hz: edges
                .get(band)
                .copied()
                .unwrap_or(f64::from(ir.sample_rate) * 0.5),
            late_gain: gain,
            gain_capped,
        });
        // Constant gains outside the loop preserve each isolated component's EDC.
        // Overlap, phase and unequal gains can still change post-sum octave T60.
        for (sum, f) in frames.iter_mut().zip(component) {
            for ch in 0..2 {
                sum[ch] += f[ch] * gain;
            }
        }
    }
    (frames, bands)
}

pub fn synthesize(ir: &Impulse, options: SynthesisOptions) -> Result<Synthesis, String> {
    let rate = f64::from(ir.sample_rate);
    if !(8000..=384000).contains(&ir.sample_rate)
        || !(1..=2).contains(&ir.channels)
        || ir.frames.is_empty()
        || ir.frames.len() > ir.sample_rate as usize * 60
        || ir.frames.iter().flatten().any(|v| !v.is_finite())
    {
        return Err("Invalid IR format, length or samples".into());
    }
    if !options.early_ms.is_finite()
        || !(0.0..=60000.0).contains(&options.early_ms)
        || !options.fade_ms.is_finite()
        || !(1.0..=1000.0).contains(&options.fade_ms)
        || !options.density.is_finite()
        || !(10.0..=rate).contains(&options.density)
        || !options.density_rise_ms.is_finite()
        || !(0.0..=10000.0).contains(&options.density_rise_ms)
    {
        return Err("early_ms must be 0..60000; fade_ms 1..1000; density 10..sample_rate; density_rise_ms 0..10000".into());
    }
    let early = (options.early_ms * rate / 1000.0).round() as usize;
    let fade = (options.fade_ms * rate / 1000.0).round() as usize;
    let late = early + fade;
    if late + (ir.sample_rate as usize / 10) >= ir.frames.len() {
        return Err(
            "Need more than 100 ms of IR after the crossfade; reduce early_ms/fade_ms".into(),
        );
    }
    let target_energy = ir.frames[late..].iter().map(energy).sum::<f64>();
    if !target_energy.is_finite() || target_energy <= 0.0 {
        return Err("Reference suffix is silent or has invalid energy".into());
    }
    let (t60, source) = if let Some(t60) = options.t60 {
        (t60, "explicit_override")
    } else {
        let curve = edc(&ir.frames);
        let estimate = [
            ("T30", -5.0, -35.0),
            ("T20", -5.0, -25.0),
            ("EDT", 0.0, -10.0),
        ]
        .into_iter()
        .find_map(|(name, upper, lower)| {
            let fit = fit_decay(&ir.frames, &curve, ir.sample_rate, upper, lower);
            if fit.status == "reference_only" {
                fit.t60.map(|t| (t, name))
            } else {
                None
            }
        });
        estimate
            .ok_or("No screened broadband T60 estimate; supply explicit t60_seconds override")?
    };
    if !t60.is_finite() || !(0.01..=60.0).contains(&t60) {
        return Err("T60 must be finite and in 0.01..60 seconds".into());
    }
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
    let fdn_fit = fdn_fit::fit(ir, t60, late, onset, options.t60.is_some())?;
    let mut fdn_band_damping = band_damping(ir, fdn_fit.nominal_t60);
    for (band, db) in fdn_band_damping.iter_mut().zip(fdn_fit.attenuation_db) {
        band.attenuation_db = db;
    }
    let raw_fdn = fdn_fit::render(ir.sample_rate, ir.frames.len(), onset, &fdn_fit)?;
    let (fdn_frames, fdn_eq_bands) = fdn_energy_eq(ir, &raw_fdn, late);
    let (fdn_stereo_frames, _) = fdn_output_eq(ir, &raw_fdn, late, true);
    let mut random = Random(options.seed);
    let mut vn = velvet(ir.frames.len(), ir.sample_rate, &mut random, options, late);
    envelope_match(&mut vn, &ir.frames, ir.sample_rate);
    if ir.channels == 2 {
        stereo_match(&mut vn, &ir.frames, late);
    }
    let mut multiband = vec![[0.0; 2]; ir.frames.len()];
    let mut vn_bands = Vec::new();
    let edges = bank::edges(ir.sample_rate);
    for band in 0..=edges.len() {
        let target = bank::filter(&ir.frames, ir.sample_rate, band);
        let mut carrier = bank::filter(
            &velvet(ir.frames.len(), ir.sample_rate, &mut random, options, late),
            ir.sample_rate,
            band,
        );
        envelope_match(&mut carrier, &target, ir.sample_rate);
        if ir.channels == 2 {
            stereo_match(&mut carrier, &target, late);
        }
        // Correct envelope-interpolation bias before summing overlapping bands.
        // As with the envelope gains, never amplify a near-silent carrier without
        // bound. The final shared gain still matches total suffix energy.
        let wanted = target[late..].iter().map(energy).sum::<f64>();
        let actual = if ir.channels == 1 {
            carrier[late..].iter().map(|f| f[0] * f[0]).sum::<f64>()
        } else {
            carrier[late..].iter().map(energy).sum::<f64>()
        };
        let gain = (wanted / actual.max(1e-24 * (carrier.len() - late) as f64))
            .sqrt()
            .min(1000.0);
        vn_bands.push(VnBand {
            lower_hz: if band == 0 { 0.0 } else { edges[band - 1] },
            upper_hz: edges.get(band).copied().unwrap_or(rate * 0.5),
            target_correlation: if ir.channels == 2 {
                crate::ir_analysis::correlation(&target[late..])
            } else {
                None
            },
            actual_correlation: if ir.channels == 2 {
                crate::ir_analysis::correlation(&carrier[late..])
            } else {
                None
            },
            late_gain: gain,
        });
        for (sum, f) in multiband.iter_mut().zip(carrier) {
            for ch in 0..2 {
                sum[ch] += f[ch] * gain;
            }
        }
    }
    let hybrid = hybrid_frames(ir, &multiband, &fdn_frames, late);
    let dvn = dvn::render(ir, options, late);
    let mut candidates = Vec::new();
    for (name, mut frames) in [
        ("early_fdn", fdn_frames),
        ("early_vn", vn),
        ("early_multiband_vn", multiband),
        ("early_fdn_stereo", fdn_stereo_frames),
        ("early_hybrid", hybrid),
        ("early_dvn", dvn),
    ] {
        if ir.channels == 1 {
            for f in &mut frames {
                f[1] = f[0];
            }
        }
        let actual = frames[late..].iter().map(energy).sum::<f64>();
        if actual <= 0.0 || !actual.is_finite() {
            return Err(format!("{name}: cannot match suffix energy"));
        }
        let gain = (target_energy / actual).sqrt();
        for (i, f) in frames.iter_mut().enumerate() {
            if i < early {
                *f = ir.frames[i];
            } else if i >= late {
                for v in f {
                    *v *= gain;
                }
            } else {
                let phase =
                    ((i - early) as f64 / fade as f64).min(1.0) * std::f64::consts::FRAC_PI_2;
                for (ch, v) in f.iter_mut().enumerate() {
                    *v = ir.frames[i][ch] * phase.cos() + *v * gain * phase.sin();
                }
            }
        }
        if frames
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
        {
            return Err(format!("{name}: output outside finite float WAV range"));
        }
        candidates.push(Candidate {
            name,
            frames,
            late_gain: gain,
        });
    }
    Ok(Synthesis {
        early_samples: early,
        fade_samples: fade,
        t60,
        t60_source: source,
        fdn_band_damping,
        fdn_fit,
        fdn_eq_bands,
        vn_bands,
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_transform_matches_correlation_and_reference_bias() {
        let mut carrier: Vec<_> = (0..8000)
            .map(|i| {
                let phase = std::f64::consts::TAU * i as f64 / 8000.0;
                [(phase * 125.0).sin(), (phase * 250.0).sin()]
            })
            .collect();
        let target: Vec<_> = carrier
            .iter()
            .map(|f| [f[0], 2.0 * (0.6 * f[0] + 0.8 * f[1])])
            .collect();
        stereo_match(&mut carrier, &target, 0);
        assert!((crate::ir_analysis::correlation(&carrier).unwrap() - 0.6).abs() < 1e-12);
        for ch in 0..2 {
            let power = |frames: &[[f64; 2]]| frames.iter().map(|f| f[ch].powi(2)).sum::<f64>();
            assert!((power(&carrier) / power(&target) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn experimental_fdn_stereo_bypasses_mono_and_degenerate_carriers() {
        let mut ir = reference();
        ir.channels = 1;
        assert_eq!(
            fdn_output_eq(&ir, &ir.frames, 800, true).0,
            fdn_energy_eq(&ir, &ir.frames, 800).0
        );
        for mut carrier in [vec![[0.0; 2]; 100], vec![[0.5; 2]; 100]] {
            let original = carrier.clone();
            stereo_match(&mut carrier, &ir.frames[..100], 0);
            assert_eq!(carrier, original);
        }
    }

    #[test]
    fn fdn_eq_gain_floor_cap_and_silence_are_bounded() {
        assert_eq!(suffix_energy_gain(4.0, 1.0, 100), (2.0, false));
        assert_eq!(suffix_energy_gain(1.0, 0.0, 100), (1000.0, true));
        assert_eq!(suffix_energy_gain(0.0, 0.0, 100), (0.0, false));
        let mut ir = reference();
        ir.frames.fill([0.0; 2]);
        let (frames, bands) = fdn_energy_eq(&ir, &ir.frames, 800);
        assert!(frames.iter().flatten().all(|v| *v == 0.0));
        assert!(bands.iter().all(|b| b.late_gain == 0.0 && !b.gain_capped));
        ir.frames.fill([1.0; 2]);
        let raw = vec![[0.0; 2]; ir.frames.len()];
        let (frames, bands) = fdn_energy_eq(&ir, &raw, 800);
        assert!(bands.iter().any(|b| b.gain_capped));
        assert!(bands.iter().all(|b| b.late_gain <= 1000.0));
        assert!(frames.iter().flatten().all(|v| *v == 0.0));
    }

    #[test]
    fn fdn_eq_reduces_colored_reference_band_energy_error() {
        let mut ir = reference();
        let raw = ir.frames.clone();
        let late = 800;
        let count = bank::edges(ir.sample_rate).len() + 1;
        ir.frames.fill([0.0; 2]);
        for band in 0..count {
            let gain = 0.25 + 2.0 * band as f64 / count as f64;
            for (sum, f) in ir
                .frames
                .iter_mut()
                .zip(bank::filter(&raw, ir.sample_rate, band))
            {
                for ch in 0..2 {
                    sum[ch] += gain * f[ch];
                }
            }
        }
        let (equalized, bands) = fdn_energy_eq(&ir, &raw, late);
        assert_eq!(bands.len(), count);
        assert_eq!(bands[0].lower_hz, 0.0);
        assert_eq!(
            bands.last().unwrap().upper_hz,
            f64::from(ir.sample_rate) * 0.5
        );
        let error = |frames: &[[f64; 2]]| {
            (0..count)
                .map(|band| {
                    let power = |f: &[[f64; 2]]| {
                        bank::filter(f, ir.sample_rate, band)[late..]
                            .iter()
                            .map(energy)
                            .sum::<f64>()
                    };
                    (power(frames) / power(&ir.frames)).ln().powi(2)
                })
                .sum::<f64>()
        };
        assert!(error(&equalized) < error(&raw) * 0.5);
        for (band, info) in bands.iter().enumerate() {
            let component = bank::filter(&raw, ir.sample_rate, band);
            let scaled: Vec<_> = component
                .iter()
                .map(|f| f.map(|v| v * info.late_gain))
                .collect();
            for (a, b) in edc(&component[late..]).iter().zip(edc(&scaled[late..])) {
                assert!((a - b).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn density_changes_pulse_counts_and_rises_over_time() {
        let options = SynthesisOptions {
            density: 1000.0,
            density_rise_ms: 200.0,
            ..Default::default()
        };
        let frames = velvet(16000, 8000, &mut Random(1), options, 0);
        let count = |f: &[[f64; 2]]| f.iter().filter(|f| f[0] != 0.0).count();
        assert!(count(&frames[8000..9600]) > count(&frames[..1600]));
        let dense = velvet(
            16000,
            8000,
            &mut Random(1),
            SynthesisOptions {
                density: 4000.0,
                density_rise_ms: 0.0,
                ..options
            },
            0,
        );
        assert!(count(&dense) > count(&frames) * 2);
    }

    #[test]
    fn density_rise_starts_at_crossfade_end_instead_of_file_start() {
        let rate = 48000;
        let options = SynthesisOptions::default();
        let anchor = ((options.early_ms + options.fade_ms) * f64::from(rate) / 1000.0) as usize;
        let frames = velvet(48000, rate, &mut Random(1), options, anchor);
        let unanchored = velvet(48000, rate, &mut Random(1), options, 0);
        let count = |f: &[[f64; 2]], ch: usize| f.iter().filter(|f| f[ch] != 0.0).count();
        for ch in 0..2 {
            let initial = count(&frames[anchor..anchor + 960], ch);
            let settled = count(&frames[anchor + 12000..anchor + 12960], ch);
            assert!(
                initial < 24,
                "rise already settled at crossfade end: {initial}"
            );
            assert!(settled > initial * 2);
            assert!(count(&unanchored[anchor..anchor + 960], ch) > initial);
        }
        let constant = SynthesisOptions {
            density_rise_ms: 0.0,
            ..options
        };
        assert_eq!(
            velvet(48000, rate, &mut Random(1), constant, anchor),
            velvet(48000, rate, &mut Random(1), constant, 0)
        );
    }

    #[test]
    fn stereo_fit_handles_positive_negative_and_undefined_correlation() {
        for rho in [-1.0, -0.7, 0.0, 0.6, 1.0] {
            let target: Vec<_> = (0..16000)
                .map(|i| {
                    let x = (i as f64 * 0.137).sin();
                    let y = (i as f64 * 0.391).cos();
                    [x, 0.4 * (rho * x + (1.0_f64 - rho * rho).sqrt() * y)]
                })
                .collect();
            let mut carrier = velvet(target.len(), 8000, &mut Random(9), Default::default(), 0);
            envelope_match(&mut carrier, &target, 8000);
            stereo_match(&mut carrier, &target, 800);
            let expected = crate::ir_analysis::correlation(&target[800..]).unwrap();
            let actual = crate::ir_analysis::correlation(&carrier[800..]).unwrap();
            assert!((actual - expected).abs() < 1e-10, "{actual} vs {expected}");
        }
        let mut silence = vec![[0.0; 2]; 100];
        stereo_match(&mut silence, &vec![[1.0; 2]; 100], 10);
        assert!(silence.iter().flatten().all(|v| *v == 0.0));
    }

    #[test]
    fn fdn_search_can_lengthen_decay_without_feedback_boost() {
        let reference_fit = FdnFit {
            nominal_t60: 2.0,
            damping_hz: 3500.0,
            attenuation_db: [0.0, 0.25],
            targets: Vec::new(),
            initial_error: None,
            fitted_error: None,
            evaluations: 0,
        };
        let ir = Impulse {
            sample_rate: 8000,
            channels: 2,
            bits: 32,
            frames: fdn_fit::render(8000, 32000, 0, &reference_fit).unwrap(),
        };
        let fit = fdn_fit::fit(&ir, 0.5, 800, 0, false).unwrap();
        assert!(!fit.targets.is_empty());
        assert!(fit.nominal_t60 > 0.5, "{fit:?}");
        assert!(
            fit.fitted_error.unwrap() < fit.initial_error.unwrap(),
            "{fit:?}"
        );
        assert!(
            fit.attenuation_db
                .iter()
                .all(|db| (0.0..=24.0).contains(db))
        );
        let rendered = fdn_fit::render(8000, 80000, 0, &fit).unwrap();
        assert!(rendered.iter().flatten().all(|x| x.is_finite()));
        let low = octave(&rendered[..32000], 8000, 250.0);
        assert!(screened_t60(&low[800..], 8000).unwrap() > 0.5);
        let first = rendered[800..8000].iter().map(energy).sum::<f64>();
        let last = rendered[72000..].iter().map(energy).sum::<f64>();
        assert!(last < first * 1e-8);
        let fixed = fdn_fit::fit(&ir, 0.5, 800, 0, true).unwrap();
        assert_eq!(fixed.nominal_t60, 0.5);
    }

    #[test]
    fn multiband_preserves_low_and_high_residual_energy_and_correlation() {
        let rate = 44100;
        let ir = Impulse {
            sample_rate: rate,
            channels: 2,
            bits: 32,
            frames: (0..rate)
                .map(|i| {
                    let t = f64::from(i) / f64::from(rate);
                    let a = 10.0_f64.powf(-3.0 * t);
                    let low = a * (std::f64::consts::TAU * 35.0 * t).sin();
                    let high = a * (std::f64::consts::TAU * 16000.0 * t).sin();
                    [low + high, low - high]
                })
                .collect(),
        };
        let result = synthesize(
            &ir,
            SynthesisOptions {
                t60: Some(1.0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.vn_bands.first().unwrap().lower_hz, 0.0);
        assert_eq!(
            result.vn_bands.last().unwrap().upper_hz,
            f64::from(rate) * 0.5
        );
        for band in &result.vn_bands {
            if let (Some(target), Some(actual)) = (band.target_correlation, band.actual_correlation)
            {
                assert!((target - actual).abs() < 1e-8, "{band:?}");
            }
        }
        let candidate = &result.candidates[2].frames;
        let late = result.early_samples + result.fade_samples;
        for band in [0, result.vn_bands.len() - 1] {
            let target = bank::filter(&ir.frames, rate, band);
            let actual = bank::filter(candidate, rate, band);
            let power = |f: &[[f64; 2]]| f[late..].iter().map(energy).sum::<f64>();
            assert!(power(&actual) / power(&target) > 0.3);
        }
    }

    #[test]
    fn screened_band_decay_controls_db_not_t60_multipliers() {
        let rate = 16000;
        let ir = Impulse {
            sample_rate: rate,
            channels: 1,
            bits: 32,
            frames: (0..rate * 3)
                .map(|i| {
                    let t = f64::from(i) / f64::from(rate);
                    let sample = [(250.0, 1.2), (2000.0, 0.4)]
                        .into_iter()
                        .map(|(hz, decay)| {
                            (std::f64::consts::TAU * hz * t).sin() * 10.0_f64.powf(-3.0 * t / decay)
                        })
                        .sum();
                    [sample; 2]
                })
                .collect(),
        };
        let bands = band_damping(&ir, 1.5);
        assert!((bands[0].t60.unwrap() - 1.2).abs() < 0.08, "{bands:?}");
        assert!((bands[1].t60.unwrap() - 0.4).abs() < 0.08, "{bands:?}");
        assert!(bands[1].attenuation_db > bands[0].attenuation_db);
        assert!(bands[0].attenuation_db > 0.0);
        assert_eq!(band_damping(&ir, 0.1).map(|b| b.attenuation_db), [0.0; 2]);
        assert!(
            band_damping(&ir, 60.0)
                .map(|b| b.attenuation_db)
                .iter()
                .all(|db| (0.0..=24.0).contains(db))
        );

        // Verify actual rendered FDN decay shortens in the damped mid band.
        let render = |damping: [f32; 2]| {
            let mut fdn = Reverb::new(Config {
                sample_rate: rate,
                t60_seconds: 1.5,
                damping_hz: 6000.0,
                matrix: Matrix::Hybrid,
            })
            .unwrap();
            fdn.set_diffusion(1.0).unwrap();
            fdn.set_loop_diffusion(1.0).unwrap();
            fdn.set_band_damping(damping[0], damping[1]).unwrap();
            fdn.set_damping_shape(500.0, 2000.0, 0.7).unwrap();
            fdn.reset();
            let frames: Vec<_> = (0..rate * 3)
                .map(|i| {
                    fdn.process_sample(if i == 0 { 1.0 } else { 0.0 })
                        .map(f64::from)
                })
                .collect();
            screened_t60(&octave(&frames, rate, 2000.0), rate).unwrap()
        };
        assert!(render(bands.map(|b| b.attenuation_db)) < render([0.0; 2]) * 0.9);
    }

    #[test]
    fn failed_band_fits_bypass_and_attenuation_is_bounded() {
        let mut ir = reference();
        ir.frames.fill([0.0; 2]);
        assert!(
            band_damping(&ir, 1.0)
                .iter()
                .all(|b| b.t60.is_none() && b.attenuation_db == 0.0)
        );
        ir.frames.fill([0.5; 2]);
        assert!(
            band_damping(&ir, 1.0)
                .iter()
                .all(|b| b.t60.is_none() && b.attenuation_db == 0.0)
        );
    }

    #[test]
    fn envelope_gains_are_bounded_for_tiny_and_silent_cells() {
        let target = vec![[1.0; 2]; 800];
        for level in [0.0, 1e-150, 1e-10] {
            let mut carrier = vec![[level; 2]; target.len()];
            envelope_match(&mut carrier, &target, 8000);
            assert!(
                carrier
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && v.abs() <= level * 1000.0 * (1.0 + 1e-12))
            );
        }
    }
    pub(super) fn reference() -> Impulse {
        Impulse {
            sample_rate: 8000,
            channels: 2,
            bits: 32,
            frames: (0..16000)
                .map(|i| {
                    let a = 10.0_f64.powf(-3.0 * f64::from(i) / 8000.0);
                    [a, -a * 0.7]
                })
                .collect(),
        }
    }
    #[test]
    fn deterministic_finite_same_length_and_suffix_energy_matched() {
        let ir = reference();
        let a = synthesize(&ir, SynthesisOptions::default()).unwrap();
        let b = synthesize(&ir, SynthesisOptions::default()).unwrap();
        let late = a.early_samples + a.fade_samples;
        assert_eq!(a.candidates.len(), 6);
        assert_eq!(a.candidates[4].name, "early_hybrid");
        let wanted = ir.frames[late..].iter().map(energy).sum::<f64>();
        for (x, y) in a.candidates.iter().zip(&b.candidates) {
            assert_eq!(x.frames, y.frames);
            assert_eq!(x.frames.len(), ir.frames.len());
            assert_eq!(&x.frames[..a.early_samples], &ir.frames[..a.early_samples]);
            assert!(x.frames.iter().flatten().all(|v| v.is_finite()));
            let actual = x.frames[late..].iter().map(energy).sum::<f64>();
            assert!((actual / wanted - 1.0).abs() < 1e-12);
        }
        let changed = synthesize(
            &ir,
            SynthesisOptions {
                seed: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.candidates[0].frames, changed.candidates[0].frames);
        assert_ne!(a.candidates[1].frames, changed.candidates[1].frames);
        assert_ne!(a.candidates[2].frames, changed.candidates[2].frames);
        assert_ne!(a.candidates[4].frames, changed.candidates[4].frames);
    }

    #[test]
    fn hybrid_uses_vn_at_start_fdn_at_end_and_shared_stereo_weights() {
        let mut ir = reference();
        ir.frames.fill([1.0, -0.5]);
        let vn = vec![[2.0, -1.0]; ir.frames.len()];
        let fdn = vec![[-4.0, 2.0]; ir.frames.len()];
        let late = 800;
        let result = hybrid_frames(&ir, &vn, &fdn, late);
        assert_eq!(result[late], [1.0, -0.5]);
        assert_eq!(result[late + 800], [1.0, -0.5]);
        assert_eq!(result[late + 4000], [-1.0, 0.5]);
        assert!(result.iter().all(|f| f[0] == -2.0 * f[1]));
        let mut silent = ir;
        silent.frames.fill([0.0; 2]);
        assert!(
            hybrid_frames(&silent, &silent.frames, &silent.frames, late)
                .iter()
                .flatten()
                .all(|v| *v == 0.0)
        );
    }
    #[test]
    fn mono_and_zero_early_work_at_multiple_rates() {
        for rate in [8000, 44100, 96000] {
            let ir = Impulse {
                sample_rate: rate,
                channels: 1,
                bits: 32,
                frames: (0..rate / 3)
                    .map(|i| [10.0_f64.powf(-3.0 * f64::from(i) / f64::from(rate)); 2])
                    .collect(),
            };
            let result = synthesize(
                &ir,
                SynthesisOptions {
                    early_ms: 0.0,
                    fade_ms: 10.0,
                    t60: Some(1.0),
                    ..Default::default()
                },
            )
            .unwrap();
            for candidate in result.candidates {
                assert!(
                    candidate
                        .frames
                        .iter()
                        .all(|f| f[0] == f[1] && f[0].is_finite())
                );
            }
        }
    }
    #[test]
    fn rejects_invalid_options_silence_and_short_ir() {
        for options in [
            SynthesisOptions {
                early_ms: f64::NAN,
                ..Default::default()
            },
            SynthesisOptions {
                fade_ms: 0.0,
                ..Default::default()
            },
            SynthesisOptions {
                t60: Some(0.0),
                ..Default::default()
            },
            SynthesisOptions {
                density: f64::NAN,
                ..Default::default()
            },
            SynthesisOptions {
                density: 0.0,
                ..Default::default()
            },
            SynthesisOptions {
                density_rise_ms: -1.0,
                ..Default::default()
            },
        ] {
            assert!(synthesize(&reference(), options).is_err());
        }
        let mut ir = reference();
        ir.frames.fill([0.0; 2]);
        assert!(synthesize(&ir, Default::default()).is_err());
        ir.frames.truncate(10);
        assert!(synthesize(&ir, Default::default()).is_err());
    }
}
