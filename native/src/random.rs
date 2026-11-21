//! Bit-exact port of Minecraft 1.18.2 Xoroshiro128++ random
//! (net.minecraft.world.level.levelgen.XoroshiroRandomSource / RandomSupport).
//!
//! All arithmetic is wrapping to match Java's long/int overflow semantics.

use md5::{Digest, Md5};

/// The subset of vanilla RandomSource used by noise construction.
/// LegacyRandomSource (nether/end) will implement this later.
pub trait RandomSource {
    fn next_long(&mut self) -> i64;
    fn next_int_bounded(&mut self, bound: i32) -> i32;
    fn next_double(&mut self) -> f64;
    /// RandomSource.consumeCount
    fn consume_count(&mut self, n: i32);
}

const GOLDEN_RATIO_64: i64 = 0x9E37_79B9_7F4A_7C15_u64 as i64;
const SILVER_RATIO_64: i64 = 0x6A09_E667_F3BC_C909_u64 as i64;

/// RandomSupport.mixStafford13
#[inline]
pub fn mix_stafford13(mut z: i64) -> i64 {
    z = (z ^ ((z as u64) >> 30) as i64).wrapping_mul(0xBF58_476D_1CE4_E5B9_u64 as i64);
    z = (z ^ ((z as u64) >> 27) as i64).wrapping_mul(0x94D0_49BB_1331_11EB_u64 as i64);
    z ^ ((z as u64) >> 31) as i64
}

/// RandomSupport.upgradeSeedTo128bit
#[inline]
pub fn upgrade_seed_to_128bit(seed: i64) -> (i64, i64) {
    let lo = seed ^ SILVER_RATIO_64;
    let hi = lo.wrapping_add(GOLDEN_RATIO_64);
    (mix_stafford13(lo), mix_stafford13(hi))
}

#[derive(Clone, Debug)]
pub struct Xoroshiro {
    lo: i64,
    hi: i64,
}

impl Xoroshiro {
    /// new XoroshiroRandomSource(long seed)
    pub fn from_seed(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        Self::from_parts(lo, hi)
    }

    /// new Xoroshiro128PlusPlus(lo, hi) — all-zero state is replaced as in vanilla.
    pub fn from_parts(lo: i64, hi: i64) -> Self {
        if lo == 0 && hi == 0 {
            Self { lo: GOLDEN_RATIO_64, hi: SILVER_RATIO_64 }
        } else {
            Self { lo, hi }
        }
    }

    /// XoroshiroRandomSource.fork()
    pub fn fork(&mut self) -> Xoroshiro {
        let lo = self.next_long();
        let hi = self.next_long();
        Xoroshiro::from_parts(lo, hi)
    }

    /// XoroshiroRandomSource.forkPositional()
    pub fn fork_positional(&mut self) -> XoroshiroPositional {
        let lo = self.next_long();
        let hi = self.next_long();
        XoroshiroPositional { seed_lo: lo, seed_hi: hi }
    }

    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let l = self.lo;
        let mut m = self.hi;
        let n = l.wrapping_add(m).rotate_left(17).wrapping_add(l);
        m ^= l;
        self.lo = l.rotate_left(49) ^ m ^ (m << 21);
        self.hi = m.rotate_left(28);
        n
    }

    #[inline]
    fn next_bits(&mut self, bits: u32) -> i64 {
        ((self.next_long() as u64) >> (64 - bits)) as i64
    }

    #[inline]
    pub fn next_int(&mut self) -> i32 {
        self.next_long() as i32
    }

    /// XoroshiroRandomSource.nextInt(int bound) — Lemire's method, as in vanilla.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        let bound64 = bound as u64;
        let mut l = (self.next_int() as u32) as u64;
        let mut m = l.wrapping_mul(bound64);
        let mut n = m & 0xFFFF_FFFF;
        if n < bound64 {
            // Integer.remainderUnsigned(~bound + 1, bound)
            let j = ((!bound).wrapping_add(1) as u32 % bound as u32) as u64;
            while n < j {
                l = (self.next_int() as u32) as u64;
                m = l.wrapping_mul(bound64);
                n = m & 0xFFFF_FFFF;
            }
        }
        ((m as i64) >> 32) as i32
    }

    /// XoroshiroRandomSource.nextFloat()
    #[inline]
    pub fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.9604645E-8f32
    }

    #[inline]
    pub fn next_double(&mut self) -> f64 {
        self.next_bits(53) as f64 * 1.110_223_024_625_156_5E-16
    }
}

impl RandomSource for Xoroshiro {
    #[inline]
    fn next_long(&mut self) -> i64 {
        Xoroshiro::next_long(self)
    }
    #[inline]
    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        Xoroshiro::next_int_bounded(self, bound)
    }
    #[inline]
    fn next_double(&mut self) -> f64 {
        Xoroshiro::next_double(self)
    }
    fn consume_count(&mut self, n: i32) {
        for _ in 0..n {
            Xoroshiro::next_long(self);
        }
    }
}

/// Mth.getSeed(int x, int y, int z)
#[inline]
pub fn get_seed(x: i32, y: i32, z: i32) -> i64 {
    // `x * 3129871` is an *int* multiply in vanilla, widened afterwards.
    let mut l = (x.wrapping_mul(3_129_871) as i64) ^ (z as i64).wrapping_mul(116_129_781) ^ (y as i64);
    l = l.wrapping_mul(l).wrapping_mul(42_317_861).wrapping_add(l.wrapping_mul(11));
    l >> 16
}

/// XoroshiroRandomSource.XoroshiroPositionalRandomFactory
#[derive(Clone, Copy, Debug)]
pub struct XoroshiroPositional {
    pub seed_lo: i64,
    pub seed_hi: i64,
}

impl XoroshiroPositional {
    /// at(x, y, z)
    pub fn at(&self, x: i32, y: i32, z: i32) -> Xoroshiro {
        Xoroshiro::from_parts(get_seed(x, y, z) ^ self.seed_lo, self.seed_hi)
    }

    /// fromHashOf(String) — MD5 of the UTF-8 string, split big-endian into two longs.
    pub fn from_hash_of(&self, name: &str) -> Xoroshiro {
        let h = Md5::digest(name.as_bytes());
        let lo = i64::from_be_bytes(h[0..8].try_into().unwrap());
        let hi = i64::from_be_bytes(h[8..16].try_into().unwrap());
        Xoroshiro::from_parts(lo ^ self.seed_lo, hi ^ self.seed_hi)
    }
}
