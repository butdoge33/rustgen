package rustgen;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.List;
import java.util.SplittableRandom;

import net.minecraft.data.BuiltinRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;

import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import rustgen.translate.DensityTranslator;

/**
 * Full noise-router parity: builds each vanilla 1.18.2 dimension's seeded router exactly
 * like NoiseBasedChunkGenerator does, translates it, and compares every router output
 * against vanilla compute(SinglePointContext) bit for bit.
 */
class RouterParityTest {
	static {
		// Must run before the static fields below touch registries (static init is textual order).
		TestSupport.bootstrap();
	}

	private static final List<ResourceKey<NoiseGeneratorSettings>> SETTINGS = List.of(
			NoiseGeneratorSettings.OVERWORLD, NoiseGeneratorSettings.LARGE_BIOMES, NoiseGeneratorSettings.AMPLIFIED,
			NoiseGeneratorSettings.CAVES, NoiseGeneratorSettings.FLOATING_ISLANDS,
			NoiseGeneratorSettings.NETHER, NoiseGeneratorSettings.END);

	private static final long[] SEEDS = {0L, 1L, -1L, 42L, Long.MIN_VALUE, -4172144997902289642L, 0x5EEDL, 987654321123L};

	@BeforeAll
	static void setup() {
		TestSupport.bootstrap();
		assertTrue(NativeLib.isAvailable(), "native library failed to load for " + NativeLib.platform());
	}

	private static NoiseRouter router(ResourceKey<NoiseGeneratorSettings> key, long seed) {
		// Same call as NoiseBasedChunkGenerator's constructor.
		return BuiltinRegistries.NOISE_GENERATOR_SETTINGS.getOrThrow(key).createNoiseRouter(BuiltinRegistries.NOISE, seed);
	}

	private static int[] blocks(int count, long salt) {
		SplittableRandom r = new SplittableRandom(salt);
		int[] b = new int[count * 3];
		for (int i = 0; i < count; i++) {
			boolean near = i % 3 == 0; // a third near spawn, the rest anywhere in the world border
			b[i * 3] = near ? r.nextInt(-2000, 2000) : r.nextInt(-30_000_000, 30_000_000);
			b[i * 3 + 1] = r.nextInt(-128, 420);
			b[i * 3 + 2] = near ? r.nextInt(-2000, 2000) : r.nextInt(-30_000_000, 30_000_000);
		}
		return b;
	}

	@Test
	void allRoutersMatchVanilla() {
		int[] blocks = blocks(2500, 77);
		int n = blocks.length / 3;
		List<String> unsupportedReport = new ArrayList<>();
		long compared = 0;

		for (ResourceKey<NoiseGeneratorSettings> key : SETTINGS) {
			String dim = key.location().getPath();
			for (long seed : SEEDS) {
				NoiseRouter router = router(key, seed);
				DensityTranslator.Result tr = DensityTranslator.translate(router);
				DensityFunction[] roots = DensityTranslator.roots(router);

				try (NativeDensity nd = new NativeDensity(tr.program())) {
					double[] out = new double[n];
					for (int root = 0; root < roots.length; root++) {
						String name = DensityTranslator.ROOT_NAMES[root];
						if (!tr.isSupported(root)) {
							if (seed == SEEDS[0]) unsupportedReport.add(dim + "/" + name + ": " + tr.unsupported()[root]);
							continue;
						}
						nd.evalPoints(root, blocks, out);
						for (int i = 0; i < n; i++) {
							int x = blocks[i * 3], y = blocks[i * 3 + 1], z = blocks[i * 3 + 2];
							double expected = roots[root].compute(new DensityFunction.SinglePointContext(x, y, z));
							if (Double.doubleToRawLongBits(expected) != Double.doubleToRawLongBits(out[i])) {
								assertEquals(expected, out[i], dim + " seed=" + seed + " root=" + name + " at " + x + "," + y + "," + z
										+ " (bits " + Long.toHexString(Double.doubleToRawLongBits(expected))
										+ " vs " + Long.toHexString(Double.doubleToRawLongBits(out[i])) + ")");
								throw new AssertionError("bit mismatch (equal as doubles): " + dim + " " + name + " " + expected);
							}
							compared++;
						}
					}
				}
			}
		}

		System.out.println("[rustgen] router parity: " + compared + " values compared bit-exact");
		System.out.println("[rustgen] unsupported roots (fall back to vanilla):"
				+ (unsupportedReport.isEmpty() ? " none" : "\n  " + String.join("\n  ", unsupportedReport)));

		// Every overworld-family root must be supported — that's the main target.
		for (var key : List.of(NoiseGeneratorSettings.OVERWORLD, NoiseGeneratorSettings.LARGE_BIOMES, NoiseGeneratorSettings.AMPLIFIED)) {
			DensityTranslator.Result tr = DensityTranslator.translate(router(key, 1L));
			for (int root = 0; root < DensityTranslator.ROOT_NAMES.length; root++) {
				assertTrue(tr.isSupported(root), key.location() + "/" + DensityTranslator.ROOT_NAMES[root] + ": " + tr.unsupported()[root]);
			}
		}
	}

	/**
	 * Rough single-point throughput for final_density. NOT representative of real chunk
	 * generation (vanilla uses cell interpolation + caches in NoiseChunk); it only shows
	 * the raw cost of evaluating the graph per point. Printed, not asserted.
	 */
	@Test
	void roughPointThroughput() {
		NoiseRouter router = router(NoiseGeneratorSettings.OVERWORLD, 12345L);
		DensityFunction fd = router.finalDensity();
		int root = List.of(DensityTranslator.ROOT_NAMES).indexOf("final_density");
		int[] blocks = blocks(100_000, 3);
		int n = blocks.length / 3;
		double[] out = new double[n];

		try (NativeDensity nd = new NativeDensity(DensityTranslator.translate(router).program())) {
			double sink = 0;
			for (int warm = 0; warm < 3; warm++) { // let the JIT compile vanilla's path too
				for (int i = 0; i < n; i++) sink += fd.compute(new DensityFunction.SinglePointContext(blocks[i * 3], blocks[i * 3 + 1], blocks[i * 3 + 2]));
				nd.evalPoints(root, blocks, out);
			}
			long t0 = System.nanoTime();
			for (int i = 0; i < n; i++) sink += fd.compute(new DensityFunction.SinglePointContext(blocks[i * 3], blocks[i * 3 + 1], blocks[i * 3 + 2]));
			long t1 = System.nanoTime();
			nd.evalPoints(root, blocks, out);
			long t2 = System.nanoTime();
			System.out.printf("[rustgen] final_density single-point, %d points: vanilla %.1f ms, native %.1f ms (%.2fx)  [sink %s]%n",
					n, (t1 - t0) / 1e6, (t2 - t1) / 1e6, (double) (t1 - t0) / (t2 - t1), sink != 0);
		}
	}
}
