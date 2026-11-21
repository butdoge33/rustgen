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

#[path = "aquifer.rs"]
pub mod aquifer;
#[path = "batch.rs"]
pub(crate) mod batch;

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

impl SlideSettings {
    /// NoiseRouterData.applySlide: (int)e / cellHeight is Java int division (truncating).
    #[inline]
    fn apply(&self, d: f64, y: i32) -> f64 {
        let f = (y.wrapping_div(self.cell_height)).wrapping_sub(self.min_cell_y) as f64;
        let d = self.top.apply(d, self.cell_count_y as f64 - f);
        self.bottom.apply(d, f)
    }
}

/// DensityFunctions.Mapped.transform
#[inline]
fn map_value(kind: MapKind, d: f64) -> f64 {
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

/// NoiseInterpolator value for one block. `c` = n000, n100, n010, n110, n001, n101, n011, n111.
/// lerp3: the `fillingCell` path (Mth.lerp3, x→y→z); otherwise updateForY/X/Z (y→x→z).
#[inline]
fn interpolate(c: &[f64; 8], dx: f64, dy: f64, dz: f64, lerp3: bool) -> f64 {
    if lerp3 {
        crate::noise::lerp3(dx, dy, dz, c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7])
    } else {
        let xz00 = lerp(dy, c[0], c[2]);
        let xz10 = lerp(dy, c[1], c[3]);
        let xz01 = lerp(dy, c[4], c[6]);
        let xz11 = lerp(dy, c[5], c[7]);
        let z0 = lerp(dx, xz00, xz10);
        let z1 = lerp(dx, xz01, xz11);
        lerp(dz, z0, z1)
    }
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
    /// Per node: may the value depend on block y (in chunk mode)? Conservative.
    y_dep: Vec<bool>,
}

/// Marker kinds, in the order DensityTranslator.MARKERS writes them.
pub const MARK_INTERPOLATED: u8 = 0;
pub const MARK_FLAT_CACHE: u8 = 1;
pub const MARK_CACHE_2D: u8 = 2;
pub const MARK_CACHE_ONCE: u8 = 3;
pub const MARK_CACHE_ALL_IN_CELL: u8 = 4;

/// Evaluation mode, mirroring which context vanilla code would be using.
#[derive(Clone, Copy)]
enum Mode<'a> {
    /// SinglePointContext: every marker is pass-through (also used inside flat caches,
    /// where positions are already quart-aligned with y = 0, making nested flat caches identity).
    Point,
    /// NoiseChunk while filling interpolator slices (corner positions).
    Corner(&'a Memo),
    /// NoiseChunk at a block inside the current cell.
    Block(&'a BlockCtx<'a>),
}

impl<'a> Mode<'a> {
    /// The chunk memo (chunk modes only).
    #[inline]
    fn memo(&self) -> &'a Memo {
        match *self {
            Mode::Corner(m) => m,
            Mode::Block(b) => b.memo,
            Mode::Point => unreachable!("no memo in point mode"),
        }
    }
}

/// Per-chunk caches mirroring NoiseChunk's FlatCache / Cache2D / CacheOnce. They only
/// avoid recomputation: check_chunk guarantees the cached values equal a fresh evaluation.
struct Memo {
    /// node index -> slot in the column grids (flat_cache and cache_2d nodes)
    col_slot: Vec<u32>,
    /// node index -> slot for cache_once
    once_slot: Vec<u32>,
    min_x: i32,
    min_z: i32,
    /// grid side: chunk width + 1 (covers every column a chunk fill can touch)
    side: usize,
    col_val: Vec<std::cell::Cell<f64>>,
    col_set: Vec<std::cell::Cell<bool>>,
    once_val: Vec<std::cell::Cell<f64>>,
    once_stamp: Vec<std::cell::Cell<u64>>,
    /// Bumped for every new evaluation position (one per corner).
    stamp: std::cell::Cell<u64>,
}

impl Memo {
    /// Column-grid cell for (node, x, z), or None if the column is outside the chunk grid.
    #[inline]
    fn col(&self, node: u32, x: i32, z: i32) -> Option<usize> {
        let slot = self.col_slot[node as usize];
        let (ox, oz) = (x - self.min_x, z - self.min_z);
        if slot == u32::MAX || ox < 0 || oz < 0 || ox as usize >= self.side || oz as usize >= self.side {
            return None;
        }
        Some((slot as usize * self.side + ox as usize) * self.side + oz as usize)
    }
}

struct BlockCtx<'a> {
    memo: &'a Memo,
    /// node index -> interpolator slot
    slot_of: &'a [u32],
    /// per slot, current cell corners in Mth.lerp3 argument order:
    /// n000, n100, n010, n110, n001, n101, n011, n111
    corners: &'a [[f64; 8]],
    dx: f64,
    dy: f64,
    dz: f64,
    /// true: NoiseInterpolator while `fillingCell` (Mth.lerp3, x→y→z);
    /// false: the updateForY/X/Z path (y→x→z), used outside cell caches.
    lerp3: bool,
}

