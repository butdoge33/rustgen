//! Standalone chunk-fill benchmark (no JVM). Inputs are written by
//! ChunkParityTest.chunkThroughput: build/overworld-12345.rgdf (program) and
//! build/overworld-12345.mat (material config, optional).
//!
//!   cargo run --release --example bench -- ../build/overworld-12345
use rustgen::density::aquifer::{Fluid, MaterialConfig, VeinConfig};
use rustgen::density::{parse, ChunkOut, ChunkParams, Material, SlideSettings};
use rustgen::random::XoroshiroPositional;
use std::time::Instant;

fn main() {
    let base = std::env::args().nth(1).expect("usage: bench <path without extension>");
    let program = parse(&std::fs::read(format!("{base}.rgdf")).unwrap()).unwrap();
    let root = |r: i32| program.root(r as usize).unwrap();
    let (final_root, veins) = (root(11), [12, 13, 14].map(root));
    let params = |cx: i32| ChunkParams {
        min_block_x: cx * 16, min_block_z: -7 * 16, cell_noise_min_y: -8, cell_count_y: 48,
        cell_count_xz: 4, cell_width: 4, cell_height: 8,
    };
    let vol = params(0).volume();
    let (mut d, mut a, mut b, mut c) = (vec![0.0; vol], vec![0.0; vol], vec![0.0; vol], vec![0.0; vol]);
    let mut codes = vec![0u8; vol];
    let chunks = 8;

    let material = std::fs::read_to_string(format!("{base}.mat")).ok().map(|text| {
        let lines: Vec<&str> = text.lines().collect();
        let mi: Vec<i32> = lines[0].split(' ').map(|v| v.parse().unwrap()).collect();
        let ml: Vec<i64> = lines[1].split(' ').map(|v| v.parse().unwrap()).collect();
        let md: Vec<f64> = lines[2].split(' ').map(|v| v.parse().unwrap()).collect();
        MaterialConfig {
            aquifers_enabled: mi[0] != 0,
            sea_level: mi[1],
            default_fluid: [Fluid::Air, Fluid::Water, Fluid::Lava][(mi[2] - 1) as usize],
            way_below_min_y: mi[3],
            aquifer_random: XoroshiroPositional { seed_lo: ml[0], seed_hi: ml[1] },
            barrier: root(mi[4]),
            floodedness: root(mi[5]),
            spread: root(mi[6]),
            lava: root(mi[7]),
            initial_density: root(mi[8]),
            surface: SlideSettings::new(mi[9], mi[10], mi[11], (md[0], mi[12], mi[13]), (md[1], mi[14], mi[15])),
            veins: (mi[16] != 0).then(|| VeinConfig {
                toggle: root(mi[17]),
                ridged: root(mi[18]),
                gap: root(mi[19]),
                random: XoroshiroPositional { seed_lo: ml[2], seed_hi: ml[3] },
            }),
        }
    });

    // RUSTGEN_PPROF=<mode>: sample that mode with pprof and print the hottest functions.
    if let Ok(target) = std::env::var("RUSTGEN_PPROF") {
        let mat_cfg = material.as_ref();
        let guard = pprof::ProfilerGuardBuilder::default().frequency(4000).build().unwrap();
        for round in 0..12 {
            for i in 0..chunks {
                let extra = if target == "veins" {
                    vec![(veins[0], a.as_mut_slice()), (veins[1], b.as_mut_slice()), (veins[2], c.as_mut_slice())]
                } else {
                    vec![]
                };
                let mat = (target == "full").then(|| Material { cfg: mat_cfg.unwrap(), codes: &mut codes });
                let out = ChunkOut { density: &mut d, extra, lazy_veins: target == "veins", material: mat };
                program.fill_chunk(params(i + round * chunks), final_root, None, out).unwrap();
            }
        }
        let report = guard.report().build().unwrap();
        // Self time per function (leaf frame of each sample).
        let mut leaf: std::collections::HashMap<String, isize> = std::collections::HashMap::new();
        let mut incl: std::collections::HashMap<String, isize> = std::collections::HashMap::new();
        let mut total = 0isize;
        for (frames, count) in report.data.iter() {
            total += count;
            let names: Vec<String> = frames.frames.iter().map(|f| f.first().map(|s| s.name()).unwrap_or_default()).collect();
            if let Some(n) = names.first() {
                *leaf.entry(n.clone()).or_default() += count;
            }
            let mut seen = std::collections::HashSet::new();
            for n in &names {
                if seen.insert(n.clone()) {
                    *incl.entry(n.clone()).or_default() += count;
                }
            }
        }
        let short = |n: &str| n.split("::").filter(|p| !p.starts_with('h') || p.len() != 17).collect::<Vec<_>>().join("::");
        let mut v: Vec<_> = leaf.into_iter().collect();
        v.sort_by_key(|e| -e.1);
        println!("pprof '{target}': {total} samples. Self time:");
        for (n, c) in v.iter().take(18) {
            println!("  {:5.1}%  {}", *c as f64 * 100.0 / total as f64, short(n));
        }
        let mut w: Vec<_> = incl.into_iter().filter(|(n, _)| n.contains("rustgen")).collect();
        w.sort_by_key(|e| -e.1);
        println!("Inclusive (rustgen functions):");
        for (n, c) in w.iter().take(14) {
            println!("  {:5.1}%  {}", *c as f64 * 100.0 / total as f64, short(n));
        }
        if let Ok(f) = std::fs::File::create("/tmp/rustgen-flame.svg") {
            let _ = report.flamegraph(f);
        }
        return;
    }

    // Several rounds per mode; report the fastest round (least disturbed by other load).
    let rounds = 6;
    for mode in ["density only", "density + veins", "full states"] {
        if mode == "full states" && material.is_none() {
            continue;
        }
        let mut best = f64::MAX;
        let mut best_phases = [0u64; 5];
        for round in 0..=rounds {
            let t = Instant::now();
            for i in 0..chunks {
                let extra = if mode == "density + veins" {
                    vec![(veins[0], a.as_mut_slice()), (veins[1], b.as_mut_slice()), (veins[2], c.as_mut_slice())]
                } else {
                    vec![]
                };
                let mat = (mode == "full states").then(|| Material { cfg: material.as_ref().unwrap(), codes: &mut codes });
                let out = ChunkOut { density: &mut d, extra, lazy_veins: mode == "density + veins", material: mat };
                program.fill_chunk(params(i), final_root, None, out).unwrap();
            }
            let ms = t.elapsed().as_secs_f64() * 1e3 / chunks as f64;
            let phases = rustgen::profile::take();
            if round > 0 && ms < best {
                // round 0 is warm-up
                best = ms;
                best_phases = phases;
            }
        }
        println!("{mode:>16}: {best:.2} ms/chunk (best of {rounds})");
        if cfg!(feature = "profile") {
            for (p, ns) in rustgen::profile::PHASES.iter().zip(best_phases) {
                if ns > 0 {
                    println!("{:>24}: {:.2} ms/chunk", format!("{p:?}"), ns as f64 / 1e6 / chunks as f64);
                }
            }
        }
    }
}
