//! JNI entry points for rustgen. Java side: src/main/java/rustgen/NativeNoise.java
//!
//! Every entry point catches panics and rethrows them as Java RuntimeExceptions,
//! so a bug here surfaces as a normal Java stack trace instead of a JVM crash.

pub mod noise;
pub mod random;

use std::panic::{catch_unwind, AssertUnwindSafe};

use jni::objects::{JClass, JDoubleArray, JIntArray, JLongArray};
use jni::sys::{jdouble, jint, jlong};
use jni::JNIEnv;

use noise::ImprovedNoise;
use random::Xoroshiro;

/// Must match NativeLib.EXPECTED_ABI. Bump on any JNI signature change.
pub const ABI_VERSION: jint = 1;

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
