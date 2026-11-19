use algo_reverb::ir_analysis::{Impulse, correlation, db, edc, energy, fit_decay, octave};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

fn number(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.9}")).unwrap_or_default()
}

fn csv(dir: &Path, name: &str) -> std::io::Result<BufWriter<File>> {
    Ok(BufWriter::new(File::create_new(dir.join(name))?))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("Usage: ir-analyze INPUT.wav NEW_OUTPUT_DIRECTORY");
        println!("Native-rate offline IR analysis; output directory must not exist.");
        println!("Writes metadata.txt, waveform.csv, edc.csv, windows.csv, decay.csv.");
        return Ok(());
    }
    if args.len() != 2 {
        return Err("Expected INPUT.wav NEW_OUTPUT_DIRECTORY; see --help".into());
    }
    let input = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let impulse = Impulse::load(&input)?;
    // Atomic directory reservation prevents accidental overwrite, even in concurrent runs.
    fs::create_dir(&output)?;
    analyze(&impulse, &input, &output)?;
    println!("Analysis saved to {}", output.display());
    Ok(())
}

fn analyze(ir: &Impulse, input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let rate = f64::from(ir.sample_rate);
    let peak = ir
        .frames
        .iter()
        .flatten()
        .fold(0.0_f64, |a, b| a.max(b.abs()));
    let total = ir.frames.iter().map(energy).sum::<f64>();
    if !total.is_finite() {
        return Err("WAV energy exceeds analysis range".into());
    }
    let onset = ir
        .frames
        .iter()
        .position(|f| f.iter().any(|v| v.abs() > peak * 0.001));
    let mut meta = csv(output, "metadata.txt")?;
    writeln!(meta, "input={}", input.display())?;
    writeln!(
        meta,
        "sample_rate_hz={}\nchannels={}\nbits_per_sample={}\nframes={}\nduration_seconds={:.9}",
        ir.sample_rate,
        ir.channels,
        ir.bits,
        ir.frames.len(),
        ir.frames.len() as f64 / rate
    )?;
    writeln!(
        meta,
        "peak_dbfs={}\ntotal_mean_channel_energy={}\nonset_minus60db_peak_seconds={}",
        number(db(peak * peak)),
        total,
        number(onset.map(|i| i as f64 / rate))
    )?;
    writeln!(
        meta,
        "No resampling, normalization, clipping, DC removal, onset trimming, direct-sound removal or noise compensation."
    )?;
    writeln!(
        meta,
        "Time zero is file start. Mono is duplicated, but correlation is undefined."
    )?;
    writeln!(
        meta,
        "EDC: backwards integration of (L^2+R^2)/2; normalized separately per band. Zero energy has blank dB."
    )?;
    writeln!(
        meta,
        "Bands: causal second-order Butterworth HP+LP at center/sqrt(2), center*sqrt(2); broad overlapping approximation, NOT IEC 61260 compliant. No tail padding; filter transient can bias short decays."
    )?;
    writeln!(
        meta,
        "EDT/T20/T30 are T60 extrapolations from 0..-10/-5..-25/-5..-35 dB EDC slopes. Not certified measurements."
    )?;
    writeln!(
        meta,
        "reference_only: R^2>=0.98 and passed heuristic tail screening, NOT a confidence guarantee. nonlinear_decay: R^2<0.98; slope is only descriptive."
    )?;
    writeln!(
        meta,
        "insufficient_range/near_file_end/tail_floor_risk: T60 suppressed. Last 10% mean power is a conservative tail-floor proxy, not a measured noise floor."
    )?;
    writeln!(
        meta,
        "Fits require >=5 ms and >=16 samples, >=50 ms after the lower crossing, and >10 dB local power margin above tail proxy."
    )?;
    writeln!(
        meta,
        "windows.csv uses nonoverlapping 10 ms intervals (last may be shorter). Correlation is zero-lag mean-subtracted Pearson, NOT IACC."
    )?;
    writeln!(
        meta,
        "Onset is the first sample above -60 dB relative to peak, NOT a direct-arrival detector. No automatic mixing-time estimate."
    )?;
    meta.flush()?;

    let mut waveform = csv(output, "waveform.csv")?;
    writeln!(waveform, "sample,time_seconds,left,right")?;
    for (i, f) in ir.frames.iter().enumerate() {
        writeln!(
            waveform,
            "{i},{:.9},{:.12e},{:.12e}",
            i as f64 / rate,
            f[0],
            f[1]
        )?;
    }
    waveform.flush()?;

    let mut curves = csv(output, "edc.csv")?;
    let mut windows = csv(output, "windows.csv")?;
    let mut decays = csv(output, "decay.csv")?;
    writeln!(
        curves,
        "band_hz,sample,time_seconds,normalized_energy,edc_db"
    )?;
    writeln!(
        windows,
        "band_hz,start_seconds,end_seconds,mean_power_dbfs,energy_fraction,lr_correlation,peak_dbfs"
    )?;
    writeln!(
        decays,
        "band_hz,metric,t60_seconds,r_squared,fit_start_seconds,fit_end_seconds,status,band_energy,tail_power_dbfs"
    )?;
    let mut bands = vec![None];
    bands.extend(
        [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0]
            .into_iter()
            .filter(|hz| hz * 2.0_f64.sqrt() < rate * 0.5)
            .map(Some),
    );
    println!(
        "{}: {} Hz, {} channel(s), {:.3} s",
        input.display(),
        ir.sample_rate,
        ir.channels,
        ir.frames.len() as f64 / rate
    );
    println!("Band       Metric  T60 / s     R²        Status (all estimates provisional)");
    for band in bands {
        let filtered;
        let frames = if let Some(center) = band {
            filtered = octave(&ir.frames, ir.sample_rate, center);
            &filtered[..]
        } else {
            &ir.frames[..]
        };
        let label = band
            .map(|hz| format!("{hz:.0}"))
            .unwrap_or_else(|| "broadband".into());
        let curve = edc(frames);
        // EDC export at ~1 ms, plus the final sample; fitting uses all samples.
        let step = (ir.sample_rate as usize / 1000).max(1);
        let last = frames.len() - 1;
        for i in (0..frames.len())
            .step_by(step)
            .chain((last % step != 0).then_some(last))
        {
            writeln!(
                curves,
                "{label},{i},{:.9},{:.12e},{}",
                i as f64 / rate,
                curve[i],
                number(db(curve[i]))
            )?;
        }
        let band_energy = frames.iter().map(energy).sum::<f64>();
        let tail_len = (frames.len() / 10).max(1);
        let tail_power = frames[frames.len() - tail_len..]
            .iter()
            .map(energy)
            .sum::<f64>()
            / tail_len as f64;
        for (metric, upper, lower) in [
            ("EDT", 0.0, -10.0),
            ("T20", -5.0, -25.0),
            ("T30", -5.0, -35.0),
        ] {
            let fit = fit_decay(frames, &curve, ir.sample_rate, upper, lower);
            println!(
                "{label:10} {metric:6} {:11} {:9} {}",
                number(fit.t60),
                number(fit.r_squared),
                fit.status
            );
            writeln!(
                decays,
                "{label},{metric},{},{},{},{},{},{:.12e},{}",
                number(fit.t60),
                number(fit.r_squared),
                number(fit.start_seconds),
                number(fit.end_seconds),
                fit.status,
                band_energy,
                number(db(tail_power))
            )?;
        }
        let window = (ir.sample_rate as usize / 100).max(1);
        for (block, chunk) in frames.chunks(window).enumerate() {
            let e = chunk.iter().map(energy).sum::<f64>();
            let peak = chunk.iter().flatten().fold(0.0_f64, |a, b| a.max(b.abs()));
            let corr = if ir.channels == 2 {
                correlation(chunk)
            } else {
                None
            };
            writeln!(
                windows,
                "{label},{:.9},{:.9},{},{},{},{}",
                (block * window) as f64 / rate,
                (block * window + chunk.len()) as f64 / rate,
                number(db(e / chunk.len() as f64)),
                number((band_energy > 0.0).then(|| e / band_energy)),
                number(corr),
                number(db(peak * peak))
            )?;
        }
    }
    curves.flush()?;
    windows.flush()?;
    decays.flush()?;
    Ok(())
}
