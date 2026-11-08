//! Standalone audition support. The FDN itself has no rodio/GUI dependency.

use crate::{Config, Matrix, Reverb};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

pub const PREVIEW_RATE: u32 = 48_000;
const MAX_SECONDS: usize = 300;

/// Immutable decoded audio. Stereo is preserved in the dry path; the wet path
/// receives the average of L/R because the current FDN has a mono input.
pub struct Clip {
    pub frames: Vec<[f32; 2]>,
    pub original_rate: u32,
    pub original_channels: u16,
}

/// Decode on a worker, never from the audio callback. Limit input and output
/// duration to five minutes. Linear resampling is for audition, not mastering.
pub fn load_wav(path: &Path) -> Result<Clip, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if !(1..=2).contains(&spec.channels) {
        return Err("Only mono and stereo WAV files are supported".into());
    }
    if !(8_000..=384_000).contains(&spec.sample_rate) {
        return Err("WAV sample rate must be between 8000 and 384000 Hz".into());
    }
    let count = reader.duration() as usize;
    if count == 0 || count > spec.sample_rate as usize * MAX_SECONDS {
        return Err("WAV must contain audio and be at most five minutes long".into());
    }
    let mut decoded = Vec::with_capacity(count);
    let mut pending = None;
    let mut push = |sample: f32| -> Result<(), String> {
        if !sample.is_finite() {
            return Err("WAV contains NaN or infinite samples".into());
        }
        // Keep the audition input bounded, including float WAVs above 0 dBFS.
        let sample = sample.clamp(-1.0, 1.0);
        if spec.channels == 1 {
            decoded.push([sample; 2]);
        } else if let Some(left) = pending.take() {
            decoded.push([left, sample]);
        } else {
            pending = Some(sample);
        }
        Ok(())
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for sample in reader.samples::<f32>() {
                push(sample.map_err(|e| e.to_string())?)?;
            }
        }
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                return Err("Unsupported PCM bit depth".into());
            }
            let scale = 2.0_f64.powi(i32::from(spec.bits_per_sample) - 1);
            for sample in reader.samples::<i32>() {
                push((f64::from(sample.map_err(|e| e.to_string())?) / scale) as f32)?;
            }
        }
    }
    if pending.is_some() || decoded.len() != count {
        return Err("Incomplete WAV frames".into());
    }
    let frames = if spec.sample_rate == PREVIEW_RATE {
        decoded
    } else {
        let output_len =
            (count as u64 * u64::from(PREVIEW_RATE)).div_ceil(u64::from(spec.sample_rate)) as usize;
        (0..output_len)
            .map(|i| {
                let position = i as f64 * f64::from(spec.sample_rate) / f64::from(PREVIEW_RATE);
                let a = position.floor() as usize;
                let b = (a + 1).min(decoded.len() - 1);
                let fraction = position.fract() as f32;
                std::array::from_fn(|ch| {
                    decoded[a][ch] + fraction * (decoded[b][ch] - decoded[a][ch])
                })
            })
            .collect()
    };
    Ok(Clip {
        frames,
        original_rate: spec.sample_rate,
        original_channels: spec.channels,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub t60: f32,
    pub damping: f32,
    pub diffusion: f32,
    pub wet: f32,
    pub gain_db: f32,
    pub matrix: Matrix,
    pub bypass: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            t60: 2.5,
            damping: 6_000.0,
            diffusion: 0.0,
            wet: 0.35,
            gain_db: -12.0,
            matrix: Matrix::Hadamard,
            bypass: false,
        }
    }
}

/// Single GUI producer; audio reads independent targets at a 128-frame control
/// interval. No mutex is shared with the audio iterator.
pub struct Controls {
    t60: AtomicU32,
    damping: AtomicU32,
    diffusion: AtomicU32,
    wet: AtomicU32,
    gain_db: AtomicU32,
    matrix: AtomicU32,
    bypass: AtomicBool,
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub rewind: AtomicBool,
    pub position: AtomicUsize,
    pub peak: AtomicU32,
    pub clipped: AtomicBool,
}

