package rustgen;

import net.fabricmc.api.ModInitializer;

public class RustGenMod implements ModInitializer {
	@Override
	public void onInitialize() {
		// Triggers extraction + load; logs the outcome. Nothing is hooked yet.
		NativeLib.isAvailable();
	}
}
