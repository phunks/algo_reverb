//! Offline, exploratory IR analysis, not an ISO 3382 / IEC 61260 measurement.
//! No resampling, normalization, clipping, DC removal, or noise compensation.
use std::path::Path;

pub struct Impulse {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub frames: Vec<[f64; 2]>,
}

impl Impulse {
    /// Mono is duplicated for energy calculations, but has no stereo correlation.
    /// A 60-second limit bounds offline memory use. Float samples above unity survive.
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        let spec = reader.spec();
        let count = reader.duration() as usize;
        if !(1..=2).contains(&spec.channels)
            || !(8000..=384000).contains(&spec.sample_rate)
            || count == 0
            || count > spec.sample_rate as usize * 60
        {
            return Err(
                "Expected nonempty mono/stereo WAV, 8000–384000 Hz, at most 60 seconds".into(),
            );
        }
        let samples: Vec<f64> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .map(|v| v.map(f64::from))
                .collect::<Result<_, _>>(),
            hound::SampleFormat::Int => {
                if !(1..=32).contains(&spec.bits_per_sample) {
                    return Err("Unsupported PCM bit depth".into());
                }
                let scale = 2.0_f64.powi(i32::from(spec.bits_per_sample) - 1);
                reader
                    .samples::<i32>()
                    .map(|v| v.map(|v| f64::from(v) / scale))
                    .collect::<Result<_, _>>()
            }
        }
        .map_err(|e| e.to_string())?;
        if samples.len() != count * usize::from(spec.channels)
            || samples.iter().any(|v| !v.is_finite())
        {
            return Err("Incomplete frames or non-finite WAV samples".into());
        }
        let frames = samples
            .chunks_exact(usize::from(spec.channels))
            .map(|f| [f[0], *f.get(1).unwrap_or(&f[0])])
            .collect();
        Ok(Self {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
            bits: spec.bits_per_sample,
            frames,
        })
    }
}

/// CSV uses blank fields for undefined values, never artificial finite dB floors.
pub fn db(energy: f64) -> Option<f64> {
    (energy > 0.0).then(|| 10.0 * energy.log10())
}

pub fn energy(frame: &[f64; 2]) -> f64 {
    (frame[0] * frame[0] + frame[1] * frame[1]) * 0.5
}

/// Normalized, backwards-integrated sum of L² and R², not (L+R)².
pub fn edc(frames: &[[f64; 2]]) -> Vec<f64> {
    let mut remaining = 0.0;
    let mut curve: Vec<_> = frames
        .iter()
        .rev()
        .map(|f| {
            remaining += energy(f);
            remaining
        })
        .collect();
    curve.reverse();
    if remaining > 0.0 {
        for value in &mut curve {
            *value /= remaining;
        }
    }
    curve
}

