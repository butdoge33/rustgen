package rustgen;

import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.util.List;

import net.minecraft.data.BuiltinRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.Mth;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.DensityFunctions;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.NoiseSettings;
import net.minecraft.world.level.levelgen.blending.Blender;

import org.junit.jupiter.api.Test;

import rustgen.translate.DensityTranslator;

/**
 * Chunk-level parity: drives a real vanilla {@link NoiseChunk} through the exact call
 * sequence of NoiseBasedChunkGenerator.doFill and compares, at every block, the density
 * fed to the aquifer and the ore-vein inputs against {@link NativeDensity#fillChunk}.
 */
class ChunkParityTest {
	static {
		TestSupport.bootstrap();
	}

	private static final List<ResourceKey<NoiseGeneratorSettings>> PRESETS = List.of(
			NoiseGeneratorSettings.OVERWORLD, NoiseGeneratorSettings.LARGE_BIOMES, NoiseGeneratorSettings.AMPLIFIED,
			NoiseGeneratorSettings.CAVES, NoiseGeneratorSettings.FLOATING_ISLANDS, NoiseGeneratorSettings.NETHER);
	private static final long[] SEEDS = {0L, 42L, -4172144997902289642L};
	private static final int[][] CHUNKS = {{0, 0}, {-1, -1}, {123, -456}, {-62500, 31250}, {1874999, -1874999}};

	private static final int FINAL = indexOf("final_density");
	private static final int[] VEINS = {indexOf("vein_toggle"), indexOf("vein_ridged"), indexOf("vein_gap")};

	private static int indexOf(String root) {
		return List.of(DensityTranslator.ROOT_NAMES).indexOf(root);
	}

	/** Vanilla reference values for one chunk, same layout as the native outputs. */
	record Vanilla(double[] density, double[][] veins) {}

	static Vanilla vanillaChunk(NoiseGeneratorSettings settings, NoiseRouter router, int chunkX, int chunkZ) throws Exception {
		NoiseSettings ns = settings.noiseSettings();
		int cellW = ns.getCellWidth(), cellH = ns.getCellHeight();
		int minY = ns.minY(), maxY = ns.minY() + ns.height(); // vanilla dimensions cover the noise range
		int cellMinY = Mth.intFloorDiv(minY, cellH);
		int cellCountY = Mth.intFloorDiv(maxY - minY, cellH);
		int minX = chunkX * 16, minZ = chunkZ * 16;

		// Private constructor, called exactly like NoiseChunk.forChunk with no structures and Blender.empty().
		Constructor<NoiseChunk> ctor = NoiseChunk.class.getDeclaredConstructor(int.class, int.class, int.class, NoiseRouter.class,
				int.class, int.class, DensityFunctions.BeardifierOrMarker.class, NoiseGeneratorSettings.class,
				Aquifer.FluidPicker.class, Blender.class);
		ctor.setAccessible(true);
		DensityFunctions.BeardifierOrMarker beard = beardifierMarker();
		Aquifer.FluidPicker fluids = (x, y, z) -> new Aquifer.FluidStatus(-54, Blocks.WATER.defaultBlockState());
		NoiseChunk nc = ctor.newInstance(16 / cellW, cellCountY, cellMinY, router, minX, minZ, beard, settings, fluids, Blender.empty());

		// NoiseChunk memoizes wrap() by structural equality, so this returns the very same
		// wrapped objects the constructor built for the block-state rules.
		Method wrap = NoiseChunk.class.getDeclaredMethod("wrap", DensityFunction.class);
		wrap.setAccessible(true);
		DensityFunction.Visitor wrapper = f -> {
			try {
				return (DensityFunction) wrap.invoke(nc, f);
			} catch (ReflectiveOperationException e) {
				throw new RuntimeException(e);
			}
		};
		DensityFunction density = DensityFunctions.cacheAllInCell(DensityFunctions.add(router.finalDensity(), beard)).mapAll(wrapper);
		DensityFunction[] veins = {router.veinToggle().mapAll(wrapper), router.veinRidged().mapAll(wrapper), router.veinGap().mapAll(wrapper)};

		int width = 16, height = cellCountY * cellH;
		double[] outDensity = new double[width * width * height];
		double[][] outVeins = new double[3][width * width * height];

		// --- NoiseBasedChunkGenerator.doFill, minus block placement ---
		nc.initializeForFirstCellX();
		int cellsXZ = 16 / cellW;
		for (int q = 0; q < cellsXZ; q++) {
			nc.advanceCellX(q);
			for (int r = 0; r < cellsXZ; r++) {
				for (int s = cellCountY - 1; s >= 0; s--) {
					nc.selectCellYZ(s, r);
					for (int t = cellH - 1; t >= 0; t--) {
						int y = (cellMinY + s) * cellH + t;
						nc.updateForY(y, (double) t / cellH);
						for (int dx = 0; dx < cellW; dx++) {
							int x = minX + q * cellW + dx;
							nc.updateForX(x, (double) dx / cellW);
							for (int dz = 0; dz < cellW; dz++) {
								int z = minZ + r * cellW + dz;
								nc.updateForZ(z, (double) dz / cellW);
								int idx = ((y - minY) * width + (z - minZ)) * width + (x - minX);
								outDensity[idx] = density.compute(nc);
								for (int v = 0; v < 3; v++) outVeins[v][idx] = veins[v].compute(nc);
							}
						}
					}
				}
			}
			nc.swapSlices();
		}
		nc.stopInterpolation();
		return new Vanilla(outDensity, outVeins);
	}

