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

use jni::objects::{JByteArray, JClass, JDoubleArray, JIntArray, JLongArray, JString};
use jni::sys::{jboolean, jdouble, jint, jlong};
use jni::JNIEnv;

use blended::{BlendedNoise, SamplingSettings};
use noise::ImprovedNoise;
use perlin::{NormalNoise, PerlinNoise};
use random::Xoroshiro;

/// Must match NativeLib.EXPECTED_ABI. Bump on any JNI signature change.
pub const ABI_VERSION: jint = 3;

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
