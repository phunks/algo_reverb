use algo_reverb::{Config, DelayMode, Matrix, Reverb};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;
thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
fn count() {
    let _ = COUNT.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn processing_controls_and_reset_do_not_allocate_or_free() {
    let mut reverb = Reverb::new(Config::default()).unwrap();
    COUNT.with(|count| count.set(Some(0)));
    for matrix in [Matrix::Householder, Matrix::Hadamard, Matrix::Hybrid] {
        reverb.set_parameters(1.2, 4000.0, matrix).unwrap();
        reverb.set_diffusion(0.4).unwrap();
        reverb.set_loop_diffusion(0.3).unwrap();
        reverb.set_band_damping(2.0, 3.0).unwrap();
        reverb.set_damping_shape(300.0, 1200.0, 1.0).unwrap();
        reverb
            .set_delay_modulation(DelayMode::RandomAllpass, 0.5, 0.7)
            .unwrap();
        for i in 0..4096 {
            std::hint::black_box(reverb.process_sample(if i == 0 { 1.0 } else { 0.0 }));
        }
        reverb.reset();
    }
    let allocations = COUNT.with(|count| count.replace(None).unwrap());
    assert_eq!(allocations, 0);
}

#[test]
fn classic_has_original_onset_stereo_decay_and_exact_reset() {
    for matrix in [Matrix::Householder, Matrix::Hadamard, Matrix::Hybrid] {
        let mut reverb = Reverb::new(Config {
            matrix,
            t60_seconds: 0.3,
            ..Config::default()
        })
        .unwrap();
        reverb.reset();
        let response: Vec<_> = (0..48_000)
            .map(|i| reverb.process_sample(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        assert!(
            response
                .iter()
                .flatten()
                .all(|x| x.is_finite() && x.abs() < 2.0)
        );
        let onset = response
            .iter()
            .position(|f| f.iter().any(|x| x.abs() > 1e-6))
            .unwrap();
        assert_eq!(onset, 1493);
        let energy = |slice: &[[f32; 2]]| {
            slice
                .iter()
                .flatten()
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>()
        };
        assert!(energy(&response[..24_000]) > 0.01);
        assert!(energy(&response[36_000..]) < energy(&response[..24_000]) * 1e-6);
        assert!(response.iter().any(|f| (f[0] - f[1]).abs() > 1e-4));
        reverb.reset();
        for (i, expected) in response.iter().enumerate() {
            assert_eq!(
                reverb.process_sample(if i == 0 { 1.0 } else { 0.0 }),
                *expected
            );
        }
    }
}

#[test]
fn classic_modulation_at_extreme_sample_rates_remains_bounded() {
    for sample_rate in [8000, 384000] {
        for matrix in [Matrix::Householder, Matrix::Hadamard, Matrix::Hybrid] {
            let mut reverb = Reverb::new(Config {
                sample_rate,
                matrix,
                t60_seconds: 20.0,
                damping_hz: sample_rate as f32 * 0.5,
            })
            .unwrap();
            reverb
                .set_delay_modulation(DelayMode::RandomAllpass, 1.0, 2.0)
                .unwrap();
            for i in 0..sample_rate {
                let frame = reverb.process_sample(if i == 0 { 1.0 } else { 0.0 });
                assert!(frame.iter().all(|x| x.is_finite() && x.abs() < 4.0));
            }
        }
    }
}
