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

	/** XoroshiroRandomSource(seed).forkPositional().fromHashOf(name), then nextLong() into {@code out}. */
	public static native void positionalFromHash(long seed, String name, long[] out);

	/** XoroshiroRandomSource(seed).forkPositional().at(x, y, z), then nextLong() into {@code out}. */
	public static native void positionalAt(long seed, int x, int y, int z, long[] out);

	/**
	 * PerlinNoise over XoroshiroRandomSource(seed).
	 * legacy=false: PerlinNoise.create(...); legacy=true: createLegacyForLegacyNetherBiome(...).
	 */
	public static native void perlinNoise(long seed, int firstOctave, double[] amplitudes, boolean legacy, double[] points, double[] out);

	/**
	 * NormalNoise.create (or createLegacyNetherBiome if legacy) over a random source that is
	 * XoroshiroRandomSource(seed) if {@code name == null}, else
	 * XoroshiroRandomSource(seed).forkPositional().fromHashOf(name) — the game's seeding path.
	 *
	 * @return maxValue()
	 */
	public static native double normalNoise(long seed, String name, int firstOctave, double[] amplitudes, boolean legacy, double[] points, double[] out);

	/**
	 * new BlendedNoise(random, new NoiseSamplingSettings(...), cellWidth, cellHeight).compute(...)
	 * with the same random-source rule as {@link #normalNoise}.
	 *
	 * @param blocks packed block coords x0,y0,z0, x1,y1,z1, ...
	 * @return maxValue()
	 */
	public static native double blendedNoise(long seed, String name, double xzScale, double yScale, double xzFactor, double yFactor,
			int cellWidth, int cellHeight, int[] blocks, double[] out);
}
