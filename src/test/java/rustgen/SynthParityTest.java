package rustgen;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import it.unimi.dsi.fastutil.doubles.DoubleArrayList;
import java.util.SplittableRandom;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.NoiseSamplingSettings;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;
import net.minecraft.world.level.levelgen.RandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.BlendedNoise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Bit-exact parity for the noise building blocks: positional random,
 * PerlinNoise, NormalNoise, BlendedNoise. Compares against vanilla 1.18.2 classes.
 */
class SynthParityTest {
	/** Real NoiseParameters from 1.18.2 NoiseData, as (name, firstOctave, amplitudes...). */
	private static final Object[][] VANILLA_NOISES = {
		{"minecraft:temperature", -10, new double[]{1.5, 0.0, 1.0, 0.0, 0.0, 0.0}},
		{"minecraft:vegetation", -8, new double[]{1.0, 1.0, 0.0, 0.0, 0.0, 0.0}},
		{"minecraft:continentalness", -9, new double[]{1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0}},
		{"minecraft:erosion", -9, new double[]{1.0, 1.0, 0.0, 1.0, 1.0}},
		{"minecraft:ridge", -7, new double[]{1.0, 2.0, 1.0, 0.0, 0.0, 0.0}},
		{"minecraft:offset", -3, new double[]{1.0, 1.0, 1.0, 0.0}},
		{"minecraft:cave_entrance", -7, new double[]{0.4, 0.5, 1.0}},
		{"minecraft:cave_cheese", -8, new double[]{0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0}},
		{"minecraft:jagged", -16, new double[]{1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1}},
		{"minecraft:aquifer_lava", -1, new double[]{1.0}},
		{"minecraft:soul_sand_layer", -8, new double[]{1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.013333333333333334}},
		{"minecraft:netherrack", -3, new double[]{1.0, 0.0, 0.0, 0.35}},
	};

	private static final long[] SEEDS = seeds(40);

	private static long[] seeds(int extra) {
		long[] fixed = {0L, 1L, -1L, 42L, Long.MIN_VALUE, Long.MAX_VALUE, -4172144997902289642L};
		SplittableRandom r = new SplittableRandom(5678);
		long[] out = new long[fixed.length + extra];
		System.arraycopy(fixed, 0, out, 0, fixed.length);
		for (int i = fixed.length; i < out.length; i++) out[i] = r.nextLong();
		return out;
	}

	@BeforeAll
	static void requireNative() {
		TestSupport.bootstrap();
		assertTrue(NativeLib.isAvailable(), "native library failed to load for " + NativeLib.platform());
	}

	// ------------------------------------------------------------------ random

	@Test
	void positionalFromHash() {
		String[] names = {"minecraft:terrain", "minecraft:aquifer", "minecraft:ore", "octave_-7", "octave_0", "", "ünïcödé"};
		for (long seed : SEEDS) {
			for (String name : names) {
				RandomSource v = new XoroshiroRandomSource(seed).forkPositional().fromHashOf(name);
				long[] n = new long[64];
				NativeNoise.positionalFromHash(seed, name, n);
				for (int i = 0; i < n.length; i++) assertEquals(v.nextLong(), n[i], "seed=" + seed + " name=" + name + " i=" + i);
			}
		}
	}

	@Test
	void positionalAt() {
		SplittableRandom r = new SplittableRandom(7);
		for (long seed : SEEDS) {
			PositionalRandomFactory f = new XoroshiroRandomSource(seed).forkPositional();
			for (int t = 0; t < 50; t++) {
				int x = t < 3 ? new int[]{0, Integer.MIN_VALUE, Integer.MAX_VALUE}[t] : r.nextInt(-30_000_000, 30_000_000);
				int y = r.nextInt(-64, 320);
				int z = r.nextInt(-30_000_000, 30_000_000);
				RandomSource v = f.at(x, y, z);
				long[] n = new long[16];
				NativeNoise.positionalAt(seed, x, y, z, n);
				for (int i = 0; i < n.length; i++) assertEquals(v.nextLong(), n[i], "seed=" + seed + " at=" + x + "," + y + "," + z);
			}
		}
	}

	// ------------------------------------------------------------------ noise

	@Test
	void perlinNoiseModernAndLegacy() {
		double[] pts = points(1500, 11);
		for (long seed : SEEDS) {
			for (Object[] spec : VANILLA_NOISES) {
				int firstOctave = (int) spec[1];
				double[] amps = (double[]) spec[2];
				for (boolean legacy : new boolean[]{false, true}) {
					// Legacy mode forbids positive octaves, same as vanilla.
					if (legacy && firstOctave + amps.length - 1 > 0) continue;
					PerlinNoise v = legacy
							? PerlinNoise.createLegacyForLegacyNetherBiome(new XoroshiroRandomSource(seed), firstOctave, new DoubleArrayList(amps))
							: PerlinNoise.create(new XoroshiroRandomSource(seed), firstOctave, new DoubleArrayList(amps));
					double[] n = new double[pts.length / 3];
					NativeNoise.perlinNoise(seed, firstOctave, amps, legacy, pts, n);
					for (int i = 0; i < n.length; i++) {
						assertBitEqual(v.getValue(pts[i * 3], pts[i * 3 + 1], pts[i * 3 + 2]), n[i],
								"perlin " + spec[0] + " legacy=" + legacy + " seed=" + seed + " i=" + i);
					}
				}
			}
		}
	}

