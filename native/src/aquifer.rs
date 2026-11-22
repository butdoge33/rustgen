//! Native block-state rules for chunk mode — a child module of `density`.
//!
//! Bit-exact ports of 1.18.2 `Aquifer.NoiseBasedAquifer` (+ `Aquifer.createDisabled`),
//! `NoiseBasedChunkGenerator`'s global fluid picker, `NoiseChunk.preliminarySurfaceLevel`
//! and `OreVeinifier`. The output per block is a [`code`] byte; Java maps it back to the
//! identical vanilla `BlockState` objects and performs placement.
//!
//! Every aquifer cache here is pure (computed from position only), so evaluation order
//! cannot change results — the same holds in vanilla.

use std::collections::HashMap;

use super::*;
use crate::random::{Xoroshiro, XoroshiroPositional};

/// Block-state codes written per block. Bit 7 = "schedule fluid update" (aquifer flag).
pub mod code {
    /// `NoiseGeneratorSettings.defaultBlock()` (all rules returned null).
    pub const DEFAULT_BLOCK: u8 = 0;
    pub const AIR: u8 = 1;
    pub const LAVA: u8 = 2;
    pub const WATER: u8 = 3;
    pub const COPPER_ORE: u8 = 4;
    pub const RAW_COPPER_BLOCK: u8 = 5;
    pub const GRANITE: u8 = 6;
    pub const DEEPSLATE_IRON_ORE: u8 = 7;
    pub const RAW_IRON_BLOCK: u8 = 8;
    pub const TUFF: u8 = 9;
    pub const FLUID_UPDATE: u8 = 0x80;
}

/// Fluid block types used by aquifer fluid statuses (always default block states).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fluid {
    Air,
    Water,
    Lava,
}

impl Fluid {
    pub(super) fn code_with(self, flag: u8) -> u8 {
        flag | match self {
            Fluid::Air => code::AIR,
            Fluid::Water => code::WATER,
            Fluid::Lava => code::LAVA,
        }
    }
}

/// Aquifer.FluidStatus
#[derive(Clone, Copy, Debug)]
struct FluidStatus {
    level: i32,
    kind: Fluid,
}

impl FluidStatus {
    #[inline]
    fn at(&self, y: i32) -> Fluid {
        if y < self.level { self.kind } else { Fluid::Air }
    }
}

pub struct VeinConfig {
    pub toggle: u32,
    pub ridged: u32,
    pub gap: u32,
    pub random: XoroshiroPositional,
}

/// Everything the block-state rules need besides the chunk geometry.
pub struct MaterialConfig {
    pub aquifers_enabled: bool,
    pub sea_level: i32,
    /// NoiseGeneratorSettings.defaultFluid() (verified on the Java side to be a default state).
    pub default_fluid: Fluid,
    /// DimensionType.WAY_BELOW_MIN_Y
    pub way_below_min_y: i32,
    pub aquifer_random: XoroshiroPositional,
    pub barrier: u32,
    pub floodedness: u32,
    pub spread: u32,
    pub lava: u32,
    /// initial_density_without_jaggedness, scanned for the preliminary surface level.
    pub initial_density: u32,
    /// The generator's NoiseSettings (cell height/count and slides) used by the scan.
    pub surface: SlideSettings,
    pub veins: Option<VeinConfig>,
}

impl SlideSettings {
    pub fn new(cell_height: i32, min_cell_y: i32, cell_count_y: i32, top: (f64, i32, i32), bottom: (f64, i32, i32)) -> Self {
        SlideSettings {
            cell_height,
            min_cell_y,
            cell_count_y,
            top: Slider { target: top.0, size: top.1, offset: top.2 },
            bottom: Slider { target: bottom.0, size: bottom.1, offset: bottom.2 },
        }
    }
}

const LAVA_LEVEL: i32 = -54;

/// NoiseBasedChunkGenerator's globalFluidPicker: y < min(-54, seaLevel) ? lava@-54 : defaultFluid@seaLevel.
#[inline]
fn global_fluid(cfg: &MaterialConfig, y: i32) -> FluidStatus {
    if y < LAVA_LEVEL.min(cfg.sea_level) {
        FluidStatus { level: LAVA_LEVEL, kind: Fluid::Lava }
    } else {
        FluidStatus { level: cfg.sea_level, kind: cfg.default_fluid }
    }
}

/// NoiseBasedAquifer.similarity
#[inline]
fn similarity(i: i32, j: i32) -> f64 {
    1.0 - (j.wrapping_sub(i)).wrapping_abs() as f64 / 25.0
}

