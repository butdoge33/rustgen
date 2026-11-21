//! Bit-exact ports of 1.18.2 PerlinNoise and NormalNoise
//! (net.minecraft.world.level.levelgen.synth).

use crate::noise::ImprovedNoise;
use crate::random::{RandomSource, Xoroshiro};

/// Mth.lfloor(double)
#[inline]
pub fn lfloor(d: f64) -> i64 {
    let l = d as i64;
    if d < l as f64 { l.wrapping_sub(1) } else { l }
}

/// PerlinNoise.wrap — keeps coordinates within ±2^25 to preserve precision.
#[inline]
pub fn wrap(d: f64) -> f64 {
    d - lfloor(d / 3.3554432E7 + 0.5) as f64 * 3.3554432E7
}

#[derive(Clone)]
pub struct PerlinNoise {
    noise_levels: Vec<Option<ImprovedNoise>>,
    amplitudes: Vec<f64>,
    lowest_freq_value_factor: f64,
    lowest_freq_input_factor: f64,
    max_value: f64,
}

impl PerlinNoise {
    /// PerlinNoise.create(random, firstOctave, amplitudes) — the modern path:
    /// each octave seeded from forkPositional().fromHashOf("octave_<n>").
    pub fn new(random: &mut Xoroshiro, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let n = amplitudes.len();
        let positional = random.fork_positional();
        let noise_levels = (0..n)
            .map(|k| {
                (amplitudes[k] != 0.0).then(|| {
                    let octave = first_octave + k as i32;
                    ImprovedNoise::new(&mut positional.from_hash_of(&format!("octave_{octave}")))
                })
            })
            .collect();
        Self::finish(noise_levels, first_octave, amplitudes)
    }

    /// Rebuilds an already-seeded vanilla PerlinNoise from its fields.
    pub fn from_parts(
        noise_levels: Vec<Option<ImprovedNoise>>,
        amplitudes: Vec<f64>,
        lowest_freq_input_factor: f64,
        lowest_freq_value_factor: f64,
        max_value: f64,
    ) -> Self {
        assert_eq!(noise_levels.len(), amplitudes.len(), "octave/amplitude count mismatch");
        Self { noise_levels, amplitudes, lowest_freq_value_factor, lowest_freq_input_factor, max_value }
    }

    /// The legacy path (`useNewFactory == false`): octaves are drawn sequentially
    /// from `random`, highest-frequency-last, skipping 262 values for absent octaves.
    /// Used by BlendedNoise (createLegacyForBlendedNoise) and the legacy nether biome noise.
    pub fn new_legacy<R: RandomSource>(random: &mut R, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let i = amplitudes.len() as i32;
        let j = -first_octave;
        let mut noise_levels: Vec<Option<ImprovedNoise>> = vec![None; amplitudes.len()];

        let first = ImprovedNoise::new(random);
        if j >= 0 && j < i && amplitudes[j as usize] != 0.0 {
            noise_levels[j as usize] = Some(first);
        }
        let mut k = j - 1;
        while k >= 0 {
            if k < i && amplitudes[k as usize] != 0.0 {
                noise_levels[k as usize] = Some(ImprovedNoise::new(random));
            } else {
                random.consume_count(262);
            }
            k -= 1;
        }
        assert!(j >= i - 1, "Positive octaves are temporarily disabled");
        Self::finish(noise_levels, first_octave, amplitudes)
    }

    /// PerlinNoise.makeAmplitudes(sorted octave set) → (firstOctave, amplitudes)
    pub fn amplitudes_from_octaves(octaves: &[i32]) -> (i32, Vec<f64>) {
        let mut set: Vec<i32> = octaves.to_vec();
        set.sort_unstable();
        set.dedup();
        assert!(!set.is_empty(), "Need some octaves!");
        let i = -set[0];
        let j = *set.last().unwrap();
        let k = i + j + 1;
        assert!(k >= 1, "Total number of octaves needs to be >= 1");
        let mut amps = vec![0.0; k as usize];
        for l in set {
            amps[(l + i) as usize] = 1.0;
        }
        (-i, amps)
    }

