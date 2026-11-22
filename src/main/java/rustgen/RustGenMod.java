package rustgen;

import net.fabricmc.api.ModInitializer;
import net.fabricmc.loader.api.FabricLoader;

public class RustGenMod implements ModInitializer {
	@Override
	public void onInitialize() {
		// Triggers extraction + load; logs the outcome.
		NativeLib.isAvailable();
		RustgenConfig.load();
		StageStats.startReporter();

		// Debug-only verification/benchmark harness (needs Fabric API for the server event).
		if (Integer.getInteger("rustgen.pregen", -1) >= 0 && FabricLoader.getInstance().isModLoaded("fabric-api")) {
			rustgen.debug.PregenHarness.register();
		}
	}
}