/// Mth.map
#[inline]
fn map(d: f64, e: f64, f: f64, g: f64, h: f64) -> f64 {
    lerp((d - e) / (f - e), g, h)
}

/// Mth.clampedMap
#[inline]
fn clamped_map(d: f64, e: f64, f: f64, g: f64, h: f64) -> f64 {
    clamped_lerp(g, h, (d - e) / (f - e))
}

/// Levels evaluated per batch in the preliminary-surface scan.
const SCAN_GROUP: usize = 16;

/// NoiseBasedAquifer.SURFACE_SAMPLING_OFFSETS_IN_CHUNKS
const SURFACE_OFFSETS: [(i32, i32); 13] = [
    (-2, -1), (-1, -1), (0, -1), (1, -1), (-3, 0), (-2, 0), (-1, 0), (0, 0), (1, 0), (-2, 1), (-1, 1), (0, 1), (1, 1),
];

/// Per-chunk aquifer state (NoiseBasedAquifer's caches + NoiseChunk's surface-level cache).
pub(super) struct Aquifer<'a> {
    p: &'a Program,
    memo: &'a Memo,
    pool: &'a super::batch::Pool,
    shared: &'a super::batch::Shared,
    cfg: &'a MaterialConfig,
    min_gx: i32,
    min_gy: i32,
    min_gz: i32,
    size_x: i32,
    size_y: i32,
    size_z: i32,
    location: Vec<Option<(i32, i32, i32)>>,
    status: Vec<Option<FluidStatus>>,
    /// The 12 candidate aquifer centers of the last grid key (l, m, n), in vanilla's
    /// iteration order. Consecutive blocks almost always share the key.
    candidates_key: Option<(i32, i32, i32)>,
    candidates: [(i32, i32, i32); 12],
    /// Fluid status of each candidate center (filled lazily; mirrors `status`).
    candidate_status: [Option<FluidStatus>; 12],
    surface: HashMap<(i32, i32), i32>,
    /// y-independent values of the column being scanned (shared across its groups).
    scan_column: super::batch::ColumnMemo,
}

