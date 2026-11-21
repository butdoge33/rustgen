package rustgen;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Locale;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Extracts the bundled Rust library for the current platform and loads it.
 * If anything goes wrong, {@link #isAvailable()} returns false and the mod
 * must leave vanilla code paths untouched.
 */
public final class NativeLib {
	/** Must match ABI_VERSION in native/src/lib.rs. Bump on any signature change. */
	public static final int EXPECTED_ABI = 6;

	private static final Logger LOG = LoggerFactory.getLogger("rustgen");
	private static final boolean AVAILABLE = load();

	private NativeLib() {}

	public static boolean isAvailable() {
		return AVAILABLE;
	}

	public static String platform() {
		String os = System.getProperty("os.name").toLowerCase(Locale.ROOT);
		String arch = System.getProperty("os.arch").toLowerCase(Locale.ROOT);
		String o = os.contains("win") ? "windows" : os.contains("mac") ? "macos" : "linux";
		String a = (arch.equals("amd64") || arch.equals("x86_64")) ? "x86_64" : arch;
		return o + "-" + a;
	}

	private static String fileName() {
		String p = platform();
		if (p.startsWith("windows")) return "rustgen.dll";
		if (p.startsWith("macos")) return "librustgen.dylib";
		return "librustgen.so";
	}

	private static boolean load() {
		if (Boolean.getBoolean("rustgen.disable")) {
			LOG.info("[rustgen] disabled via -Drustgen.disable=true; using vanilla worldgen");
			return false;
		}
		String resource = "/natives/" + platform() + "/" + fileName();
		try (InputStream in = NativeLib.class.getResourceAsStream(resource)) {
			if (in == null) {
				LOG.warn("[rustgen] no native library for {} ({}); using vanilla worldgen", platform(), resource);
				return false;
			}
			Path tmp = Files.createTempFile("rustgen-", "-" + fileName());
			tmp.toFile().deleteOnExit();
			Files.copy(in, tmp, StandardCopyOption.REPLACE_EXISTING);
			System.load(tmp.toAbsolutePath().toString());

			int abi = NativeNoise.abiVersion();
			if (abi != EXPECTED_ABI) {
				LOG.error("[rustgen] native ABI {} != expected {}; using vanilla worldgen", abi, EXPECTED_ABI);
				return false;
			}
			LOG.info("[rustgen] native library loaded for {}", platform());
			return true;
		} catch (IOException | UnsatisfiedLinkError | SecurityException e) {
			LOG.error("[rustgen] failed to load native library; using vanilla worldgen", e);
			return false;
		}
	}
}
