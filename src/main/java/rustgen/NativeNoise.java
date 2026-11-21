package rustgen;

/**
 * JNI bindings to native/src/lib.rs.
 *
 * Phase 1 functions are stateless batch calls used for parity testing against
 * vanilla. Only call these when {@link NativeLib#isAvailable()} is true.
 */
public final class NativeNoise {
	private NativeNoise() {}

	public static native int abiVersion();

	/** Fills {@code out} with successive nextLong() of XoroshiroRandomSource(seed). */
	public static native void xoroshiroNextLong(long seed, long[] out);

	/** Fills {@code out} with successive nextInt(bound) of XoroshiroRandomSource(seed). */
	public static native void xoroshiroNextInt(long seed, int bound, int[] out);

	/** Fills {@code out} with successive nextDouble() of XoroshiroRandomSource(seed). */
	public static native void xoroshiroNextDouble(long seed, double[] out);

	/**
	 * Builds ImprovedNoise(new XoroshiroRandomSource(seed)) and evaluates
	 * noise(x, y, z, yScale, yMax) for each point.
	 *
	 * @param points packed as x0,y0,z0, x1,y1,z1, ... (length = 3 * out.length)
	 */
	public static native void improvedNoise(long seed, double yScale, double yMax, double[] points, double[] out);
}
