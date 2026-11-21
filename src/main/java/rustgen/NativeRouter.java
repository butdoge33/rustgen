package rustgen;

import java.lang.ref.Cleaner;
import java.util.List;

import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import rustgen.translate.DensityTranslator;

/** A chunk generator's router compiled to a native program, plus what it may be used for. */
public final class NativeRouter {
	private static final Logger LOG = LoggerFactory.getLogger("rustgen");
	private static final Cleaner CLEANER = Cleaner.create();

	static final int FINAL = root("final_density");
	static final int[] VEINS = {root("vein_toggle"), root("vein_ridged"), root("vein_gap")};

	final NativeDensity density;
	/** Whether ore veins are enabled for these settings (and therefore computed natively). */
	final boolean veins;

	private NativeRouter(NativeDensity density, boolean veins) {
		this.density = density;
		this.veins = veins;
	}

	private static int root(String name) {
		return List.of(DensityTranslator.ROOT_NAMES).indexOf(name);
	}

	/**
	 * Compiles {@code router}. Returns null (after logging why, once) if the native path
	 * can't reproduce vanilla for it; the caller then keeps using vanilla.
	 *
	 * @param owner the generator; the native program is freed when it becomes unreachable
	 */
	public static NativeRouter create(Object owner, NoiseRouter router, NoiseGeneratorSettings settings, String name) {
		if (!NativeLib.isAvailable()) return null;
		try {
			DensityTranslator.Result tr = DensityTranslator.translate(router);
			boolean veins = settings.oreVeinsEnabled();
			int[] needed = veins ? new int[] {FINAL, VEINS[0], VEINS[1], VEINS[2]} : new int[] {FINAL};
			for (int r : needed) {
				if (!tr.isSupported(r)) {
					LOG.info("[rustgen] {}: using vanilla noise fill ({}: {})", name, DensityTranslator.ROOT_NAMES[r], tr.unsupported()[r]);
					return null;
				}
			}
			NativeDensity nd = new NativeDensity(tr.program());
			for (int r : needed) {
				String why = nd.checkChunk(r);
				if (why != null) {
					nd.close();
					LOG.info("[rustgen] {}: using vanilla noise fill ({}: {})", name, DensityTranslator.ROOT_NAMES[r], why);
					return null;
				}
			}
			CLEANER.register(owner, nd::close); // must not capture `owner`
			LOG.info("[rustgen] {}: native noise fill enabled", name);
			return new NativeRouter(nd, veins);
		} catch (RuntimeException e) {
			LOG.error("[rustgen] {}: failed to compile noise router, using vanilla", name, e);
			return null;
		}
	}
}
