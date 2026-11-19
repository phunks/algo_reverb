//! Synthetic single-instance DSP timing; not a DAW real-time certification.
//! Run: cargo run -p algo_vn --release --example convolution_bench
#[allow(dead_code)]
#[path = "../src/convolution.rs"]
mod legacy;
#[allow(dead_code, unused_imports)]
#[path = "../src/zero_convolution.rs"]
mod zero;

use std::{hint::black_box, time::Instant};

fn measure(name: &str, mut process: impl FnMut([f32; 2]), block: usize) {
    let rate = 88200;
    for _ in 0..8192 {
        process([0.1, -0.03]);
    }
    let mut durations = Vec::new();
    let mut sample = 0;
    for _ in 0..(rate * 2 / block) {
        let start = Instant::now();
        for _ in 0..block {
            sample += 1;
            let x = (sample as f32 * 0.07).sin() * 0.1;
            process(black_box([x, x * 0.3]));
        }
        durations.push(start.elapsed().as_secs_f64());
    }
    let deadline = block as f64 / rate as f64;
    let mean = durations.iter().sum::<f64>() / durations.len() as f64;
    let misses = durations.iter().filter(|&&t| t > deadline).count();
    durations.sort_by(f64::total_cmp);
    println!(
        "{name}, block={block}, mean_cpu={:.2}%, p99={:.0}us, max={:.0}us, deadline={:.0}us, over_budget={misses}/{}",
        mean / deadline * 100.0,
        durations[durations.len() * 99 / 100] * 1e6,
        durations.last().unwrap() * 1e6,
        deadline * 1e6,
        durations.len()
    );
}

fn main() {
    println!("88.2 kHz, synthetic dense stereo IR, mono excitation, one instance, DSP only");
    for seconds in [1, 5, 30] {
        let ir: Vec<_> = (0..88200 * seconds)
            .map(|i| {
                let envelope = (-6.9 * i as f64 / (88200 * seconds) as f64).exp() * 0.01;
                [
                    (i as f64 * 0.71).sin() * envelope,
                    (i as f64 * 0.33).cos() * envelope,
                ]
            })
            .collect();
        for block in [32, 64, 128, 256] {
            let mut engine = zero::Convolver::new(&ir, 88200).unwrap();
            measure(
                &format!("zero IR={seconds}s"),
                |x| {
                    black_box(engine.process(x));
                },
                block,
            );
            let mut engine = legacy::Convolver::new(&ir, 88200).unwrap();
            measure(
                &format!("legacy IR={seconds}s"),
                |x| {
                    black_box(engine.process(x));
                },
                block,
            );
        }
    }
}