    fn finish(noise_levels: Vec<Option<ImprovedNoise>>, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let i = amplitudes.len() as i32;
        let j = -first_octave;
        // Math.pow(2, n) is exact for these integer exponents, as is powi.
        let lowest_freq_input_factor = 2.0f64.powi(-j);
        let lowest_freq_value_factor = 2.0f64.powi(i - 1) / (2.0f64.powi(i) - 1.0);
        let mut p = Self {
            noise_levels,
            amplitudes,
            lowest_freq_value_factor,
            lowest_freq_input_factor,
            max_value: 0.0,
        };
        p.max_value = p.edge_value(2.0);
        p
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    /// maxBrokenValue(d)
    pub fn max_broken_value(&self, d: f64) -> f64 {
        self.edge_value(d + 2.0)
    }

    fn edge_value(&self, d: f64) -> f64 {
        let mut e = 0.0;
        let mut f = self.lowest_freq_value_factor;
        for (i, lvl) in self.noise_levels.iter().enumerate() {
            if lvl.is_some() {
                e += self.amplitudes[i] * d * f;
            }
            f /= 2.0;
        }
        e
    }

    /// getOctaveNoise(i) — indexed from the highest frequency down.
    #[inline]
    pub fn octave_noise(&self, i: usize) -> Option<&ImprovedNoise> {
        self.noise_levels[self.noise_levels.len() - 1 - i].as_ref()
    }

    /// getValue(x, y, z)
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut total = 0.0;
        let mut input = self.lowest_freq_input_factor;
        let mut value = self.lowest_freq_value_factor;
        for (l, lvl) in self.noise_levels.iter().enumerate() {
            if let Some(n) = lvl {
                let m = n.noise(wrap(x * input), wrap(y * input), wrap(z * input), 0.0 * input, 0.0 * input);
                total += self.amplitudes[l] * m * value;
            }
            input *= 2.0;
            value /= 2.0;
        }
        total
    }
}

/// NormalNoise
#[derive(Clone)]
pub struct NormalNoise {
    value_factor: f64,
    first: PerlinNoise,
    second: PerlinNoise,
    max_value: f64,
}

const INPUT_FACTOR: f64 = 1.0181268882175227;

impl NormalNoise {
    /// NormalNoise.create(random, params) — modern path.
    pub fn new(random: &mut Xoroshiro, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let first = PerlinNoise::new(random, first_octave, amplitudes.clone());
        let second = PerlinNoise::new(random, first_octave, amplitudes.clone());
        Self::finish(first, second, &amplitudes)
    }

    /// NormalNoise.createLegacyNetherBiome(random, params) — legacy octave path.
    pub fn new_legacy<R: RandomSource>(random: &mut R, first_octave: i32, amplitudes: Vec<f64>) -> Self {
        let first = PerlinNoise::new_legacy(random, first_octave, amplitudes.clone());
        let second = PerlinNoise::new_legacy(random, first_octave, amplitudes.clone());
        Self::finish(first, second, &amplitudes)
    }

    /// Rebuilds an already-seeded vanilla NormalNoise from its fields.
    pub fn from_parts(value_factor: f64, first: PerlinNoise, second: PerlinNoise, max_value: f64) -> Self {
        Self { value_factor, first, second, max_value }
    }

    fn finish(first: PerlinNoise, second: PerlinNoise, amplitudes: &[f64]) -> Self {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for (l, &d) in amplitudes.iter().enumerate() {
            if d != 0.0 {
                lo = lo.min(l as i32);
                hi = hi.max(l as i32);
            }
        }
        let value_factor = 0.16666666666666666 / expected_deviation(hi.wrapping_sub(lo));
        let max_value = (first.max_value() + second.max_value()) * value_factor;
        Self { value_factor, first, second, max_value }
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    /// getValue(x, y, z)
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let g = x * INPUT_FACTOR;
        let h = y * INPUT_FACTOR;
        let i = z * INPUT_FACTOR;
        (self.first.get_value(x, y, z) + self.second.get_value(g, h, i)) * self.value_factor
    }
}

#[inline]
fn expected_deviation(i: i32) -> f64 {
    0.1 * (1.0 + 1.0 / i.wrapping_add(1) as f64)
}
