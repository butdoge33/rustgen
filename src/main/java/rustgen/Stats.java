package rustgen;

import java.util.concurrent.atomic.LongAdder;

/** Cheap timing counters (nanoTime sums) for doFill, reported by the pregen harness. */
public final class Stats {
	public static final LongAdder DOFILL_NANOS = new LongAdder();
	public static final LongAdder DOFILL_COUNT = new LongAdder();
	public static final LongAdder BEARD_NANOS = new LongAdder();
	public static final LongAdder NATIVE_NANOS = new LongAdder();
	public static final LongAdder PLACE_NANOS = new LongAdder();

	private Stats() {}

	public static void reset() {
		for (LongAdder a : new LongAdder[] {DOFILL_NANOS, DOFILL_COUNT, BEARD_NANOS, NATIVE_NANOS, PLACE_NANOS}) a.reset();
	}

	public static String summary() {
		long n = Math.max(1, DOFILL_COUNT.sum());
		return String.format("doFill %.2f ms/chunk (n=%d) [beardifier %.2f, native call %.2f, aquifer+placement %.2f]",
				DOFILL_NANOS.sum() / 1e6 / n, DOFILL_COUNT.sum(), BEARD_NANOS.sum() / 1e6 / n, NATIVE_NANOS.sum() / 1e6 / n,
				PLACE_NANOS.sum() / 1e6 / n);
	}
}
