//! Preallocated, stereo, musical-length slices with filtered block feedback.
use super::*;

#[derive(Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    #[id = "space"]
    #[name = "Space Repeat"]
    Space,
    #[id = "input"]
    #[name = "Input Repeat"]
    Input,
}

#[derive(Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Division {
    #[id = "quarter"]
    #[name = "1/4"]
    Quarter,
    #[id = "eighth"]
    #[name = "1/8"]
    Eighth,
    #[id = "sixteenth"]
    #[name = "1/16"]
    Sixteenth,
    #[id = "thirty_second"]
    #[name = "1/32"]
    ThirtySecond,
}

impl Division {
    fn beats(self) -> f64 {
        match self {
            Self::Quarter => 1.,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
        }
    }
}

pub(super) struct Repeat {
    blocks: [Vec<[f32; 2]>; 2],
    history: Vec<[f32; 2]>,
    history_cursor: usize,
    history_valid: usize,
    offset: f64,
    output_low: [f32; 2],
    output_high: [f32; 2],
    write: usize,
    cursor: usize,
    read_len: usize,
    read_start: usize,
    capture_start: usize,
    phase: f64,
    interval: f64,
    rate: f64,
    mode: Mode,
    division: Division,
    enabled: bool,
    level: f32,
    mix: f32,
    feedback: f32,
    target_mix: f32,
    target_feedback: f32,
    fade_out: bool,
    low: [f32; 2],
    high: [f32; 2],
    lp: f32,
    hp: f32,
    target_lp: f32,
    target_hp: f32,
    expected_beat: Option<f64>,
    current_beat: Option<f64>,
    align: Option<f64>,
    tempo: f64,
    playing: bool,
}

impl Repeat {
    pub fn new(rate: u32) -> Self {
        Self {
            blocks: std::array::from_fn(|_| {
                vec![[0.; 2]; (rate as f64 * 3.05).ceil() as usize + 2]
            }),
            history: vec![[0.; 2]; (rate as f64 * 3.05).ceil() as usize + 4],
            history_cursor: 0,
            history_valid: 0,
            offset: 0.,
            output_low: [0.; 2],
            output_high: [0.; 2],
            write: 0,
            cursor: 0,
            read_len: 0,
            read_start: 0,
            capture_start: 0,
            phase: 0.,
            interval: rate as f64 * 0.25,
            rate: rate as f64,
            mode: Mode::Space,
            division: Division::Eighth,
            enabled: false,
            level: 0.,
            mix: 0.,
            feedback: 0.,
            target_mix: 0.,
            target_feedback: 0.,
            fade_out: false,
            low: [0.; 2],
            high: [0.; 2],
            lp: 1.,
            hp: 0.,
            target_lp: 1.,
            target_hp: 0.,
            expected_beat: None,
            current_beat: None,
            align: None,
            tempo: 120.,
            playing: false,
        }
    }

    pub fn reset(&mut self) {
        // Length guards invalidate old storage without clearing megabytes in the callback.
        self.cursor = 0;
        self.read_len = 0;
        self.read_start = 0;
        self.capture_start = 0;
        self.phase = 0.;
        self.low = [0.; 2];
        self.high = [0.; 2];
        self.level = 0.;
        self.history_cursor = 0;
        self.history_valid = 0;
        self.output_low = [0.; 2];
        self.output_high = [0.; 2];
    }

    pub fn transport(
        &mut self,
        tempo: Option<f64>,
        beat: Option<f64>,
        playing: bool,
        frames: usize,
    ) {
        let tempo = tempo
            .filter(|v| v.is_finite() && *v > 0.)
            .unwrap_or(120.)
            .clamp(20., 400.);
        let beat = beat.filter(|v| v.is_finite());
        self.current_beat = beat;
        let jumped = match (beat, self.expected_beat) {
            (Some(a), Some(b)) => (a - b).abs() > 2. * tempo / (60. * self.rate),
            (Some(_), None) => true,
            _ => false,
        };
        if jumped || playing != self.playing || tempo != self.tempo || self.align.is_some() {
            self.align = Some(beat.unwrap_or(0.));
        }
        self.tempo = tempo;
        self.playing = playing;
        self.expected_beat = beat.map(|v| {
            v + if playing {
                frames as f64 * tempo / (60. * self.rate)
            } else {
                0.
            }
        });
    }

