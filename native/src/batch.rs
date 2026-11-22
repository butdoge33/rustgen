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

/// Lanes of `act` satisfying a predicate: usually all or none, so avoid building a list.
pub(super) enum Sel {
    All,
    Empty,
    Some(Lanes),
}

#[inline]
pub(super) fn select(act: Act, mut pred: impl FnMut(usize) -> bool) -> Sel {
    let (mut yes, mut total) = (0usize, 0usize);
    act.each(|i| {
        total += 1;
        if pred(i) {
            yes += 1;
        }
    });
    if yes == total {
        Sel::All
    } else if yes == 0 {
        Sel::Empty
    } else {
        let mut l = Lanes::new();
        act.each(|i| {
            if pred(i) {
                l.push(i)
            }
        });
        Sel::Some(l)
    }
}

/// Active lane indices as a dense list (for column evaluation).
#[inline]
fn gather(act: Act, idx: &mut [u8; L]) -> usize {
    let mut n = 0;
    act.each(|i| {
        idx[n] = i as u8;
        n += 1;
    });
    n
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

/// Values of y-independent nodes for one (x, z) column, kept across several batches of
/// the same column (e.g. the groups of a surface scan). Valid only while `uniform_xz` and
/// the column stays the same; `reset` before each new column.
pub(super) struct ColumnMemo {
    val: RefCell<Vec<f64>>,
    gen: Vec<Cell<u64>>,
    cur: Cell<u64>,
}

impl ColumnMemo {
    pub(super) fn new(nodes: usize) -> Self {
        ColumnMemo { val: RefCell::new(vec![0.0; nodes]), gen: (0..nodes).map(|_| Cell::new(0)).collect(), cur: Cell::new(0) }
    }
    #[inline]
    pub(super) fn reset(&self) {
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
    /// Optional cross-batch memo for y-independent nodes of this column.
    pub(super) column: Option<&'a ColumnMemo>,
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

    /// For the `act` lanes where `need(out, lane)`: out[lane] = combine(out[lane], node c).
    #[inline]
    fn eval_where(
        &self,
        c: u32,
        b: &Batch,
        act: Act,
        out: &mut [f64; L],
        need: impl Fn(&[f64; L], usize) -> bool,
        combine: impl Fn(f64, f64) -> f64,
    ) {
        let lanes = match select(act, |i| need(out, i)) {
            Sel::Empty => return,
            Sel::All => None,
            Sel::Some(l) => Some(l),
        };
        let sub = lanes.as_ref().map_or(act, |l| l.act());
        let mut t = b.pool.take();
        self.evb(c, b, sub, &mut t);
        sub.each(|i| out[i] = combine(out[i], t[i]));
        b.pool.give(t);
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
                let v = match b.column {
                    Some(c) if c.gen[n as usize].get() == c.cur.get() => c.val.borrow()[n as usize],
                    Some(c) => {
                        let v = self.scalar_lane(n, b, i);
                        c.val.borrow_mut()[n as usize] = v;
                        c.gen[n as usize].set(c.cur.get());
                        v
                    }
                    None => self.scalar_lane(n, b, i),
                };
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
                // d == 0.0 ? 0.0 : d * arg2 — arg2 only where d != 0 (and -0.0 becomes 0.0).
                act.each(|i| if out[i] == 0.0 { out[i] = 0.0 });
                self.eval_where(c, b, act, out, |o, i| o[i] != 0.0, |d, v| d * v);
            }
            Op::Min(a, c) => {
                self.evb(a, b, act, out);
                let bound = self.nodes[c as usize].min;
                self.eval_where(c, b, act, out, |o, i| !(o[i] < bound), java_min);
            }
            Op::Max(a, c) => {
                self.evb(a, b, act, out);
                let bound = self.nodes[c as usize].max;
                self.eval_where(c, b, act, out, |o, i| !(o[i] > bound), java_max);
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
                // Match once per batch, not per lane (same formulas as map_value).
                match kind {
                    MapKind::Abs => act.each(|j| out[j] = map_value(MapKind::Abs, out[j])),
                    MapKind::Square => act.each(|j| out[j] = map_value(MapKind::Square, out[j])),
                    MapKind::Cube => act.each(|j| out[j] = map_value(MapKind::Cube, out[j])),
                    MapKind::HalfNegative => act.each(|j| out[j] = map_value(MapKind::HalfNegative, out[j])),
                    MapKind::QuarterNegative => act.each(|j| out[j] = map_value(MapKind::QuarterNegative, out[j])),
                    MapKind::Squeeze => act.each(|j| out[j] = map_value(MapKind::Squeeze, out[j])),
                }
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
                if b.uniform_xz {
                    let mut idx = [0u8; L];
                    let n = gather(act, &mut idx);
                    let (mut yin, mut res) = ([0.0f64; L], [0.0f64; L]);
                    for q in 0..n {
                        yin[q] = ys[idx[q] as usize] as f64 * ysc;
                    }
                    let f = idx[0] as usize;
                    noise.get_value_column(xs[f] as f64 * xz, zs[f] as f64 * xz, &yin[..n], &mut res);
                    for q in 0..n {
                        out[idx[q] as usize] = res[q];
                    }
                } else {
                    act.each(|i| out[i] = noise.get_value(xs[i] as f64 * xz, ys[i] as f64 * ysc, zs[i] as f64 * xz));
                }
            }
            Op::ShiftedNoise { sx, sy, sz, xz_scale, y_scale, noise: Some(k) } => {
                let (mut tx, mut ty) = (b.pool.take(), b.pool.take());
                self.evb(sx, b, act, &mut tx);
                self.evb(sy, b, act, &mut ty);
                self.evb(sz, b, act, out);
                let noise = &self.noises[k as usize];
                let mut idx = [0u8; L];
                let n = gather(act, &mut idx);
                let f0 = idx[0] as usize;
                let d0 = xs[f0] as f64 * xz_scale + tx[f0];
                let z0 = zs[f0] as f64 * xz_scale + out[f0];
                // Column path only if every lane's x and z inputs are bit-identical.
                let uniform = b.uniform_xz
                    && idx[..n].iter().all(|&q| {
                        let q = q as usize;
                        (xs[q] as f64 * xz_scale + tx[q]).to_bits() == d0.to_bits()
                            && (zs[q] as f64 * xz_scale + out[q]).to_bits() == z0.to_bits()
                    });
                if uniform {
                    let (mut yin, mut res) = ([0.0f64; L], [0.0f64; L]);
                    for q in 0..n {
                        let l = idx[q] as usize;
                        yin[q] = ys[l] as f64 * y_scale + ty[l];
                    }
                    noise.get_value_column(d0, z0, &yin[..n], &mut res);
                    for q in 0..n {
                        out[idx[q] as usize] = res[q];
                    }
                } else {
                    act.each(|i| {
                        let d = xs[i] as f64 * xz_scale + tx[i];
                        let e = ys[i] as f64 * y_scale + ty[i];
                        let f = zs[i] as f64 * xz_scale + out[i];
                        out[i] = noise.get_value(d, e, f);
                    });
                }
                b.pool.give(tx);
                b.pool.give(ty);
            }
            Op::RangeChoice { input, min_inclusive, max_exclusive, in_range, out_of_range } => {
                self.evb(input, b, act, out);
                let inside = |o: &[f64; L], i: usize| o[i] >= min_inclusive && o[i] < max_exclusive;
                match select(act, |i| inside(out, i)) {
                    Sel::All => self.evb(in_range, b, act, out),
                    Sel::Empty => self.evb(out_of_range, b, act, out),
                    Sel::Some(ins) => {
                        let mut outs = Lanes::new();
                        act.each(|i| if !inside(out, i) { outs.push(i) });
                        self.evb(in_range, b, ins.act(), out);
                        self.evb(out_of_range, b, outs.act(), out);
                    }
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
            Op::OldBlendedNoise(bi) if b.uniform_xz => {
                let mut idx = [0u8; L];
                let n = gather(act, &mut idx);
                let (mut yin, mut res) = ([0i32; L], [0.0f64; L]);
                for q in 0..n {
                    yin[q] = ys[idx[q] as usize];
                }
                let f = idx[0] as usize;
                self.blended[bi as usize].compute_column(xs[f], zs[f], &yin[..n], &mut res);
                for q in 0..n {
                    out[idx[q] as usize] = res[q];
                }
            }
            Op::BlendAlpha => act.each(|i| out[i] = 1.0),
            Op::BlendOffset => act.each(|i| out[i] = 0.0),
            Op::BlendDensity(i) => self.evb(i, b, act, out),
            // Splines, shift noises, old blended noise, ...: scalar per lane.
            _ => act.each(|i| out[i] = self.scalar_lane(n, b, i)),
        }
    }
}
