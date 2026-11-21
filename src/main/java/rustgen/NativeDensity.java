package rustgen;

import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * Owning handle to a native density program (native/src/density.rs).
 *
 * Evaluation is thread-safe and may run concurrently; {@link #close()} waits for
 * in-flight evaluations, after which further calls throw instead of touching freed memory.
 */
public final class NativeDensity implements AutoCloseable {
	private final ReentrantReadWriteLock lock = new ReentrantReadWriteLock();
	private long handle;

	public NativeDensity(byte[] program) {
		if (!NativeLib.isAvailable()) throw new IllegalStateException("rustgen native library not available");
		this.handle = create(program);
	}

	/** Evaluates program root {@code root} at packed block positions (x0,y0,z0, x1,...). */
	public void evalPoints(int root, int[] blocks, double[] out) {
		lock.readLock().lock();
		try {
			if (handle == 0) throw new IllegalStateException("NativeDensity already closed");
			evalPoints(handle, root, blocks, out);
		} finally {
			lock.readLock().unlock();
		}
	}

	/** @return null if {@code root} can be chunk-filled natively, else the reason it can't */
	public String checkChunk(int root) {
		lock.readLock().lock();
		try {
			if (handle == 0) throw new IllegalStateException("NativeDensity already closed");
			return checkChunk(handle, root);
		} finally {
			lock.readLock().unlock();
		}
	}

	/**
	 * Native equivalent of NoiseChunk sampling during doFill (Blender.empty()).
	 * Arrays are indexed {@code ((y - minY) * width + (z - minZ)) * width + (x - minX)}.
	 *
	 * @param params     {minBlockX, minBlockZ, cellNoiseMinY, cellCountY, cellCountXZ, cellWidth, cellHeight}
	 * @param beardifier per-block beardifier values, or null for none
	 * @param densityOut cacheAllInCell(add(finalDensity, beardifier)) per block
	 * @param extraRoots roots evaluated per block like OreVeinifier does
	 * @param extraOut   one array per extra root
	 * @param lazyVeins  extraRoots are exactly {vein_toggle, vein_ridged, vein_gap}; only values
	 *                   OreVeinifier can read are computed, the rest are NaN
	 */
	public void fillChunk(int finalRoot, int[] params, double[] beardifier, double[] densityOut, int[] extraRoots, double[][] extraOut,
			boolean lazyVeins) {
		lock.readLock().lock();
		try {
			if (handle == 0) throw new IllegalStateException("NativeDensity already closed");
			fillChunk(handle, finalRoot, extraRoots, params, beardifier, densityOut, extraOut, lazyVeins);
		} finally {
			lock.readLock().unlock();
		}
	}

	/** @return null if {@code root} may be scanned for the preliminary surface level, else why not */
	public String checkSurfaceRoot(int root) {
		lock.readLock().lock();
		try {
			if (handle == 0) throw new IllegalStateException("NativeDensity already closed");
			return checkSurfaceRoot(handle, root);
		} finally {
			lock.readLock().unlock();
		}
	}

	/**
	 * Whole noise stage for one chunk: density, aquifer and ore veins. Writes one code per
	 * block (see native aquifer::code; bit 7 = schedule fluid update), indexed like
	 * {@link #fillChunk}. Config layouts are built by {@link NativeRouter}.
	 *
	 * @param densityOut optional (may be null), for tests
	 */
	public void fillChunkStates(int finalRoot, int[] params, double[] beardifier, int[] matInts, long[] matLongs, double[] matDoubles,
			byte[] codesOut, double[] densityOut) {
		lock.readLock().lock();
		try {
			if (handle == 0) throw new IllegalStateException("NativeDensity already closed");
			fillChunkStates(handle, finalRoot, params, beardifier, matInts, matLongs, matDoubles, codesOut, densityOut);
		} finally {
			lock.readLock().unlock();
		}
	}

	@Override
	public void close() {
		lock.writeLock().lock();
		try {
			if (handle != 0) {
				free(handle);
				handle = 0;
			}
		} finally {
			lock.writeLock().unlock();
		}
	}

	private static native long create(byte[] program);

	private static native void free(long handle);

	private static native void evalPoints(long handle, int root, int[] blocks, double[] out);

	private static native String checkChunk(long handle, int root);

	private static native String checkSurfaceRoot(long handle, int root);

	private static native void fillChunkStates(long handle, int finalRoot, int[] params, double[] beardifier,
			int[] matInts, long[] matLongs, double[] matDoubles, byte[] codesOut, double[] densityOut);

	private static native void fillChunk(long handle, int finalRoot, int[] extraRoots, int[] params,
			double[] beardifier, double[] densityOut, Object[] extraOut, boolean lazyVeins);
}
