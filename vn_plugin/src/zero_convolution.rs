//! Algo VN-only zero-added-latency mono-to-stereo convolution.
//! Direct FIR [0,128), then FFT stages [128,256), [256,512),
//! [512,1024), [1024,2048), [2048,end). All preparation is off-thread.
//! FFT work is synchronous; allocation-free does not imply a deadline guarantee.
use std::f32::consts::TAU;

pub const BLOCK: usize = 128;
pub const LATENCY: u32 = 0;
const TAIL_BLOCK: usize = 2048;

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
    fft: usize,
}
impl Plan {
    fn new(fft: usize) -> Self {
        Self {
            fft,
            roots: (0..fft / 2)
                .map(|i| {
                    let phase = -TAU * i as f32 / fft as f32;
                    Complex {
                        re: phase.cos(),
                        im: phase.sin(),
                    }
                })
                .collect(),
            reverse: (0..fft)
                .map(|i| i.reverse_bits() >> (usize::BITS - fft.ilog2()))
                .collect(),
        }
    }
    fn transform(&self, data: &mut [Complex], inverse: bool) {
        let fft = self.fft;
        for (i, &j) in self.reverse.iter().enumerate() {
            if j > i {
                data.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= fft {
            for start in (0..fft).step_by(size) {
                for i in 0..size / 2 {
                    let mut root = self.roots[i * fft / size];
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
                value.re /= fft as f32;
                value.im /= fft as f32;
            }
        }
    }
}

struct Partitioned {
    plan: Plan,
    kernels: [Vec<Complex>; 2],
    history: Vec<Complex>,
    work: Vec<Complex>,
    overlap: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    input: Vec<f32>,
    cursor: usize,
    head: usize,
    partitions: usize,
    block: usize,
    fft: usize,
}
impl Partitioned {
    // This stage's coefficients begin at IR offset `block`. Its ordinary
    // overlap-add latency is exactly that offset: no extra output delay.
    fn new(ir: &[[f64; 2]], block: usize) -> Self {
        let fft = block * 2;
        let plan = Plan::new(fft);
        let partitions = ir.len().div_ceil(block);
        let mut kernels = std::array::from_fn(|_| vec![Complex::default(); partitions * fft]);
        for (ch, kernel) in kernels.iter_mut().enumerate() {
            for (index, partition) in kernel.chunks_exact_mut(fft).enumerate() {
                for (out, frame) in partition
                    .iter_mut()
                    .zip(ir.iter().skip(index * block).take(block))
                {
                    out.re = frame[ch] as f32;
                }
                plan.transform(partition, false);
            }
        }
        Self {
            plan,
            kernels,
            history: vec![Complex::default(); partitions * fft],
            work: vec![Complex::default(); fft],
            overlap: std::array::from_fn(|_| vec![0.0; block]),
            output: std::array::from_fn(|_| vec![0.0; block]),
            input: vec![0.0; block],
            cursor: 0,
            head: 0,
            partitions,
            block,
            fft,
        }
    }
    fn reset(&mut self) {
        self.history.fill(Complex::default());
        self.work.fill(Complex::default());
        for ch in 0..2 {
            self.overlap[ch].fill(0.0);
            self.output[ch].fill(0.0);
        }
        self.input.fill(0.0);
        self.cursor = 0;
        self.head = 0;
    }
    fn process(&mut self, input: f32) -> [f32; 2] {
        let wet = [self.output[0][self.cursor], self.output[1][self.cursor]];
        self.input[self.cursor] = input;
        self.cursor += 1;
        if self.cursor == self.block {
            self.cursor = 0;
            self.render();
        }
        wet
    }
    fn render(&mut self) {
        let current = &mut self.history[self.head * self.fft..(self.head + 1) * self.fft];
        current.fill(Complex::default());
        for (out, &v) in current.iter_mut().zip(&self.input) {
            out.re = v;
        }
        self.plan.transform(current, false);
        for ch in 0..2 {
            self.work.fill(Complex::default());
            for partition in 0..self.partitions {
                let offset = (self.head + self.partitions - partition) % self.partitions * self.fft;
                let kernel = &self.kernels[ch][partition * self.fft..(partition + 1) * self.fft];
                for ((sum, &x), &h) in self
                    .work
                    .iter_mut()
                    .zip(&self.history[offset..offset + self.fft])
                    .zip(kernel)
                {
                    let v = x.mul(h);
                    sum.re += v.re;
                    sum.im += v.im;
                }
            }
            self.plan.transform(&mut self.work, true);
            for i in 0..self.block {
                self.output[ch][i] = self.work[i].re + self.overlap[ch][i];
                self.overlap[ch][i] = self.work[i + self.block].re;
            }
        }
        self.head = (self.head + 1) % self.partitions;
    }
}

pub struct Convolver {
    pub calibration_gain: f32,
    pub rate: u32,
    pub frames: usize,
    head: Vec<[f32; 2]>,
    input: [f32; BLOCK],
    cursor: usize,
    stages: Vec<Partitioned>,
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
        let mut stages = Vec::new();
        let mut block = BLOCK;
        while block < ir.len() {
            let end = if block == TAIL_BLOCK {
                ir.len()
            } else {
                (block * 2).min(ir.len())
            };
            stages.push(Partitioned::new(&ir[block..end], block));
            if block == TAIL_BLOCK {
                break;
            }
            block *= 2;
        }
        Ok(Self {
            calibration_gain: 1.0,
            rate,
            frames: ir.len(),
            head: ir[..ir.len().min(BLOCK)]
                .iter()
                .map(|f| f.map(|x| x as f32))
                .collect(),
            input: [0.0; BLOCK],
            cursor: 0,
            stages,
        })
    }
    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.cursor = 0;
        for stage in &mut self.stages {
            stage.reset();
        }
    }
    /// Returns undelayed stereo dry and mono-excited stereo wet. No allocation/locks.
    pub fn process(&mut self, frame: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let dry = frame.map(|x| if x.is_finite() { x } else { 0.0 });
        let input = dry[0] * 0.5 + dry[1] * 0.5;
        self.input[self.cursor] = input;
        let mut wet = [0.0; 2];
        // Walk the two contiguous ring slices, avoiding per-tap modulo.
        let recent = self.input[..=self.cursor]
            .iter()
            .rev()
            .chain(self.input[self.cursor + 1..].iter().rev());
        for (h, &x) in self.head.iter().zip(recent) {
            wet[0] += h[0] * x;
            wet[1] += h[1] * x;
        }
        self.cursor = (self.cursor + 1) % BLOCK;
        for stage in &mut self.stages {
            let tail = stage.process(input);
            wet[0] += tail[0];
            wet[1] += tail[1];
        }
        (dry, wet)
    }
}

// Keep WAV rate conversion identical to the integrated/legacy engine.
#[path = "resample.rs"]
mod resampling;
pub use resampling::resample;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_tail_and_reset_at_arbitrary_positions() {
        assert_eq!(LATENCY, 0);
        for len in [
            1, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1023, 1024, 1025, 2047, 2048, 2049,
            4096, 8209,
        ] {
            let ir: Vec<_> = (0..len)
                .map(|i| [(i as f64 * 0.71).sin() * 0.1, (i as f64 * 0.33).cos() * 0.2])
                .collect();
            let mut engine = Convolver::new(&ir, 88200).unwrap();
            // Invalidate partial direct and FFT blocks before each impulse run.
            for offset in [0, 1, 127, 513, 2047, 4139] {
                for _ in 0..offset {
                    engine.process([0.2, -0.1]);
                }
                engine.reset();
                for i in 0..len + 4096 {
                    let x = if i == 0 { [2.0, 0.0] } else { [0.0; 2] };
                    let (dry, wet) = engine.process(x);
                    assert_eq!(dry, x);
                    let expected = ir.get(i).copied().unwrap_or_default();
                    for ch in 0..2 {
                        assert!(
                            (f64::from(wet[ch]) - expected[ch]).abs() < 2e-6,
                            "len={len} offset={offset} sample={i} ch={ch}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn random_stereo_matches_direct_across_all_stages_and_host_blocks() {
        let mut seed = 123456789_u64;
        let mut random = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed as i64 as f64 / i64::MAX as f64) * 0.1
        };
        let ir: Vec<_> = (0..8317).map(|_| [random(), random()]).collect();
        let input: Vec<_> = (0..2351)
            .map(|_| [random() as f32, random() as f32])
            .collect();
        let mut engine = Convolver::new(&ir, 88200).unwrap();
        let mut position = 0;
        let end = input.len() + ir.len() + 2048;
        // Calls model arbitrary/changing host block sizes, including non-powers of two.
        for block in [1, 7, 32, 63, 128, 257, 511].into_iter().cycle() {
            for i in position..(position + block).min(end) {
                let frame = input.get(i).copied().unwrap_or_default();
                let (dry, wet) = engine.process(frame);
                assert_eq!(dry, frame);
                for ch in 0..2 {
                    let expected: f64 = (0..input.len().min(i + 1))
                        .filter(|&k| i - k < ir.len())
                        .map(|k| f64::from(input[k][0] * 0.5 + input[k][1] * 0.5) * ir[i - k][ch])
                        .sum();
                    assert!(
                        (f64::from(wet[ch]) - expected).abs() < 3e-6,
                        "sample={i} ch={ch} wet={} expected={expected}",
                        wet[ch]
                    );
                }
            }
            position = (position + block).min(end);
            if position == end {
                break;
            }
        }
        engine.reset();
        for _ in 0..8192 {
            assert_eq!(engine.process([0.5, -0.5]), ([0.5, -0.5], [0.0; 2]));
        }
    }

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
                let delayed = Some(i);
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
            let expected = ir.get(i).copied().unwrap_or([0.0; 2]);
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