impl Default for Controls {
    fn default() -> Self {
        let s = Settings::default();
        Self {
            t60: AtomicU32::new(s.t60.to_bits()),
            damping: AtomicU32::new(s.damping.to_bits()),
            diffusion: AtomicU32::new(s.diffusion.to_bits()),
            wet: AtomicU32::new(s.wet.to_bits()),
            gain_db: AtomicU32::new(s.gain_db.to_bits()),
            matrix: AtomicU32::new(1),
            bypass: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            rewind: AtomicBool::new(false),
            position: AtomicUsize::new(0),
            peak: AtomicU32::new(0),
            clipped: AtomicBool::new(false),
        }
    }
}

impl Controls {
    pub fn publish(&self, settings: Settings) {
        // Clamp even programmatic callers. NaNs are replaced with defaults.
        let finite = |v: f32, default: f32, min: f32, max: f32| {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                default
            }
        };
        self.t60.store(
            finite(settings.t60, 2.5, 0.1, 20.0).to_bits(),
            Ordering::Relaxed,
        );
        self.damping.store(
            finite(settings.damping, 6000.0, 100.0, 20_000.0).to_bits(),
            Ordering::Relaxed,
        );
        self.wet.store(
            finite(settings.wet, 0.35, 0.0, 1.0).to_bits(),
            Ordering::Relaxed,
        );
        self.gain_db.store(
            finite(settings.gain_db, -12.0, -60.0, 0.0).to_bits(),
            Ordering::Relaxed,
        );
        self.diffusion.store(
            finite(settings.diffusion, 0.0, 0.0, 1.0).to_bits(),
            Ordering::Relaxed,
        );
        self.matrix.store(
            match settings.matrix {
                Matrix::Householder => 0,
                Matrix::Hadamard => 1,
                Matrix::Hybrid => 2,
            },
            Ordering::Relaxed,
        );
        self.bypass.store(settings.bypass, Ordering::Relaxed);
    }

    fn settings(&self) -> Settings {
        Settings {
            t60: f32::from_bits(self.t60.load(Ordering::Relaxed)),
            damping: f32::from_bits(self.damping.load(Ordering::Relaxed)),
            diffusion: f32::from_bits(self.diffusion.load(Ordering::Relaxed)),
            wet: f32::from_bits(self.wet.load(Ordering::Relaxed)),
            gain_db: f32::from_bits(self.gain_db.load(Ordering::Relaxed)),
            matrix: match self.matrix.load(Ordering::Relaxed) {
                0 => Matrix::Householder,
                2 => Matrix::Hybrid,
                _ => Matrix::Hadamard,
            },
            bypass: self.bypass.load(Ordering::Relaxed),
        }
    }
}

pub struct PreviewSource {
    clip: Arc<Clip>,
    controls: Arc<Controls>,
    reverb: Reverb,
    position: usize,
    tail: usize,
    right: Option<f32>,
    ticks: usize,
    settings: Settings,
    wet: f32,
    gain: f32,
    target_gain: f32,
    gate: f32,
    blend: f32,
    block_peak: f32,
}

impl PreviewSource {
    /// Allocate delay memory on the caller's thread before appending to rodio.
    pub fn new(clip: Arc<Clip>, controls: Arc<Controls>) -> Self {
        let settings = controls.settings();
        let mut reverb = Reverb::new(Config {
            sample_rate: PREVIEW_RATE,
            t60_seconds: settings.t60,
            damping_hz: settings.damping,
            matrix: settings.matrix,
        })
        .unwrap();
        let _ = reverb.set_diffusion(settings.diffusion);
        Self {
            clip,
            controls,
            reverb,
            position: 0,
            tail: 0,
            right: None,
            ticks: 0,
            settings,
            wet: settings.wet,
            gain: 10.0_f32.powf(settings.gain_db / 20.0),
            target_gain: 10.0_f32.powf(settings.gain_db / 20.0),
            gate: 0.0,
            blend: if settings.bypass { 1.0 } else { 0.0 },
            block_peak: 0.0,
        }
    }

