//! Standalone chunk-fill benchmark (no JVM). Program file is written by
//! ChunkParityTest.chunkThroughput to build/overworld-12345.rgdf.
//!
//!   cargo run --release --example bench -- ../build/overworld-12345.rgdf
use rustgen::density::{parse, ChunkOut, ChunkParams};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench <program.rgdf>");
    let program = parse(&std::fs::read(path).unwrap()).unwrap();
    let (final_root, veins) = (program.root(11).unwrap(), [12, 13, 14].map(|r| program.root(r).unwrap()));
    let params = |cx: i32| ChunkParams {
        min_block_x: cx * 16, min_block_z: -7 * 16, cell_noise_min_y: -8, cell_count_y: 48,
        cell_count_xz: 4, cell_width: 4, cell_height: 8,
    };
    let vol = params(0).volume();
    let (mut d, mut a, mut b, mut c) = (vec![0.0; vol], vec![0.0; vol], vec![0.0; vol], vec![0.0; vol]);
    let chunks = 32;

    for (label, with_veins) in [("density only", false), ("density + veins", true)] {
        for warm in [true, false] {
            let t = Instant::now();
            for i in 0..chunks {
                let extra = if with_veins {
                    vec![(veins[0], a.as_mut_slice()), (veins[1], b.as_mut_slice()), (veins[2], c.as_mut_slice())]
                } else {
                    vec![]
                };
                program.fill_chunk(params(i), final_root, None, ChunkOut { density: &mut d, extra, lazy_veins: with_veins, material: None }).unwrap();
            }
            if !warm {
                println!("{label:>16}: {:.2} ms/chunk", t.elapsed().as_secs_f64() * 1e3 / chunks as f64);
            }
        }
    }
}
