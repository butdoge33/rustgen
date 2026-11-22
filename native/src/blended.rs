//! Bit-exact port of 1.18.2 BlendedNoise (net.minecraft.world.level.levelgen.synth.BlendedNoise),
//! the "old-style" 3D terrain noise still referenced by the overworld noise router.

use crate::noise::lerp;
use crate::perlin::{wrap, PerlinNoise};
use crate::random::RandomSource;

/// NoiseSamplingSettings
#[derive(Clone, Copy, Debug)]
pub struct SamplingSettings {
    pub xz_scale: f64,
    pub y_scale: f64,
    pub xz_factor: f64,
    pub y_factor: f64,
}

#[derive(Clone)]
pub struct BlendedNoise {
    min_limit: PerlinNoise,
    max_limit: PerlinNoise,
    main: PerlinNoise,
    xz_scale: f64,
    y_scale: f64,
    xz_main_scale: f64,
    y_main_scale: f64,
    cell_width: i32,
    cell_height: i32,
    max_value: f64,
}

/// Mth.clampedLerp
#[inline]
fn clamped_lerp(a: f64, b: f64, t: f64) -> f64 {
    if t < 0.0 {
        a
    } else if t > 1.0 {
        b
    } else {
        lerp(t, a, b)
    }
}

impl BlendedNoise {
    /// new BlendedNoise(random, samplingSettings, cellWidth, cellHeight)
    pub fn new<R: RandomSource>(random: &mut R, s: SamplingSettings, cell_width: i32, cell_height: i32) -> Self {
        // createLegacyForBlendedNoise(random, rangeClosed(-15, 0)) etc.
        let (fo16, amps16) = PerlinNoise::amplitudes_from_octaves(&(-15..=0).collect::<Vec<_>>());
        let (fo8, amps8) = PerlinNoise::amplitudes_from_octaves(&(-7..=0).collect::<Vec<_>>());
        let min_limit = PerlinNoise::new_legacy(random, fo16, amps16.clone());
        let max_limit = PerlinNoise::new_legacy(random, fo16, amps16);
        let main = PerlinNoise::new_legacy(random, fo8, amps8);

        let xz_scale = 684.412 * s.xz_scale;
        let y_scale = 684.412 * s.y_scale;
        let max_value = min_limit.max_broken_value(y_scale);
        Self {
            min_limit,
            max_limit,
            main,
            xz_scale,
            y_scale,
            xz_main_scale: xz_scale / s.xz_factor,
            y_main_scale: y_scale / s.y_factor,
            cell_width,
            cell_height,
            max_value,
        }
    }

    /// Rebuilds an already-seeded vanilla BlendedNoise from its fields.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        min_limit: PerlinNoise, max_limit: PerlinNoise, main: PerlinNoise,
        xz_scale: f64, y_scale: f64, xz_main_scale: f64, y_main_scale: f64,
        cell_width: i32, cell_height: i32, max_value: f64,
    ) -> Self {
        Self { min_limit, max_limit, main, xz_scale, y_scale, xz_main_scale, y_main_scale, cell_width, cell_height, max_value }
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn min_value(&self) -> f64 {
        -self.max_value
    }

    /// compute() at (block_x, ys[k], block_z) for every k (at most 128) into out[k], doing the
    /// x/z work of each octave once. Each lane performs exactly the operations of compute().
    pub fn compute_column(&self, block_x: i32, block_z: i32, ys: &[i32], out: &mut [f64]) {
        let n = ys.len();
        assert!(n <= 128, "column too long");
        let i = block_x.div_euclid(self.cell_width) as f64;
        let k = block_z.div_euclid(self.cell_width) as f64;
        let mut js = [0.0f64; 128];
        for q in 0..n {
            js[q] = ys[q].div_euclid(self.cell_height) as f64;
        }
        let (mut d, mut e, mut f) = ([0.0f64; 128], [0.0f64; 128], [0.0f64; 128]);

        let mut g = 1.0;
        for l in 0..8 {
            if let Some(noise) = self.main.octave_noise(l) {
                let xz = noise.xz_part(wrap(i * self.xz_main_scale * g), wrap(k * self.xz_main_scale * g));
                let scale = self.y_main_scale * g;
                for q in 0..n {
                    // vanilla: wrap(j * yMainScale * g), yMainScale * g, j * yMainScale * g
                    let y = wrap(js[q] * self.y_main_scale * g);
                    f[q] += noise.noise_y(&xz, y, scale, js[q] * self.y_main_scale * g) / g;
                }
            }
            g /= 2.0;
        }

        let mut h = [0.0f64; 128];
        for q in 0..n {
            h[q] = (f[q] / 10.0 + 1.0) / 2.0;
        }
        g = 1.0;
        for m in 0..16 {
            let nx = wrap(i * self.xz_scale * g);
            let pz = wrap(k * self.xz_scale * g);
            let qs = self.y_scale * g;
            let min_n = self.min_limit.octave_noise(m).map(|nz| (nz, nz.xz_part(nx, pz)));
            let max_n = self.max_limit.octave_noise(m).map(|nz| (nz, nz.xz_part(nx, pz)));
            for q in 0..n {
                let o = wrap(js[q] * self.y_scale * g);
                // vanilla: noise(n, o, p, q, j * q) with q = yScale * g
                if !(h[q] >= 1.0) {
                    if let Some((nz, xz)) = &min_n {
                        d[q] += nz.noise_y(xz, o, qs, js[q] * qs) / g;
                    }
                }
                if !(h[q] <= 0.0) {
                    if let Some((nz, xz)) = &max_n {
                        e[q] += nz.noise_y(xz, o, qs, js[q] * qs) / g;
                    }
                }
            }
            g /= 2.0;
        }
        for q in 0..n {
            out[q] = clamped_lerp(d[q] / 512.0, e[q] / 512.0, h[q]) / 128.0;
        }
    }

    /// compute(FunctionContext)
    pub fn compute(&self, block_x: i32, block_y: i32, block_z: i32) -> f64 {
        let i = block_x.div_euclid(self.cell_width) as f64; // Math.floorDiv, positive divisor
        let j = block_y.div_euclid(self.cell_height) as f64;
        let k = block_z.div_euclid(self.cell_width) as f64;
        let mut d = 0.0;
        let mut e = 0.0;
        let mut f = 0.0;
        let mut g = 1.0;

        for l in 0..8 {
            if let Some(n) = self.main.octave_noise(l) {
                f += n.noise(
                    wrap(i * self.xz_main_scale * g),
                    wrap(j * self.y_main_scale * g),
                    wrap(k * self.xz_main_scale * g),
                    self.y_main_scale * g,
                    j * self.y_main_scale * g,
                ) / g;
            }
            g /= 2.0;
        }

        let h = (f / 10.0 + 1.0) / 2.0;
        let only_max = h >= 1.0;
        let only_min = h <= 0.0;
        g = 1.0;

        for m in 0..16 {
            let n = wrap(i * self.xz_scale * g);
            let o = wrap(j * self.y_scale * g);
            let p = wrap(k * self.xz_scale * g);
            let q = self.y_scale * g;
            if !only_max {
                if let Some(noise) = self.min_limit.octave_noise(m) {
                    d += noise.noise(n, o, p, q, j * q) / g;
                }
            }
            if !only_min {
                if let Some(noise) = self.max_limit.octave_noise(m) {
                    e += noise.noise(n, o, p, q, j * q) / g;
                }
            }
            g /= 2.0;
        }

        clamped_lerp(d / 512.0, e / 512.0, h) / 128.0
    }
}
