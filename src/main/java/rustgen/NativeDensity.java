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
}
