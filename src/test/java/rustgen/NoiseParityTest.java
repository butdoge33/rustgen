package rustgen;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.SplittableRandom;

import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.ImprovedNoise;

import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Bit-exact comparison of the Rust port against vanilla 1.18.2 classes.
 * Any mismatch here means worlds would differ from vanilla — never "close enough".
 */
class NoiseParityTest {
	private static final long[] SEEDS = buildSeeds();

	private static long[] buildSeeds() {
		long[] fixed = {0L, 1L, -1L, 42L, Long.MIN_VALUE, Long.MAX_VALUE,
				-4172144997902289642L /* a classic speedrun seed */, 0x9E3779B97F4A7C15L};
		SplittableRandom r = new SplittableRandom(1234);
		long[] out = new long[fixed.length + 200];
		System.arraycopy(fixed, 0, out, 0, fixed.length);
		for (int i = fixed.length; i < out.length; i++) out[i] = r.nextLong();
		return out;
	}

	@BeforeAll
	static void requireNative() {
		assertTrue(NativeLib.isAvailable(), "native library failed to load for " + NativeLib.platform());
	}

	@Test
	void xoroshiroNextLong() {
		for (long seed : SEEDS) {
			XoroshiroRandomSource v = new XoroshiroRandomSource(seed);
			long[] n = new long[1000];
			NativeNoise.xoroshiroNextLong(seed, n);
			for (int i = 0; i < n.length; i++) {
				assertEquals(v.nextLong(), n[i], "seed=" + seed + " i=" + i);
			}
		}
	}

	@Test
	void xoroshiroNextIntBounded() {
		// Includes bounds that exercise the rejection loop (large, non-power-of-two).
		int[] bounds = {1, 2, 3, 7, 16, 255, 256, 1000, 65537, 1 << 30, (1 << 30) + 1, Integer.MAX_VALUE};
		for (long seed : SEEDS) {
			for (int bound : bounds) {
				XoroshiroRandomSource v = new XoroshiroRandomSource(seed);
				int[] n = new int[500];
				NativeNoise.xoroshiroNextInt(seed, bound, n);
				for (int i = 0; i < n.length; i++) {
					assertEquals(v.nextInt(bound), n[i], "seed=" + seed + " bound=" + bound + " i=" + i);
				}
			}
		}
	}

	@Test
	void xoroshiroNextDouble() {
		for (long seed : SEEDS) {
			XoroshiroRandomSource v = new XoroshiroRandomSource(seed);
			double[] n = new double[1000];
			NativeNoise.xoroshiroNextDouble(seed, n);
			for (int i = 0; i < n.length; i++) {
				assertBitEqual(v.nextDouble(), n[i], "seed=" + seed + " i=" + i);
			}
		}
	}

	@Test
	@SuppressWarnings("deprecation")
	void improvedNoise() {
		// (yScale, yMax) pairs: plain 3D noise, plus the "y-snapping" variant used by
		// legacy terrain/blended noise, including yMax edge cases.
		double[][] yParams = {{0, 0}, {0.5, 0.5}, {0.25, 1.0}, {1.0 / 3.0, -1.0}, {0.125, 0.0625}};
		SplittableRandom r = new SplittableRandom(99);
		int count = 4000;
		double[] pts = new double[count * 3];

		for (long seed : SEEDS) {
			ImprovedNoise v = new ImprovedNoise(new XoroshiroRandomSource(seed));
			for (int i = 0; i < pts.length; i++) {
				// Mix of small, world-scale and extreme coordinates, plus exact integers.
				pts[i] = switch (i % 4) {
					case 0 -> r.nextDouble(-1.0, 1.0);
					case 1 -> r.nextDouble(-30_000_000.0, 30_000_000.0);
					case 2 -> (double) r.nextInt(-100_000, 100_000);
					default -> r.nextDouble(-1e9, 1e9);
				};
			}
			for (double[] yp : yParams) {
				double[] n = new double[count];
				NativeNoise.improvedNoise(seed, yp[0], yp[1], pts, n);
				for (int i = 0; i < count; i++) {
					double x = pts[i * 3], y = pts[i * 3 + 1], z = pts[i * 3 + 2];
					assertBitEqual(v.noise(x, y, z, yp[0], yp[1]), n[i],
							"seed=" + seed + " yScale=" + yp[0] + " yMax=" + yp[1] + " p=(" + x + "," + y + "," + z + ")");
				}
			}
		}
	}

	private static void assertBitEqual(double expected, double actual, String msg) {
		assertEquals(Double.doubleToRawLongBits(expected), Double.doubleToRawLongBits(actual),
				() -> msg + " expected=" + expected + " actual=" + actual);
	}
}
