use algo_reverb::ir_analysis::{Impulse, correlation, db, edc, energy, fit_decay, octave};
use algo_reverb::ir_synthesis::{
    HYBRID_HOLD_MS, HYBRID_TRANSITION_MS, SynthesisOptions, band_centers, synthesize,
};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

fn number(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.9}")).unwrap_or_default()
}
fn file(dir: &Path, name: &str) -> std::io::Result<BufWriter<File>> {
    Ok(BufWriter::new(File::create_new(dir.join(name))?))
}
fn write_wav(
    path: &Path,
    frames: &[[f64; 2]],
    rate: u32,
    channels: u16,
) -> Result<(), Box<dyn Error>> {
    let mut writer = hound::WavWriter::new(
        BufWriter::new(File::create_new(path)?),
        hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for f in frames {
        for v in f.iter().take(usize::from(channels)) {
            writer.write_sample(*v as f32)?;
        }
    }
    writer.finalize()?;
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!(
            "Usage: ir-compare INPUT.wav NEW_OUTPUT_DIRECTORY [early_ms=80] [fade_ms=20] [seed=1] [t60_seconds|auto] [density=2000] [density_rise_ms=50]"
        );
        println!(
            "Offline early-IR + FDN / VN / multiband VN / experimental routed-pulse Dark Velvet Noise."
        );
        println!(
            "Writes reference + 6 candidate float WAVs (including Hybrid and DVN), metrics.csv, curves.csv, windows.csv, report.txt."
        );
        return Ok(());
    }
    if !(2..=8).contains(&args.len()) {
        return Err("Expected input and new output directory; see --help".into());
    }
    let parse = |index: usize, default: f64| -> Result<f64, Box<dyn Error>> {
        Ok(match args.get(index) {
            Some(v) => v.to_str().ok_or("Non-UTF8 numeric argument")?.parse()?,
            None => default,
        })
    };
    let seed = args
        .get(4)
        .map(|v| {
            v.to_str()
                .ok_or("Non-UTF8 seed")?
                .parse::<u64>()
                .map_err(|_| "Seed must be a u64")
        })
        .transpose()?
        .unwrap_or(1);
    let options = SynthesisOptions {
        early_ms: parse(2, 80.0)?,
        fade_ms: parse(3, 20.0)?,
        seed,
        t60: if args.len() >= 6 && args[5] != "auto" {
            Some(parse(5, 0.0)?)
        } else {
            None
        },
        density: parse(6, 2000.0)?,
        density_rise_ms: parse(7, 50.0)?,
    };
    let input = Path::new(&args[0]);
    let output = Path::new(&args[1]);
    let ir = Impulse::load(input)?;
    let synthesis = synthesize(&ir, options)?;
    fs::create_dir(output)?;
    let mut report = file(output, "report.txt")?;
    writeln!(
        report,
        "Experimental early_hybrid: parallel time-domain handoff from Multiband VN to accepted Temporal FDN; component suffix energies matched first (+60 dB gain cap), common L/R weights, smoothstep equal-power transition. Copied early segment and final shared suffix-energy matching as other candidates. Not a feedback hybrid, not an optimized transition; no loudness or peak guarantee.\nhybrid_hold_ms={HYBRID_HOLD_MS}\nhybrid_transition_ms={HYBRID_TRANSITION_MS}\nhybrid_anchor_samples={}",
        synthesis.early_samples + synthesis.fade_samples
    )?;
    writeln!(
        report,
        "input={}\nsample_rate={}\nchannels={}\nframes={}\nearly_samples={}\nfade_samples={}\nseed={}\nfdn_t60_seconds={}\nfdn_t60_source={}",
        input.display(),
        ir.sample_rate,
        ir.channels,
        ir.frames.len(),
        synthesis.early_samples,
        synthesis.fade_samples,
        options.seed,
        synthesis.t60,
        synthesis.t60_source
    )?;
    writeln!(
        report,
        "EXPERIMENTAL OFFLINE BASELINES. No plugin changes, automatic winner, listening-quality claim, or real-time CPU comparison."
    )?;
    writeln!(
        report,
        "All WAVs are native-rate, original-length float32, no peak normalization/clipping/limiter. Peaks can exceed unity: start at LOW volume. reference.wav is a format conversion, not a byte copy."
    )?;
    writeln!(
        report,
        "All candidates copy the early segment verbatim, then equal-power crossfade for fade_samples. These are FILE-relative times, not automatically detected mixing time. No common final tail fade; original-length truncation can click."
    )?;
    writeln!(
        report,
        "FDN: Classic Hybrid, full diffusion, no modulation, threshold-onset-aligned excitation. Initial nominal T60: screened broadband T30/T20/EDT or explicit override. Bounded native-rate coordinate search adjusts nominal T60, low-pass Hz and Low/Mid dB (NOT T60 multipliers), shape=500/2000 Hz Q=0.7. Objective: post-EQ temporal shape, mean band dB-squared score with equal normalized EDC / absolute 20 ms power weights, broadband plus available 125..8000 Hz octaves. Reference-only masks retain EDC >= -35 dB and power > max(terminal-ten-window median +20 dB, band peak -50 dB, -240 dBFS); >=3 windows and relative band energy >1e-8 required. Errors clipped to +/-40 dB. Terminal floor is a conservative heuristic, not a noise classification. Also tests a baseline at the longest valid screened T60 target. Three search scales, maximum 26 evaluations, maximum six-second render horizon. Explicit override freezes nominal T60 only. No exact fit guarantee; absent shape masks skip optimization."
    )?;
    writeln!(
        report,
        "FDN post-loop energy EQ: phase-compensated sequential LR4 synthesis bank including DC/Nyquist residuals; static shared-L/R gains match reference suffix component energies with 1e-24 mean-square floor and +60 dB cap, then sum and apply final shared normalization. Search evaluates this EQ/normalization path, with onset and causal filter history retained; horizons longer than six seconds may differ from final full-length EQ. Isolated component normalized EDC is gain-invariant, but overlap/phase and crossover transients can change post-sum octave T60; no exact post-sum band-energy guarantee. Analyzer filters unchanged."
    )?;
    for band in &synthesis.fdn_eq_bands {
        writeln!(
            report,
            "fdn_eq_band_hz={}..{}: suffix_amplitude_gain={:.12e}, gain_capped={}, gain_source=post_loop_pre_sum_suffix_energy",
            band.lower_hz, band.upper_hz, band.late_gain, band.gain_capped
        )?;
        if band.gain_capped {
            writeln!(
                report,
                "WARNING: fdn_eq_band_hz={}..{} reached +60 dB gain cap; reference band energy may be unattainable. Final shared normalization is separate from this cap.",
                band.lower_hz, band.upper_hz
            )?;
        }
    }
    writeln!(
        report,
        "Experimental early_fdn_stereo: SAME fitted FDN parameters as early_fdn; static zero-offset two-carrier L/R transform fits suffix Pearson correlation and channel energies in synthesis bands whose upper edge is <= 500*sqrt(2) Hz, including the DC residual. Applied before band energy EQ/summation and shared suffix normalization, outside the feedback loop. Mono bypasses stereo transform. Degenerate correlation/carriers skip correction; transform coefficients bounded as in VN. Does not fit time-varying IACC or guarantee post-sum octave correlation. May alter time envelopes and spectral balance; NOT included in temporal search and NOT a replacement for accepted early_fdn. Preserves reference bias rather than centering it."
    )?;
    for band in synthesis.fdn_band_damping {
        writeln!(
            report,
            "fdn_band_hz={}: screened_t60_seconds={}, additional_attenuation_db={:.9}, fit_status={}, t60_source=full_reference_analytic_screen, attenuation_source=post_search_applied",
            band.center_hz,
            number(band.t60),
            band.attenuation_db,
            if band.t60.is_some() {
                "reference_only"
            } else {
                "unavailable"
            }
        )?;
        if band.attenuation_db <= 0.0 || band.attenuation_db >= 24.0 {
            writeln!(
                report,
                "WARNING: fdn_band_hz={} attenuation reached 0/24 dB boundary; inspect target attainability, search limits and nominal all-pass delay conversion bias.",
                band.center_hz
            )?;
        }
    }
    writeln!(
        report,
        "fdn_fitted_nominal_t60={}\nfdn_fitted_lowpass_hz={}\nfdn_fit_objective=post_eq_edc_window_power_db_squared\nfdn_fit_initial_shape_error_db2={}\nfdn_fit_final_shape_error_db2={}\nfdn_fit_evaluations={}",
        synthesis.fdn_fit.nominal_t60,
        synthesis.fdn_fit.damping_hz,
        number(synthesis.fdn_fit.initial_error),
        number(synthesis.fdn_fit.fitted_error),
        synthesis.fdn_fit.evaluations
    )?;
    let fdn_candidate = synthesis
        .candidates
        .iter()
        .find(|candidate| candidate.name == "early_fdn")
        .ok_or("Missing early_fdn candidate for achieved decay reporting")?;
    for &(hz, target) in &synthesis.fdn_fit.targets {
        writeln!(report, "fdn_late_target_hz={hz}: t60_seconds={target}")?;
        let rendered = octave(&fdn_candidate.frames, ir.sample_rate, hz);
        let suffix = &rendered[synthesis.early_samples + synthesis.fade_samples..];
        let curve = edc(suffix);
        let achieved = [(-5.0, -35.0), (-5.0, -25.0), (0.0, -10.0)]
            .into_iter()
            .find_map(|(upper, lower)| {
                let fit = fit_decay(suffix, &curve, ir.sample_rate, upper, lower);
                (fit.status == "reference_only")
                    .then_some(fit.t60)
                    .flatten()
            });
        writeln!(
            report,
            "fdn_achieved_late_hz={hz}: t60_seconds={}",
            number(achieved)
        )?;
    }
    writeln!(
        report,
        "vn_density={}\nvn_density_rise_ms={}\nvn_density_anchor_samples={}",
        options.density,
        options.density_rise_ms,
        synthesis.early_samples + synthesis.fade_samples
    )?;
    for band in &synthesis.vn_bands {
        writeln!(
            report,
            "vn_band_hz={}..{}: target_correlation={}, fitted_correlation={}, late_gain={}",
            band.lower_hz,
            band.upper_hz,
            number(band.target_correlation),
            number(band.actual_correlation),
            band.late_gain
        )?;
    }
    writeln!(
        report,
        "VN: random +/-1 pulse per variable cell/channel. Density rises exponentially from 25% to requested density anchored at crossfade end (first fully synthetic sample), held at 25% before that anchor; rise_ms=0 selects constant density. Per-channel 20 ms RMS envelope gains interpolated between centers, 1e-24 mean-square floor and +60 dB cap. Suffix Pearson correlation fitted by orthogonalized two-carrier mixing; correlation fitting can smear per-channel envelope differences."
    )?;
    writeln!(
        report,
        "Multiband VN: synthesis-only phase-compensated sequential LR4 bank, octave-edge crossovers with low-pass and high-pass residuals covering DC..Nyquist. Coherent unmodified sum is flat in magnitude, not phase; independent noise bands do not have flat summed power at crossovers. Envelopes and suffix Pearson correlations fitted per band, then per-band suffix-energy corrected (+60 dB cap). Final shared gain follows summation. NOT paper DVN. Band overlap, correlation mixing and envelope modulation can bias post-sum spectra; no exact post-sum band energy/correlation guarantee. Analyzer octave filters unchanged."
    )?;
    writeln!(
        report,
        "VN envelopes use the entire reference including noise: descriptive reconstruction, not compact extrapolation. No noise compensation. Stereo correlation fitting is global per synthetic suffix (broadband VN) or synthesis band (multiband VN), not time-varying IACC; undefined/degenerate fits are skipped. Mono candidates duplicate L."
    )?;
    writeln!(
        report,
        "Dark Velvet Noise (early_dvn): one sparse pulse train/channel, each pulse routed to exactly one fixed LR4 synthesis filter. Interpolated 20 ms reference band powers set time-varying routing probabilities (95% energy-proportional + 5% uniform) and inverse-probability/impulse-energy-normalized pulse weights (+60 dB cap). Independent seed stream; no FDN, no independent per-band pulse generators. Global per-band stereo fit follows filtering. Reference noise is included; not a compact decay extrapolator or a specific paper reproduction. Sparse routing, overlap and stereo fitting can bias spectral/envelope matching."
    )?;
    writeln!(
        report,
        "Every candidate gets one shared amplitude gain matching reference energy ONLY after early+fade. No per-method peak matching. Full metrics include copied early IR; late metrics EXCLUDE the crossfade and copied samples."
    )?;
    writeln!(
        report,
        "Band filters run from file start before region slicing. Region EDC normalized independently; RMSE compares dB where reference is -5..-35 dB, >=16 valid samples. Candidate zero energy in that range makes RMSE undefined. This is a shape comparison, not a confidence score."
    )?;
    writeln!(
        report,
        "Window power RMSE compares nonoverlapping 20 ms blocks with reference >=-60 dB of its strongest block. Blank if candidate power is zero. Correlation is zero-lag Pearson, NOT IACC. EDT/T20/T30 statuses follow ir-analyze; late fit times are relative to late-region start."
    )?;
    writeln!(
        report,
        "curves.csv is sampled at ~1 ms, windows.csv at 20 ms. Metrics use all samples. All fitting gains and measured peaks are below. IR-derived output may be subject to reference redistribution license."
    )?;
    write_wav(
        &output.join("reference.wav"),
        &ir.frames,
        ir.sample_rate,
        ir.channels,
    )?;
    let mut metrics = file(output, "metrics.csv")?;
    let mut curves = file(output, "curves.csv")?;
    let mut windows = file(output, "windows.csv")?;
    writeln!(
        metrics,
        "method,region,band_hz,energy_db_vs_reference,edc_rmse_db,edc_points,window_power_rmse_db,window_count,lr_correlation,edt_seconds,edt_status,t20_seconds,t20_status,t30_seconds,t30_status"
    )?;
    writeln!(
        curves,
        "method,region,band_hz,time_seconds,reference_edc_db,candidate_edc_db"
    )?;
    writeln!(
        windows,
        "method,region,band_hz,start_seconds,reference_power_dbfs,candidate_power_dbfs,reference_lr_correlation,candidate_lr_correlation,reference_left_power_dbfs,reference_right_power_dbfs,candidate_left_power_dbfs,candidate_right_power_dbfs"
    )?;
    let mut methods = vec![("reference", &ir.frames[..])];
    for candidate in &synthesis.candidates {
        write_wav(
            &output.join(format!("{}.wav", candidate.name)),
            &candidate.frames,
            ir.sample_rate,
            ir.channels,
        )?;
        let peak = candidate
            .frames
            .iter()
            .flatten()
            .fold(0.0_f64, |a, v| a.max(v.abs()));
        writeln!(
            report,
            "{}: late_amplitude_gain={:.12e}, peak_dbfs={}",
            candidate.name,
            candidate.late_gain,
            number(db(peak * peak))
        )?;
        methods.push((candidate.name, &candidate.frames));
    }
    let mut bands = vec![None];
    bands.extend(band_centers(ir.sample_rate).into_iter().map(Some));
    let late = synthesis.early_samples + synthesis.fade_samples;
    let rate = f64::from(ir.sample_rate);
    for band in bands {
        let label = band
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "broadband".into());
        let filtered_reference;
        let reference = if let Some(hz) = band {
            filtered_reference = octave(&ir.frames, ir.sample_rate, hz);
            &filtered_reference[..]
        } else {
            &ir.frames[..]
        };
        for (name, raw) in &methods {
            let filtered;
            let frames = if let Some(hz) = band {
                filtered = octave(raw, ir.sample_rate, hz);
                &filtered[..]
            } else {
                raw
            };
            for (region, start) in [("full", 0), ("late", late)] {
                let target = &reference[start..];
                let candidate = &frames[start..];
                let r_edc = edc(target);
                let c_edc = edc(candidate);
                let mut errors = Vec::new();
                let mut missing = false;
                for (&r, &c) in r_edc.iter().zip(&c_edc) {
                    if let Some(r) = db(r).filter(|v| (-35.0..=-5.0).contains(v)) {
                        if let Some(c) = db(c) {
                            errors.push((c - r).powi(2));
                        } else {
                            missing = true;
                        }
                    }
                }
                let rmse = (!missing && errors.len() >= 16)
                    .then(|| (errors.iter().sum::<f64>() / errors.len() as f64).sqrt());
                let r_energy = target.iter().map(energy).sum::<f64>();
                let c_energy = candidate.iter().map(energy).sum::<f64>();
                let energy_error = if r_energy > 0.0 {
                    db(c_energy / r_energy)
                } else {
                    None
                };
                let edt = fit_decay(candidate, &c_edc, ir.sample_rate, 0.0, -10.0);
                let t20 = fit_decay(candidate, &c_edc, ir.sample_rate, -5.0, -25.0);
                let t30 = fit_decay(candidate, &c_edc, ir.sample_rate, -5.0, -35.0);
                let corr = if ir.channels == 2 {
                    correlation(candidate)
                } else {
                    None
                };
                let block = (ir.sample_rate as usize / 50).max(1);
                let peak_power = target
                    .chunks(block)
                    .map(|c| c.iter().map(energy).sum::<f64>() / c.len() as f64)
                    .fold(0.0_f64, f64::max);
                let mut power_errors = Vec::new();
                let mut power_missing = false;
                for (i, (r, c)) in target
                    .chunks(block)
                    .zip(candidate.chunks(block))
                    .enumerate()
                {
                    let rp = r.iter().map(energy).sum::<f64>() / r.len() as f64;
                    let cp = c.iter().map(energy).sum::<f64>() / c.len() as f64;
                    if rp > 0.0 && rp >= peak_power * 1e-6 {
                        if let Some(error) = db(cp / rp) {
                            power_errors.push(error * error);
                        } else {
                            power_missing = true;
                        }
                    }
                    writeln!(
                        windows,
                        "{name},{region},{label},{:.9},{},{},{},{},{},{},{},{}",
                        (start + i * block) as f64 / rate,
                        number(db(rp)),
                        number(db(cp)),
                        number(if ir.channels == 2 {
                            correlation(r)
                        } else {
                            None
                        }),
                        number(if ir.channels == 2 {
                            correlation(c)
                        } else {
                            None
                        }),
                        number(db(
                            r.iter().map(|f| f[0].powi(2)).sum::<f64>() / r.len() as f64
                        )),
                        number(db(
                            r.iter().map(|f| f[1].powi(2)).sum::<f64>() / r.len() as f64
                        )),
                        number(db(
                            c.iter().map(|f| f[0].powi(2)).sum::<f64>() / c.len() as f64
                        )),
                        number(db(
                            c.iter().map(|f| f[1].powi(2)).sum::<f64>() / c.len() as f64
                        ))
                    )?;
                }
                let power_rmse = (!power_missing && !power_errors.is_empty())
                    .then(|| (power_errors.iter().sum::<f64>() / power_errors.len() as f64).sqrt());
                writeln!(
                    metrics,
                    "{name},{region},{label},{},{},{},{},{},{},{},{},{},{},{},{}",
                    number(energy_error),
                    number(rmse),
                    errors.len(),
                    number(power_rmse),
                    power_errors.len(),
                    number(corr),
                    number(edt.t60),
                    edt.status,
                    number(t20.t60),
                    t20.status,
                    number(t30.t60),
                    t30.status
                )?;
                let step = (ir.sample_rate as usize / 1000).max(1);
                let last = target.len() - 1;
                for i in (0..target.len())
                    .step_by(step)
                    .chain((last % step != 0).then_some(last))
                {
                    writeln!(
                        curves,
                        "{name},{region},{label},{:.9},{},{}",
                        (start + i) as f64 / rate,
                        number(db(r_edc[i])),
                        number(db(c_edc[i]))
                    )?;
                }
            }
        }
    }
    report.flush()?;
    metrics.flush()?;
    curves.flush()?;
    windows.flush()?;
    println!("Comparison saved to {}", output.display());
    println!(
        "FDN nominal T60: {:.6} s ({})",
        synthesis.t60, synthesis.t60_source
    );
    println!("Compare late metrics separately; listen at LOW volume. No automatic winner.");
    Ok(())
}
