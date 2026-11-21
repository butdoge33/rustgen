//! Bit-exact port of Minecraft 1.18.2 ImprovedNoise
//! (net.minecraft.world.level.levelgen.synth.ImprovedNoise) and the Mth helpers it uses.
//!
//! Determinism rules for this file (and everything ported later):
//!  - keep Java's exact operation order; never reassociate
//!  - never use mul_add / FMA
//!  - never enable fast-math style flags

use crate::random::RandomSource;

/// SimplexNoise.GRADIENT
const GRADIENT: [[f64; 3]; 16] = [
    [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0], [1.0, 0.0, -1.0], [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0], [0.0, -1.0, 1.0], [0.0, 1.0, -1.0], [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0], [0.0, -1.0, 1.0], [-1.0, 1.0, 0.0], [0.0, -1.0, -1.0],
];

/// Mth.floor(double): Java (int) cast saturates and maps NaN to 0, same as Rust `as`.
#[inline]
pub fn floor(d: f64) -> i32 {
    let i = d as i32;
    if d < i as f64 { i.wrapping_sub(1) } else { i }
}

#[inline]
pub fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline]
pub fn lerp2(t1: f64, t2: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    lerp(t2, lerp(t1, a, b), lerp(t1, c, d))
}

#[allow(clippy::too_many_arguments)]
#[inline]
pub fn lerp3(
    t1: f64, t2: f64, t3: f64,
    a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64,
) -> f64 {
    lerp(t3, lerp2(t1, t2, a, b, c, d), lerp2(t1, t2, e, f, g, h))
}

#[inline]
pub fn smoothstep(d: f64) -> f64 {
    d * d * d * (d * (d * 6.0 - 15.0) + 10.0)
}

#[inline]
fn grad_dot(hash: i32, x: f64, y: f64, z: f64) -> f64 {
    let g = &GRADIENT[(hash & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

#[derive(Clone)]
pub struct ImprovedNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl ImprovedNoise {
    /// new ImprovedNoise(RandomSource)
    pub fn new<R: RandomSource>(random: &mut R) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256usize {
            let j = random.next_int_bounded(256 - i as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }

    /// Rebuilds an already-seeded vanilla ImprovedNoise from its fields.
    pub fn from_parts(p: [u8; 256], xo: f64, yo: f64, zo: f64) -> Self {
        Self { p, xo, yo, zo }
    }

    #[inline]
    fn perm(&self, i: i32) -> i32 {
        self.p[(i & 0xFF) as usize] as i32
    }

    /// noise(x, y, z, yScale, yMax)
    pub fn noise(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let dx = x + self.xo;
        let dy = y + self.yo;
        let dz = z + self.zo;
        let ix = floor(dx);
        let iy = floor(dy);
        let iz = floor(dz);
        let fx = dx - ix as f64;
        let fy = dy - iy as f64;
        let fz = dz - iz as f64;
        let y_off = if y_scale != 0.0 {
            let r = if y_max >= 0.0 && y_max < fy { y_max } else { fy };
            // Note: vanilla adds a *float* literal 1.0E-7F here.
            floor(r / y_scale + 1.0e-7_f32 as f64) as f64 * y_scale
        } else {
            0.0
        };
        self.sample_and_lerp(ix, iy, iz, fx, fy - y_off, fz, fy)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn sample_and_lerp(&self, i: i32, j: i32, k: i32, d: f64, e: f64, f: f64, g: f64) -> f64 {
        // Java int arithmetic wraps; mirror that explicitly.
        let l = self.perm(i);
        let m = self.perm(i.wrapping_add(1));
        let n = self.perm(l.wrapping_add(j));
        let o = self.perm(l.wrapping_add(j).wrapping_add(1));
        let p = self.perm(m.wrapping_add(j));
        let q = self.perm(m.wrapping_add(j).wrapping_add(1));
        let h = grad_dot(self.perm(n.wrapping_add(k)), d, e, f);
        let r = grad_dot(self.perm(p.wrapping_add(k)), d - 1.0, e, f);
        let s = grad_dot(self.perm(o.wrapping_add(k)), d, e - 1.0, f);
        let t = grad_dot(self.perm(q.wrapping_add(k)), d - 1.0, e - 1.0, f);
        let u = grad_dot(self.perm(n.wrapping_add(k).wrapping_add(1)), d, e, f - 1.0);
        let v = grad_dot(self.perm(p.wrapping_add(k).wrapping_add(1)), d - 1.0, e, f - 1.0);
        let w = grad_dot(self.perm(o.wrapping_add(k).wrapping_add(1)), d, e - 1.0, f - 1.0);
        let x = grad_dot(self.perm(q.wrapping_add(k).wrapping_add(1)), d - 1.0, e - 1.0, f - 1.0);
        // Vanilla uses the *unadjusted* y fraction (g) for the y weight.
        let sx = smoothstep(d);
        let sy = smoothstep(g);
        let sz = smoothstep(f);
        lerp3(sx, sy, sz, h, r, s, t, u, v, w, x)
    }
}
