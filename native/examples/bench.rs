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
    let chunks = 16;

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

    for mode in ["density only", "density + veins", "full states"] {
        if mode == "full states" && material.is_none() {
            continue;
        }
        for warm in [true, false] {
            let t = Instant::now();
            for i in 0..chunks {
                let extra = if mode == "density + veins" {
                    vec![(veins[0], a.as_mut_slice()), (veins[1], b.as_mut_slice()), (veins[2], c.as_mut_slice())]
                } else {
                    vec![]
                };
                let mat = (mode == "full states").then(|| Material { cfg: material.as_ref().unwrap(), codes: &mut codes });
                let out = ChunkOut { density: &mut d, extra, lazy_veins: mode == "density + veins", material: mat };
                program.fill_chunk(params(i + if warm { 100 } else { 0 }), final_root, None, out).unwrap();
            }
            if !warm {
                println!("{mode:>16}: {:.2} ms/chunk", t.elapsed().as_secs_f64() * 1e3 / chunks as f64);
            }
        }
    }
}
