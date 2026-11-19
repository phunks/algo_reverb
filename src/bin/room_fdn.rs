//! Render offline-designed room FDNs without convolution; report realtime cost.
use algo_reverb::colorless::StaticFdn;
use algo_reverb::difffdn::DiffFdn;
use std::{error::Error, fs, path::Path, time::Instant};

enum Tank {
    Static(StaticFdn),
    Diff(DiffFdn),
}
impl Tank {
    fn reset(&mut self) {
        match self {
            Self::Static(f) => f.reset(),
            Self::Diff(f) => f.reset(),
        }
    }
    fn process_sample(&mut self, x: f64) -> [f64; 2] {
        match self {
            Self::Static(f) => f.process_sample(x),
            Self::Diff(f) => [f.process_sample(x); 2],
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!(
            "Usage: room-fdn NEW_DIRECTORY SECONDS ROOM_PROFILE [ROOM_PROFILE ...]\nWet-only unit impulses; common peak attenuation, no independent normalization. Profiles must have the same rate. Writes CPU timing and stereo metrics."
        );
        return Ok(());
    }
    if args.len() < 3 {
        return Err("see --help".into());
    }
    let seconds: f64 = args[1].parse()?;
    if !seconds.is_finite() || !(0.2..=60.0).contains(&seconds) {
        return Err("seconds outside 0.2..60".into());
    }
    let mut results = Vec::new();
    let mut rate = 0;
    let mut peak = 0.0_f64;
    let mut report = String::from(
        "profile,ns_per_sample,realtime_percent,energy,lr_cross_energy,side_over_mid_db\n",
    );
    for path in &args[2..] {
        let text = fs::read_to_string(path)?;
        let current_rate: u32 = text
            .split_whitespace()
            .nth(1)
            .ok_or("missing rate")?
            .parse()?;
        if rate != 0 && current_rate != rate {
            return Err("profile rates differ".into());
        }
        rate = current_rate;
        let mut tank = if text.starts_with("difffdn-scattering-v1") {
            Tank::Diff(DiffFdn::parse(&text)?)
        } else {
            Tank::Static(StaticFdn::from_room_profile(&text)?)
        };
        let len = (seconds * f64::from(rate)).ceil() as usize;
        let mut frames = vec![[0.0; 2]; len];
        for i in 0..rate / 10 {
            std::hint::black_box(tank.process_sample(if i == 0 { 1.0 } else { 0.0 }));
        }
        tank.reset();
        let start = Instant::now();
        for (i, f) in frames.iter_mut().enumerate() {
            *f = tank.process_sample(if i == 0 { 1.0 } else { 0.0 });
        }
        let elapsed = start.elapsed().as_secs_f64();
        let (mut ll, mut rr, mut lr, mut mid, mut side) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &[l, r] in &frames {
            if !l.is_finite() || !r.is_finite() {
                return Err("nonfinite render".into());
            }
            peak = peak.max(l.abs()).max(r.abs());
            ll += l * l;
            rr += r * r;
            lr += l * r;
            mid += (l + r).powi(2);
            side += (l - r).powi(2);
        }
        if ll + rr <= 1e-24 {
            return Err("silent render".into());
        }
        // Use numbered names to avoid collisions between identically named profiles.
        report.push_str(&format!(
            "{:?},{},{},{},{},{}\n",
            path,
            elapsed * 1e9 / len as f64,
            elapsed / (len as f64 / f64::from(rate)) * 100.0,
            ll + rr,
            lr / (ll * rr).sqrt().max(1e-30),
            10.0 * (side.max(1e-30) / mid.max(1e-30)).log10()
        ));
        results.push(frames);
    }
    let common = 0.8 / peak.max(0.8);
    fs::create_dir(&args[0])?;
    for (i, frames) in results.iter().enumerate() {
        let mut w = hound::WavWriter::create(
            Path::new(&args[0]).join(format!("room_{i}_wet.wav")),
            hound::WavSpec {
                channels: 2,
                sample_rate: rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )?;
        for f in frames {
            for v in f {
                w.write_sample((v * common) as f32)?;
            }
        }
        w.finalize()?;
    }
    fs::write(Path::new(&args[0]).join("metrics.csv"), &report)?;
    fs::write(
        Path::new(&args[0]).join("report.txt"),
        format!(
            "Common WAV gain: {common}\nNo per-IR normalization. Target late-field levels are retained; not loudness matched.\nCPU: single wall-clock pass, includes output stores, excludes coefficient preparation; not a plugin/host benchmark.\n"
        ),
    )?;
    print!("{report}");
    Ok(())
}
