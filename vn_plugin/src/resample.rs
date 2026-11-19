/// Windowed-sinc resampling of a convolution kernel, including dt gain correction.
/// Same-rate data are copied exactly. No normalization or final fade is applied.
pub fn resample(ir: &[[f64; 2]], source: u32, target: u32) -> Result<Vec<[f64; 2]>, String> {
    if !(8000..=384000).contains(&source)
        || !(8000..=192000).contains(&target)
        || ir.is_empty()
        || ir.len() > source as usize * 30
    {
        return Err("IR must be <=30 s; source 8–384 kHz, host 8–192 kHz".into());
    }
    if source == target {
        return Ok(ir.to_vec());
    }
    let ratio = f64::from(source) / f64::from(target);
    let cutoff = (1.0 / ratio).min(1.0) * 0.94;
    let radius = (32.0 / cutoff).ceil() as isize;
    let len = (ir.len() as f64 / ratio).ceil() as usize;
    let mut output = vec![[0.0; 2]; len];
    for (i, out) in output.iter_mut().enumerate() {
        let time = i as f64 * ratio;
        for j in time.floor() as isize - radius..=time.floor() as isize + radius {
            if j < 0 || j >= ir.len() as isize {
                continue;
            }
            let d = time - j as f64;
            if d.abs() >= radius as f64 {
                continue;
            }
            let x = std::f64::consts::PI * d * cutoff;
            let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
            let window = 0.5 + 0.5 * (std::f64::consts::PI * d / radius as f64).cos();
            let weight = ratio * cutoff * sinc * window;
            for ch in 0..2 {
                out[ch] += ir[j as usize][ch] * weight;
            }
        }
    }
    Ok(output)
}
