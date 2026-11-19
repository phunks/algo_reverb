//! Two prebuilt spaces: onset reset, dip/swell and release of the previous tail.
//! No fitting, allocation, delay retuning or feedback boost in the callback.
use super::*;

pub(super) struct Space {
    voices: [DiffFdn; 2],
    active: usize,
    age: u64,
    started: bool,
    gains: [f32; 2],
    release_start: f32,
    release_age: u64,
    pending: usize,
    preroll: Vec<f32>,
    cursor: usize,
    detector: DuckTransient,
    cooldown: usize,
    mode: bool,
    level: f32,
    rate: u32,
    settings: Settings,
    timing: DuckTiming,
}

#[derive(Clone, Copy)]
struct Settings {
    enabled: bool,
    threshold: f32,
    decay: f32,
    swell: f32,
    release: f32,
    steps: u32,
}

impl Space {
    pub fn new(mut voices: [DiffFdn; 2]) -> Self {
        let rate = voices[0].sample_rate();
        for f in &mut voices {
            f.enable_live_controls();
        }
        Self {
            voices,
            active: 0,
            age: 0,
            started: false,
            gains: [0.; 2],
            release_start: 0.,
            release_age: 0,
            pending: 0,
            preroll: vec![0.; (rate as f32 * 0.005).ceil() as usize],
            cursor: 0,
            detector: DuckTransient {
                armed: true,
                ..Default::default()
            },
            cooldown: 0,
            mode: false,
            level: 1.,
            rate,
            settings: Settings {
                enabled: false,
                threshold: 0.063,
                decay: rate as f32 * 0.15,
                swell: rate as f32 * 0.5,
                release: rate as f32 * 0.3,
                steps: 0,
            },
            timing: {
                let coefficient = |ms: f32| 1. - (-1. / (ms * 0.001 * rate as f32)).exp();
                DuckTiming {
                    fast: coefficient(30.),
                    slow_attack: coefficient(30.),
                    slow_release: coefficient(100.),
                    hold: 1,
                }
            },
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn set_damping(&mut self, amount: f64) {
        for f in &mut self.voices {
            f.set_damping(amount);
        }
    }

    pub fn update(&mut self, p: &ReverbParams) {
        for f in &mut self.voices {
            apply_live_controls(f, p);
        }
        let samples = |ms: f32| ms * 0.001 * self.rate as f32;
        self.settings = Settings {
            enabled: p.space_enabled.value(),
            threshold: util::db_to_gain(p.space_threshold.value()),
            decay: samples(p.space_decay.value()),
            swell: samples(p.space_swell.value()),
            release: samples(p.space_release.value()),
            steps: p.space_steps.value().round() as u32,
        };
    }

    pub fn reset(&mut self) {
        for f in &mut self.voices {
            f.reset();
        }
        self.active = 0;
        self.age = 0;
        self.started = false;
        self.gains = [0.; 2];
        self.release_start = 0.;
        self.release_age = 0;
        self.pending = 0;
        self.preroll.fill(0.);
        self.cursor = 0;
        self.detector = DuckTransient {
            armed: true,
            ..Default::default()
        };
        self.cooldown = 0;
        self.mode = self.settings.enabled;
        self.level = 1.;
    }

    pub fn process_sample(&mut self, input: f64) -> f64 {
        self.process_stereo(input)[0]
    }

    pub fn process_stereo(&mut self, input: f64) -> [f64; 2] {
        // Enable/disable through silence: never expose reset state or stale tails.
        let step = 1. / (self.rate as f32 * 0.01);
        if self.mode != self.settings.enabled {
            self.level = (self.level - step).max(0.);
            if self.level == 0. {
                self.reset();
                self.level = 0.;
            }
        } else {
            self.level = (self.level + step).min(1.);
        }
        if !self.mode {
            return self.voices[0]
                .process_stereo(input)
                .map(|x| x * self.level as f64);
        }
        let input = input as f32;
        let was_armed = self.detector.armed;
        self.detector
            .detect(input.abs(), self.settings.threshold, 1., &self.timing);
        self.cooldown = self.cooldown.saturating_sub(1);
        if was_armed && !self.detector.armed && self.cooldown == 0 && self.pending == 0 {
            // Retiring voice is muted before reuse. The input is delayed by this
            // same 5 ms so the new voice still receives the triggering attack.
            self.pending = self.preroll.len();
            self.cooldown = (self.rate as f32 * 0.04) as usize;
        }
        if self.pending > 0 {
            self.pending -= 1;
            if self.pending == 0 {
                let next = 1 - self.active;
                self.voices[next].reset();
                self.release_start = self.gains[self.active];
                self.release_age = 0;
                self.active = next;
                self.age = 0;
                self.started = true;
            }
        }
        let delayed = std::mem::replace(&mut self.preroll[self.cursor], input);
        self.cursor = (self.cursor + 1) % self.preroll.len();
        if self.started {
            let attack = (self.age as f32 / self.preroll.len() as f32).min(1.);
            let attack = attack * attack * (3. - 2. * attack);
            self.gains[self.active] = attack
                * stepped_envelope(
                    self.age as f32,
                    self.settings.decay,
                    self.settings.swell,
                    self.settings.steps,
                    self.rate as f32 * 0.005,
                );
            self.gains[1 - self.active] = self.release_start
                * (1. - self.release_age as f32 / self.settings.release).clamp(0., 1.);
            self.age = self.age.saturating_add(1);
            self.release_age = self.release_age.saturating_add(1);
        }
        let mut gains = self.gains;
        if self.pending > 0 {
            gains[1 - self.active] *= self.pending as f32 / self.preroll.len() as f32;
        }
        // Correlated spaces must not gain +6 dB during overlap.
        let scale = self.level / (gains[0] + gains[1]).max(1.);
        let mut output = [0.; 2];
        for (i, voice) in self.voices.iter_mut().enumerate() {
            let excitation = if self.started && i == self.active {
                delayed
            } else {
                0.
            };
            let wet = voice.process_stereo(excitation as f64);
            for c in 0..2 {
                output[c] += wet[c] * (gains[i] * scale) as f64;
            }
        }
        output
    }
}

fn stepped_envelope(age: f32, decay: f32, swell: f32, steps: u32, edge: f32) -> f32 {
    if steps == 0 || age < decay {
        return envelope(age, decay, swell);
    }
    let elapsed = (age - decay).max(0.);
    if elapsed >= swell {
        return 1.;
    }
    let interval = swell / steps as f32;
    let step = (elapsed / interval).floor();
    // Hold each level, then use at most 5 ms (or 1/4 of a short step)
    // for the rise. Complete the last rise at exactly the original Swell end.
    let width = edge.min(interval * 0.25);
    let phase = ((elapsed - step * interval - (interval - width)) / width).clamp(0., 1.);
    let rise = phase * phase * (3. - 2. * phase);
    0.125 + 0.875 * (step + rise) / steps as f32
}

fn envelope(age: f32, decay: f32, swell: f32) -> f32 {
    // Smooth endpoints, -18 dB dip; bounded output gain, never feedback gain.
    let smooth = |x: f32| {
        let x = x.clamp(0., 1.);
        x * x * (3. - 2. * x)
    };
    if age < decay {
        1. - 0.875 * smooth(age / decay)
    } else {
        0.125 + 0.875 * smooth((age - decay) / swell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::params::InternalParamMut;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };

    thread_local! { static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
    struct Allocator;
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.with(|n| {
                if let Some(v) = n.get() {
                    n.set(Some(v + 1));
                }
            });
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            ALLOCATIONS.with(|n| {
                if let Some(v) = n.get() {
                    n.set(Some(v + 1));
                }
            });
            unsafe { System.dealloc(ptr, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;

    fn space(rate: u32, enabled: bool) -> Space {
        let p = ReverbParams::default();
        unsafe {
            p.space_enabled._internal_set_plain_value(enabled);
        }
        let mut s = Space::new(std::array::from_fn(|_| {
            DiffFdn::parse(room_text(RoomPreset::Salon))
                .unwrap()
                .at_sample_rate(rate)
                .unwrap()
        }));
        s.update(&p);
        s.reset();
        s
    }

    #[test]
    fn disabled_is_exact_original_and_onsets_retain_release_tail() {
        let p = ReverbParams::default();
        let mut s = space(8000, false);
        let mut reference = DiffFdn::parse(room_text(RoomPreset::Salon))
            .unwrap()
            .at_sample_rate(8000)
            .unwrap();
        reference.enable_live_controls();
        apply_live_controls(&mut reference, &p);
        reference.reset();
        for i in 0..8000 {
            let x = if i == 0 { 1. } else { 0. };
            assert_eq!(s.process_stereo(x), reference.process_stereo(x));
        }
        let mut s = space(8000, true);
        for i in 0..4000 {
            s.process_stereo(if i == 0 { 1. } else { 0. });
        }
        assert!(s.started);
        let old = s.active;
        let previous_gain = s.gains[old];
        for i in 0..40 {
            s.process_stereo(if i == 0 { 1. } else { 0. });
        }
        assert_ne!(s.active, old);
        assert!((s.release_start - previous_gain).abs() < 0.02);
        assert!(s.gains[old] > 0.);
        assert_eq!(s.voices[s.active].sample_rate(), 8000);
        for _ in 0..2500 {
            s.process_stereo(0.);
        }
        assert_eq!(s.gains[old], 0.);
        s.reset();
        for _ in 0..800 {
            assert_eq!(s.process_stereo(0.), [0.; 2]);
        }
    }

    #[test]
    fn slice_repeat_processing_controls_and_reset_allocate_nothing() {
        for rate in [8000, 48000, 192000, 384000] {
            let p = ReverbParams::default();
            let mut repeat = slice_repeat::Repeat::new(rate);
            ALLOCATIONS.with(|n| n.set(Some(0)));
            for mode in [slice_repeat::Mode::Space, slice_repeat::Mode::Input] {
                unsafe {
                    p.space_enabled._internal_set_plain_value(true);
                    p.repeat_enabled._internal_set_plain_value(true);
                    p.repeat_mode._internal_set_plain_value(mode);
                    p.repeat_feedback._internal_set_plain_value(0.95);
                    p.repeat_nudge._internal_set_plain_value(50.);
                }
                repeat.transport(Some(20.), Some(0.25), true, 64);
                repeat.update(&p);
                for _ in 0..rate / 100 {
                    repeat.process([0.5; 2]);
                }
                repeat.update(&p);
                repeat.reset();
            }
            let count = ALLOCATIONS.with(|n| n.replace(None).unwrap());
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn rapid_retrigger_toggle_and_reset_are_allocation_free_and_finite() {
        for rate in [8000, 48000, 192000] {
            let mut s = space(rate, true);
            s.settings.steps = 4;
            let p = ReverbParams::default();
            let interval = rate as usize / 10;
            ALLOCATIONS.with(|n| n.set(Some(0)));
            let mut energy = 0.;
            for i in 0..rate as usize {
                let y = s.process_stereo(if i % interval == 0 { 0.5 } else { 0. });
                assert!(y.iter().all(|x| x.is_finite()));
                energy += y[0] * y[0];
            }
            s.update(&p);
            for _ in 0..rate / 20 {
                s.process_stereo(0.);
            }
            s.reset();
            let count = ALLOCATIONS.with(|n| n.replace(None).unwrap());
            assert_eq!(count, 0);
            assert!(energy > 0.);
            assert!(!s.mode);
        }
    }

    #[test]
    fn steady_tone_does_not_keep_resetting_spaces() {
        let mut s = space(8000, true);
        for i in 0..16000 {
            s.process_stereo((i as f64 * std::f64::consts::TAU * 80. / 8000.).sin() * 0.5);
        }
        assert!(s.started);
        assert!(s.age > 15000, "sustained tone retriggered: {}", s.age);
    }

    #[test]
    fn staircase_holds_then_rises_monotonically_and_finishes_on_time() {
        for rate in [8000, 44100, 48000, 192000, 384000] {
            for ms in [10., 500., 3000.] {
                let swell = rate as f32 * ms * 0.001;
                let edge = rate as f32 * 0.005;
                let decay = rate as f32 * 0.15;
                for steps in 1..=8 {
                    let interval = swell / steps as f32;
                    for i in 0..steps {
                        let expected = 0.125 + 0.875 * i as f32 / steps as f32;
                        for fraction in [0.1, 0.5, 0.7] {
                            let age = decay + (i as f32 + fraction) * interval;
                            assert!(
                                (stepped_envelope(age, decay, swell, steps, edge) - expected).abs()
                                    < 1e-5
                            );
                        }
                    }
                    let mut previous = 0.125;
                    for n in 0..=swell.ceil() as u32 + 1 {
                        let gain = stepped_envelope(decay + n as f32, decay, swell, steps, edge);
                        assert!((0.125..=1.).contains(&gain));
                        assert!(gain >= previous - 1e-6);
                        assert!(
                            gain - previous < 0.07,
                            "abrupt step at {rate} Hz, {ms} ms, {steps} steps"
                        );
                        previous = gain;
                    }
                    assert_eq!(previous, 1.);
                    assert_eq!(stepped_envelope(0., decay, swell, steps, edge), 1.);
                }
            }
        }
    }

    #[test]
    fn zero_steps_preserves_original_smooth_envelope_exactly() {
        for age in 0..10000 {
            assert_eq!(
                stepped_envelope(age as f32, 1200., 4000., 0, 40.),
                envelope(age as f32, 1200., 4000.)
            );
        }
    }

    #[test]
    fn dip_then_swell_is_bounded_and_continuous() {
        assert_eq!(envelope(0., 100., 200.), 1.);
        assert_eq!(envelope(100., 100., 200.), 0.125);
        assert_eq!(envelope(300., 100., 200.), 1.);
        for i in 0..1000 {
            let a = envelope(i as f32, 100., 200.);
            assert!((0.125..=1.).contains(&a));
            assert!((a - envelope(i as f32 + 1., 100., 200.)).abs() < 0.014);
        }
    }
}
