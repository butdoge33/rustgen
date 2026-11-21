//! Native evaluator for 1.18.2 density-function graphs.
//!
//! The Java side (rustgen.translate.DensityTranslator) walks an already-seeded vanilla
//! NoiseRouter and serializes it into the binary "program" format parsed here. Noise
//! objects are shipped as *snapshots* of their seeded state, so no seeding logic is
//! needed at runtime and any seeding path (xoroshiro or legacy) is supported.
//!
//! Semantics mirror vanilla `DensityFunction.compute(SinglePointContext)` exactly,
//! including the min/max short-circuits in Ap2 (min/max bounds are copied from Java,
//! never recomputed). Cache/interpolation markers are pass-through here; chunk-level
//! interpolation (NoiseChunk) is a separate layer.
//!
//! # Format (little endian)
//! ```text
//! u32 magic "RGDF", u32 version
//! u32 n; NormalNoise × n
//! u32 n; BlendedNoise × n
//! u32 n; Item × n          (item = u8 kind {0 node, 1 spline} + body)
//! u32 n; i32 root × n      (node index, or -1 = unsupported)
//! ```
//! Items may only reference items defined *earlier*, which the parser enforces;
//! the graph is therefore acyclic by construction.

use crate::blended::BlendedNoise;
use crate::noise::{lerp, ImprovedNoise};
use crate::perlin::{NormalNoise, PerlinNoise};

pub const MAGIC: u32 = u32::from_le_bytes(*b"RGDF");
pub const FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Java-exact math helpers
// ---------------------------------------------------------------------------

/// java.lang.Math.max(double, double): NaN-propagating, +0.0 > -0.0.
#[inline]
fn java_max(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return a;
    }
    if a == 0.0 && b == 0.0 && a.to_bits() == (-0.0f64).to_bits() {
        return b;
    }
    if a >= b { a } else { b }
}

/// java.lang.Math.min(double, double): NaN-propagating, -0.0 < +0.0.
#[inline]
fn java_min(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return a;
    }
    if a == 0.0 && b == 0.0 && b.to_bits() == (-0.0f64).to_bits() {
        return b;
    }
    if a <= b { a } else { b }
}

/// Mth.clamp(double, double, double)
#[inline]
fn clamp(d: f64, lo: f64, hi: f64) -> f64 {
    if d < lo {
        lo
    } else if d > hi {
        hi
    } else {
        d
    }
}

/// Mth.clampedLerp(double, double, double)
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

/// Mth.lerp(float, float, float)
#[inline]
fn lerp_f(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

/// TerrainShaper.peaksAndValleys(float)
#[inline]
fn peaks_and_valleys(f: f32) -> f32 {
    -((f.abs() - 0.6666667f32).abs() - 0.33333334f32) * 3.0f32
}

// ---------------------------------------------------------------------------
// Program representation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum MapKind {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Squeeze,
}

#[derive(Clone, Copy, Debug)]
pub struct Slider {
    target: f64,
    size: i32,
    offset: i32,
}

