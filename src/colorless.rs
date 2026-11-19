//! Experimental static FDN. No convolution or modulation.
//! Profiles are prepared offline; line count is fixed for the lifetime of a tank.

/// Validated coefficients at their native sample rate (no implicit resampling).
#[derive(Clone, Debug)]
pub struct Profile {
    sample_rate: u32,
    delays: Vec<usize>,
    matrix: Vec<f64>,
    input: Vec<f64>,
    output: Vec<f64>,
}

impl Profile {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut tokens = text.split_whitespace();
        if tokens.next() != Some("fdn-colorless-v1") {
            return Err("unsupported FDN profile".into());
        }
        let mut number = || -> Result<f64, String> {
            let x: f64 = tokens
                .next()
                .ok_or("truncated profile")?
                .parse()
                .map_err(|_| "invalid number")?;
            if !x.is_finite() {
                return Err("nonfinite coefficient".into());
            }
            Ok(x)
        };
        let rate = number()?;
        let count = number()?;
        if !(8000.0..=192000.0).contains(&rate)
            || rate.fract() != 0.0
            || !(2.0..=32.0).contains(&count)
            || count.fract() != 0.0
        {
            return Err("invalid sample rate or line count".into());
        }
        let n = count as usize;
        let mut delays = Vec::with_capacity(n);
        for _ in 0..n {
            let x = number()?;
            if !(1.0..=192000.0).contains(&x) || x.fract() != 0.0 {
                return Err("invalid delay length".into());
            }
            delays.push(x as usize);
        }
        let matrix = (0..n * n)
            .map(|_| number())
            .collect::<Result<Vec<_>, _>>()?;
        let input = (0..n).map(|_| number()).collect::<Result<Vec<_>, _>>()?;
        let output = (0..2 * n)
            .map(|_| number())
            .collect::<Result<Vec<_>, _>>()?;
        if tokens.next().is_some() {
            return Err("extra profile data".into());
        }
        for i in 0..n {
            for j in 0..n {
                let dot: f64 = (0..n).map(|k| matrix[k * n + i] * matrix[k * n + j]).sum();
                if (dot - if i == j { 1.0 } else { 0.0 }).abs() > 1e-10 {
                    return Err("feedback matrix is not orthogonal".into());
                }
            }
        }
        if input.iter().chain(&output).any(|x| x.abs() > 100.0) {
            return Err("excessive input/output gain".into());
        }
        Ok(Self {
            sample_rate: rate as u32,
            delays,
            matrix,
            input,
            output,
        })
    }

    pub fn lines(&self) -> usize {
        self.delays.len()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Allocate at construction, never in `process_sample`. Input must be finite.
pub struct StaticFdn {
    profile: Profile,
    buffers: Vec<Vec<f64>>,
    positions: Vec<usize>,
    gains: Vec<f64>,
    reads: Vec<f64>,
    feedback_filters: Vec<Vec<Section>>,
    output_filters: [Vec<Section>; 2],
}

/// First-order IIR `(b0 + b1 z^-1) / (1 + a1 z^-1)`.
/// Room profiles use monotonic shelving sections, with certified passive loops.
#[derive(Clone)]
struct Section {
    b0: f64,
    b1: f64,
    a1: f64,
    state: f64,
}

impl Section {
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.state;
        self.state = self.b1 * x - self.a1 * y;
        y
    }

    fn magnitude(&self, w: f64) -> f64 {
        ((self.b0 * self.b0 + self.b1 * self.b1 + 2.0 * self.b0 * self.b1 * w.cos()).max(0.0)
            / (1.0 + self.a1 * self.a1 + 2.0 * self.a1 * w.cos()))
        .sqrt()
    }
}

