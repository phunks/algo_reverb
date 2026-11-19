#[path = "../src/zero_convolution.rs"]
mod convolution;
#[path = "../src/damping.rs"]
mod damping;
#[path = "../src/limiter.rs"]
mod limiter;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counter;
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.get() {
            ALLOCS.set(ALLOCS.get() + 1);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK.get() {
            FREES.set(FREES.get() + 1);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;

#[test]
fn audio_processing_and_reset_do_not_allocate_or_free() {
    // Thread-local counters exclude allocations from the parallel test harness.
    let ir = vec![[0.001, -0.002]; 88200];
    let mut engine = convolution::Convolver::new(&ir, 88200).unwrap();
    assert_eq!(engine.rate, 88200);
    assert_eq!(engine.frames, 88200);
    engine.calibration_gain = 0.5;
    let mut limiter = limiter::Limiter::default();
    limiter.configure(88200);
    let damping_params = damping::DampingParams::default();
    let mut tone = damping::WetFilter::default();
    tone.reset(88200, damping_params.wet());
    TRACK.set(true);
    // Include live coefficient updates, not just steady state.
    tone.update(damping::Settings {
        enabled: true,
        low: 6.0,
        mid: 3.0,
        ..damping_params.wet()
    });
    for _ in 0..8192 {
        let (_, wet) = engine.process([0.1, 0.3]);
        let wet = wet.map(|sample| sample * engine.calibration_gain);
        std::hint::black_box(limiter.process(tone.process(wet), true));
        std::hint::black_box(limiter.gain());
    }
    engine.reset();
    for _ in 0..8192 {
        assert_eq!(engine.process([0.0; 2]), ([0.0; 2], [0.0; 2]));
    }
    limiter.reset();
    tone.reset(88200, damping_params.wet());
    TRACK.set(false);
    assert_eq!(ALLOCS.get(), 0);
    assert_eq!(FREES.get(), 0);
}