    pub fn update(&mut self, p: &ReverbParams) {
        let enabled = p.space_enabled.value() && p.repeat_enabled.value();
        let mode = p.repeat_mode.value();
        let division = p.repeat_division.value();
        let interval = self.rate * 60. / self.tempo * division.beats();
        let offset =
            (self.rate * p.repeat_nudge.value() as f64 * 0.001).max(self.rate * 0.001 - interval);
        // Mode, tempo and grid changes fade the old state out before invalidating it.
        let changed = enabled != self.enabled
            || mode != self.mode
            || division != self.division
            || interval != self.interval
            || offset != self.offset
            || self.align.is_some();
        if changed && self.level == 0. {
            let align = self.align.take().or(self.current_beat);
            self.reset();
            self.enabled = enabled;
            self.mode = mode;
            self.division = division;
            self.interval = interval;
            self.offset = offset;
            if let Some(beat) = align {
                self.phase = beat.rem_euclid(division.beats()) / division.beats() * interval;
                // Length guards zero-pad a partial first slice without clearing storage.
                self.cursor = self.phase.floor() as usize;
                self.capture_start = self.cursor;
            }
        }
        // Store the fade direction separately from the user mix.
        self.fade_out = changed && self.level > 0.;
        self.target_mix = p.repeat_mix.value();
        self.target_feedback = p.repeat_feedback.value();
        let coefficient = |hz: f32| {
            1. - (-std::f64::consts::TAU * (hz as f64).min(self.rate * 0.45) / self.rate).exp()
                as f32
        };
        self.target_lp = coefficient(p.repeat_high_cut.value());
        self.target_hp = coefficient(p.repeat_high_pass.value());
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn process(&mut self, source: [f32; 2]) -> [f32; 2] {
        let speed = (1. / (self.rate * 0.005)) as f32;
        self.level = if self.fade_out || !self.enabled {
            (self.level - speed).max(0.)
        } else {
            (self.level + speed).min(1.)
        };
        self.mix += speed * (self.target_mix - self.mix);
        self.feedback += speed * (self.target_feedback - self.feedback);
        self.lp += speed * (self.target_lp - self.lp);
        self.hp += speed * (self.target_hp - self.hp);
        if !self.enabled {
            return source;
        }
        let old = if self.cursor >= self.read_start && self.cursor < self.read_len {
            self.blocks[1 - self.write][self.cursor]
        } else {
            [0.; 2]
        };
        let shifted = self.read_history(self.interval + self.offset);
        let phase = (self.phase - self.offset).rem_euclid(self.interval);
        let edge = (self.rate * 0.002).min(self.interval * 0.125);
        let gate = (phase.min(self.interval - phase) / edge).clamp(0., 1.) as f32;
        let gate = gate * gate * (3. - 2. * gate);
        let mut result = source;
        for c in 0..2 {
            self.low[c] += self.lp * (old[c] - self.low[c]);
            self.high[c] += self.hp * (self.low[c] - self.high[c]);
            let filtered = self.low[c] - self.high[c];
            // Bound internal feedback too: the final output limiter cannot protect this state.
            self.blocks[self.write][self.cursor][c] =
                (source[c] + filtered * self.feedback).clamp(-16., 16.);
            self.history[self.history_cursor][c] = self.blocks[self.write][self.cursor][c];
            self.output_low[c] += self.lp * (shifted[c] - self.output_low[c]);
            self.output_high[c] += self.hp * (self.output_low[c] - self.output_high[c]);
            let amount = self.mix * self.level;
            result[c] = source[c] * (1. - amount)
                + (self.output_low[c] - self.output_high[c]) * gate * amount;
        }
        self.history_cursor = (self.history_cursor + 1) % self.history.len();
        self.history_valid = (self.history_valid + 1).min(self.history.len());
        self.cursor += 1;
        self.phase += 1.;
        if self.phase >= self.interval {
            self.phase -= self.interval;
            self.read_len = self.cursor;
            self.read_start = self.capture_start;
            self.capture_start = 0;
            self.cursor = 0;
            self.write = 1 - self.write;
        }
        result
    }

    fn read_history(&self, delay: f64) -> [f32; 2] {
        let whole = delay.floor() as usize;
        let fraction = (delay - whole as f64) as f32;
        let sample = |age: usize| {
            if age == 0 || age > self.history_valid {
                [0.; 2]
            } else {
                self.history[(self.history_cursor + self.history.len() - age) % self.history.len()]
            }
        };
        let a = sample(whole);
        let b = sample(whole + 1);
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * fraction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::params::InternalParamMut;

    fn params() -> ReverbParams {
        let p = ReverbParams::default();
        unsafe {
            p.space_enabled._internal_set_plain_value(true);
            p.repeat_enabled._internal_set_plain_value(true);
            p.repeat_mix._internal_set_plain_value(1.);
        }
        p
    }

    #[test]
    fn groove_offset_moves_every_generation_equally_without_changing_period() {
        let p = params();
        for ms in [-50., -5., 0., 5., 50.] {
            unsafe {
                p.repeat_nudge._internal_set_plain_value(ms);
            }
            let mut r = Repeat::new(8000);
            r.update(&p);
            // Isolate timing from filter tails and startup smoothing.
            r.level = 1.;
            r.mix = 1.;
            r.target_mix = 1.;
            r.feedback = 0.6;
            r.target_feedback = 0.6;
            r.lp = 1.;
            r.target_lp = 1.;
            r.hp = 0.;
            r.target_hp = 0.;
            assert_eq!(r.interval, 2000.);
            let offset = (ms * 8.) as isize;
            let mut peaks = Vec::new();
            for i in 0..11500 {
                let y = r.process(if i == 800 { [1.; 2] } else { [0.; 2] });
                if y[0] > 0.01 {
                    peaks.push(i);
                }
            }
            assert_eq!(
                peaks,
                (1..=5)
                    .map(|n| (800 + n * 2000 + offset) as usize)
                    .collect::<Vec<_>>()
            );
        }
        unsafe {
            p.repeat_nudge._internal_set_plain_value(-50.);
            p.repeat_division
                ._internal_set_plain_value(Division::ThirtySecond);
        }
        let mut r = Repeat::new(8000);
        r.transport(Some(400.), None, false, 0);
        r.update(&p);
        assert_eq!(r.interval, 150.);
        assert_eq!(r.interval + r.offset, 8.); // Minimum causal playback delay: 1 ms.
        unsafe {
            p.repeat_nudge._internal_set_plain_value(50.);
            p.repeat_division
                ._internal_set_plain_value(Division::Quarter);
        }
        r.reset();
        r.transport(Some(20.), None, false, 0);
        r.update(&p);
        for _ in 0..50000 {
            assert!(r.process([0.; 2]).iter().all(|v| v.is_finite()));
        }
        assert_eq!(r.interval, 24000.);
        assert_eq!(r.offset, 400.);
    }

    #[test]
    fn divisions_follow_tempo_and_fractional_lengths_do_not_drift() {
        for rate in [8000, 44100, 48000, 192000, 384000] {
            let mut r = Repeat::new(rate);
            let p = params();
            for tempo in [20., 123., 400.] {
                for division in [
                    Division::Quarter,
                    Division::Eighth,
                    Division::Sixteenth,
                    Division::ThirtySecond,
                ] {
                    unsafe {
                        p.repeat_division._internal_set_plain_value(division);
                    }
                    r.reset();
                    r.transport(Some(tempo), Some(0.), true, 0);
                    r.update(&p);
                    let length = rate as f64 * 60. / tempo * division.beats();
                    assert!((r.interval - length).abs() < 1e-8);
                    for _ in 0..(length * 3.1) as usize {
                        r.process([0.; 2]);
                    }
                    assert!(
                        (r.phase - ((length * 3.1) as usize as f64 - 3. * length)).abs() < 1e-6
                    );
                }
            }
        }
    }

    #[test]
    fn impulse_repeats_one_slice_later_and_feedback_decays() {
        let p = params();
        let mut r = Repeat::new(8000);
        r.update(&p);
        let mut energies = [0.; 5];
        let mut first = None;
        for i in 0..10000 {
            let y = r.process(if i == 400 { [1., -0.5] } else { [0.; 2] });
            if i >= 2000 && y[0].abs() > 1e-7 && first.is_none() {
                first = Some(i);
            }
            energies[i / 2000] += y[0] * y[0];
            assert!((y[1] + y[0] * 0.5).abs() < 1e-6);
        }
        assert_eq!(first, Some(2400));
        assert!(energies[1] > 0.01);
        for i in 2..5 {
            assert!(energies[i] < energies[i - 1] * 0.5);
        }
        r.reset();
        for _ in 0..5000 {
            assert_eq!(r.process([0.; 2]), [0.; 2]);
        }
    }

    fn tone_energy(hz: f64, lp: f32, hp: f32, cycles: usize) -> f64 {
        let p = params();
        unsafe {
            p.repeat_high_cut._internal_set_plain_value(lp);
            p.repeat_high_pass._internal_set_plain_value(hp);
            p.repeat_feedback._internal_set_plain_value(0.95);
        }
        let mut r = Repeat::new(48000);
        r.update(&p);
        let mut energy = 0.;
        for i in 0..12000 * (cycles + 1) {
            let x = if i < 12000 {
                (i as f64 * std::f64::consts::TAU * hz / 48000.).sin() as f32 * 0.1
            } else {
                0.
            };
            let y = r.process([x; 2]);
            if i >= 12000 * cycles {
                energy += (y[0] as f64).powi(2);
            }
        }
        energy
    }

    #[test]
    fn high_cut_darkens_each_generation_and_high_pass_removes_bass() {
        let high_first = tone_energy(8000., 1500., 20., 1);
        let low_first = tone_energy(500., 1500., 20., 1);
        assert!(high_first < low_first * 0.1);
        let high_later = tone_energy(8000., 1500., 20., 3);
        let low_later = tone_energy(500., 1500., 20., 3);
        assert!(high_later / low_later < high_first / low_first * 0.1);
        assert!(tone_energy(60., 20000., 1000., 1) < tone_energy(60., 20000., 20., 1) * 0.02);
    }

    #[test]
    fn grid_seek_mode_changes_and_invalid_tempo_never_replay_stale_storage() {
        let p = params();
        let mut r = Repeat::new(8000);
        r.transport(Some(120.), Some(0.25), true, 0);
        r.update(&p);
        assert_eq!(r.cursor, 1000);
        for _ in 0..5000 {
            r.process([0.5; 2]);
        }
        unsafe {
            p.repeat_mode._internal_set_plain_value(Mode::Input);
        }
        r.update(&p);
        for _ in 0..100 {
            r.process([0.; 2]);
        }
        r.transport(Some(f64::NAN), Some(-1.), true, 0);
        r.update(&p);
        assert_eq!(r.mode(), Mode::Input);
        assert_eq!(r.tempo, 120.);
        for _ in 0..5000 {
            assert_eq!(r.process([0.; 2]), [0.; 2]);
        }
        unsafe {
            p.repeat_enabled._internal_set_plain_value(false);
        }
        r.update(&p);
        for _ in 0..100 {
            r.process([0.; 2]);
        }
        r.update(&p);
        for _ in 0..5000 {
            assert_eq!(r.process([0.1, -0.2]), [0.1, -0.2]);
        }
    }
}