    fn frame(&mut self) -> [f32; 2] {
        if self.ticks == 0 {
            let next = self.controls.settings();
            if next.diffusion != self.settings.diffusion {
                let _ = self.reverb.set_diffusion(next.diffusion);
            }
            if next.t60 != self.settings.t60
                || next.damping != self.settings.damping
                || next.matrix != self.settings.matrix
            {
                // All targets are validated by Controls::publish.
                let _ = self
                    .reverb
                    .set_parameters(next.t60, next.damping, next.matrix);
            }
            self.settings = next;
            self.target_gain = 10.0_f32.powf(next.gain_db / 20.0);
            self.controls
                .position
                .store(self.position, Ordering::Relaxed);
            self.controls
                .peak
                .store(self.block_peak.to_bits(), Ordering::Relaxed);
            self.block_peak = 0.0;
        }
        self.ticks = (self.ticks + 1) % 128;
        if self.controls.rewind.swap(false, Ordering::Relaxed) {
            self.position = 0;
            self.tail = 0;
            // Keep the existing tail: rewind does not clear or allocate buffers.
        }
        let mut playing = self.controls.playing.load(Ordering::Relaxed);
        if self.position >= self.clip.frames.len() && self.controls.looping.load(Ordering::Relaxed)
        {
            self.position = 0;
            self.tail = 0;
        }
        let mut dry = [0.0; 2];
        if playing {
            if let Some(frame) = self.clip.frames.get(self.position) {
                dry = *frame;
                self.position += 1;
            } else {
                self.tail += 1;
                if self.tail > ((self.settings.t60 + 1.0) * PREVIEW_RATE as f32) as usize {
                    self.controls.playing.store(false, Ordering::Relaxed);
                    playing = false;
                }
            }
        }
        let wet = self.reverb.process_sample((dry[0] + dry[1]) * 0.5);
        // ~10 ms time constant for level/mix controls. Pause fades the output;
        // the FDN continues decaying rather than freezing its tail.
        const SMOOTH: f32 = 0.002_081_16;
        self.wet += SMOOTH * (self.settings.wet - self.wet);
        self.gain += SMOOTH * (self.target_gain - self.gain);
        self.gate += SMOOTH * (if playing { 1.0 } else { 0.0 } - self.gate);
        self.blend += SMOOTH * (if self.settings.bypass { 1.0 } else { 0.0 } - self.blend);
        std::array::from_fn(|ch| {
            let processed = dry[ch] * (1.0 - self.wet) + wet[ch] * self.wet;
            let sample = (processed + self.blend * (dry[ch] - processed)) * self.gain * self.gate;
            self.block_peak = self.block_peak.max(sample.abs());
            if sample.abs() > 1.0 {
                self.controls.clipped.store(true, Ordering::Relaxed);
            }
            sample.clamp(-1.0, 1.0)
        })
    }
}

impl Iterator for PreviewSource {
    type Item = f32;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        let [left, right] = self.frame();
        self.right = Some(right);
        Some(left)
    }
}