impl BlockCtx<'_> {
    #[inline]
    fn interpolate(&self, node: u32) -> f64 {
        interpolate(&self.corners[self.slot_of[node as usize] as usize], self.dx, self.dy, self.dz, self.lerp3)
    }
}

/// Chunk geometry, as NoiseChunk.forChunk computes it.
#[derive(Clone, Copy, Debug)]
pub struct ChunkParams {
    pub min_block_x: i32,
    pub min_block_z: i32,
    pub cell_noise_min_y: i32,
    pub cell_count_y: i32,
    pub cell_count_xz: i32,
    pub cell_width: i32,
    pub cell_height: i32,
}

impl ChunkParams {
    pub fn width(&self) -> usize {
        (self.cell_count_xz * self.cell_width) as usize
    }
    pub fn height(&self) -> usize {
        (self.cell_count_y * self.cell_height) as usize
    }
    /// Blocks covered; output arrays must have exactly this length.
    pub fn volume(&self) -> usize {
        self.width() * self.width() * self.height()
    }
    fn validate(&self) -> Result<(), String> {
        let ok = self.cell_width > 0 && self.cell_width <= 16 && self.cell_height > 0 && self.cell_height <= 16
            && self.cell_count_xz > 0 && self.cell_count_xz * self.cell_width <= 64
            && self.cell_count_y > 0 && self.cell_count_y <= 4096
            && self.min_block_x.rem_euclid(self.cell_width) == 0 && self.min_block_z.rem_euclid(self.cell_width) == 0;
        if ok { Ok(()) } else { Err(format!("invalid chunk params {self:?}")) }
    }
}

/// Outputs of [`Program::fill_chunk`]; each slice has length `ChunkParams::volume()`,
/// indexed `((y - minY) * width + (z - minZ)) * width + (x - minX)`.
pub struct ChunkOut<'a> {
    /// cacheAllInCell(add(finalDensity, beardifier)) — what NoiseChunk feeds the aquifer.
    pub density: &'a mut [f64],
    /// Router roots evaluated per block like OreVeinifier does (vein_toggle, vein_ridged, vein_gap).
    pub extra: Vec<(u32, &'a mut [f64])>,
    /// If true, `extra` must be exactly [vein_toggle, vein_ridged, vein_gap] and values are
    /// only computed where vanilla's OreVeinifier can read them (see [`vein_needs`]);
    /// everything else is NaN.
    pub lazy_veins: bool,
    /// Block-state rules (aquifer + ore veins): one [`aquifer::code`] byte per block.
    pub material: Option<Material<'a>>,
}

pub struct Material<'a> {
    pub cfg: &'a aquifer::MaterialConfig,
    pub codes: &'a mut [u8],
}

/// Vanilla OreVeinifier.VeinType bands: IRON -60..=-8 (toggle <= 0), COPPER 0..=50 (toggle > 0).
const VEIN_BAND: (i32, i32) = (-60, 50);

