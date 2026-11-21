package rustgen;

import java.lang.ref.Cleaner;
import java.util.List;

import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.NoiseSettings;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import rustgen.mixin.FluidStatusAccessor;
import rustgen.mixin.XoroshiroPositionalAccessor;
import rustgen.translate.DensityTranslator;

/**
 * A chunk generator's router compiled to a native program, plus the configuration of the
 * native block-state rules (aquifer + ore veins). Built once per generator; every check
 * that could make the native path differ from vanilla happens here.
 */
public final class NativeRouter {
	private static final Logger LOG = LoggerFactory.getLogger("rustgen");
	private static final Cleaner CLEANER = Cleaner.create();

	static final int FINAL = root("final_density");
	static final int[] VEINS = {root("vein_toggle"), root("vein_ridged"), root("vein_gap")};
	static final int BARRIER = root("barrier"), FLOODEDNESS = root("fluid_level_floodedness"),
			SPREAD = root("fluid_level_spread"), LAVA = root("lava"), INITIAL = root("initial_density_without_jaggedness");

	/** Native lava level of the global fluid picker (NoiseBasedChunkGenerator constructor). */
	private static final int LAVA_LEVEL = -54;

	final NativeDensity density;
	final int[] matInts;
	final long[] matLongs;
	final double[] matDoubles;
	/** Native code (low 7 bits) -> block state; code 0 is the settings' default block. */
	final BlockState[] codeStates;

	private NativeRouter(NativeDensity density, int[] matInts, long[] matLongs, double[] matDoubles, BlockState[] codeStates) {
		this.density = density;
		this.matInts = matInts;
		this.matLongs = matLongs;
		this.matDoubles = matDoubles;
		this.codeStates = codeStates;
	}

	private static int root(String name) {
		return List.of(DensityTranslator.ROOT_NAMES).indexOf(name);
	}

	/** Thrown (and caught in {@link #create}) when the native path can't reproduce vanilla. */
	private static final class Unsupported extends Exception {
		Unsupported(String message) {
			super(message, null, false, false);
		}
	}

	/**
	 * Compiles {@code router} and the block-state rule configuration. Returns null (after
	 * logging why, once) if anything can't be reproduced exactly; the caller keeps vanilla.
	 *
	 * @param owner       the generator; the native program is freed when it becomes unreachable
	 * @param fluidPicker the generator's global fluid picker (verified against the native model)
	 */
	public static NativeRouter create(Object owner, NoiseRouter router, NoiseGeneratorSettings settings, Aquifer.FluidPicker fluidPicker,
			String name) {
		if (!NativeLib.isAvailable()) return null;
		NativeDensity nd = null;
		try {
			DensityTranslator.Result tr = DensityTranslator.translate(router);
			nd = new NativeDensity(tr.program());
			boolean aquifers = settings.isAquifersEnabled();
			boolean veins = settings.oreVeinsEnabled();

			require(tr, nd, FINAL, true);
			if (veins) for (int v : VEINS) require(tr, nd, v, true);
			if (aquifers) {
				for (int r : new int[] {BARRIER, FLOODEDNESS, SPREAD, LAVA, INITIAL}) require(tr, nd, r, false);
				String why = nd.checkSurfaceRoot(INITIAL);
				if (why != null) throw new Unsupported("initial_density_without_jaggedness: " + why);
			}

			int defaultFluid = fluidCode(settings.defaultFluid());
			verifyFluidPicker(fluidPicker, settings.seaLevel(), settings.defaultFluid());
			long[] aquiferSeeds = aquifers ? xoroshiroSeeds(router.aquiferPositionalRandomFactory(), "aquifer") : new long[2];
			long[] oreSeeds = veins ? xoroshiroSeeds(router.oreVeinsPositionalRandomFactory(), "ore veins") : new long[2];

			NoiseSettings ns = settings.noiseSettings();
			// Unused roots must still be valid indices: point them at final_density.
			int[] matInts = {
				aquifers ? 1 : 0, settings.seaLevel(), defaultFluid, DimensionType.WAY_BELOW_MIN_Y,
				aquifers ? BARRIER : FINAL, aquifers ? FLOODEDNESS : FINAL, aquifers ? SPREAD : FINAL, aquifers ? LAVA : FINAL,
				aquifers ? INITIAL : FINAL,
				ns.getCellHeight(), ns.getMinCellY(), ns.getCellCountY(),
				ns.topSlideSettings().size(), ns.topSlideSettings().offset(), ns.bottomSlideSettings().size(), ns.bottomSlideSettings().offset(),
				veins ? 1 : 0, veins ? VEINS[0] : FINAL, veins ? VEINS[1] : FINAL, veins ? VEINS[2] : FINAL,
			};
			long[] matLongs = {aquiferSeeds[0], aquiferSeeds[1], oreSeeds[0], oreSeeds[1]};
			double[] matDoubles = {ns.topSlideSettings().target(), ns.bottomSlideSettings().target()};
			BlockState[] codeStates = {
				settings.defaultBlock(), Blocks.AIR.defaultBlockState(), Blocks.LAVA.defaultBlockState(), Blocks.WATER.defaultBlockState(),
				Blocks.COPPER_ORE.defaultBlockState(), Blocks.RAW_COPPER_BLOCK.defaultBlockState(), Blocks.GRANITE.defaultBlockState(),
				Blocks.DEEPSLATE_IRON_ORE.defaultBlockState(), Blocks.RAW_IRON_BLOCK.defaultBlockState(), Blocks.TUFF.defaultBlockState(),
			};

			NativeDensity owned = nd;
			CLEANER.register(owner, owned::close); // must not capture `owner`
			LOG.info("[rustgen] {}: native noise fill enabled (aquifers {}, ore veins {})", name, aquifers ? "on" : "off", veins ? "on" : "off");
			return new NativeRouter(nd, matInts, matLongs, matDoubles, codeStates);
		} catch (Unsupported e) {
			if (nd != null) nd.close();
			LOG.info("[rustgen] {}: using vanilla noise fill ({})", name, e.getMessage());
			return null;
		} catch (RuntimeException e) {
			if (nd != null) nd.close();
			LOG.error("[rustgen] {}: failed to compile noise router, using vanilla", name, e);
			return null;
		}
	}

