//! JNI entry points for rustgen. Java side: src/main/java/rustgen/NativeNoise.java
//!
//! Every entry point catches panics and rethrows them as Java RuntimeExceptions,
//! so a bug here surfaces as a normal Java stack trace instead of a JVM crash.

pub mod blended;
pub mod density;
pub mod noise;
pub mod perlin;
pub mod random;

use std::panic::{catch_unwind, AssertUnwindSafe};

use jni::objects::{JByteArray, JClass, JDoubleArray, JIntArray, JLongArray, JObjectArray, JString};
use jni::sys::jstring;
use jni::sys::{jboolean, jdouble, jint, jlong};
use jni::JNIEnv;

use blended::{BlendedNoise, SamplingSettings};
use noise::ImprovedNoise;
use perlin::{NormalNoise, PerlinNoise};
use random::Xoroshiro;

/// Must match NativeLib.EXPECTED_ABI. Bump on any JNI signature change.
pub const ABI_VERSION: jint = 6;

/// Runs `f`, converting both JNI errors and panics into Java exceptions.
fn guard<R: Default>(env: &mut JNIEnv, f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<R>) -> R {
    let result = catch_unwind(AssertUnwindSafe(|| f(env)));
    let msg = match result {
        Ok(Ok(v)) => return v,
        // A pending Java exception (e.g. from an array access) is already set.
        Ok(Err(jni::errors::Error::JavaException)) => return R::default(),
        Ok(Err(e)) => format!("rustgen native error: {e}"),
        Err(p) => {
            let detail = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            format!("rustgen native panic: {detail}")
        }
    };
    let _ = env.throw_new("java/lang/RuntimeException", msg);
    R::default()
}

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_abiVersion(_env: JNIEnv, _cls: JClass) -> jint {
    ABI_VERSION
}

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_xoroshiroNextLong(
    mut env: JNIEnv, _cls: JClass, seed: jlong, out: JLongArray,
) {
    guard(&mut env, |env| {
        let n = env.get_array_length(&out)? as usize;
        let mut rng = Xoroshiro::from_seed(seed);
        let buf: Vec<i64> = (0..n).map(|_| rng.next_long()).collect();
        env.set_long_array_region(&out, 0, &buf)
    })
}

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_xoroshiroNextInt(
    mut env: JNIEnv, _cls: JClass, seed: jlong, bound: jint, out: JIntArray,
) {
    guard(&mut env, |env| {
        let n = env.get_array_length(&out)? as usize;
        let mut rng = Xoroshiro::from_seed(seed);
        let buf: Vec<i32> = (0..n).map(|_| rng.next_int_bounded(bound)).collect();
        env.set_int_array_region(&out, 0, &buf)
    })
}

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_xoroshiroNextDouble(
    mut env: JNIEnv, _cls: JClass, seed: jlong, out: JDoubleArray,
) {
    guard(&mut env, |env| {
        let n = env.get_array_length(&out)? as usize;
        let mut rng = Xoroshiro::from_seed(seed);
        let buf: Vec<f64> = (0..n).map(|_| rng.next_double()).collect();
        env.set_double_array_region(&out, 0, &buf)
    })
}

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_improvedNoise(
    mut env: JNIEnv, _cls: JClass, seed: jlong, y_scale: jdouble, y_max: jdouble,
    points: JDoubleArray, out: JDoubleArray,
) {
    guard(&mut env, |env| {
        let n = env.get_array_length(&out)? as usize;
        let np = env.get_array_length(&points)? as usize;
        if np != n * 3 {
            panic!("points.length ({np}) must be 3 * out.length ({n})");
        }
        let mut pts = vec![0.0f64; np];
        env.get_double_array_region(&points, 0, &mut pts)?;

        let mut rng = Xoroshiro::from_seed(seed);
        let noise = ImprovedNoise::new(&mut rng);
        let buf: Vec<f64> = pts
            .chunks_exact(3)
            .map(|c| noise.noise(c[0], c[1], c[2], y_scale, y_max))
            .collect();
        env.set_double_array_region(&out, 0, &buf)
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `name == null` → XoroshiroRandomSource(seed)
/// `name != null` → XoroshiroRandomSource(seed).forkPositional().fromHashOf(name)
/// The second form is exactly how the game seeds each named noise from the world seed.
fn seeded(env: &mut JNIEnv, seed: jlong, name: &JString) -> jni::errors::Result<Xoroshiro> {
    if name.is_null() {
        Ok(Xoroshiro::from_seed(seed))
    } else {
        let s: String = env.get_string(name)?.into();
        Ok(Xoroshiro::from_seed(seed).fork_positional().from_hash_of(&s))
    }
}

fn read_doubles(env: &mut JNIEnv, arr: &JDoubleArray) -> jni::errors::Result<Vec<f64>> {
    let mut v = vec![0.0; env.get_array_length(arr)? as usize];
    env.get_double_array_region(arr, 0, &mut v)?;
    Ok(v)
}

fn read_ints(env: &mut JNIEnv, arr: &JIntArray) -> jni::errors::Result<Vec<i32>> {
    let mut v = vec![0; env.get_array_length(arr)? as usize];
    env.get_int_array_region(arr, 0, &mut v)?;
    Ok(v)
}

/// Evaluates `f` on packed xyz triples and writes results to `out`.
fn eval_points<T: Copy>(
    env: &mut JNIEnv,
    pts: &[T],
    out: &JDoubleArray,
    f: impl Fn(T, T, T) -> f64,
) -> jni::errors::Result<()> {
    let n = env.get_array_length(out)? as usize;
    assert_eq!(pts.len(), n * 3, "points.length must be 3 * out.length");
    let buf: Vec<f64> = pts.chunks_exact(3).map(|c| f(c[0], c[1], c[2])).collect();
    env.set_double_array_region(out, 0, &buf)
}

// ---------------------------------------------------------------------------
// Positional random
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_positionalFromHash(
    mut env: JNIEnv, _cls: JClass, seed: jlong, name: JString, out: JLongArray,
) {
    guard(&mut env, |env| {
        let mut rng = seeded(env, seed, &name)?;
        let n = env.get_array_length(&out)? as usize;
        let buf: Vec<i64> = (0..n).map(|_| rng.next_long()).collect();
        env.set_long_array_region(&out, 0, &buf)
    })
}

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_positionalAt(
    mut env: JNIEnv, _cls: JClass, seed: jlong, x: jint, y: jint, z: jint, out: JLongArray,
) {
    guard(&mut env, |env| {
        let mut rng = Xoroshiro::from_seed(seed).fork_positional().at(x, y, z);
        let n = env.get_array_length(&out)? as usize;
        let buf: Vec<i64> = (0..n).map(|_| rng.next_long()).collect();
        env.set_long_array_region(&out, 0, &buf)
    })
}

// ---------------------------------------------------------------------------
// PerlinNoise / NormalNoise / BlendedNoise
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_perlinNoise(
    mut env: JNIEnv, _cls: JClass, seed: jlong, first_octave: jint, amplitudes: JDoubleArray,
    legacy: jboolean, points: JDoubleArray, out: JDoubleArray,
) {
    guard(&mut env, |env| {
        let amps = read_doubles(env, &amplitudes)?;
        let pts = read_doubles(env, &points)?;
        let mut rng = Xoroshiro::from_seed(seed);
        let noise = if legacy != 0 {
            PerlinNoise::new_legacy(&mut rng, first_octave, amps)
        } else {
            PerlinNoise::new(&mut rng, first_octave, amps)
        };
        eval_points(env, &pts, &out, |x, y, z| noise.get_value(x, y, z))
    })
}

/// Returns maxValue().
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_normalNoise(
    mut env: JNIEnv, _cls: JClass, seed: jlong, name: JString, first_octave: jint,
    amplitudes: JDoubleArray, legacy: jboolean, points: JDoubleArray, out: JDoubleArray,
) -> jdouble {
    guard(&mut env, |env| {
        let amps = read_doubles(env, &amplitudes)?;
        let pts = read_doubles(env, &points)?;
        let mut rng = seeded(env, seed, &name)?;
        let noise = if legacy != 0 {
            NormalNoise::new_legacy(&mut rng, first_octave, amps)
        } else {
            NormalNoise::new(&mut rng, first_octave, amps)
        };
        eval_points(env, &pts, &out, |x, y, z| noise.get_value(x, y, z))?;
        Ok(noise.max_value())
    })
}

/// Returns maxValue().
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeNoise_blendedNoise(
    mut env: JNIEnv, _cls: JClass, seed: jlong, name: JString,
    xz_scale: jdouble, y_scale: jdouble, xz_factor: jdouble, y_factor: jdouble,
    cell_width: jint, cell_height: jint, blocks: JIntArray, out: JDoubleArray,
) -> jdouble {
    guard(&mut env, |env| {
        let pts = read_ints(env, &blocks)?;
        let mut rng = seeded(env, seed, &name)?;
        let settings = SamplingSettings { xz_scale, y_scale, xz_factor, y_factor };
        let noise = BlendedNoise::new(&mut rng, settings, cell_width, cell_height);
        eval_points(env, &pts, &out, |x, y, z| noise.compute(x, y, z))?;
        Ok(noise.max_value())
    })
}

// ---------------------------------------------------------------------------
// Density programs (see density.rs). Java side: rustgen.NativeDensity
// ---------------------------------------------------------------------------

/// Parses a program and returns an owning handle (never 0 on success).
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_create(mut env: JNIEnv, _cls: JClass, bytes: JByteArray) -> jlong {
    guard(&mut env, |env| {
        let buf = env.convert_byte_array(&bytes)?;
        match density::parse(&buf) {
            Ok(p) => Ok(Box::into_raw(Box::new(p)) as jlong),
            Err(msg) => {
                env.throw_new("java/lang/IllegalArgumentException", format!("rustgen: invalid density program: {msg}"))?;
                Ok(0)
            }
        }
    })
}

/// Frees a handle from `create`. Java guarantees it is called at most once.
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_free(_env: JNIEnv, _cls: JClass, handle: jlong) {
    if handle != 0 {
        // SAFETY: handle came from Box::into_raw in `create` and is freed exactly once.
        drop(unsafe { Box::from_raw(handle as *mut density::Program) });
    }
}

/// Evaluates root `root` at each packed block position. Thread-safe: programs are immutable.
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_evalPoints(
    mut env: JNIEnv, _cls: JClass, handle: jlong, root: jint, blocks: JIntArray, out: JDoubleArray,
) {
    guard(&mut env, |env| {
        assert!(handle != 0, "null program handle");
        // SAFETY: Java keeps the handle alive (not freed) for the duration of this call.
        let program = unsafe { &*(handle as *const density::Program) };
        let node = program
            .root(root as usize)
            .unwrap_or_else(|| panic!("root {root} is unsupported or out of range (count {})", program.root_count()));
        let pts = read_ints(env, &blocks)?;
        let n = env.get_array_length(&out)? as usize;
        assert_eq!(pts.len(), n * 3, "blocks.length must be 3 * out.length");
        let buf: Vec<f64> = pts.chunks_exact(3).map(|c| program.eval(node, c[0], c[1], c[2])).collect();
        env.set_double_array_region(&out, 0, &buf)
    })
}

/// Returns null if `root` can be chunk-filled, otherwise the reason it can't.
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_checkChunk(mut env: JNIEnv, _cls: JClass, handle: jlong, root: jint) -> jstring {
    guard(&mut env, |env| {
        assert!(handle != 0, "null program handle");
        // SAFETY: see evalPoints.
        let program = unsafe { &*(handle as *const density::Program) };
        let result = match program.root(root as usize) {
            None => Err(format!("root {root} is unsupported or out of range")),
            Some(n) => program.check_chunk(n),
        };
        match result {
            Ok(()) => Ok(std::ptr::null_mut()),
            Err(msg) => Ok(env.new_string(msg)?.into_raw()),
        }
    })
}

/// See density::Program::fill_chunk. `extraRoots[i]` is written into `extraOut[i]`
/// (an Object[] of double[]). `beardifier` may be null.
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_fillChunk(
    mut env: JNIEnv, _cls: JClass, handle: jlong, final_root: jint, extra_roots: JIntArray,
    params: JIntArray, beardifier: JDoubleArray, density_out: JDoubleArray, extra_out: JObjectArray, lazy_veins: jboolean,
) {
    guard(&mut env, |env| {
        assert!(handle != 0, "null program handle");
        // SAFETY: see evalPoints.
        let program = unsafe { &*(handle as *const density::Program) };
        let root_node = |r: i32| program.root(r as usize).unwrap_or_else(|| panic!("root {r} is unsupported or out of range"));

        let pr = read_ints(env, &params)?;
        assert_eq!(pr.len(), 7, "params must be [minX, minZ, cellNoiseMinY, cellCountY, cellCountXZ, cellWidth, cellHeight]");
        let p = density::ChunkParams {
            min_block_x: pr[0], min_block_z: pr[1], cell_noise_min_y: pr[2], cell_count_y: pr[3],
            cell_count_xz: pr[4], cell_width: pr[5], cell_height: pr[6],
        };
        let vol = p.volume();

        let beard = if beardifier.is_null() { None } else { Some(read_doubles(env, &beardifier)?) };
        let roots = read_ints(env, &extra_roots)?;
        let n_extra = env.get_array_length(&extra_out)? as usize;
        assert_eq!(roots.len(), n_extra, "extraRoots and extraOut lengths differ");

        let mut density = vec![0.0; vol];
        let mut extra_bufs: Vec<Vec<f64>> = (0..n_extra).map(|_| vec![0.0; vol]).collect();
        let extra = roots.iter().map(|&r| root_node(r)).zip(extra_bufs.iter_mut().map(|v| v.as_mut_slice())).collect();
        program
            .fill_chunk(p, root_node(final_root), beard.as_deref(), density::ChunkOut { density: &mut density, extra, lazy_veins: lazy_veins != 0, material: None })
            .unwrap_or_else(|e| panic!("{e}"));

        env.set_double_array_region(&density_out, 0, &density)?;
        for (i, buf) in extra_bufs.iter().enumerate() {
            let arr: JDoubleArray = env.get_object_array_element(&extra_out, i as i32)?.into();
            env.set_double_array_region(&arr, 0, buf)?;
        }
        Ok(())
    })
}

/// Returns null if `root` may be used as the preliminary-surface root, else the reason.
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_checkSurfaceRoot(mut env: JNIEnv, _cls: JClass, handle: jlong, root: jint) -> jstring {
    guard(&mut env, |env| {
        assert!(handle != 0, "null program handle");
        // SAFETY: see evalPoints.
        let program = unsafe { &*(handle as *const density::Program) };
        let result = match program.root(root as usize) {
            None => Err(format!("root {root} is unsupported or out of range")),
            Some(n) => program.check_surface_root(n),
        };
        match result {
            Ok(()) => Ok(std::ptr::null_mut()),
            Err(msg) => Ok(env.new_string(msg)?.into_raw()),
        }
    })
}

/// Full noise stage for one chunk: density, aquifer and ore veins, producing one
/// `density::aquifer::code` byte per block. Layouts are documented in NativeDensity.fillChunkStates.
#[no_mangle]
pub extern "system" fn Java_rustgen_NativeDensity_fillChunkStates(
    mut env: JNIEnv, _cls: JClass, handle: jlong, final_root: jint, params: JIntArray, beardifier: JDoubleArray,
    mat_ints: JIntArray, mat_longs: JLongArray, mat_doubles: JDoubleArray, codes_out: JByteArray, density_out: JDoubleArray,
) {
    use density::aquifer::{Fluid, MaterialConfig, VeinConfig};
    guard(&mut env, |env| {
        assert!(handle != 0, "null program handle");
        // SAFETY: see evalPoints.
        let program = unsafe { &*(handle as *const density::Program) };
        let root_node = |r: i32| program.root(r as usize).unwrap_or_else(|| panic!("root {r} is unsupported or out of range"));

        let pr = read_ints(env, &params)?;
        assert_eq!(pr.len(), 7, "params must be [minX, minZ, cellNoiseMinY, cellCountY, cellCountXZ, cellWidth, cellHeight]");
        let p = density::ChunkParams {
            min_block_x: pr[0], min_block_z: pr[1], cell_noise_min_y: pr[2], cell_count_y: pr[3],
            cell_count_xz: pr[4], cell_width: pr[5], cell_height: pr[6],
        };
        let mi = read_ints(env, &mat_ints)?;
        assert_eq!(mi.len(), 20, "material ints must have 20 entries");
        let mut ml = [0i64; 4];
        assert_eq!(env.get_array_length(&mat_longs)?, 4, "material longs must have 4 entries");
        env.get_long_array_region(&mat_longs, 0, &mut ml)?;
        let md = read_doubles(env, &mat_doubles)?;
        assert_eq!(md.len(), 2, "material doubles must have 2 entries");

        let cfg = MaterialConfig {
            aquifers_enabled: mi[0] != 0,
            sea_level: mi[1],
            default_fluid: match mi[2] {
                1 => Fluid::Air,
                2 => Fluid::Water,
                3 => Fluid::Lava,
                f => panic!("invalid default fluid {f}"),
            },
            way_below_min_y: mi[3],
            aquifer_random: random::XoroshiroPositional { seed_lo: ml[0], seed_hi: ml[1] },
            barrier: root_node(mi[4]),
            floodedness: root_node(mi[5]),
            spread: root_node(mi[6]),
            lava: root_node(mi[7]),
            initial_density: root_node(mi[8]),
            surface: density::SlideSettings::new(mi[9], mi[10], mi[11], (md[0], mi[12], mi[13]), (md[1], mi[14], mi[15])),
            veins: (mi[16] != 0).then(|| VeinConfig {
                toggle: root_node(mi[17]),
                ridged: root_node(mi[18]),
                gap: root_node(mi[19]),
                random: random::XoroshiroPositional { seed_lo: ml[2], seed_hi: ml[3] },
            }),
        };

        let vol = p.volume();
        let beard = if beardifier.is_null() { None } else { Some(read_doubles(env, &beardifier)?) };
        let mut density = vec![0.0; vol];
        let mut codes = vec![0u8; vol];
        program
            .fill_chunk(
                p,
                root_node(final_root),
                beard.as_deref(),
                density::ChunkOut {
                    density: &mut density,
                    extra: vec![],
                    lazy_veins: false,
                    material: Some(density::Material { cfg: &cfg, codes: &mut codes }),
                },
            )
            .unwrap_or_else(|e| panic!("{e}"));

        // SAFETY: u8 and i8 have identical size and alignment.
        let signed: &[i8] = unsafe { std::slice::from_raw_parts(codes.as_ptr() as *const i8, codes.len()) };
        env.set_byte_array_region(&codes_out, 0, signed)?;
        if !density_out.is_null() {
            env.set_double_array_region(&density_out, 0, &density)?;
        }
        Ok(())
    })
}