impl rodio::Source for PreviewSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        PREVIEW_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestFile(std::path::PathBuf);
    impl TestFile {
        fn new() -> Self {
            static ID: AtomicUsize = AtomicUsize::new(0);
            Self(std::env::temp_dir().join(format!(
                "reverb-test-{}-{}.wav",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn loads_pcm_and_resamples_mono() {
        let file = TestFile::new();
        let mut writer = hound::WavWriter::create(
            &file.0,
            hound::WavSpec {
                channels: 1,
                sample_rate: 24_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(0_i16).unwrap();
        writer.write_sample(16384_i16).unwrap();
        writer.finalize().unwrap();
        let clip = load_wav(&file.0).unwrap();
        assert_eq!(clip.frames, vec![[0.0; 2], [0.25; 2], [0.5; 2], [0.5; 2]]);
    }

    #[test]
    fn loads_float_stereo_and_rejects_invalid_audio() {
        let file = TestFile::new();
        for (channels, samples, valid) in [
            (2, vec![0.25, -0.5, 2.0, -2.0], true),
            (1, vec![f32::NAN], false),
            (1, vec![], false),
            (3, vec![0.0; 3], false),
        ] {
            let mut writer = hound::WavWriter::create(
                &file.0,
                hound::WavSpec {
                    channels,
                    sample_rate: PREVIEW_RATE,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                },
            )
            .unwrap();
            for sample in samples {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
            let result = load_wav(&file.0);
            assert_eq!(result.is_ok(), valid);
            if let Ok(clip) = result {
                assert_eq!(clip.frames, vec![[0.25, -0.5], [1.0, -1.0]]);
            }
        }
        std::fs::write(&file.0, b"not a WAV").unwrap();
        assert!(load_wav(&file.0).is_err());
    }

    #[test]
    fn invalid_control_values_are_sanitized() {
        let controls = Controls::default();
        controls.publish(Settings {
            t60: f32::NAN,
            damping: f32::INFINITY,
            wet: -1.0,
            gain_db: 100.0,
            ..Settings::default()
        });
        let settings = controls.settings();
        assert_eq!(settings.t60, 2.5);
        assert_eq!(settings.damping, 6000.0);
        assert_eq!(settings.wet, 0.0);
        assert_eq!(settings.gain_db, 0.0);
    }

    #[test]
    fn transport_and_live_controls_stay_finite() {
        let clip = Arc::new(Clip {
            frames: vec![[0.25, -0.125]; 1000],
            original_rate: PREVIEW_RATE,
            original_channels: 2,
        });
        let controls = Arc::new(Controls::default());
        let mut source = PreviewSource::new(clip, controls.clone());
        for _ in 0..256 {
            assert_eq!(source.next(), Some(0.0));
        }
        controls.playing.store(true, Ordering::Relaxed);
        controls.looping.store(true, Ordering::Relaxed);
        let mut energy = 0.0;
        for i in 0..100_000 {
            if i % 4096 == 0 {
                controls.publish(Settings {
                    matrix: match (i / 4096) % 3 {
                        0 => Matrix::Householder,
                        1 => Matrix::Hadamard,
                        _ => Matrix::Hybrid,
                    },
                    diffusion: (i % 7) as f32 / 6.0,
                    t60: 0.2 + (i % 10) as f32,
                    ..Settings::default()
                });
            }
            let sample = source.next().unwrap();
            assert!(sample.is_finite() && sample.abs() <= 1.0);
            energy += sample.abs();
        }
        assert!(energy > 1.0);
        controls.rewind.store(true, Ordering::Relaxed);
        source.next();
        source.next();
        assert!(source.position <= 1);
        controls.playing.store(false, Ordering::Relaxed);
        let position = source.position;
        for _ in 0..2000 {
            source.next();
        }
        assert_eq!(source.position, position);
    }

    #[test]
    fn eof_stops_after_tail_and_can_rewind() {
        let controls = Arc::new(Controls::default());
        controls.publish(Settings {
            t60: 0.1,
            ..Settings::default()
        });
        controls.playing.store(true, Ordering::Relaxed);
        let clip = Arc::new(Clip {
            frames: vec![[1.0; 2]],
            original_rate: PREVIEW_RATE,
            original_channels: 1,
        });
        let mut source = PreviewSource::new(clip, controls.clone());
        for _ in 0..120_000 {
            source.next();
        }
        assert!(!controls.playing.load(Ordering::Relaxed));
        controls.rewind.store(true, Ordering::Relaxed);
        controls.playing.store(true, Ordering::Relaxed);
        for _ in 0..2 {
            source.next();
        }
        assert_eq!(source.position, 1);
    }
}
