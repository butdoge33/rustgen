package rustgen;

import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Method;
import java.util.Arrays;

import net.fabricmc.api.EnvType;
import net.fabricmc.loader.api.FabricLoader;

import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;

/**
 * Loading a class runs the mixin transformation, which fails if an injection point does not
 * exist; then we check our handler methods were actually merged in. Classes are loaded
 * without initialization (no rendering needed).
 */
class ClientMixinTest {
	@Test
	void clientMixinsApply() throws Exception {
		Assumptions.assumeTrue(FabricLoader.getInstance().getEnvironmentType() == EnvType.CLIENT, "client mixins only apply in a client environment");
		ClassLoader cl = ClientMixinTest.class.getClassLoader();
		assertHandler(Class.forName("net.minecraft.client.gui.components.DebugScreenOverlay", false, cl), "worldgenLine");
		assertHandler(Class.forName("net.minecraft.client.gui.screens.VideoSettingsScreen", false, cl), "addOption");
	}

	private static void assertHandler(Class<?> c, String handler) {
		boolean found = Arrays.stream(c.getDeclaredMethods()).map(Method::getName).anyMatch(n -> n.contains(handler));
		assertTrue(found, c.getName() + " is missing the rustgen handler " + handler);
		System.out.println("[rustgen] client mixin applied: " + c.getSimpleName() + " <- " + handler);
	}
}