impl<'a> Aquifer<'a> {
    /// NoiseBasedAquifer constructor: `min_y`/`height` are cellNoiseMinY*cellHeight and
    /// cellCountY*cellHeight, as NoiseChunk passes them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        p: &'a Program,
        cfg: &'a MaterialConfig,
        min_block_x: i32,
        min_block_z: i32,
        min_y: i32,
        height: i32,
        memo: &'a Memo,
        pool: &'a super::batch::Pool,
        shared: &'a super::batch::Shared,
    ) -> Self {
        let min_gx = min_block_x.div_euclid(16) - 1;
        let size_x = ((min_block_x + 15).div_euclid(16) + 1) - min_gx + 1;
        let min_gy = min_y.div_euclid(12) - 1;
        let size_y = ((min_y + height).div_euclid(12) + 1) - min_gy + 1;
        let min_gz = min_block_z.div_euclid(16) - 1;
        let size_z = ((min_block_z + 15).div_euclid(16) + 1) - min_gz + 1;
        let n = (size_x * size_y * size_z) as usize;
        Aquifer {
            p,
            memo,
            pool,
            shared,
            cfg,
            min_gx,
            min_gy,
            min_gz,
            size_x,
            size_y,
            size_z,
            location: vec![None; n],
            status: vec![None; n],
            candidates_key: None,
            candidates: [(0, 0, 0); 12],
            candidate_status: [None; 12],
            surface: HashMap::new(),
            scan_column: super::batch::ColumnMemo::new(p.nodes.len()),
        }
    }

    #[inline]
    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        let (l, m, n) = (gx - self.min_gx, gy - self.min_gy, gz - self.min_gz);
        assert!(
            (0..self.size_x).contains(&l) && (0..self.size_y).contains(&m) && (0..self.size_z).contains(&n),
            "aquifer grid index out of range ({gx}, {gy}, {gz})"
        );
        ((m * self.size_z + n) * self.size_x + l) as usize
    }

    /// Aquifer.computeSubstance. Returns (state or None for "solid", shouldScheduleFluidUpdate).
    pub(super) fn compute(&mut self, x: i32, y: i32, z: i32, d: f64) -> (Option<Fluid>, bool) {
        if d > 0.0 {
            return (None, false);
        }
        let picked = global_fluid(self.cfg, y);
        if !self.cfg.aquifers_enabled {
            // Aquifer.createDisabled
            return (Some(picked.at(y)), false);
        }
        if picked.at(y) == Fluid::Lava {
            return (Some(Fluid::Lava), false);
        }

        let key = ((x - 5).div_euclid(16), (y + 1).div_euclid(12), (z - 5).div_euclid(16));
        if self.candidates_key != Some(key) {
            self.load_candidates(key);
        }
        let (mut o, mut p, mut q) = (i32::MAX, i32::MAX, i32::MAX);
        // Candidate indices of the three nearest centers (vanilla tracks the positions;
        // index 12 stands for vanilla's initial BlockPos 0 and is never selected in practice).
        let (mut r, mut s, mut t) = (12usize, 12usize, 12usize);
        // Same order as vanilla's u/v/w loops, so ties resolve identically.
        for (ci, &pos) in self.candidates.iter().enumerate() {
            let (ad, ae, af) = (pos.0 - x, pos.1 - y, pos.2 - z);
            let ag = ad.wrapping_mul(ad).wrapping_add(ae.wrapping_mul(ae)).wrapping_add(af.wrapping_mul(af));
            if o >= ag {
                t = s;
                s = r;
                r = ci;
                q = p;
                p = o;
                o = ag;
            } else if p >= ag {
                t = s;
                s = ci;
                q = p;
                p = ag;
            } else if q >= ag {
                t = ci;
                q = ag;
            }
        }

        let status_r = self.candidate_fluid(r);
        let e = similarity(o, p);
        let state = status_r.at(y);
        if e <= 0.0 {
            // FLOWING_UPDATE_SIMULARITY = similarity(10², 12²)
            return (Some(state), e >= similarity(100, 144));
        }
        if state == Fluid::Water && global_fluid(self.cfg, y - 1).at(y - 1) == Fluid::Lava {
            return (Some(state), true);
        }

        let mut barrier = f64::NAN; // MutableDouble(NaN): computed at most once per block
        let status_s = self.candidate_fluid(s);
        let f = e * self.pressure(x, y, z, &mut barrier, status_r, status_s);
        if d + f > 0.0 {
            return (None, false);
        }
        let status_t = self.candidate_fluid(t);
        let g = similarity(o, q);
        if g > 0.0 {
            let h = e * g * self.pressure(x, y, z, &mut barrier, status_r, status_t);
            if d + h > 0.0 {
                return (None, false);
            }
        }
        let h = similarity(p, q);
        if h > 0.0 {
            let ah = e * h * self.pressure(x, y, z, &mut barrier, status_s, status_t);
            if d + ah > 0.0 {
                return (None, false);
            }
        }
        (Some(state), true)
    }

    /// The 12 aquifer centers around grid key (l, m, n), in vanilla's u/v/w loop order,
    /// computing (and caching) each center's random position on first use.
    fn load_candidates(&mut self, (l, m, n): (i32, i32, i32)) {
        let mut k = 0;
        for u in 0..=1 {
            for v in -1..=1 {
                for w in 0..=1 {
                    let (gx, gy, gz) = (l + u, m + v, n + w);
                    let idx = self.index(gx, gy, gz);
                    self.candidates[k] = match self.location[idx] {
                        Some(pos) => pos,
                        None => {
                            let mut rng = self.cfg.aquifer_random.at(gx, gy, gz);
                            // Java evaluates the asLong arguments left to right.
                            let px = gx * 16 + rng.next_int_bounded(10);
                            let py = gy * 12 + rng.next_int_bounded(9);
                            let pz = gz * 16 + rng.next_int_bounded(10);
                            self.location[idx] = Some((px, py, pz));
                            (px, py, pz)
                        }
                    };
                    k += 1;
                }
            }
        }
        self.candidates_key = Some((l, m, n));
        self.candidate_status = [None; 12];
    }

    /// Fluid status of candidate `ci` (12 = vanilla's BlockPos 0 fallback).
    #[inline]
    fn candidate_fluid(&mut self, ci: usize) -> FluidStatus {
        if ci == 12 {
            return self.aquifer_status((0, 0, 0));
        }
        if let Some(s) = self.candidate_status[ci] {
            return s;
        }
        let s = self.aquifer_status(self.candidates[ci]);
        self.candidate_status[ci] = Some(s);
        s
    }

    /// NoiseBasedAquifer.calculatePressure
    fn pressure(&self, x: i32, y: i32, z: i32, barrier: &mut f64, a: FluidStatus, b: FluidStatus) -> f64 {
        let (sa, sb) = (a.at(y), b.at(y));
        if (sa == Fluid::Lava && sb == Fluid::Water) || (sa == Fluid::Water && sb == Fluid::Lava) {
            return 2.0;
        }
        let j = a.level.wrapping_sub(b.level).wrapping_abs();
        if j == 0 {
            return 0.0;
        }
        let d = 0.5 * a.level.wrapping_add(b.level) as f64;
        let e = y as f64 + 0.5 - d;
        let f = j as f64 / 2.0;
        let o = f - e.abs();
        let q = if e > 0.0 {
            let p = 0.0 + o;
            if p > 0.0 { p / 1.5 } else { p / 2.5 }
        } else {
            let p = 3.0 + o;
            if p > 0.0 { p / 3.0 } else { p / 10.0 }
        };
        let r = if !(q < -2.0) && !(q > 2.0) {
            if barrier.is_nan() {
                *barrier = self.p.eval(self.cfg.barrier, x, y, z);
            }
            *barrier
        } else {
            0.0
        };
        2.0 * (r + q)
    }

    /// NoiseBasedAquifer.getAquiferStatus
    fn aquifer_status(&mut self, pos: (i32, i32, i32)) -> FluidStatus {
        let idx = self.index(pos.0.div_euclid(16), pos.1.div_euclid(12), pos.2.div_euclid(16));
        if let Some(s) = self.status[idx] {
            return s;
        }
        let s = self.compute_fluid(pos.0, pos.1, pos.2);
        self.status[idx] = Some(s);
        s
    }

    /// NoiseBasedAquifer.computeFluid
    fn compute_fluid(&mut self, i: i32, j: i32, k: i32) -> FluidStatus {
        let picked = global_fluid(self.cfg, j);
        let mut l = i32::MAX;
        let m = j.wrapping_add(12);
        let n = j.wrapping_sub(12);
        let mut surface_fluid = false;
        for (ox, oz) in SURFACE_OFFSETS {
            let o = i + (ox << 4);
            let p = k + (oz << 4);
            let q = self.surface_level(o, p);
            let r = q.wrapping_add(8);
            let center = ox == 0 && oz == 0;
            if center && n > r {
                return picked;
            }
            let above = m > r;
            if above || center {
                let s = global_fluid(self.cfg, r);
                if s.at(r) != Fluid::Air {
                    if center {
                        surface_fluid = true;
                    }
                    if above {
                        return s;
                    }
                }
            }
            l = l.min(q);
        }

        let s = l.wrapping_add(8).wrapping_sub(j);
        let d = if surface_fluid { clamped_map(s as f64, 0.0, 64.0, 1.0, 0.0) } else { 0.0 };
        let e = clamp(self.p.eval(self.cfg.floodedness, i, j, k), -1.0, 1.0);
        let f = map(d, 1.0, 0.0, -0.3, 0.8);
        if e > f {
            return picked;
        }
        let g = map(d, 1.0, 0.0, -0.8, 0.4);
        if e <= g {
            return FluidStatus { level: self.cfg.way_below_min_y, kind: picked.kind };
        }

        let (w, x, y) = (i.div_euclid(16), j.div_euclid(40), k.div_euclid(16));
        let z = x.wrapping_mul(40).wrapping_add(20);
        let h = self.p.eval(self.cfg.spread, w, x, y) * 10.0;
        let ab = crate::noise::floor(h / 3.0).wrapping_mul(3); // Mth.quantize(h, 3)
        let ac = z.wrapping_add(ab);
        let ad = l.min(ac);
        if ac <= -10 {
            let aj = self.p.eval(self.cfg.lava, i.div_euclid(64), j.div_euclid(40), k.div_euclid(64));
            if aj.abs() > 0.3 {
                return FluidStatus { level: ad, kind: Fluid::Lava };
            }
        }
        FluidStatus { level: ad, kind: picked.kind }
    }

    /// NoiseChunk.preliminarySurfaceLevel → NoiseRouterData.computePreliminarySurfaceLevelScanning,
    /// cached per quart column like vanilla. Point evaluation is exact here because
    /// Program::check_surface_root verified NoiseChunk's wrappers are transparent for this root.
    fn surface_level(&mut self, x: i32, z: i32) -> i32 {
        let key = (x >> 2, z >> 2);
        if let Some(&v) = self.surface.get(&key) {
            return v;
        }
        let _t = crate::profile::time(crate::profile::Phase::SurfaceScan);
        let (bx, bz) = (key.0 << 2, key.1 << 2); // QuartPos.toBlock
        let s = self.cfg.surface;
        let mut result = i32::MAX; // (int) 2.147483647E9

        // Vanilla scans cell levels top-down and stops at the first solid one. We evaluate
        // the levels as point batches (top-down chunks of up to L): the y-independent 2D
        // parts — what vanilla's per-column caches make cheap — are computed once per batch.
        // Levels below the answer are computed but unused; everything here is pure.
        use super::batch::{Act, Batch, L};
        let top = s.min_cell_y + s.cell_count_y;
        let mut k0 = top;
        self.scan_column.reset(); // new column
        let mut out = [0.0f64; L];
        'scan: while k0 >= s.min_cell_y {
            // Groups of 16: shares the 2D parts within a group, yet stops near the surface
            // like vanilla's scan instead of evaluating 3D noise for every level.
            let n = ((k0 - s.min_cell_y + 1) as usize).min(SCAN_GROUP);
            let mut bt = Batch {
                xs: [bx; L],
                ys: [0; L],
                zs: [bz; L],
                block: None,
                point: true,
                uniform_xz: true,
                column: Some(&self.scan_column),
                memo: self.memo,
                pool: self.pool,
                shared: self.shared,
            };
            for i in 0..n {
                bt.ys[i] = (k0 - i as i32) * s.cell_height;
            }
            self.shared.next();
            self.p.evb(self.cfg.initial_density, &bt, Act::All(n), &mut out);
            for i in 0..n {
                let l = bt.ys[i];
                let e = out[i] + -0.703125;
                let f = s.apply(clamp(e, -64.0, 64.0), l);
                if f > 0.390625 {
                    result = l;
                    break 'scan;
                }
            }
            k0 -= n as i32;
        }
        self.surface.insert(key, result);
        result
    }
}