	static int[] params(NoiseGeneratorSettings settings, int chunkX, int chunkZ) {
		NoiseSettings ns = settings.noiseSettings();
		int cellW = ns.getCellWidth(), cellH = ns.getCellHeight();
		return new int[] {chunkX * 16, chunkZ * 16, Mth.intFloorDiv(ns.minY(), cellH), Mth.intFloorDiv(ns.height(), cellH), 16 / cellW, cellW, cellH};
	}

	static DensityFunctions.BeardifierOrMarker beardifierMarker() throws ClassNotFoundException {
		Class<?> c = Class.forName("net.minecraft.world.level.levelgen.DensityFunctions$BeardifierMarker", true, NoiseChunk.class.getClassLoader());
		return (DensityFunctions.BeardifierOrMarker) c.getEnumConstants()[0];
	}

	@Test
	void chunksMatchVanilla() throws Exception {
		long blocks = 0;
		for (ResourceKey<NoiseGeneratorSettings> key : PRESETS) {
			NoiseGeneratorSettings settings = BuiltinRegistries.NOISE_GENERATOR_SETTINGS.getOrThrow(key);
			for (long seed : SEEDS) {
				NoiseRouter router = settings.createNoiseRouter(BuiltinRegistries.NOISE, seed);
				try (NativeDensity nd = new NativeDensity(DensityTranslator.translate(router).program())) {
					assertNull(nd.checkChunk(FINAL), key.location() + " final_density not chunk-fillable");
					for (int v : VEINS) assertNull(nd.checkChunk(v), key.location() + " vein root not chunk-fillable");

					for (int[] c : CHUNKS) {
						Vanilla van = vanillaChunk(settings, router, c[0], c[1]);
						double[] density = new double[van.density().length];
						double[][] veins = new double[3][density.length];
						String where = key.location().getPath() + " seed=" + seed + " chunk=" + c[0] + "," + c[1];

						// Eager: every vein value everywhere.
						nd.fillChunk(FINAL, params(settings, c[0], c[1]), null, density, VEINS, veins, false);
						compare(where + " density", van.density(), density, c, settings);
						for (int v = 0; v < 3; v++) compare(where + " " + DensityTranslator.ROOT_NAMES[VEINS[v]], van.veins()[v], veins[v], c, settings);

						// Lazy (what the game uses): computed values must match, and everything
						// vanilla's OreVeinifier can read must have been computed.
						nd.fillChunk(FINAL, params(settings, c[0], c[1]), null, density, VEINS, veins, true);
						compare(where + " density (lazy)", van.density(), density, c, settings);
						checkLazyVeins(where, van.veins(), veins, c, settings);
						blocks += density.length;
					}
				}
			}
		}
		System.out.println("[rustgen] chunk parity: " + blocks + " blocks x 4 values compared bit-exact");
	}

