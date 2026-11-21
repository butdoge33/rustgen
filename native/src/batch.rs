//! Batched ("lane") evaluation for chunk mode — a child module of `density`.
//!
//! Instead of walking the graph once per position, each node is evaluated for a whole
//! batch of positions (a column of interpolator corners, or all blocks of a cell). This
//! removes per-node dispatch from the inner loop and lets simple per-lane arithmetic
//! auto-vectorize.
//!
//! Exactness rules (results must stay bit-identical to vanilla):
//!  - every lane computes exactly the scalar formula, in the same operation order;
//!  - short-circuits are honored per lane: `mul` evaluates its second argument only for
//!    lanes where the first is non-zero, `min`/`max` only where the bound check fails,
//!    `range_choice` evaluates each branch only for the lanes that select it. Evaluating
//!    more would be harmless (everything here is pure) but slower; evaluating less is
//!    impossible because skipped lanes never read the value;
//!  - a y-independent subtree in a batch whose lanes share one (x, z) column is computed
//!    once and broadcast (`y_dep` is conservative);
//!  - anything without a dedicated batched form falls back to the scalar evaluator per
//!    lane, with the same mode and memo the scalar path would use.

use std::cell::{Cell, RefCell};

use super::*;

/// Maximum lanes per batch. A cell of 4x8x4 = 128 blocks fits exactly; bigger cells and
/// taller corner columns are split into several batches.
pub(super) const L: usize = 128;

type Buf = Box<[f64; L]>;

/// Recycles temporary lane buffers (avoids 1 KiB stack frames per recursion level).
pub(super) struct Pool(RefCell<Vec<Buf>>);

impl Pool {
    pub(super) fn new() -> Self {
        Pool(RefCell::new(Vec::new()))
    }
    #[inline]
    fn take(&self) -> Buf {
        self.0.borrow_mut().pop().unwrap_or_else(|| Box::new([0.0; L]))
    }
    #[inline]
    fn give(&self, b: Buf) {
        self.0.borrow_mut().push(b);
    }
}

/// Which lanes of the batch to compute.
#[derive(Clone, Copy)]
pub(super) enum Act<'a> {
    All(usize),
    Some(&'a [u8]),
}

impl Act<'_> {
    #[inline]
    fn each(&self, mut f: impl FnMut(usize)) {
        match *self {
            Act::All(n) => (0..n).for_each(f),
            Act::Some(list) => list.iter().for_each(|&i| f(i as usize)),
        }
    }
    #[inline]
    fn first(&self) -> Option<usize> {
        match *self {
            Act::All(n) => (n > 0).then_some(0),
            Act::Some(list) => list.first().map(|&i| i as usize),
        }
    }
}

/// A subset of lanes, built while evaluating a short-circuiting node.
pub(super) struct Lanes {
    list: [u8; L],
    len: usize,
}

impl Lanes {
    #[inline]
    pub(super) fn new() -> Self {
        Lanes { list: [0; L], len: 0 }
    }
    #[inline]
    pub(super) fn push(&mut self, i: usize) {
        self.list[self.len] = i as u8;
        self.len += 1;
    }
    #[inline]
    pub(super) fn act(&self) -> Act<'_> {
        Act::Some(&self.list[..self.len])
    }
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[inline]
    pub(super) fn lanes(&self) -> &[u8] {
        &self.list[..self.len]
    }
    #[inline]
    pub(super) fn act_each(&self, f: impl FnMut(usize)) {
        self.act().each(f)
    }
}

/// Results of shared (multi-parent) nodes for the current batch, so each is computed once.
pub(super) struct Shared {
    slot: Vec<u32>,
    val: RefCell<Vec<[f64; L]>>,
    /// Lanes known to be valid for a slot in the current generation.
    known: RefCell<Vec<[bool; L]>>,
    gen: Vec<Cell<u64>>,
    cur: Cell<u64>,
}

impl Shared {
    pub(super) fn new(p: &Program) -> Self {
        let mut parents = vec![0u32; p.nodes.len()];
        for n in 0..p.nodes.len() {
            p.for_each_child(n as u32, |c| parents[c as usize] += 1);
        }
        let mut slot = vec![u32::MAX; p.nodes.len()];
        let mut k = 0u32;
        for (n, &count) in parents.iter().enumerate() {
            if count > 1 {
                slot[n] = k;
                k += 1;
            }
        }
        Shared {
            slot,
            val: RefCell::new(vec![[0.0; L]; k as usize]),
            known: RefCell::new(vec![[false; L]; k as usize]),
            gen: (0..k).map(|_| Cell::new(0)).collect(),
            cur: Cell::new(0),
        }
    }

    /// Starts a new batch (new positions or a different evaluation path): forget everything.
    #[inline]
    pub(super) fn next(&self) {
        self.cur.set(self.cur.get() + 1);
    }
}