	@Test
	void normalNoiseGameSeedingPath() {
		double[] pts = points(1500, 12);
		for (long seed : SEEDS) {
			for (Object[] spec : VANILLA_NOISES) {
				String name = (String) spec[0];
				int firstOctave = (int) spec[1];
				double[] amps = (double[]) spec[2];
				// Same as Noises.instantiate(positionalFactory, holder) in the game.
				NormalNoise v = NormalNoise.create(
						new XoroshiroRandomSource(seed).forkPositional().fromHashOf(name),
						new NormalNoise.NoiseParameters(firstOctave, new DoubleArrayList(amps)));
				double[] n = new double[pts.length / 3];
				double max = NativeNoise.normalNoise(seed, name, firstOctave, amps, false, pts, n);
				assertBitEqual(v.maxValue(), max, "normal maxValue " + name + " seed=" + seed);
				for (int i = 0; i < n.length; i++) {
					assertBitEqual(v.getValue(pts[i * 3], pts[i * 3 + 1], pts[i * 3 + 2]), n[i],
							"normal " + name + " seed=" + seed + " i=" + i);
				}
			}
		}
	}

	@Test
	void normalNoiseLegacyNetherBiome() {
		// What the LEGACY algorithm uses for nether temperature/vegetation: (-7, 1.0, 1.0).
		double[] amps = {1.0, 1.0};
		double[] pts = points(1500, 13);
		for (long seed : SEEDS) {
			NormalNoise v = NormalNoise.createLegacyNetherBiome(new XoroshiroRandomSource(seed),
					new NormalNoise.NoiseParameters(-7, new DoubleArrayList(amps)));
			double[] n = new double[pts.length / 3];
			double max = NativeNoise.normalNoise(seed, null, -7, amps, true, pts, n);
			assertBitEqual(v.maxValue(), max, "legacy normal maxValue seed=" + seed);
			for (int i = 0; i < n.length; i++) {
				assertBitEqual(v.getValue(pts[i * 3], pts[i * 3 + 1], pts[i * 3 + 2]), n[i], "legacy normal seed=" + seed + " i=" + i);
			}
		}
	}

	@Test
	void blendedNoise() {
		// Overworld 1.18.2: sampling (1, 1, 80, 160), cell 4 x 8. Plus an unusual config.
		double[][] settings = {{1.0, 1.0, 80.0, 160.0, 4, 8}, {0.25, 0.375, 80.0, 60.0, 8, 4}};
		SplittableRandom r = new SplittableRandom(14);
		int count = 1000;
		int[] blocks = new int[count * 3];
		for (int i = 0; i < count; i++) {
			blocks[i * 3] = r.nextInt(-30_000_000, 30_000_000);
			blocks[i * 3 + 1] = r.nextInt(-64, 320);
			blocks[i * 3 + 2] = r.nextInt(-30_000_000, 30_000_000);
		}
		blocks[0] = 0; blocks[1] = 0; blocks[2] = 0;
		blocks[3] = -1; blocks[4] = -64; blocks[5] = -1;

		for (long seed : SEEDS) {
			for (double[] s : settings) {
				NoiseSamplingSettings nss = new NoiseSamplingSettings(s[0], s[1], s[2], s[3]);
				// Same as NoiseRouterData.createNoiseRouter for the XOROSHIRO algorithm.
				BlendedNoise v = new BlendedNoise(
						new XoroshiroRandomSource(seed).forkPositional().fromHashOf("minecraft:terrain"), nss, (int) s[4], (int) s[5]);
				double[] n = new double[count];
				double max = NativeNoise.blendedNoise(seed, "minecraft:terrain", s[0], s[1], s[2], s[3], (int) s[4], (int) s[5], blocks, n);
				assertBitEqual(v.maxValue(), max, "blended maxValue seed=" + seed);
				for (int i = 0; i < count; i++) {
					int x = blocks[i * 3], y = blocks[i * 3 + 1], z = blocks[i * 3 + 2];
					assertBitEqual(v.compute(ctx(x, y, z)), n[i], "blended seed=" + seed + " block=" + x + "," + y + "," + z);
				}
			}
		}
	}

	// ------------------------------------------------------------------ helpers

	private static DensityFunction.FunctionContext ctx(int x, int y, int z) {
		return new DensityFunction.FunctionContext() {
			@Override public int blockX() { return x; }
			@Override public int blockY() { return y; }
			@Override public int blockZ() { return z; }
		};
	}

	/** Mix of local, world-scale and extreme coordinates (packed xyz). */
	private static double[] points(int count, long salt) {
		SplittableRandom r = new SplittableRandom(salt);
		double[] pts = new double[count * 3];
		for (int i = 0; i < pts.length; i++) {
			pts[i] = switch (i % 4) {
				case 0 -> r.nextDouble(-64.0, 64.0);
				case 1 -> r.nextDouble(-30_000_000.0, 30_000_000.0);
				case 2 -> (double) r.nextInt(-100_000, 100_000) * 0.25;
				default -> r.nextDouble(-1e9, 1e9);
			};
		}
		return pts;
	}

	private static void assertBitEqual(double expected, double actual, String msg) {
		assertEquals(Double.doubleToRawLongBits(expected), Double.doubleToRawLongBits(actual),
				() -> msg + " expected=" + expected + " actual=" + actual);
	}
}
