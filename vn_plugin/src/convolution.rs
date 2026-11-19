//! Uniform partitioned overlap-add. Mono excitation, stereo IR, fixed block latency.
//! All buffers and FFT coefficients are constructed off the audio thread.
use std::f32::consts::TAU;

pub const BLOCK: usize = 1024;
const FFT: usize = BLOCK * 2;

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}
impl Complex {
    fn mul(self, b: Self) -> Self {
        Self {
            re: self.re * b.re - self.im * b.im,
            im: self.re * b.im + self.im * b.re,
        }
    }
}

struct Plan {
    roots: Vec<Complex>,
    reverse: Vec<usize>,
}
impl Plan {
    fn new() -> Self {
        Self {
            roots: (0..FFT / 2)
                .map(|i| {
                    let phase = -TAU * i as f32 / FFT as f32;
                    Complex {
                        re: phase.cos(),
                        im: phase.sin(),
                    }
                })
                .collect(),
            reverse: (0..FFT)
                .map(|i| i.reverse_bits() >> (usize::BITS - FFT.ilog2()))
                .collect(),
        }
    }
    fn transform(&self, data: &mut [Complex], inverse: bool) {
        for (i, &j) in self.reverse.iter().enumerate() {
            if j > i {
                data.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= FFT {
            for start in (0..FFT).step_by(size) {
                for i in 0..size / 2 {
                    let mut root = self.roots[i * FFT / size];
                    if inverse {
                        root.im = -root.im;
                    }
                    let a = data[start + i];
                    let b = data[start + i + size / 2].mul(root);
                    data[start + i] = Complex {
                        re: a.re + b.re,
                        im: a.im + b.im,
                    };
                    data[start + i + size / 2] = Complex {
                        re: a.re - b.re,
                        im: a.im - b.im,
                    };
                }
            }
            size *= 2;
        }
        if inverse {
            for value in data {
                value.re /= FFT as f32;
                value.im /= FFT as f32;
            }
        }
    }
}

pub struct Convolver {
    pub calibration_gain: f32,
    plan: Plan,
    kernels: [Vec<Complex>; 2],
    history: Vec<Complex>,
    work: Vec<Complex>,
    overlap: [[f32; BLOCK]; 2],
    output: [[f32; BLOCK]; 2],
    input: [f32; BLOCK],
    dry: [[f32; 2]; BLOCK],
    cursor: usize,
    head: usize,
    partitions: usize,
    pub rate: u32,
    pub frames: usize,
}
impl Convolver {
    pub fn new(ir: &[[f64; 2]], rate: u32) -> Result<Self, String> {
        if !(8000..=192000).contains(&rate)
            || ir.is_empty()
            || ir.len() > rate as usize * 30
            || ir.iter().flatten().any(|x| !x.is_finite() || x.abs() > 1e6)
        {
            return Err(
                "Expected finite nonempty IR, at most 30 s, host rate 8–192 kHz, samples <= 1e6"
                    .into(),
            );
        }
        let plan = Plan::new();
        let partitions = ir.len().div_ceil(BLOCK);
        let mut kernels = std::array::from_fn(|_| vec![Complex::default(); partitions * FFT]);
        for (ch, kernel) in kernels.iter_mut().enumerate() {
            for (index, partition) in kernel.as_chunks_mut::<FFT>().0.iter_mut().enumerate() {
                for (out, frame) in partition
                    .iter_mut()
                    .zip(ir.iter().skip(index * BLOCK).take(BLOCK))
                {
                    out.re = frame[ch] as f32;
                }
                plan.transform(partition, false);
            }
        }
        Ok(Self {
            calibration_gain: 1.0,
            plan,
            kernels,
            history: vec![Complex::default(); partitions * FFT],
            work: vec![Complex::default(); FFT],
            overlap: [[0.0; BLOCK]; 2],
            output: [[0.0; BLOCK]; 2],
            input: [0.0; BLOCK],
            dry: [[0.0; 2]; BLOCK],
            cursor: 0,
            head: 0,
            partitions,
            rate,
            frames: ir.len(),
        })
    }
    pub fn reset(&mut self) {
        self.history.fill(Complex::default());
        self.work.fill(Complex::default());
        self.overlap.fill([0.0; BLOCK]);
        self.output.fill([0.0; BLOCK]);
        self.input.fill(0.0);
        self.dry.fill([0.0; 2]);
        self.cursor = 0;
        self.head = 0;
    }
    /// Returns (latency-aligned stereo dry, stereo wet). No allocation or locks.
    pub fn process(&mut self, frame: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let frame = frame.map(|v| if v.is_finite() { v } else { 0.0 });
        let dry = self.dry[self.cursor];
        self.dry[self.cursor] = frame;
        let wet = [self.output[0][self.cursor], self.output[1][self.cursor]];
        self.input[self.cursor] = frame[0] * 0.5 + frame[1] * 0.5;
        self.cursor += 1;
        if self.cursor == BLOCK {
            self.cursor = 0;
            self.render();
        }
        (dry, wet)
    }
    fn render(&mut self) {
        let current = &mut self.history[self.head * FFT..(self.head + 1) * FFT];
        current.fill(Complex::default());
        for (out, &v) in current.iter_mut().zip(&self.input) {
            out.re = v;
        }
        self.plan.transform(current, false);
        for ch in 0..2 {
            self.work.fill(Complex::default());
            for partition in 0..self.partitions {
                let offset = (self.head + self.partitions - partition) % self.partitions * FFT;
                let kernel = &self.kernels[ch][partition * FFT..(partition + 1) * FFT];
                for ((sum, &x), &h) in self
                    .work
                    .iter_mut()
                    .zip(&self.history[offset..offset + FFT])
                    .zip(kernel)
                {
                    let v = x.mul(h);
                    sum.re += v.re;
                    sum.im += v.im;
                }
            }
            self.plan.transform(&mut self.work, true);
            for i in 0..BLOCK {
                self.output[ch][i] = self.work[i].re + self.overlap[ch][i];
                self.overlap[ch][i] = self.work[i + BLOCK].re;
            }
        }
        self.head = (self.head + 1) % self.partitions;
    }
}

/// Windowed-sinc resampling of a convolution kernel, including dt gain correction.
/// Same-rate data are copied exactly. No normalization or final fade is applied.
pub fn resample(ir: &[[f64; 2]], source: u32, target: u32) -> Result<Vec<[f64; 2]>, String> {
    if !(8000..=384000).contains(&source)
        || !(8000..=192000).contains(&target)
        || ir.is_empty()
        || ir.len() > source as usize * 30
    {
        return Err("IR must be <=30 s; source 8–384 kHz, host 8–192 kHz".into());
    }
    if source == target {
        return Ok(ir.to_vec());
    }
    let ratio = f64::from(source) / f64::from(target);
    let cutoff = (1.0 / ratio).min(1.0) * 0.94;
    let radius = (32.0 / cutoff).ceil() as isize;
    let len = (ir.len() as f64 / ratio).ceil() as usize;
    let mut output = vec![[0.0; 2]; len];
    for (i, out) in output.iter_mut().enumerate() {
        let time = i as f64 * ratio;
        for j in time.floor() as isize - radius..=time.floor() as isize + radius {
            if j < 0 || j >= ir.len() as isize {
                continue;
            }
            let d = time - j as f64;
            if d.abs() >= radius as f64 {
                continue;
            }
            let x = std::f64::consts::PI * d * cutoff;
            let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
            let window = 0.5 + 0.5 * (std::f64::consts::PI * d / radius as f64).cos();
            let weight = ratio * cutoff * sinc * window;
            for ch in 0..2 {
                out[ch] += ir[j as usize][ch] * weight;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hot_convolution_remains_linear_above_full_scale() {
        // A unity-peak IR can still produce >1 when delayed contributions add.
        // Preserve headroom here: Output attenuation is applied by the plugin.
        let ir = vec![[1.0, -0.5]; 32];
        let mut loud = Convolver::new(&ir, 48000).unwrap();
        let mut quiet = Convolver::new(&ir, 48000).unwrap();
        let mut peak = 0.0_f32;
        for i in 0..BLOCK * 3 {
            let input = if i < 64 { 0.5 } else { 0.0 };
            let (_, a) = loud.process([input; 2]);
            let (_, b) = quiet.process([input * 0.125; 2]);
            for ch in 0..2 {
                assert!(a[ch].is_finite());
                assert!((a[ch] * 0.125 - b[ch]).abs() < 1e-5);
                peak = peak.max(a[ch].abs());
            }
        }
        assert!(peak > 15.9, "convolution must not clip at full scale");
    }

    #[test]
    fn matches_direct_convolution_across_partitions_and_reset() {
        let ir: Vec<_> = (0..BLOCK * 3 + 17)
            .map(|i| {
                [
                    (i as f64 * 0.17).sin() * 0.01,
                    (i as f64 * 0.11).cos() * 0.007,
                ]
            })
            .collect();
        let input: Vec<_> = (0..BLOCK + 31)
            .map(|i| (i as f32 * 0.071).sin() * 0.2)
            .collect();
        let mut engine = Convolver::new(&ir, 48000).unwrap();
        for run in 0..2 {
            engine.reset();
            for i in 0..input.len() + ir.len() + BLOCK {
                let x = input.get(i).copied().unwrap_or(0.0);
                let (dry, wet) = engine.process([x; 2]);
                let delayed = i.checked_sub(BLOCK);
                assert_eq!(
                    dry,
                    [delayed.and_then(|j| input.get(j)).copied().unwrap_or(0.0); 2]
                );
                for ch in 0..2 {
                    let expected = delayed.map_or(0.0, |j| {
                        (0..input.len().min(j + 1))
                            .filter(|&k| j - k < ir.len())
                            .map(|k| f64::from(input[k]) * ir[j - k][ch])
                            .sum::<f64>()
                    });
                    assert!(
                        (f64::from(wet[ch]) - expected).abs() < 2e-5,
                        "run={run} sample={i}"
                    );
                }
            }
        }
    }
    #[test]
    fn impulse_reproduces_ir_and_sanitizes_input() {
        let ir = vec![[0.3, -0.7], [0.1, 0.4]];
        let mut engine = Convolver::new(&ir, 44100).unwrap();
        for i in 0..BLOCK * 3 {
            let (_, wet) = engine.process(if i == 0 {
                [1.0; 2]
            } else {
                [f32::NAN, f32::INFINITY]
            });
            let expected = i
                .checked_sub(BLOCK)
                .and_then(|j| ir.get(j))
                .copied()
                .unwrap_or([0.0; 2]);
            for ch in 0..2 {
                assert!((f64::from(wet[ch]) - expected[ch]).abs() < 1e-6);
            }
        }
        assert!(Convolver::new(&[], 48000).is_err());
        assert!(Convolver::new(&[[f64::NAN; 2]], 48000).is_err());
    }
    #[test]
    fn resampling_preserves_duration_and_interior_kernel_dc_gain() {
        let mut ir = vec![[0.0; 2]; 4800];
        ir[2400] = [1.0, -0.5];
        assert_eq!(resample(&ir, 48000, 48000).unwrap(), ir);
        for target in [8000, 44100, 96000, 192000] {
            let out = resample(&ir, 48000, target).unwrap();
            assert_eq!(out.len(), target as usize / 10);
            let sum: f64 = out.iter().map(|f| f[0]).sum();
            assert!((sum - 1.0).abs() < 0.002, "{target}: {sum}");
            assert!(out.iter().flatten().all(|v| v.is_finite()));
        }
    }
}