	/** Exact vanilla OreVeinifier read conditions (VeinType bands, 0.4F threshold). */
	private static void checkLazyVeins(String where, double[][] expected, double[][] lazy, int[] chunk, NoiseGeneratorSettings settings) {
		int minY = settings.noiseSettings().minY();
		for (int i = 0; i < expected[0].length; i++) {
			int y = i / 256 + minY;
			double d = expected[0][i];
			boolean toggleRead = y >= -60 && y <= 50; // outside both bands the toggle value is irrelevant
			int lo = d > 0.0 ? 0 : -60, hi = d > 0.0 ? 50 : -8;
			boolean restRead = false;
			if (y >= lo && y <= hi) {
				double f = Mth.clampedMap(Math.min(hi - y, y - lo), 0.0, 20.0, -0.2, 0.0);
				restRead = !(Math.abs(d) + f < 0.4F);
			}
			for (int v = 0; v < 3; v++) {
				boolean read = v == 0 ? toggleRead : restRead;
				double got = lazy[v][i];
				if (Double.isNaN(got)) {
					if (read) fail(where + " lazy " + DensityTranslator.ROOT_NAMES[VEINS[v]] + " not computed at index " + i + " but vanilla reads it");
				} else if (Double.doubleToRawLongBits(got) != Double.doubleToRawLongBits(expected[v][i])) {
					fail(where + " lazy " + DensityTranslator.ROOT_NAMES[VEINS[v]] + " mismatch at index " + i);
				}
			}
		}
	}

	private static void compare(String what, double[] expected, double[] actual, int[] chunk, NoiseGeneratorSettings settings) {
		for (int i = 0; i < expected.length; i++) {
			if (Double.doubleToRawLongBits(expected[i]) != Double.doubleToRawLongBits(actual[i])) {
				int x = i % 16, z = (i / 16) % 16, y = i / 256 + settings.noiseSettings().minY();
				fail(what + " at block " + (chunk[0] * 16 + x) + "," + y + "," + (chunk[1] * 16 + z)
						+ ": vanilla " + expected[i] + " native " + actual[i]);
			}
		}
	}

	/**
	 * Noise-sampling cost per chunk (density + vein inputs), single-threaded.
	 * Vanilla side includes NoiseChunk construction, as in real generation. Excludes the
	 * aquifer and block placement, which stay in Java for now. Printed, not asserted.
	 */
	@Test
	void chunkThroughput() throws Exception {
		NoiseGeneratorSettings settings = BuiltinRegistries.NOISE_GENERATOR_SETTINGS.getOrThrow(NoiseGeneratorSettings.OVERWORLD);
		NoiseRouter router = settings.createNoiseRouter(BuiltinRegistries.NOISE, 12345L);
		int chunks = 64;
		byte[] program = DensityTranslator.translate(router).program();
		// Saved for the standalone native benchmark (native/examples/bench.rs).
		java.nio.file.Files.write(java.nio.file.Path.of("build", "overworld-12345.rgdf"), program);
		try (NativeDensity nd = new NativeDensity(program)) {
			int vol = 16 * 16 * 384;
			double[] density = new double[vol];
			double[][] veins = new double[3][vol];
			for (int i = 0; i < 16; i++) { // warm up both sides
				vanillaChunk(settings, router, i, 100);
				nd.fillChunk(FINAL, params(settings, i, 100), null, density, VEINS, veins, true);
			}
			long t0 = System.nanoTime();
			for (int i = 0; i < chunks; i++) vanillaChunk(settings, router, i, -7);
			long t1 = System.nanoTime();
			for (int i = 0; i < chunks; i++) nd.fillChunk(FINAL, params(settings, i, -7), null, density, VEINS, veins, true);
			long t2 = System.nanoTime();
			double van = (t1 - t0) / 1e6 / chunks, nat = (t2 - t1) / 1e6 / chunks;
			System.out.printf("[rustgen] overworld noise fill per chunk: vanilla %.2f ms, native %.2f ms (%.2fx)%n", van, nat, van / nat);
			assertTrue(nat > 0);
		}
	}
}
