package rustgen;

import java.io.IOException;
import java.io.Reader;
import java.io.Writer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

import net.fabricmc.loader.api.FabricLoader;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * User setting: generate new chunks with the Rust or the Java (vanilla) noise stage.
 * Stored in config/rustgen.properties ({@code worldgen=rust|java}); editable in-game in
 * Video Settings. Both produce identical chunks, so switching at any time is safe.
 */
public final class RustgenConfig {
	private static final Logger LOG = LoggerFactory.getLogger("rustgen");
	private static final String KEY = "worldgen";
	private static volatile boolean useRust = true;
	private static volatile boolean loaded;

	private RustgenConfig() {}

	private static Path file() {
		return FabricLoader.getInstance().getConfigDir().resolve("rustgen.properties");
	}

	/** Whether the user wants Rust world generation (the native library may still be unavailable). */
	public static boolean useRust() {
		if (!loaded) load();
		return useRust;
	}

	public static synchronized void setUseRust(boolean value) {
		if (!loaded) load();
		if (useRust == value) return;
		useRust = value;
		LOG.info("[rustgen] world generation set to {}", value ? "Rust" : "Java");
		save();
	}

	public static synchronized void load() {
		if (loaded) return;
		Path f = file();
		if (Files.exists(f)) {
			Properties p = new Properties();
			try (Reader r = Files.newBufferedReader(f)) {
				p.load(r);
				useRust = !"java".equalsIgnoreCase(p.getProperty(KEY, "rust").trim());
			} catch (IOException | IllegalArgumentException e) {
				LOG.warn("[rustgen] could not read {}, using defaults", f, e);
			}
		} else {
			useRust = true;
			loaded = true;
			save(); // write the default so it's discoverable
		}
		loaded = true;
	}

	private static void save() {
		Properties p = new Properties();
		p.setProperty(KEY, useRust ? "rust" : "java");
		try {
			Files.createDirectories(file().getParent());
			try (Writer w = Files.newBufferedWriter(file())) {
				p.store(w, "rustgen: world generation for new chunks (rust | java). Both produce identical chunks.");
			}
		} catch (IOException e) {
			LOG.warn("[rustgen] could not write {}", file(), e);
		}
	}
}