impl Slider {
    /// NoiseSlider.applySlide
    #[inline]
    fn apply(&self, d: f64, e: f64) -> f64 {
        if self.size <= 0 {
            return d;
        }
        let f = (e - self.offset as f64) / self.size as f64;
        clamped_lerp(self.target, d, f)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SlideSettings {
    cell_height: i32,
    min_cell_y: i32,
    cell_count_y: i32,
    top: Slider,
    bottom: Slider,
}

#[derive(Clone, Copy, Debug)]
pub enum Coord {
    Continents,
    Erosion,
    Ridges,
    Weirdness,
    /// CoordinateCustom: a density function evaluated at the current position.
    Node(u32),
}

#[derive(Debug)]
pub enum Spline {
    Constant(f32),
    Multipoint { coord: Coord, locations: Vec<f32>, derivatives: Vec<f32>, values: Vec<u32> },
}

#[derive(Debug)]
pub enum Op {
    Constant(f64),
    Add(u32, u32),
    Mul(u32, u32),
    Min(u32, u32),
    Max(u32, u32),
    AddConst(u32, f64),
    MulConst(u32, f64),
    Mapped(MapKind, u32),
    Clamp(u32, f64, f64),
    /// Cache/interpolation marker; pass-through in point evaluation.
    Marker(u8, u32),
    Noise(Option<u32>, f64, f64),
    ShiftA(Option<u32>),
    ShiftB(Option<u32>),
    Shift(Option<u32>),
    ShiftedNoise { sx: u32, sy: u32, sz: u32, xz_scale: f64, y_scale: f64, noise: Option<u32> },
    RangeChoice { input: u32, min_inclusive: f64, max_exclusive: f64, in_range: u32, out_of_range: u32 },
    Slide(u32, Option<SlideSettings>),
    Spline { spline: u32, min: f64, max: f64 },
    TerrainShaperSpline { c: u32, e: u32, w: u32, spline: Option<u32>, add_offset: bool, min: f64, max: f64 },
    WeirdScaled { input: u32, noise: Option<u32>, type2: bool },
    YClampedGradient { from_y: i32, to_y: i32, from_value: f64, to_value: f64 },
    BlendAlpha,
    BlendOffset,
    Beardifier,
    BlendDensity(u32),
    OldBlendedNoise(u32),
}

#[derive(Debug)]
pub struct Node {
    pub op: Op,
    pub min: f64,
    pub max: f64,
}

pub struct Program {
    noises: Vec<NormalNoise>,
    blended: Vec<BlendedNoise>,
    splines: Vec<Spline>,
    nodes: Vec<Node>,
    roots: Vec<Option<u32>>,
}

/// Point used by TerrainShaper splines (continents, erosion, ridges, weirdness).
#[derive(Clone, Copy)]
struct ShaperPoint {
    c: f32,
    e: f32,
    r: f32,
    w: f32,
}

impl Program {
    pub fn root(&self, i: usize) -> Option<u32> {
        self.roots.get(i).copied().flatten()
    }

    pub fn root_count(&self) -> usize {
        self.roots.len()
    }

    /// DensityFunction.compute(SinglePointContext(x, y, z))
    pub fn eval(&self, n: u32, x: i32, y: i32, z: i32) -> f64 {
        let node = &self.nodes[n as usize];
        match node.op {
            Op::Constant(v) => v,
            Op::Add(a, b) => self.eval(a, x, y, z) + self.eval(b, x, y, z),
            Op::Mul(a, b) => {
                let d = self.eval(a, x, y, z);
                if d == 0.0 { 0.0 } else { d * self.eval(b, x, y, z) }
            }
            Op::Min(a, b) => {
                let d = self.eval(a, x, y, z);
                if d < self.nodes[b as usize].min { d } else { java_min(d, self.eval(b, x, y, z)) }
            }
            Op::Max(a, b) => {
                let d = self.eval(a, x, y, z);
                if d > self.nodes[b as usize].max { d } else { java_max(d, self.eval(b, x, y, z)) }
            }
            Op::AddConst(i, c) => self.eval(i, x, y, z) + c,
            Op::MulConst(i, c) => self.eval(i, x, y, z) * c,
            Op::Mapped(kind, i) => {
                let d = self.eval(i, x, y, z);
                match kind {
                    MapKind::Abs => d.abs(),
                    MapKind::Square => d * d,
                    MapKind::Cube => d * d * d,
                    MapKind::HalfNegative => if d > 0.0 { d } else { d * 0.5 },
                    MapKind::QuarterNegative => if d > 0.0 { d } else { d * 0.25 },
                    MapKind::Squeeze => {
                        let e = clamp(d, -1.0, 1.0);
                        e / 2.0 - e * e * e / 24.0
                    }
                }
            }
            Op::Clamp(i, lo, hi) => clamp(self.eval(i, x, y, z), lo, hi),
            Op::Marker(_, i) => self.eval(i, x, y, z),
            Op::Noise(noise, xz, ys) => match noise {
                None => 0.0,
                Some(k) => self.noises[k as usize].get_value(x as f64 * xz, y as f64 * ys, z as f64 * xz),
            },
            Op::ShiftA(noise) => self.shift(noise, x as f64, 0.0, z as f64),
            Op::ShiftB(noise) => self.shift(noise, z as f64, x as f64, 0.0),
            Op::Shift(noise) => self.shift(noise, x as f64, y as f64, z as f64),
            Op::ShiftedNoise { sx, sy, sz, xz_scale, y_scale, noise } => match noise {
                None => 0.0,
                Some(k) => {
                    let d = x as f64 * xz_scale + self.eval(sx, x, y, z);
                    let e = y as f64 * y_scale + self.eval(sy, x, y, z);
                    let f = z as f64 * xz_scale + self.eval(sz, x, y, z);
                    self.noises[k as usize].get_value(d, e, f)
                }
            },
            Op::RangeChoice { input, min_inclusive, max_exclusive, in_range, out_of_range } => {
                let d = self.eval(input, x, y, z);
                if d >= min_inclusive && d < max_exclusive {
                    self.eval(in_range, x, y, z)
                } else {
                    self.eval(out_of_range, x, y, z)
                }
            }
            Op::Slide(i, settings) => {
                let d = self.eval(i, x, y, z);
                match settings {
                    None => d,
                    Some(s) => {
                        // NoiseRouterData.applySlide: (int)e / cellHeight is Java int division (truncating).
                        let f = (y.wrapping_div(s.cell_height)).wrapping_sub(s.min_cell_y) as f64;
                        let d = s.top.apply(d, s.cell_count_y as f64 - f);
                        s.bottom.apply(d, f)
                    }
                }
            }
            Op::Spline { spline, min, max } => {
                clamp(self.spline_apply(spline, None, x, y, z) as f64, min, max)
            }
            Op::TerrainShaperSpline { c, e, w, spline, add_offset, min, max } => match spline {
                None => 0.0,
                Some(s) => {
                    let c = self.eval(c, x, y, z) as f32;
                    let e = self.eval(e, x, y, z) as f32;
                    let w = self.eval(w, x, y, z) as f32;
                    let pt = ShaperPoint { c, e, r: peaks_and_valleys(w), w };
                    let mut v = self.spline_apply(s, Some(&pt), x, y, z);
                    if add_offset {
                        v += -0.50375f32; // TerrainShaper.GLOBAL_OFFSET
                    }
                    clamp(v as f64, min, max)
                }
            },
            Op::WeirdScaled { input, noise, type2 } => {
                let d = self.eval(input, x, y, z);
                match noise {
                    None => 0.0,
                    Some(k) => {
                        let e = if type2 { rarity_2d(d) } else { rarity_3d(d) };
                        e * self.noises[k as usize].get_value(x as f64 / e, y as f64 / e, z as f64 / e).abs()
                    }
                }
            }
            Op::YClampedGradient { from_y, to_y, from_value, to_value } => {
                // Mth.clampedMap(y, fromY, toY, fromValue, toValue)
                let t = (y as f64 - from_y as f64) / (to_y as f64 - from_y as f64);
                clamped_lerp(from_value, to_value, t)
            }
            Op::BlendAlpha => 1.0,
            Op::BlendOffset => 0.0,
            Op::Beardifier => 0.0,
            // Blender.empty().blendDensity(ctx, d) == d. Non-empty blending (next to
            // pre-1.18 chunks) is handled by falling back to vanilla.
            Op::BlendDensity(i) => self.eval(i, x, y, z),
            Op::OldBlendedNoise(b) => self.blended[b as usize].compute(x, y, z),
        }
    }

    #[inline]
    fn shift(&self, noise: Option<u32>, d: f64, e: f64, f: f64) -> f64 {
        match noise {
            None => 0.0,
            Some(k) => self.noises[k as usize].get_value(d * 0.25, e * 0.25, f * 0.25) * 4.0,
        }
    }

    /// CubicSpline.apply — all arithmetic in f32, as in vanilla.
    fn spline_apply(&self, s: u32, pt: Option<&ShaperPoint>, x: i32, y: i32, z: i32) -> f32 {
        let (coord, locations, derivatives, values) = match &self.splines[s as usize] {
            Spline::Constant(v) => return *v,
            Spline::Multipoint { coord, locations, derivatives, values } => (coord, locations, derivatives, values),
        };
        let shaper = || pt.expect("TerrainShaper coordinate used outside terrain_shaper_spline");
        let f: f32 = match *coord {
            Coord::Continents => shaper().c,
            Coord::Erosion => shaper().e,
            Coord::Ridges => shaper().r,
            Coord::Weirdness => shaper().w,
            Coord::Node(n) => self.eval(n, x, y, z) as f32,
        };

        // Mth.binarySearch(0, len, ix -> f < locations[ix]) - 1
        let len = locations.len();
        let (mut lo, mut k) = (0usize, len);
        while k > 0 {
            let half = k / 2;
            let m = lo + half;
            if f < locations[m] {
                k = half;
            } else {
                lo = m + 1;
                k -= half + 1;
            }
        }
        let i = lo as isize - 1;
        let last = len - 1;

        if i < 0 {
            return self.spline_apply(values[0], pt, x, y, z) + derivatives[0] * (f - locations[0]);
        }
        let i = i as usize;
        if i == last {
            return self.spline_apply(values[last], pt, x, y, z) + derivatives[last] * (f - locations[last]);
        }
        let g = locations[i];
        let h = locations[i + 1];
        let t = (f - g) / (h - g);
        let l = derivatives[i];
        let m = derivatives[i + 1];
        let n = self.spline_apply(values[i], pt, x, y, z);
        let o = self.spline_apply(values[i + 1], pt, x, y, z);
        let p = l * (h - g) - (o - n);
        let q = -m * (h - g) + (o - n);
        lerp_f(t, n, o) + t * (1.0f32 - t) * lerp_f(t, p, q)
    }
}

/// NoiseRouterData.QuantizedSpaghettiRarity.getSpaghettiRarity3D (type_1)
#[inline]
fn rarity_3d(d: f64) -> f64 {
    if d < -0.5 {
        0.75
    } else if d < 0.0 {
        1.0
    } else if d < 0.5 {
        1.5
    } else {
        2.0
    }
}

/// NoiseRouterData.QuantizedSpaghettiRarity.getSphaghettiRarity2D (type_2)
#[inline]
fn rarity_2d(d: f64) -> f64 {
    if d < -0.75 {
        0.5
    } else if d < -0.5 {
        0.75
    } else if d < 0.5 {
        1.0
    } else if d < 0.75 {
        2.0
    } else {
        3.0
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

type R<T> = Result<T, String>;

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let end = end.ok_or_else(|| format!("unexpected end of program at byte {}", self.pos))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> R<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> R<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> R<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// Element count, sanity-limited so corrupt input can't trigger huge allocations.
    fn count(&mut self, what: &str) -> R<usize> {
        let n = self.u32()? as usize;
        if n > 1 << 20 {
            return Err(format!("{what} count {n} is unreasonably large"));
        }
        Ok(n)
    }
}

struct Parser<'a> {
    r: Reader<'a>,
    p: Program,
}

impl<'a> Parser<'a> {
    fn node_ref(&mut self) -> R<u32> {
        let i = self.r.u32()?;
        if (i as usize) < self.p.nodes.len() {
            Ok(i)
        } else {
            Err(format!("forward/invalid node reference {i} (have {})", self.p.nodes.len()))
        }
    }

    fn spline_ref(&mut self) -> R<u32> {
        let i = self.r.u32()?;
        if (i as usize) < self.p.splines.len() {
            Ok(i)
        } else {
            Err(format!("forward/invalid spline reference {i} (have {})", self.p.splines.len()))
        }
    }

    fn opt_noise(&mut self) -> R<Option<u32>> {
        let i = self.r.i32()?;
        match i {
            -1 => Ok(None),
            i if i >= 0 && (i as usize) < self.p.noises.len() => Ok(Some(i as u32)),
            i => Err(format!("invalid noise reference {i}")),
        }
    }

    fn perlin(&mut self) -> R<PerlinNoise> {
        let input_factor = self.r.f64()?;
        let value_factor = self.r.f64()?;
        let max_value = self.r.f64()?;
        let n = self.r.count("octave")?;
        let mut levels = Vec::with_capacity(n);
        let mut amps = Vec::with_capacity(n);
        for _ in 0..n {
            amps.push(self.r.f64()?);
            levels.push(match self.r.u8()? {
                0 => None,
                1 => {
                    let xo = self.r.f64()?;
                    let yo = self.r.f64()?;
                    let zo = self.r.f64()?;
                    let p: [u8; 256] = self.r.take(256)?.try_into().unwrap();
                    Some(ImprovedNoise::from_parts(p, xo, yo, zo))
                }
                t => return Err(format!("invalid octave presence flag {t}")),
            });
        }
        Ok(PerlinNoise::from_parts(levels, amps, input_factor, value_factor, max_value))
    }

    fn slider(&mut self) -> R<Slider> {
        Ok(Slider { target: self.r.f64()?, size: self.r.i32()?, offset: self.r.i32()? })
    }

    fn spline(&mut self) -> R<Spline> {
        match self.r.u8()? {
            0 => Ok(Spline::Constant(self.r.f32()?)),
            1 => {
                let coord = match self.r.u8()? {
                    0 => Coord::Continents,
                    1 => Coord::Erosion,
                    2 => Coord::Ridges,
                    3 => Coord::Weirdness,
                    4 => Coord::Node(self.node_ref()?),
                    t => return Err(format!("invalid spline coordinate {t}")),
                };
                let n = self.r.count("spline point")?;
                if n == 0 {
                    return Err("spline with no points".into());
                }
                let locations = (0..n).map(|_| self.r.f32()).collect::<R<Vec<_>>>()?;
                let derivatives = (0..n).map(|_| self.r.f32()).collect::<R<Vec<_>>>()?;
                let values = (0..n).map(|_| self.spline_ref()).collect::<R<Vec<_>>>()?;
                Ok(Spline::Multipoint { coord, locations, derivatives, values })
            }
            t => Err(format!("invalid spline tag {t}")),
        }
    }

    fn node(&mut self) -> R<Node> {
        let opcode = self.r.u8()?;
        let min = self.r.f64()?;
        let max = self.r.f64()?;
        let op = match opcode {
            0 => Op::Constant(self.r.f64()?),
            1 => Op::Add(self.node_ref()?, self.node_ref()?),
            2 => Op::Mul(self.node_ref()?, self.node_ref()?),
            3 => Op::Min(self.node_ref()?, self.node_ref()?),
            4 => Op::Max(self.node_ref()?, self.node_ref()?),
            5 => Op::AddConst(self.node_ref()?, self.r.f64()?),
            6 => Op::MulConst(self.node_ref()?, self.r.f64()?),
            7 => {
                let kind = match self.r.u8()? {
                    0 => MapKind::Abs,
                    1 => MapKind::Square,
                    2 => MapKind::Cube,
                    3 => MapKind::HalfNegative,
                    4 => MapKind::QuarterNegative,
                    5 => MapKind::Squeeze,
                    t => return Err(format!("invalid mapped kind {t}")),
                };
                Op::Mapped(kind, self.node_ref()?)
            }
            8 => Op::Clamp(self.node_ref()?, self.r.f64()?, self.r.f64()?),
            9 => Op::Marker(self.r.u8()?, self.node_ref()?),
            10 => Op::Noise(self.opt_noise()?, self.r.f64()?, self.r.f64()?),
            11 => Op::ShiftA(self.opt_noise()?),
            12 => Op::ShiftB(self.opt_noise()?),
            13 => Op::Shift(self.opt_noise()?),
            14 => Op::ShiftedNoise {
                sx: self.node_ref()?,
                sy: self.node_ref()?,
                sz: self.node_ref()?,
                xz_scale: self.r.f64()?,
                y_scale: self.r.f64()?,
                noise: self.opt_noise()?,
            },
            15 => Op::RangeChoice {
                input: self.node_ref()?,
                min_inclusive: self.r.f64()?,
                max_exclusive: self.r.f64()?,
                in_range: self.node_ref()?,
                out_of_range: self.node_ref()?,
            },
            16 => {
                let input = self.node_ref()?;
                let settings = match self.r.u8()? {
                    0 => None,
                    1 => {
                        let cell_height = self.r.i32()?;
                        if cell_height <= 0 {
                            return Err(format!("invalid cell height {cell_height}"));
                        }
                        Some(SlideSettings {
                            cell_height,
                            min_cell_y: self.r.i32()?,
                            cell_count_y: self.r.i32()?,
                            top: self.slider()?,
                            bottom: self.slider()?,
                        })
                    }
                    t => return Err(format!("invalid slide flag {t}")),
                };
                Op::Slide(input, settings)
            }
            17 => Op::Spline { spline: self.spline_ref()?, min: self.r.f64()?, max: self.r.f64()? },
            18 => {
                let c = self.node_ref()?;
                let e = self.node_ref()?;
                let w = self.node_ref()?;
                let spline = match self.r.u8()? {
                    0 => None,
                    1 => Some(self.spline_ref()?),
                    t => return Err(format!("invalid shaper flag {t}")),
                };
                let add_offset = self.r.u8()? != 0;
                Op::TerrainShaperSpline { c, e, w, spline, add_offset, min: self.r.f64()?, max: self.r.f64()? }
            }
            19 => Op::WeirdScaled {
                input: self.node_ref()?,
                noise: self.opt_noise()?,
                type2: match self.r.u8()? {
                    1 => false,
                    2 => true,
                    t => return Err(format!("invalid rarity mapper {t}")),
                },
            },
            20 => Op::YClampedGradient {
                from_y: self.r.i32()?,
                to_y: self.r.i32()?,
                from_value: self.r.f64()?,
                to_value: self.r.f64()?,
            },
            21 => Op::BlendAlpha,
            22 => Op::BlendOffset,
            23 => Op::Beardifier,
            24 => Op::BlendDensity(self.node_ref()?),
            25 => {
                let b = self.r.u32()?;
                if b as usize >= self.p.blended.len() {
                    return Err(format!("invalid blended noise reference {b}"));
                }
                Op::OldBlendedNoise(b)
            }
            t => return Err(format!("unknown opcode {t}")),
        };
        Ok(Node { op, min, max })
    }
}

pub fn parse(buf: &[u8]) -> R<Program> {
    let mut ps = Parser {
        r: Reader { buf, pos: 0 },
        p: Program { noises: vec![], blended: vec![], splines: vec![], nodes: vec![], roots: vec![] },
    };
    if ps.r.u32()? != MAGIC {
        return Err("bad magic (not a rustgen density program)".into());
    }
    let version = ps.r.u32()?;
    if version != FORMAT_VERSION {
        return Err(format!("program format {version}, native supports {FORMAT_VERSION}"));
    }

    for _ in 0..ps.r.count("noise")? {
        let value_factor = ps.r.f64()?;
        let max_value = ps.r.f64()?;
        let first = ps.perlin()?;
        let second = ps.perlin()?;
        ps.p.noises.push(NormalNoise::from_parts(value_factor, first, second, max_value));
    }

    for _ in 0..ps.r.count("blended noise")? {
        let min_limit = ps.perlin()?;
        let max_limit = ps.perlin()?;
        let main = ps.perlin()?;
        let (xz, y, xzm, ym) = (ps.r.f64()?, ps.r.f64()?, ps.r.f64()?, ps.r.f64()?);
        let (cw, ch) = (ps.r.i32()?, ps.r.i32()?);
        if cw <= 0 || ch <= 0 {
            return Err(format!("invalid blended cell size {cw}x{ch}"));
        }
        let max_value = ps.r.f64()?;
        ps.p.blended.push(BlendedNoise::from_parts(min_limit, max_limit, main, xz, y, xzm, ym, cw, ch, max_value));
    }

    for _ in 0..ps.r.count("item")? {
        match ps.r.u8()? {
            0 => {
                let n = ps.node()?;
                ps.p.nodes.push(n);
            }
            1 => {
                let s = ps.spline()?;
                ps.p.splines.push(s);
            }
            t => return Err(format!("invalid item kind {t}")),
        }
    }

    for _ in 0..ps.r.count("root")? {
        let i = ps.r.i32()?;
        let root = match i {
            -1 => None,
            i if i >= 0 && (i as usize) < ps.p.nodes.len() => Some(i as u32),
            i => return Err(format!("invalid root {i}")),
        };
        ps.p.roots.push(root);
    }

    if ps.r.pos != buf.len() {
        return Err(format!("{} trailing bytes after program", buf.len() - ps.r.pos));
    }
    Ok(ps.p)
}
