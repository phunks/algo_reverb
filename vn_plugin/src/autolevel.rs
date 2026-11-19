//! Off-audio-thread periodic pink RMS calibration, v1.
//! Parseval gives the same RMS as the offline random-phase probe, without
//! synthesizing noise: phases cancel over one complete steady-state period.
use std::f64::consts::TAU;

pub fn gain(ir: &[[f64; 2]], rate: u32) -> f32 {
    let n = (rate as usize * 8).max(ir.len() * 2).next_power_of_two();
    let mut work = vec![[0.0_f64; 2]; n];
    let mut weighted_power = 0.0;
    let mut weights = 0.0;
    for ch in 0..2 {
        work.fill([0.0; 2]);
        for (out, frame) in work.iter_mut().zip(ir) {
            out[0] = frame[ch];
        }
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - n.ilog2());
            if j > i {
                work.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            for start in (0..n).step_by(size) {
                for i in 0..size / 2 {
                    let phase = -TAU * i as f64 / size as f64;
                    let (sin, cos) = phase.sin_cos();
                    let a = work[start + i];
                    let b = work[start + i + size / 2];
                    let b = [b[0] * cos - b[1] * sin, b[0] * sin + b[1] * cos];
                    work[start + i] = [a[0] + b[0], a[1] + b[1]];
                    work[start + i + size / 2] = [a[0] - b[0], a[1] - b[1]];
                }
            }
            size *= 2;
        }
        for (k, bin) in work.iter().enumerate().take(n / 2).skip(1) {
            let hz = k as f64 * rate as f64 / n as f64;
            if hz >= 20.0 && hz <= 20000.0_f64.min(rate as f64 * 0.45) {
                weighted_power += (bin[0] * bin[0] + bin[1] * bin[1]) / hz;
                weights += 1.0 / hz;
            }
        }
    }
    let power = weighted_power / weights;
    // No attempt to rescue silent/extremely quiet IRs (transfer below -120 dB).
    if !power.is_finite() || power <= 1e-12 {
        return 1.0;
    }
    let db = (-10.0 * power.log10()).clamp(-24.0, 24.0);
    10.0_f64.powf(db / 20.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_delay_polarity_and_stereo_power() {
        let mut ir = vec![[0.0; 2]; 32];
        ir[17] = [2.0, -2.0];
        assert!((gain(&ir, 8000) - 0.5).abs() < 1e-6);
        ir[17] = [2.0, 0.0];
        assert!((gain(&ir, 8000) - 0.5_f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn silence_tiny_and_limits() {
        assert_eq!(gain(&[[0.0; 2]], 8000), 1.0);
        assert_eq!(gain(&[[1e-9; 2]], 8000), 1.0);
        assert!((gain(&[[1e-4; 2]], 8000) - 10.0_f32.powf(24.0 / 20.0)).abs() < 1e-5);
        assert!((gain(&[[100.0; 2]], 8000) - 10.0_f32.powf(-24.0 / 20.0)).abs() < 1e-6);
    }

    #[test]
    fn matches_analytic_colored_response_on_probe_grid() {
        // IR = [1, 1] on both channels: |H(f)|² = 2 + 2*cos(w).
        let n = 65536; // production eight-second grid
        let mut sum = 0.0;
        let mut weights = 0.0;
        for k in 1..n / 2 {
            let hz = k as f64 * 8000.0 / n as f64;
            if (20.0..=3600.0).contains(&hz) {
                sum += (2.0 + 2.0 * (TAU * hz / 8000.0).cos()) / hz;
                weights += 1.0 / hz;
            }
        }
        assert!(
            (gain(&[[1.0; 2], [1.0; 2]], 8000) as f64 - (sum / weights).sqrt().recip()).abs()
                < 1e-6
        );
    }

    #[test]
    fn calibrated_convolver_matches_input_rms_after_warmup() {
        use super::super::convolution::{BLOCK, Convolver};
        let mut ir = vec![[0.0; 2]; 32];
        ir[17] = [2.0, -2.0];
        let mut engine = Convolver::new(&ir, 8000).unwrap();
        engine.calibration_gain = gain(&ir, 8000);
        let mut input_power = 0.0_f64;
        let mut wet_power = 0.0_f64;
        for i in 0..(BLOCK * 2 + 8000) {
            let input = (TAU * 200.0 * i as f64 / 8000.0).sin() as f32 * 0.1;
            let (_, wet) = engine.process([input; 2]);
            if i >= BLOCK * 2 {
                input_power += (input as f64).powi(2);
                wet_power += wet
                    .iter()
                    .map(|x| (*x as f64 * engine.calibration_gain as f64).powi(2))
                    .sum::<f64>()
                    / 2.0;
            }
        }
        assert!((10.0 * (wet_power / input_power).log10()).abs() < 0.001);
    }
}
