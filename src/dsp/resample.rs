//! Oversampling support: the circuit runs at `os × fs`; this decimates the
//! final voltage back to the host rate with a Kaiser-windowed sinc FIR,
//! evaluated only at the output instants.

pub struct Decimator {
    factor: usize,
    taps: Vec<f64>,
    hist: Vec<f64>,
    pos: usize,
    phase: usize,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..40 {
        term *= y / (k * k) as f64;
        sum += term;
        if term < 1e-14 * sum {
            break;
        }
    }
    sum
}

impl Decimator {
    pub fn new(factor: usize) -> Self {
        let factor = factor.max(1);
        let n = if factor == 1 { 1 } else { 48 * factor + 1 };
        let mut taps = vec![0.0; n];
        if factor == 1 {
            taps[0] = 1.0;
        } else {
            // Pass band to ~0.45·fs_out, stop band from 0.55·fs_out.
            let fc = 0.5 / factor as f64 * 0.92;
            let beta = 9.0;
            let m = (n - 1) as f64;
            let denom = bessel_i0(beta);
            let mut sum = 0.0;
            for (i, t) in taps.iter_mut().enumerate() {
                let k = i as f64 - m / 2.0;
                let sinc = if k == 0.0 {
                    2.0 * fc
                } else {
                    (std::f64::consts::TAU * fc * k).sin() / (std::f64::consts::PI * k)
                };
                let r = 2.0 * i as f64 / m - 1.0;
                let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom;
                *t = sinc * w;
                sum += *t;
            }
            for t in &mut taps {
                *t /= sum;
            }
        }
        Self {
            factor,
            hist: vec![0.0; n * 2],
            taps,
            pos: 0,
            phase: 0,
        }
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    /// Latency in host samples.
    pub fn latency(&self) -> usize {
        (self.taps.len() - 1) / 2 / self.factor
    }

    /// Push one internal-rate sample; returns an output every `factor` pushes.
    #[inline]
    pub fn push(&mut self, x: f64) -> Option<f64> {
        let n = self.taps.len();
        // Mirrored history so the convolution reads one contiguous slice.
        self.hist[self.pos] = x;
        self.hist[self.pos + n] = x;
        self.pos += 1;
        if self.pos == n {
            self.pos = 0;
        }
        self.phase += 1;
        if self.phase < self.factor {
            return None;
        }
        self.phase = 0;
        let window = &self.hist[self.pos..self.pos + n];
        let mut acc = 0.0;
        for (h, t) in window.iter().zip(self.taps.iter().rev()) {
            acc += h * t;
        }
        Some(acc)
    }

    pub fn reset(&mut self) {
        self.hist.iter_mut().for_each(|v| *v = 0.0);
        self.pos = 0;
        self.phase = 0;
    }
}
