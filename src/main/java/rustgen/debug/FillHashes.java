package rustgen.debug;

import java.io.BufferedWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;

import net.minecraft.world.level.chunk.ChunkAccess;

/**
 * Debug: with {@code -Drustgen.hashFills=true}, every chunk's state right after doFill
 * (native or vanilla) is hashed and appended to {@code rustgen-fills.txt} in the run
 * directory as "x z hash". Covers chunks the pregen area doesn't (e.g. spawn), and is
 * flushed per line so even a run that hangs leaves usable data.
 */
public final class FillHashes {
	public static final boolean ENABLED = Boolean.getBoolean("rustgen.hashFills");
	private static BufferedWriter out;

	private FillHashes() {}

	public static void record(ChunkAccess chunk) {
		if (!ENABLED) return;
		String line = chunk.getPos().x + " " + chunk.getPos().z + " " + Long.toHexString(PregenHarness.hash(chunk)) + "\n";
		synchronized (FillHashes.class) {
			try {
				if (out == null) {
					out = Files.newBufferedWriter(Path.of("rustgen-fills.txt"), StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
				}
				out.write(line);
				out.flush();
			} catch (IOException e) {
				throw new RuntimeException(e);
			}
		}
	}
}