impl StaticFdn {
    pub fn new(profile: Profile, t60: f64) -> Result<Self, String> {
        if !t60.is_finite() || !(0.1..=30.0).contains(&t60) {
            return Err("T60 must be finite and in 0.1..=30 seconds".into());
        }
        let n = profile.lines();
        Ok(Self {
            buffers: profile.delays.iter().map(|&m| vec![0.0; m]).collect(),
            gains: profile
                .delays
                .iter()
                .map(|&m| 10_f64.powf(-3.0 * m as f64 / (profile.sample_rate as f64 * t60)))
                .collect(),
            positions: vec![0; n],
            reads: vec![0.0; n],
            feedback_filters: vec![Vec::new(); n],
            output_filters: std::array::from_fn(|_| Vec::new()),
            profile,
        })
    }

    /// Offline-designed room profile: base colorless profile, marker, then one
    /// section count and b0/b1/a1 triples per delay line and per stereo output.
    /// No IR samples are retained. Construction/validation is not realtime safe.
    pub fn from_room_profile(text: &str) -> Result<Self, String> {
        let (base, filters) = text
            .split_once("\nroom-filters-v1\n")
            .ok_or("missing room-filters-v1 marker")?;
        let profile = Profile::parse(base)?;
        let n = profile.lines();
        // Replace the homogeneous gains: the filters contain the entire decay.
        let mut tank = Self::new(profile, 1.0)?;
        tank.gains.fill(1.0);
        let mut tokens = filters.split_whitespace();
        for index in 0..n + 2 {
            let count: usize = tokens
                .next()
                .ok_or("missing filter count")?
                .parse()
                .map_err(|_| "invalid filter count")?;
            if !(1..=16).contains(&count) {
                return Err("filter count outside 1..=16".into());
            }
            let mut sections = Vec::with_capacity(count);
            for _ in 0..count {
                let mut number = || -> Result<f64, String> {
                    let x: f64 = tokens
                        .next()
                        .ok_or("truncated filter")?
                        .parse()
                        .map_err(|_| "invalid filter coefficient")?;
                    if !x.is_finite() || x.abs() > 100.0 {
                        return Err("invalid filter coefficient".into());
                    }
                    Ok(x)
                };
                let section = Section {
                    b0: number()?,
                    b1: number()?,
                    a1: number()?,
                    state: 0.0,
                };
                if section.a1.abs() >= 1.0 {
                    return Err("unstable filter pole".into());
                }
                sections.push(section);
            }
            if index < n {
                // Each real first-order section's magnitude is monotonic in
                // cos(w). Products of endpoint maxima bound every point in each
                // interval, unlike merely checking a sampled frequency response.
                for k in 0..2048 {
                    let lo = std::f64::consts::PI * k as f64 / 2048.0;
                    let hi = std::f64::consts::PI * (k + 1) as f64 / 2048.0;
                    let bound: f64 = sections
                        .iter()
                        .map(|s| s.magnitude(lo).max(s.magnitude(hi)))
                        .product();
                    if !bound.is_finite() || bound >= 1.0 {
                        return Err("feedback filter is not strictly passive".into());
                    }
                }
                tank.feedback_filters[index] = sections;
            } else {
                tank.output_filters[index - n] = sections;
            }
        }
        if tokens.next().is_some() {
            return Err("extra room profile data".into());
        }
        Ok(tank)
    }

    pub fn reset(&mut self) {
        for buffer in &mut self.buffers {
            buffer.fill(0.0);
        }
        self.positions.fill(0);
        self.reads.fill(0.0);
        for sections in self
            .feedback_filters
            .iter_mut()
            .chain(&mut self.output_filters)
        {
            for section in sections {
                section.state = 0.0;
            }
        }
    }