/// Per-lane data for block batches.
pub(super) struct BlockLanes<'a> {
    pub(super) slot_of: &'a [u32],
    pub(super) corners: &'a [[f64; 8]],
    pub(super) dx: [f64; L],
    pub(super) dy: [f64; L],
    pub(super) dz: [f64; L],
    pub(super) lerp3: bool,
}

pub(super) struct Batch<'a> {
    pub(super) xs: [i32; L],
    pub(super) ys: [i32; L],
    pub(super) zs: [i32; L],
    /// None: interpolator-corner batch (Mode::Corner), or a point batch if `point`.
    /// Some: block batch (Mode::Block).
    pub(super) block: Option<BlockLanes<'a>>,
    /// SinglePointContext semantics (Mode::Point): every marker is pass-through.
    pub(super) point: bool,
    /// All lanes share one (x, z) column, so y-independent subtrees are uniform.
    pub(super) uniform_xz: bool,
    pub(super) memo: &'a Memo,
    pub(super) pool: &'a Pool,
    pub(super) shared: &'a Shared,
}

impl Program {
    /// Evaluates node `n` for the `act` lanes of `b` into `out` (other lanes untouched).
    pub(super) fn evb(&self, n: u32, b: &Batch, act: Act, out: &mut [f64; L]) {
        let slot = b.shared.slot[n as usize];
        if slot == u32::MAX {
            return self.evb_compute(n, b, act, out);
        }
        let s = slot as usize;
        let cur = b.shared.cur.get();
        if b.shared.gen[s].get() != cur {
            b.shared.gen[s].set(cur);
            b.shared.known.borrow_mut()[s] = [false; L];
        }
        // Compute only lanes not already known for this batch.
        let mut missing = Lanes::new();
        {
            let known = &b.shared.known.borrow()[s];
            let val = &b.shared.val.borrow()[s];
            act.each(|i| if known[i] { out[i] = val[i] } else { missing.push(i) });
        }
        if missing.is_empty() {
            return;
        }
        self.evb_compute(n, b, missing.act(), out);
        let mut known = b.shared.known.borrow_mut();
        let mut val = b.shared.val.borrow_mut();
        missing.act().each(|i| {
            known[s][i] = true;
            val[s][i] = out[i];
        });
    }

    /// Scalar evaluation of node `n` for one lane, in the mode the batch represents.
    #[inline]
    fn scalar_lane(&self, n: u32, b: &Batch, i: usize) -> f64 {
        let (x, y, z) = (b.xs[i], b.ys[i], b.zs[i]);
        match &b.block {
            None if b.point => self.ev(n, x, y, z, Mode::Point),
            None => {
                b.memo.stamp.set(b.memo.stamp.get() + 1); // new position for cache_once
                self.ev(n, x, y, z, Mode::Corner(b.memo))
            }
            Some(bl) => {
                let ctx = BlockCtx {
                    memo: b.memo,
                    slot_of: bl.slot_of,
                    corners: bl.corners,
                    dx: bl.dx[i],
                    dy: bl.dy[i],
                    dz: bl.dz[i],
                    lerp3: bl.lerp3,
                };
                self.ev(n, x, y, z, Mode::Block(&ctx))
            }
        }
    }

