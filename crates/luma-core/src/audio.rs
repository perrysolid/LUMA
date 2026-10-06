//! Microphone sample conversion: any device format → 16 kHz mono PCM16, the
//! format the streaming speech-to-text expects.

/// Streaming linear-interpolation resampler with a simple one-pole low-pass
/// applied before decimation. Speech-band quality is all we need here, and it
/// keeps the dependency tree small.
#[derive(Debug, Clone)]
pub struct MonoResampler {
    channels: usize,
    ratio: f64, // input samples per output sample
    pos: f64,   // fractional read position into `hist`
    hist: Vec<f32>,
    lp: f32,
    lp_alpha: f32,
    frame: Vec<f32>,
}

impl MonoResampler {
    pub fn new(in_rate: u32, channels: u16, out_rate: u32) -> Self {
        let ratio = in_rate as f64 / out_rate as f64;
        // cutoff near the output Nyquist (approx. 0.45 * out_rate)
        let fc = 0.45 * out_rate as f64;
        let dt = 1.0 / in_rate as f64;
        let rc = 1.0 / (2.0 * std::f64::consts::PI * fc);
        let lp_alpha = if ratio > 1.0 { (dt / (rc + dt)) as f32 } else { 1.0 };
        Self {
            channels: channels.max(1) as usize,
            ratio,
            pos: 0.0,
            hist: Vec::new(),
            lp: 0.0,
            lp_alpha,
            frame: Vec::new(),
        }
    }

    /// Feed interleaved samples; returns 16-bit mono samples at the out rate.
    pub fn process(&mut self, interleaved: &[f32]) -> Vec<i16> {
        for &s in interleaved {
            self.frame.push(s);
            if self.frame.len() == self.channels {
                let m = self.frame.iter().sum::<f32>() / self.channels as f32;
                self.frame.clear();
                self.lp += self.lp_alpha * (m - self.lp);
                self.hist.push(self.lp);
            }
        }
        let mut out = Vec::new();
        while self.pos + 1.0 < self.hist.len() as f64 {
            let i = self.pos.floor() as usize;
            let f = (self.pos - i as f64) as f32;
            let v = self.hist[i] * (1.0 - f) + self.hist[i + 1] * f;
            out.push(to_i16(v));
            self.pos += self.ratio;
        }
        // keep only what is still needed
        let consumed = (self.pos.floor() as usize).min(self.hist.len());
        self.hist.drain(..consumed);
        self.pos -= consumed as f64;
        out
    }
}

pub fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// Root-mean-square level in 0..1, for the "listening" meter.
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / i16::MAX as f64).powi(2)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

pub fn i16_to_le_bytes(s: &[i16]) -> Vec<u8> {
    s.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, secs: f32, channels: u16) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5;
                std::iter::repeat(v).take(channels as usize)
            })
            .collect()
    }

    #[test]
    fn output_length_matches_rate_ratio_across_chunks() {
        let input = sine(48_000, 440.0, 1.0, 2);
        let mut r = MonoResampler::new(48_000, 2, 16_000);
        let mut total = 0;
        for chunk in input.chunks(997) {
            total += r.process(chunk).len();
        }
        assert!((15_990..=16_000).contains(&total), "{total}");
    }

    #[test]
    fn preserves_speech_band_tone_and_amplitude() {
        let mut r = MonoResampler::new(44_100, 1, 16_000);
        let out = r.process(&sine(44_100, 300.0, 0.5, 1));
        let level = rms(&out[1000..]);
        // 0.5 amplitude sine -> rms ~0.354; allow some low-pass attenuation
        assert!((0.30..0.37).contains(&level), "{level}");
    }

    #[test]
    fn attenuates_above_output_nyquist() {
        let mut r = MonoResampler::new(48_000, 1, 16_000);
        let out = r.process(&sine(48_000, 15_000.0, 0.5, 1));
        assert!(rms(&out[1000..]) < 0.2);
    }

    #[test]
    fn upsampling_and_passthrough_work() {
        let mut r = MonoResampler::new(16_000, 1, 16_000);
        assert_eq!(r.process(&[0.0; 1600]).len(), 1599);
        let mut r = MonoResampler::new(8_000, 1, 16_000);
        assert!(r.process(&[0.1; 800]).len() >= 1597);
    }

    #[test]
    fn clipping_and_bytes() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
        assert_eq!(i16_to_le_bytes(&[1, -1]), vec![1, 0, 255, 255]);
    }
}
