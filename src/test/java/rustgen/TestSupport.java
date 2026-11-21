package rustgen;

import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;

final class TestSupport {
	private TestSupport() {}

	/** Registries (noise parameters, density function types, settings) need the game bootstrapped. */
	static synchronized void bootstrap() {
		SharedConstants.tryDetectVersion();
		Bootstrap.bootStrap();
	}
}
