use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, Default)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

impl Complex {
    pub fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }
}

pub struct FftContext {
    size: usize,
    rev_table: Vec<usize>,
    twiddles: Vec<Complex>,
}

impl FftContext {
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two(), "FFT size must be power of two");

        // Pre-compute bit reversal
        let mut rev_table = vec![0; size];
        let bits = size.trailing_zeros();
        for i in 0..size {
            let mut r = 0;
            for j in 0..bits {
                if (i >> j) & 1 == 1 {
                    r |= 1 << (bits - 1 - j);
                }
            }
            rev_table[i] = r;
        }

        // Pre-compute twiddles
        // Only need size/2 twiddles for Cooley-Tukey
        let mut twiddles = Vec::with_capacity(size / 2);
        for i in 0..size / 2 {
            let angle = -2.0 * PI * (i as f32) / (size as f32);
            let (s, c) = angle.sin_cos();
            twiddles.push(Complex::new(c, s));
        }

        Self {
            size,
            rev_table,
            twiddles,
        }
    }

    pub fn process(&self, buffer: &mut [Complex]) {
        assert_eq!(buffer.len(), self.size);

        // Bit-reversal permutation
        for i in 0..self.size {
            let j = self.rev_table[i];
            if i < j {
                buffer.swap(i, j);
            }
        }

        // Butterfly Operations
        let mut m = 2; // Current FFT size (2, 4, 8...)
        while m <= self.size {
            let mh = m / 2;
            let step = self.size / m;

            for k in (0..self.size).step_by(m) {
                for j in 0..mh {
                    // Twiddle factor: W_m^j = exp(-2pi i j / m)
                    // We precomputed W_N^k.
                    // We need to map index j in m-sized FFT to index in N-sized twiddles.
                    let tw_idx = j * step;
                    let w = self.twiddles[tw_idx];

                    let u = buffer[k + j];
                    let t = complex_mul(w, buffer[k + j + mh]);

                    buffer[k + j] = complex_add(u, t);
                    buffer[k + j + mh] = complex_sub(u, t);
                }
            }
            m *= 2;
        }
    }
}

fn complex_add(a: Complex, b: Complex) -> Complex {
    Complex { re: a.re + b.re, im: a.im + b.im }
}

fn complex_sub(a: Complex, b: Complex) -> Complex {
    Complex { re: a.re - b.re, im: a.im - b.im }
}

fn complex_mul(a: Complex, b: Complex) -> Complex {
    Complex {
        re: a.re * b.re - a.im * b.im,
        im: a.re * b.im + a.im * b.re,
    }
}
