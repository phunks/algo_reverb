use algo_reverb::colorless::{Profile, StaticFdn};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

fn write_wav(path: &Path, frames: &[[f64; 2]], rate: u32) -> Result<(), Box<dyn Error>> {
    let mut writer = hound::WavWriter::new(
        BufWriter::new(File::create_new(path)?),
        hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for frame in frames {
        for &x in frame {
            writer.write_sample(x as f32)?;
        }
    }
    writer.finalize()?;
    Ok(())
}

fn render(
    profile: Profile,
    t60: f64,
    input: &[f64],
    frames: usize,
) -> Result<Vec<[f64; 2]>, String> {
    let mut tank = StaticFdn::new(profile, t60)?;
    let mut result = Vec::with_capacity(frames);
    for i in 0..frames {
        let frame = tank.process_sample(input.get(i).copied().unwrap_or(0.0));
        if frame.iter().any(|x| !x.is_finite()) {
            return Err("nonfinite output".into());
        }
        result.push(frame);
    }
    Ok(result)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!(
            "Usage: fdn-compare INITIAL.fdn OPTIMIZED.fdn NEW_OUTPUT_DIRECTORY [t60=2] [INPUT.wav]"
        );
        println!(
            "Direct static FDN rendering, no convolution. Optional mono/stereo WAV is downmixed to mono."
        );
        println!(
            "WAV sample rate must match the profiles. Each pair is total-energy matched, not LUFS matched."
        );
        return Ok(());
    }
    if !(3..=5).contains(&args.len()) {
        return Err("see --help".into());
    }
    let profiles = [
        Profile::parse(&fs::read_to_string(&args[0])?)?,
        Profile::parse(&fs::read_to_string(&args[1])?)?,
    ];
    let rate = profiles[0].sample_rate();
    if rate != profiles[1].sample_rate() {
        return Err("profile sample rates differ".into());
    }
    let t60: f64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(2.0);
    // Validate before creating any outputs, even when no input WAV is supplied.
    StaticFdn::new(profiles[0].clone(), t60)?;
    let mut input = vec![1.0];
    let source = if let Some(path) = args.get(4) {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.sample_rate != rate || !(1..=2).contains(&spec.channels) {
            return Err("input must be mono/stereo at the profile sample rate".into());
        }
        let samples: Vec<f64> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .map(|x| x.map(f64::from))
                .collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
                reader
                    .samples::<i32>()
                    .map(|x| x.map(|v| f64::from(v) / scale))
                    .collect::<Result<_, _>>()?
            }
        };
        if samples.is_empty() || samples.iter().any(|x| !x.is_finite()) {
            return Err("empty or nonfinite input WAV".into());
        }
        input = samples
            .chunks_exact(usize::from(spec.channels))
            .map(|f| f.iter().sum::<f64>() / f.len() as f64)
            .collect();
        "input WAV (mono downmix)"
    } else {
        "unit impulse"
    };
    let frames = input.len() + ((t60 + 1.0) * f64::from(rate)).ceil() as usize;
    let mut rendered = profiles
        .into_iter()
        .map(|p| render(p, t60, &input, frames))
        .collect::<Result<Vec<_>, _>>()?;
    let energies: Vec<f64> = rendered
        .iter()
        .map(|x| x.iter().flatten().map(|v| v * v).sum())
        .collect();
    if energies.iter().any(|e| !e.is_finite() || *e <= 1e-20) {
        return Err("silent or invalid output".into());
    }
    let gains = [1.0, (energies[0] / energies[1]).sqrt()];
    let peak = rendered
        .iter()
        .zip(gains)
        .flat_map(|(x, g)| x.iter().flatten().map(move |v| (v * g).abs()))
        .fold(0.0, f64::max);
    // A single common headroom factor preserves pair matching and stereo balance.
    let common = 0.8 / peak.max(0.8);
    fs::create_dir(&args[2])?;
    let directory = Path::new(&args[2]);
    let mut report = BufWriter::new(File::create_new(directory.join("metrics.csv"))?);
    writeln!(
        report,
        "condition,raw_energy,gain,matched_energy,peak,lr_cross_energy,side_over_mid_db"
    )?;
    for (index, name) in ["initial", "optimized"].iter().enumerate() {
        let gain = gains[index] * common;
        for frame in &mut rendered[index] {
            for x in frame {
                *x *= gain;
            }
        }
        let mut ll = 0.0;
        let mut rr = 0.0;
        let mut lr = 0.0;
        let mut mid = 0.0;
        let mut side = 0.0;
        let mut peak: f64 = 0.0;
        for &[l, r] in &rendered[index] {
            ll += l * l;
            rr += r * r;
            lr += l * r;
            mid += (l + r).powi(2);
            side += (l - r).powi(2);
            peak = peak.max(l.abs()).max(r.abs());
        }
        writeln!(
            report,
            "{name},{},{gain},{},{peak},{},{}",
            energies[index],
            ll + rr,
            lr / (ll * rr).sqrt().max(1e-30),
            10.0 * (side.max(1e-30) / mid.max(1e-30)).log10()
        )?;
        write_wav(
            &directory.join(format!("{name}_wet.wav")),
            &rendered[index],
            rate,
        )?;
    }
    report.flush()?;
    let mut notes = File::create_new(directory.join("report.txt"))?;
    writeln!(
        notes,
        "Source: {source}\nRate: {rate}\nT60: {t60}\nStatic homogeneous decay; no damping, diffusion, modulation, dry mix or convolution.\nMatching: total stereo energy over this render; NOT perceptual loudness matching.\nInput stereo is averaged to mono. Begin listening at low volume.\nNo existing plugin behavior changed."
    )?;
    println!("Wrote matched wet-only pair to {}", directory.display());
    Ok(())
}
