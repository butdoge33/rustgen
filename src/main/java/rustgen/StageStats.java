package rustgen;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.LongAdder;

import net.minecraft.world.level.chunk.ChunkStatus;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Per-stage chunk generation timing, logged periodically (default every 300 s, override
 * with -Drustgen.statsInterval=SECONDS, 0 disables).
 *
 * Stage time = from ChunkStatus.generate() start until its future completes. For the
 * synchronous stages that is the work itself; for asynchronous ones (biomes, noise,
 * light) it also includes waiting for a worker thread. The terrain fill inside the noise
 * stage (doFill) is additionally measured as pure work, split by Rust / Java.
 */
public final class StageStats {
	private static final Logger LOG = LoggerFactory.getLogger("rustgen");
	private static final Map<String, LongAdder[]> STAGES = new ConcurrentHashMap<>();
	public static final LongAdder RUST_FILLS = new LongAdder(), RUST_FILL_NANOS = new LongAdder();
	public static final LongAdder JAVA_FILLS = new LongAdder(), JAVA_FILL_NANOS = new LongAdder();

	private StageStats() {}

	public static void stage(String name, long nanos) {
		LongAdder[] a = STAGES.computeIfAbsent(name, k -> new LongAdder[] {new LongAdder(), new LongAdder()});
		a[0].increment();
		a[1].add(nanos);
	}

	public static void startReporter() {
		long interval = Long.getLong("rustgen.statsInterval", 300);
		if (interval <= 0) return;
		Thread t = new Thread(() -> {
			while (true) {
				try {
					Thread.sleep(interval * 1000);
				} catch (InterruptedException e) {
					return;
				}
				String report = drain(interval);
				if (report != null) LOG.info(report);
			}
		}, "rustgen-stats");
		t.setDaemon(true);
		t.start();
	}

	/** Builds a report of everything since the last call and resets the counters; null if idle. */
	static String drain(long seconds) {
		long rust = RUST_FILLS.sumThenReset(), rustNs = RUST_FILL_NANOS.sumThenReset();
		long java = JAVA_FILLS.sumThenReset(), javaNs = JAVA_FILL_NANOS.sumThenReset();
		List<String> lines = new ArrayList<>();
		long total = 0;
		for (ChunkStatus status : ChunkStatus.getStatusList()) {
			LongAdder[] a = STAGES.get(status.getName());
			if (a == null) continue;
			long n = a[0].sumThenReset(), ns = a[1].sumThenReset();
			if (n == 0) continue;
			total += ns;
			lines.add(String.format("  %-20s %7d chunks  %8.2f ms avg  %9.1f s total", status.getName(), n, ns / 1e6 / n, ns / 1e9));
		}
		if (lines.isEmpty() && rust + java == 0) return null;
		StringBuilder sb = new StringBuilder();
		sb.append(String.format("[rustgen] chunk generation, last %d s (stage time = start to completion, incl. queueing for async stages):%n", seconds));
		for (String l : lines) sb.append(l).append(System.lineSeparator());
		sb.append(String.format("  %-20s %9.1f s total across stages%n", "", total / 1e9));
		sb.append(String.format("  terrain fill (doFill, pure work): Rust %d chunks @ %.2f ms avg, Java %d chunks @ %.2f ms avg",
				rust, rust == 0 ? 0.0 : rustNs / 1e6 / rust, java, java == 0 ? 0.0 : javaNs / 1e6 / java));
		return sb.toString();
	}
}