/// OreVeinifier.VeinType bands and states.
pub(super) struct VeinType {
    pub(super) min_y: i32,
    pub(super) max_y: i32,
    pub(super) ore: u8,
    pub(super) raw: u8,
    pub(super) filler: u8,
}

pub(super) const COPPER: VeinType =
    VeinType { min_y: 0, max_y: 50, ore: code::COPPER_ORE, raw: code::RAW_COPPER_BLOCK, filler: code::GRANITE };
pub(super) const IRON: VeinType =
    VeinType { min_y: -60, max_y: -8, ore: code::DEEPSLATE_IRON_ORE, raw: code::RAW_IRON_BLOCK, filler: code::TUFF };

/// First stage of OreVeinifier for one block: given vein_toggle, returns the vein type and
/// |toggle| if the block passes the band and threshold checks (else the rule yields null).
#[inline]
pub(super) fn vein_stage1(toggle: f64, y: i32) -> Option<(&'static VeinType, f64)> {
    let vt = if toggle > 0.0 { &COPPER } else { &IRON };
    let e = toggle.abs();
    let (j, k) = (vt.max_y - y, y - vt.min_y);
    if k < 0 || j < 0 {
        return None;
    }
    let f = clamped_map(j.min(k) as f64, 0.0, 20.0, -0.2, 0.0);
    if e + f < 0.4f32 as f64 {
        return None;
    }
    Some((vt, e))
}