	/** The root must translate; if {@code chunk}, it must also be chunk-fillable. */
	private static void require(DensityTranslator.Result tr, NativeDensity nd, int root, boolean chunk) throws Unsupported {
		String name = DensityTranslator.ROOT_NAMES[root];
		if (!tr.isSupported(root)) throw new Unsupported(name + ": " + tr.unsupported()[root]);
		if (chunk) {
			String why = nd.checkChunk(root);
			if (why != null) throw new Unsupported(name + ": " + why);
		}
	}

	/** Native fluid kinds: 1 air, 2 water, 3 lava (default states only). */
	private static int fluidCode(BlockState state) throws Unsupported {
		if (state == Blocks.AIR.defaultBlockState()) return 1;
		if (state == Blocks.WATER.defaultBlockState()) return 2;
		if (state == Blocks.LAVA.defaultBlockState()) return 3;
		throw new Unsupported("default fluid " + state + " is not supported natively");
	}

	/**
	 * The native code models NoiseBasedChunkGenerator's picker: y < min(-54, sea) ? lava@-54 :
	 * defaultFluid@sea. Probe the real picker over the whole y range (and a few columns) so a
	 * mod that replaced it can't silently diverge.
	 */
	private static void verifyFluidPicker(Aquifer.FluidPicker picker, int seaLevel, BlockState defaultFluid) throws Unsupported {
		BlockState lava = Blocks.LAVA.defaultBlockState();
		int[][] columns = {{0, 0}, {12345, -6789}, {-30_000_000, 30_000_000}};
		for (int[] c : columns) {
			for (int y = DimensionType.MIN_Y - 64; y <= DimensionType.MAX_Y + 64; y++) {
				FluidStatusAccessor s = (FluidStatusAccessor) (Object) picker.computeFluid(c[0], y, c[1]);
				boolean below = y < Math.min(LAVA_LEVEL, seaLevel);
				int level = below ? LAVA_LEVEL : seaLevel;
				BlockState type = below ? lava : defaultFluid;
				if (s.rustgen$fluidLevel() != level || s.rustgen$fluidType() != type) {
					throw new Unsupported("global fluid picker differs from vanilla's at " + c[0] + "," + y + "," + c[1]);
				}
			}
		}
	}

	private static long[] xoroshiroSeeds(PositionalRandomFactory f, String what) throws Unsupported {
		if (f instanceof XoroshiroRandomSource.XoroshiroPositionalRandomFactory) {
			XoroshiroPositionalAccessor a = (XoroshiroPositionalAccessor) f;
			return new long[] {a.rustgen$seedLo(), a.rustgen$seedHi()};
		}
		throw new Unsupported(what + " random is " + f.getClass().getSimpleName() + ", only xoroshiro is supported natively");
	}
}