/// Whether OreVeinifier can read vein_ridged / vein_gap at `y` given the toggle value.
/// Deliberately conservative (threshold 0.39 instead of 0.4F): computing extra values
/// is harmless, missing one would not be.
#[inline]
fn vein_needs(toggle: f64, y: i32) -> bool {
    let (min_y, max_y) = if toggle > 0.0 { (0, 50) } else { (-60, -8) };
    let (j, k) = (max_y - y, y - min_y);
    if j < 0 || k < 0 {
        return false;
    }
    let l = j.min(k) as f64;
    // Mth.clampedMap(l, 0, 20, -0.2, 0)
    let f = clamped_lerp(-0.2, 0.0, l / 20.0);
    toggle.abs() + f >= 0.39
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
        self.ev(n, x, y, z, Mode::Point)
    }

    fn ev(&self, n: u32, x: i32, y: i32, z: i32, mode: Mode) -> f64 {
        let node = &self.nodes[n as usize];
        match node.op {
            Op::Constant(v) => v,
            Op::Add(a, b) => self.ev(a, x, y, z, mode) + self.ev(b, x, y, z, mode),
            Op::Mul(a, b) => {
                let d = self.ev(a, x, y, z, mode);
                if d == 0.0 { 0.0 } else { d * self.ev(b, x, y, z, mode) }
            }
            Op::Min(a, b) => {
                let d = self.ev(a, x, y, z, mode);
                if d < self.nodes[b as usize].min { d } else { java_min(d, self.ev(b, x, y, z, mode)) }
            }
            Op::Max(a, b) => {
                let d = self.ev(a, x, y, z, mode);
                if d > self.nodes[b as usize].max { d } else { java_max(d, self.ev(b, x, y, z, mode)) }
            }
            Op::AddConst(i, c) => self.ev(i, x, y, z, mode) + c,
            Op::MulConst(i, c) => self.ev(i, x, y, z, mode) * c,
            Op::Mapped(kind, i) => map_value(kind, self.ev(i, x, y, z, mode)),
            Op::Clamp(i, lo, hi) => clamp(self.ev(i, x, y, z, mode), lo, hi),
            Op::Marker(kind, i) => match (kind, mode) {
                (MARK_INTERPOLATED, Mode::Block(b)) => b.interpolate(n),
                (MARK_INTERPOLATED, Mode::Corner(_)) => unreachable!("nested interpolation rejected by check_chunk"),
                // NoiseChunk.FlatCache: value precomputed at the quart-aligned column, y = 0.
                (MARK_FLAT_CACHE, Mode::Block(_) | Mode::Corner(_)) => {
                    let memo = mode.memo();
                    let (qx, qz) = ((x >> 2) << 2, (z >> 2) << 2);
                    match memo.col(n, qx, qz) {
                        Some(c) => self.memo_col(memo, c, || self.ev(i, qx, 0, qz, Mode::Point)),
                        // Outside the cached grid vanilla evaluates at the real position.
                        None => self.ev(i, x, y, z, mode),
                    }
                }
                // NoiseChunk.Cache2D: per column (check_chunk guarantees y-independence).
                (MARK_CACHE_2D, Mode::Block(_) | Mode::Corner(_)) => match mode.memo().col(n, x, z) {
                    Some(c) => self.memo_col(mode.memo(), c, || self.ev(i, x, y, z, mode)),
                    None => self.ev(i, x, y, z, mode),
                },
                // NoiseChunk.CacheOnce: per evaluation position (only reachable in corner mode).
                (MARK_CACHE_ONCE, Mode::Corner(memo)) => {
                    let slot = memo.once_slot[n as usize] as usize;
                    let now = memo.stamp.get();
                    if memo.once_stamp[slot].get() == now {
                        memo.once_val[slot].get()
                    } else {
                        let v = self.ev(i, x, y, z, mode);
                        memo.once_val[slot].set(v);
                        memo.once_stamp[slot].set(now);
                        v
                    }
                }
                // point mode, or cache markers whose value is position-pure here.
                _ => self.ev(i, x, y, z, mode),
            },
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
                    let d = x as f64 * xz_scale + self.ev(sx, x, y, z, mode);
                    let e = y as f64 * y_scale + self.ev(sy, x, y, z, mode);
                    let f = z as f64 * xz_scale + self.ev(sz, x, y, z, mode);
                    self.noises[k as usize].get_value(d, e, f)
                }
            },
            Op::RangeChoice { input, min_inclusive, max_exclusive, in_range, out_of_range } => {
                let d = self.ev(input, x, y, z, mode);
                if d >= min_inclusive && d < max_exclusive {
                    self.ev(in_range, x, y, z, mode)
                } else {
                    self.ev(out_of_range, x, y, z, mode)
                }
            }
            Op::Slide(i, settings) => {
                let d = self.ev(i, x, y, z, mode);
                match settings {
                    None => d,
                    Some(s) => s.apply(d, y),
                }
            }
            Op::Spline { spline, min, max } => {
                clamp(self.spline_apply(spline, None, x, y, z, mode) as f64, min, max)
            }
            Op::TerrainShaperSpline { c, e, w, spline, add_offset, min, max } => match spline {
                None => 0.0,
                Some(s) => {
                    let c = self.ev(c, x, y, z, mode) as f32;
                    let e = self.ev(e, x, y, z, mode) as f32;
                    let w = self.ev(w, x, y, z, mode) as f32;
                    let pt = ShaperPoint { c, e, r: peaks_and_valleys(w), w };
                    let mut v = self.spline_apply(s, Some(&pt), x, y, z, mode);
                    if add_offset {
                        v += -0.50375f32; // TerrainShaper.GLOBAL_OFFSET
                    }
                    clamp(v as f64, min, max)
                }
            },
            Op::WeirdScaled { input, noise, type2 } => {
                let d = self.ev(input, x, y, z, mode);
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
            Op::Beardifier => match mode {
                Mode::Point => 0.0,
                _ => unreachable!("beardifier inside router rejected by check_chunk"),
            },
            // Blender.empty().blendDensity(ctx, d) == d. Non-empty blending (next to
            // pre-1.18 chunks) is handled by falling back to vanilla.
            Op::BlendDensity(i) => self.ev(i, x, y, z, mode),
            Op::OldBlendedNoise(b) => self.blended[b as usize].compute(x, y, z),
        }
    }

    #[inline]
    fn memo_col(&self, memo: &Memo, c: usize, compute: impl FnOnce() -> f64) -> f64 {
        if memo.col_set[c].get() {
            return memo.col_val[c].get();
        }
        let v = compute();
        memo.col_val[c].set(v);
        memo.col_set[c].set(true);
        v
    }

    #[inline]
    fn shift(&self, noise: Option<u32>, d: f64, e: f64, f: f64) -> f64 {
        match noise {
            None => 0.0,
            Some(k) => self.noises[k as usize].get_value(d * 0.25, e * 0.25, f * 0.25) * 4.0,
        }
    }

    /// CubicSpline.apply — all arithmetic in f32, as in vanilla.
    fn spline_apply(&self, s: u32, pt: Option<&ShaperPoint>, x: i32, y: i32, z: i32, mode: Mode) -> f32 {
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
            Coord::Node(n) => self.ev(n, x, y, z, mode) as f32,
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
            return self.spline_apply(values[0], pt, x, y, z, mode) + derivatives[0] * (f - locations[0]);
        }
        let i = i as usize;
        if i == last {
            return self.spline_apply(values[last], pt, x, y, z, mode) + derivatives[last] * (f - locations[last]);
        }
        let g = locations[i];
        let h = locations[i + 1];
        let t = (f - g) / (h - g);
        let l = derivatives[i];
        let m = derivatives[i + 1];
        let n = self.spline_apply(values[i], pt, x, y, z, mode);
        let o = self.spline_apply(values[i + 1], pt, x, y, z, mode);
        let p = l * (h - g) - (o - n);
        let q = -m * (h - g) + (o - n);
        lerp_f(t, n, o) + t * (1.0f32 - t) * lerp_f(t, p, q)
    }
}

