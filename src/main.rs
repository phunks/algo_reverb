use algo_reverb::{Config, Matrix, Reverb};
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: algo_reverb [hadamard|householder|hybrid] [output.wav] [t60_seconds]");
        println!("Renders a wet-only stereo impulse response at 48000 Hz (16-bit PCM).");
        return Ok(());
    }
    if args.len() > 3 {
        return Err("too many arguments; see --help".into());
    }
    let matrix = match args.first().map(String::as_str).unwrap_or("hadamard") {
        "hadamard" => Matrix::Hadamard,
        "householder" => Matrix::Householder,
        "hybrid" => Matrix::Hybrid,
        _ => return Err("matrix must be hadamard, householder or hybrid".into()),
    };
    let path = args.get(1).map(String::as_str).unwrap_or("impulse.wav");
    let t60_seconds = args
        .get(2)
        .map(|s| s.parse::<f32>())
        .transpose()?
        .unwrap_or(2.5);
    let config = Config {
        matrix,
        t60_seconds,
        ..Config::default()
    };
    let mut reverb = Reverb::new(config)?;
    let frames = ((t60_seconds + 1.0) * config.sample_rate as f32).ceil() as u32;
    let data_bytes = frames * 4;
    // Do not silently overwrite an existing recording.
    let mut writer = BufWriter::new(File::create_new(path)?);
    write_wav_header(&mut writer, config.sample_rate, data_bytes)?;
    let mut peak = 0.0_f32;
    let mut clipped = 0_u64;
    let started = Instant::now();
    for i in 0..frames {
        let output = reverb.process_sample(if i == 0 { 1.0 } else { 0.0 });
        for sample in output {
            peak = peak.max(sample.abs());
            if sample.abs() > 1.0 {
                clipped += 1;
            }
            let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            writer.write_all(&pcm.to_le_bytes())?;
        }
    }
    writer.flush()?;
    println!(
        "Wrote {path}: {matrix:?}, {frames} stereo frames, peak {peak:.4}, clipped samples {clipped}"
    );
    println!(
        "Render + PCM encoding + file writing: {:?} (not a DSP-only benchmark)",
        started.elapsed()
    );
    Ok(())
}

fn write_wav_header(
    writer: &mut impl Write,
    sample_rate: u32,
    data_bytes: u32,
) -> std::io::Result<()> {
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36 + data_bytes).to_le_bytes())?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?; // PCM
    writer.write_all(&2_u16.to_le_bytes())?; // Stereo
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&(sample_rate * 4).to_le_bytes())?;
    writer.write_all(&4_u16.to_le_bytes())?; // Bytes per frame
    writer.write_all(&16_u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_has_correct_layout() {
        let mut bytes = Vec::new();
        write_wav_header(&mut bytes, 48_000, 192_000).unwrap();
        assert_eq!(bytes.len(), 44);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 192_036);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(
            u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
            192_000
        );
        assert_eq!(u16::from_le_bytes(bytes[32..34].try_into().unwrap()), 4);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            192_000
        );
    }
}