    fn evb_compute(&self, n: u32, b: &Batch, act: Act, out: &mut [f64; L]) {
        if b.uniform_xz && !self.y_dep[n as usize] {
            if let Some(i) = act.first() {
                let v = self.scalar_lane(n, b, i);
                act.each(|j| out[j] = v);
            }
            return;
        }
        let (xs, ys, zs) = (&b.xs, &b.ys, &b.zs);
        match self.nodes[n as usize].op {
            Op::Constant(v) => act.each(|i| out[i] = v),
            Op::Add(a, c) => {
                self.evb(a, b, act, out);
                let mut t = b.pool.take();
                self.evb(c, b, act, &mut t);
                act.each(|i| out[i] += t[i]);
                b.pool.give(t);
            }
            Op::Mul(a, c) => {
                self.evb(a, b, act, out);
                let mut need = Lanes::new();
                act.each(|i| if out[i] == 0.0 { out[i] = 0.0 } else { need.push(i) });
                if !need.is_empty() {
                    let mut t = b.pool.take();
                    self.evb(c, b, need.act(), &mut t);
                    need.act().each(|i| out[i] *= t[i]);
                    b.pool.give(t);
                }
            }
            Op::Min(a, c) => {
                self.evb(a, b, act, out);
                let bound = self.nodes[c as usize].min;
                let mut need = Lanes::new();
                act.each(|i| if !(out[i] < bound) { need.push(i) });
                if !need.is_empty() {
                    let mut t = b.pool.take();
                    self.evb(c, b, need.act(), &mut t);
                    need.act().each(|i| out[i] = java_min(out[i], t[i]));
                    b.pool.give(t);
                }
            }
            Op::Max(a, c) => {
                self.evb(a, b, act, out);
                let bound = self.nodes[c as usize].max;
                let mut need = Lanes::new();
                act.each(|i| if !(out[i] > bound) { need.push(i) });
                if !need.is_empty() {
                    let mut t = b.pool.take();
                    self.evb(c, b, need.act(), &mut t);
                    need.act().each(|i| out[i] = java_max(out[i], t[i]));
                    b.pool.give(t);
                }
            }
            Op::AddConst(i, c) => {
                self.evb(i, b, act, out);
                act.each(|j| out[j] += c);
            }
            Op::MulConst(i, c) => {
                self.evb(i, b, act, out);
                act.each(|j| out[j] *= c);
            }
            Op::Mapped(kind, i) => {
                self.evb(i, b, act, out);
                act.each(|j| out[j] = map_value(kind, out[j]));
            }
            Op::Clamp(i, lo, hi) => {
                self.evb(i, b, act, out);
                act.each(|j| out[j] = clamp(out[j], lo, hi));
            }
            // Point semantics: all markers pass through (y-independent flat_cache/cache_2d
            // children were already handled by the uniform path above).
            Op::Marker(_, i) if b.point => self.evb(i, b, act, out),
            Op::Marker(kind, i) => match (kind, &b.block) {
                (MARK_INTERPOLATED, Some(bl)) => {
                    let c = &bl.corners[bl.slot_of[n as usize] as usize];
                    act.each(|j| out[j] = interpolate(c, bl.dx[j], bl.dy[j], bl.dz[j], bl.lerp3));
                }
                // Column-cached markers: the scalar path hits the per-chunk memo.
                (MARK_FLAT_CACHE | MARK_CACHE_2D | MARK_INTERPOLATED, _) => act.each(|j| out[j] = self.scalar_lane(n, b, j)),
                // cache_once & friends are value-transparent; sharing is handled by `Shared`.
                _ => self.evb(i, b, act, out),
            },
            Op::Noise(None, _, _) | Op::ShiftedNoise { noise: None, .. } | Op::WeirdScaled { noise: None, .. } => {
                act.each(|i| out[i] = 0.0)
            }
            Op::Noise(Some(k), xz, ysc) => {
                let noise = &self.noises[k as usize];
                act.each(|i| out[i] = noise.get_value(xs[i] as f64 * xz, ys[i] as f64 * ysc, zs[i] as f64 * xz));
            }
            Op::ShiftedNoise { sx, sy, sz, xz_scale, y_scale, noise: Some(k) } => {
                let (mut tx, mut ty) = (b.pool.take(), b.pool.take());
                self.evb(sx, b, act, &mut tx);
                self.evb(sy, b, act, &mut ty);
                self.evb(sz, b, act, out);
                let noise = &self.noises[k as usize];
                act.each(|i| {
                    let d = xs[i] as f64 * xz_scale + tx[i];
                    let e = ys[i] as f64 * y_scale + ty[i];
                    let f = zs[i] as f64 * xz_scale + out[i];
                    out[i] = noise.get_value(d, e, f);
                });
                b.pool.give(tx);
                b.pool.give(ty);
            }
            Op::RangeChoice { input, min_inclusive, max_exclusive, in_range, out_of_range } => {
                self.evb(input, b, act, out);
                let (mut inside, mut outside) = (Lanes::new(), Lanes::new());
                act.each(|i| if out[i] >= min_inclusive && out[i] < max_exclusive { inside.push(i) } else { outside.push(i) });
                if !inside.is_empty() {
                    self.evb(in_range, b, inside.act(), out);
                }
                if !outside.is_empty() {
                    self.evb(out_of_range, b, outside.act(), out);
                }
            }
            Op::Slide(i, settings) => {
                self.evb(i, b, act, out);
                if let Some(s) = settings {
                    act.each(|j| out[j] = s.apply(out[j], ys[j]));
                }
            }
            Op::WeirdScaled { input, noise: Some(k), type2 } => {
                self.evb(input, b, act, out);
                let noise = &self.noises[k as usize];
                act.each(|i| {
                    let e = if type2 { rarity_2d(out[i]) } else { rarity_3d(out[i]) };
                    out[i] = e * noise.get_value(xs[i] as f64 / e, ys[i] as f64 / e, zs[i] as f64 / e).abs();
                });
            }
            Op::YClampedGradient { from_y, to_y, from_value, to_value } => act.each(|i| {
                let t = (ys[i] as f64 - from_y as f64) / (to_y as f64 - from_y as f64);
                out[i] = clamped_lerp(from_value, to_value, t);
            }),
            Op::BlendAlpha => act.each(|i| out[i] = 1.0),
            Op::BlendOffset => act.each(|i| out[i] = 0.0),
            Op::BlendDensity(i) => self.evb(i, b, act, out),
            // Splines, shift noises, old blended noise, ...: scalar per lane.
            _ => act.each(|i| out[i] = self.scalar_lane(n, b, i)),
        }
    }
}