// ---------------------------------------------------------------------------
// Chunk mode (NoiseChunk equivalent)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CheckMode {
    Point,
    Corner,
    Block,
}

impl Program {
    fn compute_y_dep(&mut self) {
        let mut dep = vec![false; self.nodes.len()];
        for (i, node) in self.nodes.iter().enumerate() {
            let d = |j: u32| dep[j as usize];
            let spline_dep = |s: u32| self.spline_y_dep(s, &dep);
            dep[i] = match node.op {
                Op::Constant(_) | Op::BlendAlpha | Op::BlendOffset | Op::ShiftA(_) | Op::ShiftB(_) => false,
                Op::Add(a, b) | Op::Mul(a, b) | Op::Min(a, b) | Op::Max(a, b) => d(a) || d(b),
                Op::AddConst(c, _) | Op::MulConst(c, _) | Op::Mapped(_, c) | Op::Clamp(c, _, _) | Op::BlendDensity(c) => d(c),
                Op::Marker(MARK_FLAT_CACHE, _) => false, // evaluated at y = 0 in chunk mode
                Op::Marker(_, c) => d(c),
                Op::Noise(_, _, ys) => ys != 0.0,
                Op::Shift(_) => true,
                Op::ShiftedNoise { sx, sy, sz, y_scale, .. } => y_scale != 0.0 || d(sx) || d(sy) || d(sz),
                Op::RangeChoice { input, in_range, out_of_range, .. } => d(input) || d(in_range) || d(out_of_range),
                Op::Slide(c, settings) => settings.is_some() || d(c),
                Op::Spline { spline, .. } => spline_dep(spline),
                Op::TerrainShaperSpline { c, e, w, .. } => d(c) || d(e) || d(w),
                Op::WeirdScaled { .. } | Op::YClampedGradient { .. } | Op::Beardifier | Op::OldBlendedNoise(_) => true,
            };
        }
        self.y_dep = dep;
    }

    fn spline_y_dep(&self, s: u32, dep: &[bool]) -> bool {
        match &self.splines[s as usize] {
            Spline::Constant(_) => false,
            Spline::Multipoint { coord, values, .. } => {
                matches!(coord, Coord::Node(n) if dep[*n as usize]) || values.iter().any(|&v| self.spline_y_dep(v, dep))
            }
        }
    }

