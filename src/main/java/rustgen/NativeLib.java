package rustgen;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Locale;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Extracts the bundled Rust library for the current platform and loads it.
 * If anything goes wrong, {@link #isAvailable()} returns false and the mod
 * must leave vanilla code paths untouched.
 *
 * <p>The library is extracted to {@code <tmp>/rustgen-natives/<sha256>/<file>} and reused
 * by later launches. A content-addressed path matters on Windows, where a loaded DLL can't
 * be deleted: a fresh temp file per launch would pile up in %TEMP%.
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
		try {
			byte[] bytes;
			try (InputStream in = NativeLib.class.getResourceAsStream(resource)) {
				if (in == null) {
					LOG.warn("[rustgen] no native library for {} ({}); using vanilla worldgen", platform(), resource);
					return false;
				}
				bytes = in.readAllBytes();
			}
			Path lib = extract(bytes);
			System.load(lib.toAbsolutePath().toString());

			int abi = NativeNoise.abiVersion();
			if (abi != EXPECTED_ABI) {
				LOG.error("[rustgen] native ABI {} != expected {}; using vanilla worldgen", abi, EXPECTED_ABI);
				return false;
			}
			LOG.info("[rustgen] native library loaded for {}", platform());
			return true;
		} catch (IOException | UnsatisfiedLinkError | SecurityException | NoSuchAlgorithmException e) {
			LOG.error("[rustgen] failed to load native library; using vanilla worldgen", e);
			return false;
		}
	}

	/** Writes the library to a content-addressed cache path (or reuses an identical copy). */
	private static Path extract(byte[] bytes) throws IOException, NoSuchAlgorithmException {
		String hash = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
		Path dir = Path.of(System.getProperty("java.io.tmpdir"), "rustgen-natives", hash.substring(0, 16));
		Path target = dir.resolve(fileName());
		if (Files.isRegularFile(target) && Files.size(target) == bytes.length && hash.equals(sha256(target))) {
			return target; // already extracted by an earlier launch
		}
		Files.createDirectories(dir);
		// Write to a unique temp file, then move into place, so concurrent launches never
		// load a half-written library.
		// pid + nanoTime is unique enough with CREATE_NEW (fails rather than clobbering), and
		// avoids Files.createTempFile, whose SecureRandom seeding can fail in some Windows
		// environments (e.g. network adapter enumeration under Wine).
		Path tmp = dir.resolve("extract-" + ProcessHandle.current().pid() + "-" + System.nanoTime() + ".tmp");
		try {
			Files.write(tmp, bytes, StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE);
			try {
				Files.move(tmp, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
			} catch (AtomicMoveNotSupportedException e) {
				Files.move(tmp, target, StandardCopyOption.REPLACE_EXISTING);
			}
		} catch (IOException e) {
			Files.deleteIfExists(tmp);
			// Another process may have the target loaded (Windows locks it); if it is complete
			// and identical, using it is fine.
			if (Files.isRegularFile(target) && hash.equals(sha256(target))) return target;
			throw e;
		}
		return target;
	}

	private static String sha256(Path p) throws IOException, NoSuchAlgorithmException {
		return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(Files.readAllBytes(p)));
	}
}