/// Zero-lag, mean-subtracted Pearson correlation, not lag-maximized IACC.
pub fn correlation(frames: &[[f64; 2]]) -> Option<f64> {
    if frames.len() < 2 {
        return None;
    }
    let mean = [0, 1].map(|ch| frames.iter().map(|f| f[ch]).sum::<f64>() / frames.len() as f64);
    let (mut ll, mut rr, mut lr) = (0.0, 0.0, 0.0);
    for f in frames {
        let l = f[0] - mean[0];
        let r = f[1] - mean[1];
        ll += l * l;
        rr += r * r;
        lr += l * r;
    }
    (ll > 0.0 && rr > 0.0).then(|| (lr / (ll * rr).sqrt()).clamp(-1.0, 1.0))
}

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}
impl Biquad {
    fn new(rate: u32, hz: f64, highpass: bool) -> Self {
        let w = std::f64::consts::TAU * hz / f64::from(rate);
        let c = w.cos();
        let alpha = w.sin() / 2.0_f64.sqrt();
        let a0 = 1.0 + alpha;
        let b = if highpass {
            [(1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5]
        } else {
            [(1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5]
        };
        Self {
            b: b.map(|v| v / a0),
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Causal second-order Butterworth HP + LP at fc/√2 and fc*√2.
/// Broad, overlapping exploratory bands; no filter-tail padding is appended.
pub fn octave(frames: &[[f64; 2]], rate: u32, center: f64) -> Vec<[f64; 2]> {
    let mut hp = [0, 1].map(|_| Biquad::new(rate, center / 2.0_f64.sqrt(), true));
    let mut lp = [0, 1].map(|_| Biquad::new(rate, center * 2.0_f64.sqrt(), false));
    frames
        .iter()
        .map(|f| std::array::from_fn(|ch| lp[ch].process(hp[ch].process(f[ch]))))
        .collect()
}

pub struct DecayFit {
    /// T60 extrapolated from the selected slope, not a direct -60 dB crossing.
    pub t60: Option<f64>,
    pub r_squared: Option<f64>,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub status: &'static str,
}

/// Fit EDC in dB with conservative tail screening. Tail power is only a proxy:
/// it includes genuine reverb and must not be interpreted as measured noise.
pub fn fit_decay(
    frames: &[[f64; 2]],
    curve: &[f64],
    rate: u32,
    upper: f64,
    lower: f64,
) -> DecayFit {
    let mut result = DecayFit {
        t60: None,
        r_squared: None,
        start_seconds: None,
        end_seconds: None,
        status: "insufficient_range",
    };
    let crossing = |level| {
        curve
            .iter()
            .position(|&v| db(v).is_some_and(|v| v <= level))
    };
    let Some(start) = crossing(upper) else {
        return result;
    };
    let Some(end) = crossing(lower) else {
        return result;
    };
    let min_samples = (rate as usize / 200).max(16);
    if end <= start || end - start < min_samples || curve[end] <= 0.0 {
        return result;
    }
    let duration = f64::from(rate);
    result.start_seconds = Some(start as f64 / duration);
    result.end_seconds = Some(end as f64 / duration);
    let points: Vec<_> = (start..=end)
        .filter_map(|i| db(curve[i]).map(|y| (i as f64 / duration, y)))
        .collect();
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let xx = points.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>();
    let yy = points.iter().map(|p| (p.1 - my).powi(2)).sum::<f64>();
    let xy = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>();
    if xx <= 0.0 || yy <= 0.0 || xy >= 0.0 {
        return result;
    }
    let r2 = (xy * xy / (xx * yy)).clamp(0.0, 1.0);
    result.r_squared = Some(r2);
    let tail_len = (frames.len() / 10).max(1);
    let tail_power = frames[frames.len() - tail_len..]
        .iter()
        .map(energy)
        .sum::<f64>()
        / tail_len as f64;
    let window = (rate as usize / 100).max(1);
    let local_start = end.saturating_sub(window);
    let local_power =
        frames[local_start..=end].iter().map(energy).sum::<f64>() / (end - local_start + 1) as f64;
    if end + (rate as usize / 20) >= frames.len() {
        result.status = "near_file_end";
    } else if local_power <= tail_power * 10.0 {
        result.status = "tail_floor_risk";
    } else if r2 < 0.98 {
        result.status = "nonlinear_decay";
        result.t60 = Some(-60.0 / (xy / xx));
    } else {
        result.status = "reference_only";
        result.t60 = Some(-60.0 / (xy / xx));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exponential_decay_estimates_and_gain_invariance() {
        let rate = 8000;
        let frames: Vec<_> = (0..rate * 3)
            .map(|i| {
                let a = 10.0_f64.powf(-3.0 * f64::from(i) / f64::from(rate));
                [a, -a]
            })
            .collect();
        let curve = edc(&frames);
        assert_eq!(curve[0], 1.0);
        assert!(curve.windows(2).all(|p| p[1] <= p[0]));
        for (upper, lower) in [(0.0, -10.0), (-5.0, -25.0), (-5.0, -35.0)] {
            let fit = fit_decay(&frames, &curve, rate, upper, lower);
            assert_eq!(fit.status, "reference_only");
            assert!((fit.t60.unwrap() - 1.0).abs() < 0.002);
        }
        let scaled: Vec<_> = frames.iter().map(|f| f.map(|v| v * 0.01)).collect();
        for (a, b) in edc(&scaled).iter().zip(curve) {
            assert!((a - b).abs() < 1e-12);
        }
        assert!((correlation(&frames).unwrap() + 1.0).abs() < 1e-12);
    }
    #[test]
    fn silence_impulse_short_tail_and_noise_are_not_confident_fits() {
        for frames in [
            vec![[0.0; 2]; 8000],
            {
                let mut f = vec![[0.0; 2]; 8000];
                f[0] = [1.0; 2];
                f
            },
            vec![[0.5, -0.5]; 8000],
        ] {
            let fit = fit_decay(&frames, &edc(&frames), 8000, -5.0, -35.0);
            assert!(fit.t60.is_none());
        }
        assert!(correlation(&[[0.0; 2]; 10]).is_none());
        let short: Vec<_> = (0..800)
            .map(|i| [10.0_f64.powf(-3.0 * f64::from(i) / 8000.0); 2])
            .collect();
        assert_ne!(
            fit_decay(&short, &edc(&short), 8000, -5.0, -25.0).status,
            "reference_only"
        );
    }
    #[test]
    fn octave_prefers_its_center_and_remains_finite() {
        let tone = |hz: f64| {
            (0..48000)
                .map(|i| [(std::f64::consts::TAU * hz * f64::from(i) / 48000.0).sin(); 2])
                .collect::<Vec<_>>()
        };
        let power = |hz| {
            octave(&tone(hz), 48000, 1000.0)
                .iter()
                .skip(24000)
                .map(energy)
                .sum::<f64>()
        };
        assert!(power(1000.0) > power(100.0) * 100.0);
        assert!(power(1000.0) > power(10000.0) * 100.0);
    }
}