/// The probability threshold OreVeinifier compares its second random float against.
#[inline]
pub(super) fn vein_richness(e: f64) -> f64 {
    clamped_map(e, 0.4f32 as f64, 0.6f32 as f64, 0.1f32 as f64, 0.3f32 as f64)
}

impl Program {
    /// NoiseChunk.preliminarySurfaceLevel evaluates initial_density_without_jaggedness
    /// through NoiseChunk's wrappers with a SinglePointContext. That equals plain point
    /// evaluation iff every flat_cache / cache_2d below it wraps a y-independent function
    /// (flat_cache serves the y = 0 value inside the chunk; cache_2d serves the value of
    /// the first y it saw) and no beardifier marker is present.
    pub fn check_surface_root(&self, root: u32) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                continue;
            }
            match self.nodes[n as usize].op {
                Op::Marker(MARK_FLAT_CACHE | MARK_CACHE_2D, c) if self.y_dep[c as usize] => {
                    return Err(format!("node {n}: y-dependent flat_cache/cache_2d below the surface root"));
                }
                Op::Beardifier => return Err(format!("node {n}: beardifier marker below the surface root")),
                Op::Spline { spline, .. } => {
                    let mut ss = vec![spline];
                    while let Some(s) = ss.pop() {
                        if let Spline::Multipoint { coord, values, .. } = &self.splines[s as usize] {
                            if let Coord::Node(c) = coord {
                                stack.push(*c);
                            }
                            ss.extend(values.iter().copied());
                        }
                    }
                }
                _ => self.for_each_child(n, |c| stack.push(c)),
            }
        }
        Ok(())
    }
}

/// Fresh per-block random source for the ore rule.
#[inline]
pub(super) fn ore_random(cfg: &VeinConfig, x: i32, y: i32, z: i32) -> Xoroshiro {
    cfg.random.at(x, y, z)
}