    /// Checks that `root` can be evaluated in chunk mode with results identical to vanilla
    /// NoiseChunk. Rejects constructs whose vanilla value depends on evaluation history:
    /// nested interpolation, cache_all_in_cell inside the router, cache_once outside an
    /// interpolator, y-dependent cache_2d, and beardifier markers inside the router.
    pub fn check_chunk(&self, root: u32) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        self.check(root, CheckMode::Block, &mut seen)
    }

    fn check(&self, n: u32, mode: CheckMode, seen: &mut std::collections::HashSet<(u32, CheckMode)>) -> Result<(), String> {
        if !seen.insert((n, mode)) {
            return Ok(());
        }
        let node = &self.nodes[n as usize];
        let mut kids: Vec<(u32, CheckMode)> = Vec::new();
        match node.op {
            Op::Marker(kind, c) => match kind {
                MARK_INTERPOLATED => match mode {
                    CheckMode::Block => kids.push((c, CheckMode::Corner)),
                    CheckMode::Corner => return Err(format!("node {n}: nested interpolation")),
                    CheckMode::Point => kids.push((c, CheckMode::Point)),
                },
                MARK_FLAT_CACHE => kids.push((c, CheckMode::Point)),
                MARK_CACHE_2D => {
                    if self.y_dep[c as usize] {
                        return Err(format!("node {n}: cache_2d over a y-dependent function"));
                    }
                    kids.push((c, mode));
                }
                MARK_CACHE_ONCE => {
                    if mode == CheckMode::Block {
                        return Err(format!("node {n}: cache_once outside an interpolator"));
                    }
                    kids.push((c, mode));
                }
                MARK_CACHE_ALL_IN_CELL => {
                    if mode != CheckMode::Point {
                        return Err(format!("node {n}: cache_all_in_cell inside the router"));
                    }
                    kids.push((c, mode));
                }
                k => return Err(format!("node {n}: unknown marker kind {k}")),
            },
            Op::Beardifier if mode != CheckMode::Point => return Err(format!("node {n}: beardifier marker inside the router")),
            Op::Spline { spline, .. } => {
                let mut stack = vec![spline];
                while let Some(s) = stack.pop() {
                    if let Spline::Multipoint { coord, values, .. } = &self.splines[s as usize] {
                        if let Coord::Node(c) = coord {
                            kids.push((*c, mode));
                        }
                        stack.extend(values.iter().copied());
                    }
                }
            }
            _ => self.for_each_child(n, |c| kids.push((c, mode))),
        }
        for (c, m) in kids {
            self.check(c, m, seen)?;
        }
        Ok(())
    }

    fn for_each_child(&self, n: u32, mut f: impl FnMut(u32)) {
        match self.nodes[n as usize].op {
            Op::Add(a, b) | Op::Mul(a, b) | Op::Min(a, b) | Op::Max(a, b) => {
                f(a);
                f(b)
            }
            Op::AddConst(c, _) | Op::MulConst(c, _) | Op::Mapped(_, c) | Op::Clamp(c, _, _) | Op::Marker(_, c)
            | Op::Slide(c, _) | Op::BlendDensity(c) | Op::WeirdScaled { input: c, .. } => f(c),
            Op::ShiftedNoise { sx, sy, sz, .. } => {
                f(sx);
                f(sy);
                f(sz)
            }
            Op::RangeChoice { input, in_range, out_of_range, .. } => {
                f(input);
                f(in_range);
                f(out_of_range)
            }
            Op::TerrainShaperSpline { c, e, w, .. } => {
                f(c);
                f(e);
                f(w)
            }
            Op::Spline { .. } // handled by caller (spline coordinates)
            | Op::Constant(_) | Op::Noise(..) | Op::ShiftA(_) | Op::ShiftB(_) | Op::Shift(_)
            | Op::YClampedGradient { .. } | Op::BlendAlpha | Op::BlendOffset | Op::Beardifier
            | Op::OldBlendedNoise(_) => {}
        }
    }

    /// Interpolated markers reachable from `root` in Block mode (the ones NoiseChunk
    /// turns into NoiseInterpolators that this root reads).
    fn collect_interpolators(&self, root: u32, out: &mut Vec<u32>, seen: &mut std::collections::HashSet<u32>) {
        if !seen.insert(root) {
            return;
        }
        match self.nodes[root as usize].op {
            Op::Marker(MARK_INTERPOLATED, _) => out.push(root),
            Op::Marker(MARK_FLAT_CACHE, _) => {} // point-mode subtree
            Op::Spline { spline, .. } => {
                let mut stack = vec![spline];
                while let Some(s) = stack.pop() {
                    if let Spline::Multipoint { coord, values, .. } = &self.splines[s as usize] {
                        if let Coord::Node(c) = coord {
                            self.collect_interpolators(*c, out, seen);
                        }
                        stack.extend(values.iter().copied());
                    }
                }
            }
            _ => {
                let mut kids = Vec::new();
                self.for_each_child(root, |c| kids.push(c));
                for c in kids {
                    self.collect_interpolators(c, out, seen);
                }
            }
        }
    }

    /// Equivalent of NoiseBasedChunkGenerator.doFill's noise sampling for one chunk with
    /// Blender.empty(): fills `out.density` with the cell-cached final density (+ beardifier)
    /// and each `out.extra` root with its per-block value, bit-identical to vanilla.
    ///
    /// `beardifier`: per-block values (same layout as outputs), or None for "no structures".
    pub fn fill_chunk(&self, p: ChunkParams, final_root: u32, beardifier: Option<&[f64]>, out: ChunkOut) -> Result<(), String> {
        p.validate()?;
        let vol = p.volume();
        if out.density.len() != vol || out.extra.iter().any(|(_, o)| o.len() != vol) || beardifier.is_some_and(|b| b.len() != vol) {
            return Err(format!("array length mismatch: expected {vol}"));
        }
        self.check_chunk(final_root).map_err(|e| format!("final density: {e}"))?;
        if let Some(m) = &out.material {
            if m.codes.len() != vol {
                return Err(format!("codes length mismatch: expected {vol}"));
            }
            let c = m.cfg;
            for r in [c.barrier, c.floodedness, c.spread, c.lava, c.initial_density] {
                if r as usize >= self.nodes.len() {
                    return Err(format!("invalid aquifer root node {r}"));
                }
            }
            if c.aquifers_enabled {
                self.check_surface_root(c.initial_density).map_err(|e| format!("surface root: {e}"))?;
            }
            if let Some(v) = &c.veins {
                for r in [v.toggle, v.ridged, v.gap] {
                    self.check_chunk(r).map_err(|e| format!("vein root node {r}: {e}"))?;
                }
            }
        }
        for (r, _) in &out.extra {
            self.check_chunk(*r).map_err(|e| format!("root node {r}: {e}"))?;
        }

        // NoiseInterpolators: every Interpolated node reachable from any root.
        let mut interps = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_interpolators(final_root, &mut interps, &mut seen);
        for (r, _) in &out.extra {
            self.collect_interpolators(*r, &mut interps, &mut seen);
        }
        if let Some(v) = out.material.as_ref().and_then(|m| m.cfg.veins.as_ref()) {
            for r in [v.toggle, v.ridged, v.gap] {
                self.collect_interpolators(r, &mut interps, &mut seen);
            }
        }
        let mut slot_of = vec![u32::MAX; self.nodes.len()];
        for (slot, &n) in interps.iter().enumerate() {
            slot_of[n as usize] = slot as u32;
        }

        let (w, h) = (p.cell_width, p.cell_height);
        let nxz = p.cell_count_xz as usize;
        let ny = p.cell_count_y as usize;
        let first_cell_x = p.min_block_x.div_euclid(w);
        let first_cell_z = p.min_block_z.div_euclid(w);

        // Per-chunk caches for every flat_cache / cache_2d / cache_once node.
        let width = p.width();
        let side = width + 1;
        let mut col_slot = vec![u32::MAX; self.nodes.len()];
        let mut once_slot = vec![u32::MAX; self.nodes.len()];
        let (mut n_col, mut n_once) = (0u32, 0u32);
        for (i, node) in self.nodes.iter().enumerate() {
            match node.op {
                Op::Marker(MARK_FLAT_CACHE | MARK_CACHE_2D, _) => {
                    col_slot[i] = n_col;
                    n_col += 1;
                }
                Op::Marker(MARK_CACHE_ONCE, _) => {
                    once_slot[i] = n_once;
                    n_once += 1;
                }
                _ => {}
            }
        }
        let cells = n_col as usize * side * side;
        let memo = Memo {
            col_slot,
            once_slot,
            min_x: p.min_block_x,
            min_z: p.min_block_z,
            side,
            col_val: (0..cells).map(|_| std::cell::Cell::new(0.0)).collect(),
            col_set: (0..cells).map(|_| std::cell::Cell::new(false)).collect(),
            once_val: (0..n_once).map(|_| std::cell::Cell::new(0.0)).collect(),
            once_stamp: (0..n_once).map(|_| std::cell::Cell::new(u64::MAX)).collect(),
            stamp: std::cell::Cell::new(0),
        };

        // Slice filling: each interpolator's wrapped function at every cell corner, batched
        // per corner column (all interpolators share the batch, like vanilla's per-column
        // cache_once reuse across interpolators).
        let gidx = |cx: usize, cz: usize, cy: usize| (cx * (nxz + 1) + cz) * (ny + 1) + cy;
        let children: Vec<u32> = interps
            .iter()
            .map(|&n| match self.nodes[n as usize].op {
                Op::Marker(_, c) => c,
                _ => unreachable!(),
            })
            .collect();
        let mut grids: Vec<Vec<f64>> = vec![vec![0.0; (nxz + 1) * (nxz + 1) * (ny + 1)]; interps.len()];
        let pool = batch::Pool::new();
        let shared = batch::Shared::new(self);
        let mut buf = [0.0f64; batch::L];
        for cx in 0..=nxz {
            let bx = (first_cell_x + cx as i32) * w;
            for cz in 0..=nxz {
                let bz = (first_cell_z + cz as i32) * w;
                let mut start = 0;
                while start <= ny {
                    let n = (ny + 1 - start).min(batch::L);
                    let mut bt = batch::Batch {
                        xs: [bx; batch::L],
                        ys: [0; batch::L],
                        zs: [bz; batch::L],
                        block: None,
                        point: false,
                        uniform_xz: true,
                        memo: &memo,
                        pool: &pool,
                        shared: &shared,
                    };
                    for k in 0..n {
                        bt.ys[k] = ((start + k) as i32 + p.cell_noise_min_y) * h;
                    }
                    shared.next();
                    for (slot, &child) in children.iter().enumerate() {
                        self.evb(child, &bt, batch::Act::All(n), &mut buf);
                        for k in 0..n {
                            grids[slot][gidx(cx, cz, start + k)] = buf[k];
                        }
                    }
                    start += n;
                }
            }
        }

        let min_y = p.cell_noise_min_y * h;
        let mut corners = vec![[0.0f64; 8]; interps.len()];
        let lazy_veins = out.lazy_veins;
        if lazy_veins && out.extra.len() != 3 {
            return Err("lazy_veins requires exactly [vein_toggle, vein_ridged, vein_gap]".into());
        }
        let mut extra = out.extra;
        let density = out.density;
        let mut material = out.material;
        let mut aquifer = material.as_ref().map(|m| {
            aquifer::Aquifer::new(self, m.cfg, p.min_block_x, p.min_block_z, min_y, p.height() as i32, &memo, &pool, &shared)
        });

        // Block positions of one cell: (x, y, z, dx, dy, dz, output index).
        let mut cell_pos: Vec<(i32, i32, i32, f64, f64, f64, usize)> = Vec::with_capacity((w * w * h) as usize);
        let mut buf2 = [0.0f64; batch::L];

        for cx in 0..nxz {
            for cz in 0..nxz {
                for cy in 0..ny {
                    // NoiseInterpolator.selectCellYZ
                    for (slot, g) in grids.iter().enumerate() {
                        corners[slot] = [
                            g[gidx(cx, cz, cy)],             // n000
                            g[gidx(cx + 1, cz, cy)],         // n100
                            g[gidx(cx, cz, cy + 1)],         // n010
                            g[gidx(cx + 1, cz, cy + 1)],     // n110
                            g[gidx(cx, cz + 1, cy)],         // n001
                            g[gidx(cx + 1, cz + 1, cy)],     // n101
                            g[gidx(cx, cz + 1, cy + 1)],     // n011
                            g[gidx(cx + 1, cz + 1, cy + 1)], // n111
                        ];
                    }
                    cell_pos.clear();
                    for ty in 0..h {
                        let y = (p.cell_noise_min_y + cy as i32) * h + ty;
                        for tx in 0..w {
                            let x = (first_cell_x + cx as i32) * w + tx;
                            for tz in 0..w {
                                let z = (first_cell_z + cz as i32) * w + tz;
                                let idx = ((y - min_y) as usize * width + (z - p.min_block_z) as usize) * width
                                    + (x - p.min_block_x) as usize;
                                cell_pos.push((x, y, z, tx as f64 / w as f64, ty as f64 / h as f64, tz as f64 / w as f64, idx));
                            }
                        }
                    }

                    for lanes in cell_pos.chunks(batch::L) {
                        let n = lanes.len();
                        let mut bl = batch::BlockLanes {
                            slot_of: &slot_of,
                            corners: &corners,
                            dx: [0.0; batch::L],
                            dy: [0.0; batch::L],
                            dz: [0.0; batch::L],
                            lerp3: true,
                        };
                        let (mut xs, mut ys, mut zs) = ([0; batch::L], [0; batch::L], [0; batch::L]);
                        for (k, &(x, y, z, dx, dy, dz, _)) in lanes.iter().enumerate() {
                            (xs[k], ys[k], zs[k]) = (x, y, z);
                            (bl.dx[k], bl.dy[k], bl.dz[k]) = (dx, dy, dz);
                        }
                        let mut bt = batch::Batch {
                            xs,
                            ys,
                            zs,
                            block: Some(bl),
                            point: false,
                            uniform_xz: false,
                            memo: &memo,
                            pool: &pool,
                            shared: &shared,
                        };

                        // cacheAllInCell(add(finalDensity, beardifier)): the fillingCell (lerp3) path.
                        shared.next();
                        self.evb(final_root, &bt, batch::Act::All(n), &mut buf);
                        for (k, l) in lanes.iter().enumerate() {
                            // Ap2(ADD, finalDensity, beardifier): d + e, always (so -0.0 + 0.0 == 0.0 as in vanilla).
                            density[l.6] = buf[k] + beardifier.map_or(0.0, |b| b[l.6]);
                        }

                        if let (Some(m), Some(aq)) = (material.as_mut(), aquifer.as_mut()) {
                            self.cell_materials(&mut bt, lanes, density, m, aq, &shared, &mut buf, &mut buf2);
                        }

                        if extra.is_empty() {
                            continue;
                        }
                        // Per-block roots (OreVeinifier): the updateForY/X/Z path.
                        if let Some(bl) = bt.block.as_mut() {
                            bl.lerp3 = false;
                        }
                        shared.next();
                        if lazy_veins {
                            let (t, rest) = extra.split_at_mut(1);
                            // Outside both vein bands OreVeinifier ignores the toggle value.
                            let mut in_band = batch::Lanes::new();
                            for (k, l) in lanes.iter().enumerate() {
                                if l.1 >= VEIN_BAND.0 && l.1 <= VEIN_BAND.1 {
                                    in_band.push(k);
                                } else {
                                    t[0].1[l.6] = f64::NAN;
                                }
                            }
                            let mut needed = batch::Lanes::new();
                            if !in_band.is_empty() {
                                self.evb(t[0].0, &bt, in_band.act(), &mut buf);
                                for (k, l) in lanes.iter().enumerate() {
                                    if l.1 >= VEIN_BAND.0 && l.1 <= VEIN_BAND.1 {
                                        t[0].1[l.6] = buf[k];
                                        if !buf[k].is_nan() && vein_needs(buf[k], l.1) {
                                            needed.push(k);
                                        }
                                    }
                                }
                            }
                            for (r, o) in rest.iter_mut() {
                                for l in lanes {
                                    o[l.6] = f64::NAN;
                                }
                                if !needed.is_empty() {
                                    self.evb(*r, &bt, needed.act(), &mut buf2);
                                    needed.act_each(|k| o[lanes[k].6] = buf2[k]);
                                }
                            }
                        } else {
                            for (r, o) in extra.iter_mut() {
                                self.evb(*r, &bt, batch::Act::All(n), &mut buf);
                                for (k, l) in lanes.iter().enumerate() {
                                    o[l.6] = buf[k];
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Program {
    /// MaterialRuleList(aquifer rule, ore veinifier) for one batch of blocks, then the
    /// default block. `density` already holds cacheAllInCell(add(finalDensity, beardifier)).
    #[allow(clippy::too_many_arguments)]
    fn cell_materials(
        &self,
        bt: &mut batch::Batch,
        lanes: &[(i32, i32, i32, f64, f64, f64, usize)],
        density: &[f64],
        m: &mut Material,
        aq: &mut aquifer::Aquifer,
        shared: &batch::Shared,
        buf: &mut [f64; batch::L],
        buf2: &mut [f64; batch::L],
    ) {
        use aquifer::code;
        let codes = &mut *m.codes;

        // 1. Aquifer rule (vanilla calls it for every block).
        let mut solid = batch::Lanes::new();
        for (k, l) in lanes.iter().enumerate() {
            let (state, update) = aq.compute(l.0, l.1, l.2, density[l.6]);
            let flag = if update { code::FLUID_UPDATE } else { 0 };
            codes[l.6] = match state {
                Some(f) => f.code_with(flag),
                None => {
                    solid.push(k);
                    code::DEFAULT_BLOCK | flag
                }
            };
        }
        let Some(v) = &m.cfg.veins else { return };
        if solid.is_empty() {
            return;
        }

        // 2. Ore veinifier on blocks the aquifer left solid. Values come from the
        //    per-block (updateForY/X/Z) interpolation path, as for OreVeinifier in vanilla.
        if let Some(bl) = bt.block.as_mut() {
            bl.lerp3 = false;
        }
        shared.next();
        // Outside both vein bands the rule returns null whatever the toggle is.
        let mut in_band = batch::Lanes::new();
        for &k in solid.lanes() {
            let y = lanes[k as usize].1;
            if y >= aquifer::IRON.min_y && y <= aquifer::COPPER.max_y {
                in_band.push(k as usize);
            }
        }
        if in_band.is_empty() {
            return;
        }
        self.evb(v.toggle, bt, in_band.act(), buf);

        let mut rngs: [Option<crate::random::Xoroshiro>; batch::L] = std::array::from_fn(|_| None);
        let mut kinds: [Option<(&aquifer::VeinType, f64)>; batch::L] = [None; batch::L];
        let mut need_ridged = batch::Lanes::new();
        for &k in in_band.lanes() {
            let k = k as usize;
            let l = &lanes[k];
            let Some(kind) = aquifer::vein_stage1(buf[k], l.1) else { continue };
            let mut rng = aquifer::ore_random(v, l.0, l.1, l.2);
            if rng.next_float() > 0.7f32 {
                continue;
            }
            rngs[k] = Some(rng);
            kinds[k] = Some(kind);
            need_ridged.push(k);
        }
        if need_ridged.is_empty() {
            return;
        }
        self.evb(v.ridged, bt, need_ridged.act(), buf);

        let mut need_gap = batch::Lanes::new();
        for &k in need_ridged.lanes() {
            let k = k as usize;
            if buf[k] >= 0.0 {
                continue; // rule yields null -> default block
            }
            let (vt, e) = kinds[k].unwrap();
            let rng = rngs[k].as_mut().unwrap();
            if (rng.next_float() as f64) < aquifer::vein_richness(e) {
                need_gap.push(k); // `&&` continues to the gap check
            } else {
                codes[lanes[k].6] = vt.filler;
            }
        }
        if need_gap.is_empty() {
            return;
        }
        self.evb(v.gap, bt, need_gap.act(), buf2);
        for &k in need_gap.lanes() {
            let k = k as usize;
            let (vt, _) = kinds[k].unwrap();
            codes[lanes[k].6] = if buf2[k] > (-0.3f32) as f64 {
                if rngs[k].as_mut().unwrap().next_float() < 0.02f32 { vt.raw } else { vt.ore }
            } else {
                vt.filler
            };
        }
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
        p: Program { noises: vec![], blended: vec![], splines: vec![], nodes: vec![], roots: vec![], y_dep: vec![] },
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
    let mut p = ps.p;
    p.compute_y_dep();
    Ok(p)
}
