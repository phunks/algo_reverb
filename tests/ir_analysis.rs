#![cfg(feature = "ir-analysis")]

use algo_reverb::ir_analysis::Impulse;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ir-analysis-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli(input: &std::path::Path, output: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ir-analyze"))
        .arg(input)
        .arg(output)
        .output()
        .unwrap()
}

#[test]
fn pcm_loading_and_cli_exports_preserve_source_and_refuse_overwrite() {
    let scratch = Scratch::new();
    let input = scratch.0.join("source.wav");
    let mut writer = hound::WavWriter::create(
        &input,
        hound::WavSpec {
            channels: 2,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..16000 {
        let sample = (10.0_f64.powf(-3.0 * f64::from(i) / 8000.0) * 32000.0).round() as i16;
        writer.write_sample(sample).unwrap();
        writer.write_sample(-sample).unwrap();
    }
    writer.finalize().unwrap();
    let original = std::fs::read(&input).unwrap();
    let loaded = Impulse::load(&input).unwrap();
    assert_eq!(loaded.frames.len(), 16000);
    assert_eq!(loaded.frames[0], [32000.0 / 32768.0, -32000.0 / 32768.0]);
    let output = scratch.0.join("report");
    let result = cli(&input, &output);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for name in [
        "metadata.txt",
        "waveform.csv",
        "edc.csv",
        "windows.csv",
        "decay.csv",
    ] {
        let text = std::fs::read_to_string(output.join(name)).unwrap();
        assert!(!text.is_empty());
        assert!(!text.contains("NaN"));
        assert!(!text.contains(",inf"));
    }
    let decay = std::fs::read_to_string(output.join("decay.csv")).unwrap();
    let t20: f64 = decay
        .lines()
        .find(|line| line.starts_with("broadband,T20,"))
        .unwrap()
        .split(',')
        .nth(2)
        .unwrap()
        .parse()
        .unwrap();
    assert!((t20 - 1.0).abs() < 0.005, "{t20}");
    let edc = std::fs::read_to_string(output.join("edc.csv")).unwrap();
    assert!(edc.contains("broadband,15999,"));
    let before = std::fs::read(output.join("decay.csv")).unwrap();
    assert!(!cli(&input, &output).status.success());
    assert_eq!(before, std::fs::read(output.join("decay.csv")).unwrap());
    assert_eq!(original, std::fs::read(&input).unwrap());
}

#[test]
fn float_mono_preserves_level_and_invalid_samples_are_rejected() {
    let scratch = Scratch::new();
    for (name, sample, valid) in [("mono.wav", 1.25, true), ("nan.wav", f32::NAN, false)] {
        let input = scratch.0.join(name);
        let mut writer = hound::WavWriter::create(
            &input,
            hound::WavSpec {
                channels: 1,
                sample_rate: 44100,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..441 {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let loaded = Impulse::load(&input);
        if valid {
            assert_eq!(loaded.unwrap().frames[0], [1.25; 2]);
            let output = scratch.0.join("mono-report");
            assert!(cli(&input, &output).status.success());
            let windows = std::fs::read_to_string(output.join("windows.csv")).unwrap();
            assert_eq!(windows.lines().nth(1).unwrap().split(',').nth(5), Some(""));
        } else {
            assert!(loaded.is_err());
            let output = scratch.0.join("invalid-report");
            assert!(!cli(&input, &output).status.success());
            assert!(!output.exists());
        }
    }
    let input = scratch.0.join("broken.wav");
    std::fs::write(&input, b"not a wav").unwrap();
    assert!(Impulse::load(&input).is_err());
}

#[test]
fn comparison_cli_exports_reproducible_wavs_and_valid_metrics() {
    let scratch = Scratch::new();
    let input = scratch.0.join("reference.wav");
    let mut writer = hound::WavWriter::create(
        &input,
        hound::WavSpec {
            channels: 2,
            sample_rate: 8000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..8000 {
        let a = 10.0_f32.powf(-3.0 * i as f32 / 8000.0);
        writer.write_sample(a).unwrap();
        writer.write_sample(-a).unwrap();
    }
    writer.finalize().unwrap();
    let original = std::fs::read(&input).unwrap();
    let run = |dest: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_ir-compare"))
            .arg(&input)
            .arg(dest)
            .args(["60", "20", "42", "1.0"])
            .output()
            .unwrap()
    };
    let first = scratch.0.join("first");
    let second = scratch.0.join("second");
    for dest in [&first, &second] {
        let result = run(dest);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for name in [
            "reference",
            "early_fdn",
            "early_vn",
            "early_multiband_vn",
            "early_fdn_stereo",
            "early_hybrid",
            "early_dvn",
        ] {
            let wav = Impulse::load(&dest.join(format!("{name}.wav"))).unwrap();
            assert_eq!(wav.sample_rate, 8000);
            assert_eq!(wav.frames.len(), 8000);
            let reference = Impulse::load(&input).unwrap();
            assert_eq!(&wav.frames[..480], &reference.frames[..480]);
        }
        for name in ["metrics.csv", "curves.csv", "windows.csv"] {
            let text = std::fs::read_to_string(dest.join(name)).unwrap();
            let mut lines = text.lines();
            let width = lines.next().unwrap().split(',').count();
            assert!(lines.all(|line| line.split(',').count() == width));
            assert!(!text.contains("NaN"));
            assert!(!text.contains(",inf"));
        }
        let text = std::fs::read_to_string(dest.join("metrics.csv")).unwrap();
        let report = std::fs::read_to_string(dest.join("report.txt")).unwrap();
        assert!(report.contains("Dark Velvet Noise (early_dvn)"));
        assert!(text.contains("early_dvn,late,broadband,"));
        assert!(report.contains("NOT T60 multipliers"));
        assert!(report.contains("fdn_band_hz=250:"));
        assert!(report.contains("fdn_band_hz=2000:"));
        assert!(report.contains("additional_attenuation_db="));
        assert!(report.contains("attenuation_source=post_search_applied"));
        assert!(report.contains("FDN post-loop energy EQ:"));
        assert!(report.contains("Experimental early_fdn_stereo:"));
        assert!(report.contains("Experimental early_hybrid:"));
        assert!(
            report.contains(
                "hybrid_hold_ms=100\nhybrid_transition_ms=400\nhybrid_anchor_samples=640"
            )
        );
        assert!(text.contains("early_hybrid,late,broadband,"));
        assert!(report.contains("fdn_eq_band_hz=0.."));
        assert!(report.contains("gain_source=post_loop_pre_sum_suffix_energy"));
        assert!(report.contains("gain_capped="));
        assert!(report.contains("t60_source=full_reference_analytic_screen"));
        assert!(report.contains("vn_density_anchor_samples=640\n"));
        assert!(report.contains("anchored at crossfade end"));
        assert!(report.contains("correlation fitting can smear per-channel envelope differences"));
        let windows = std::fs::read_to_string(dest.join("windows.csv")).unwrap();
        assert!(
            windows
                .lines()
                .next()
                .unwrap()
                .ends_with("candidate_left_power_dbfs,candidate_right_power_dbfs")
        );
        for row in windows
            .lines()
            .skip(1)
            .filter(|line| line.starts_with("reference,"))
        {
            let fields: Vec<_> = row.split(',').collect();
            assert_eq!(fields[8], fields[10]);
            assert_eq!(fields[9], fields[11]);
        }
        assert!(report.contains("per-band suffix-energy corrected"));
        assert!(report.contains("fdn_fitted_nominal_t60=1\n"));
        assert!(report.contains("fdn_fit_evaluations="));
        assert!(report.contains("fdn_fit_objective=post_eq_edc_window_power_db_squared"));
        assert!(report.contains("fdn_fit_final_shape_error_db2="));
        assert!(!report.contains("fdn_fit_final_log_error="));
        assert!(report.contains("vn_band_hz=0.."));
        assert!(report.contains("fitted_correlation="));
        for line in text
            .lines()
            .skip(1)
            .filter(|line| line.starts_with("reference,"))
        {
            let cells: Vec<_> = line.split(',').collect();
            assert_eq!(cells[3], "0.000000000");
            if !cells[4].is_empty() {
                assert_eq!(cells[4], "0.000000000");
            }
        }
    }
    for name in [
        "reference.wav",
        "early_fdn.wav",
        "early_fdn_stereo.wav",
        "early_vn.wav",
        "early_multiband_vn.wav",
        "early_hybrid.wav",
        "early_dvn.wav",
        "metrics.csv",
    ] {
        assert_eq!(
            std::fs::read(first.join(name)).unwrap(),
            std::fs::read(second.join(name)).unwrap()
        );
    }
    assert!(!run(&first).status.success());
    assert_eq!(original, std::fs::read(&input).unwrap());
    let invalid = scratch.0.join("invalid");
    let result = Command::new(env!("CARGO_BIN_EXE_ir-compare"))
        .arg(&input)
        .arg(&invalid)
        .arg("NaN")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!invalid.exists());
}