    pub fn process_sample(&mut self, input: f64) -> [f64; 2] {
        let n = self.profile.lines();
        for i in 0..n {
            self.reads[i] = self.buffers[i][self.positions[i]];
        }
        let output = std::array::from_fn(|ch| {
            let mut x = (0..n)
                .map(|i| self.profile.output[ch * n + i] * self.reads[i])
                .sum();
            for section in &mut self.output_filters[ch] {
                x = section.process(x);
            }
            x
        });
        for i in 0..n {
            for section in &mut self.feedback_filters[i] {
                self.reads[i] = section.process(self.reads[i]);
            }
        }
        for i in 0..n {
            let feedback: f64 = (0..n)
                .map(|j| self.profile.matrix[i * n + j] * self.gains[j] * self.reads[j])
                .sum();
            self.buffers[i][self.positions[i]] = feedback + self.profile.input[i] * input;
            self.positions[i] = (self.positions[i] + 1) % self.buffers[i].len();
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PROFILE: &str = "fdn-colorless-v1 48000 2 3 5 0 1 1 0 1 0 1 0 0 1";

    #[test]
    fn validation() {
        assert!(Profile::parse(PROFILE).is_ok());
        for text in [
            PROFILE.replace("0 1 1 0", "1 1 1 0"),
            PROFILE.replace("48000", "NaN"),
            format!("{PROFILE} 0"),
            PROFILE.replace("2 3 5", "2 0 5"),
        ] {
            assert!(Profile::parse(&text).is_err());
        }
        assert!(StaticFdn::new(Profile::parse(PROFILE).unwrap(), f64::NAN).is_err());
    }

    #[test]
    fn room_validation_routing_and_reset() {
        let text = format!("{PROFILE}\nroom-filters-v1\n1 .8 0 0\n1 .7 0 0\n1 2 0 0\n1 3 0 0\n");
        let mut tank = StaticFdn::from_room_profile(&text).unwrap();
        let rendered: Vec<_> = (0..48000)
            .map(|i| tank.process_sample(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        assert_eq!(rendered[3], [2.0, 0.0]);
        assert!((rendered[8][1] - 2.4).abs() < 1e-12);
        assert!(rendered[24000..].iter().flatten().all(|v| v.abs() < 1e-20));
        tank.reset();
        for (i, expected) in rendered.iter().enumerate() {
            assert_eq!(
                tank.process_sample(if i == 0 { 1.0 } else { 0.0 }),
                *expected
            );
        }
        for invalid in [
            text.replace(".8 0 0", "1.01 0 0"),
            text.replace(".8 0 0", ".8 0 -1"),
            text.replace(".8 0 0", "NaN 0 0"),
            format!("{text} 9"),
            text.replace("1 .8 0 0", "0"),
        ] {
            assert!(StaticFdn::from_room_profile(&invalid).is_err());
        }
    }

    #[test]
    fn room_stateful_filters_reset_exactly() {
        let text = format!(
            "{PROFILE}\nroom-filters-v1\n1 .6 .1 -.1\n1 .5 -.1 .1\n1 1 .2 -.3\n1 1 -.1 .2\n"
        );
        let mut tank = StaticFdn::from_room_profile(&text).unwrap();
        let rendered: Vec<_> = (0..4096)
            .map(|i| tank.process_sample(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        tank.reset();
        for (i, expected) in rendered.iter().enumerate() {
            assert_eq!(
                tank.process_sample(if i == 0 { 1.0 } else { 0.0 }),
                *expected
            );
        }
    }

    #[test]
    fn onset_routing_decay_and_reset() {
        let mut tank = StaticFdn::new(Profile::parse(PROFILE).unwrap(), 0.1).unwrap();
        let rendered: Vec<_> = (0..48000)
            .map(|i| tank.process_sample(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        assert_eq!(&rendered[..3], &[[0.0; 2]; 3]);
        assert_eq!(rendered[3], [1.0, 0.0]);
        assert!((rendered[8][1] - 10_f64.powf(-3.0 * 3.0 / 4800.0)).abs() < 1e-12);
        assert!(rendered[24000..].iter().flatten().all(|x| x.abs() < 1e-12));
        tank.reset();
        for (i, expected) in rendered.into_iter().enumerate() {
            assert_eq!(
                tank.process_sample(if i == 0 { 1.0 } else { 0.0 }),
                expected
            );
        }
    }
}
