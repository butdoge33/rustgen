package rustgen;

import java.io.File;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.SplittableRandom;

/**
 * Platform cross-check: exercises every native entry point with fixed inputs and prints a
 * SHA-256 over all outputs (raw bits). Run with each platform's library; identical digests
 * mean identical results. The Linux library is verified bit-exact against vanilla by the
 * Gradle parity tests, so a matching digest carries that over to the other platform.
 *
 *   java -cp <classes> rustgen.CrossCheck <library> <program.rgdf> <program.mat>
 */
public final class CrossCheck {
	private static MessageDigest md;
	private static long values;

	public static void main(String[] args) throws Exception {
		System.load(new File(args[0]).getAbsolutePath());
		md = MessageDigest.getInstance("SHA-256");
		if (NativeNoise.abiVersion() != NativeLib.EXPECTED_ABI) throw new IllegalStateException("ABI " + NativeNoise.abiVersion());

		long[] seeds = {0L, 1L, -1L, 42L, Long.MIN_VALUE, Long.MAX_VALUE, 12345L, -4172144997902289642L};
		SplittableRandom r = new SplittableRandom(7);
		double[] pts = new double[3000 * 3];
		for (int i = 0; i < pts.length; i++) pts[i] = i % 3 == 1 ? r.nextDouble(-300, 600) : r.nextDouble(-3e7, 3e7);
		int[] blocks = new int[2000 * 3];
		for (int i = 0; i < blocks.length; i++) blocks[i] = i % 3 == 1 ? r.nextInt(-128, 420) : r.nextInt(-30_000_000, 30_000_000);
		double[] amps = {1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0};

		// RNG + noise primitives
		for (long seed : seeds) {
			long[] l = new long[512];
			NativeNoise.xoroshiroNextLong(seed, l);
			feed(l);
			int[] n = new int[512];
			NativeNoise.xoroshiroNextInt(seed, 1000003, n);
			feed(n);
			double[] d = new double[512];
			NativeNoise.xoroshiroNextDouble(seed, d);
			feed(d);
			NativeNoise.positionalFromHash(seed, "minecraft:terrain", l);
			feed(l);
			NativeNoise.positionalAt(seed, -123456, 64, 987654, l);
			feed(l);
			double[] out = new double[pts.length / 3];
			NativeNoise.improvedNoise(seed, 0.5, 0.25, pts, out);
			feed(out);
			NativeNoise.perlinNoise(seed, -9, amps, false, pts, out);
			feed(out);
			NativeNoise.perlinNoise(seed, -9, amps, true, pts, out);
			feed(out);
			feed(NativeNoise.normalNoise(seed, "minecraft:continentalness", -9, amps, false, pts, out));
			feed(out);
			double[] bout = new double[blocks.length / 3];
			feed(NativeNoise.blendedNoise(seed, "minecraft:terrain", 1, 1, 80, 160, 4, 8, blocks, bout));
			feed(bout);
		}

		// Density program: every router output at points, chunk fills, full block states.
		byte[] program = Files.readAllBytes(Path.of(args[1]));
		String[] mat = Files.readString(Path.of(args[2])).trim().split("\n");
		int[] mi = java.util.Arrays.stream(mat[0].trim().split(" ")).mapToInt(Integer::parseInt).toArray();
		long[] ml = java.util.Arrays.stream(mat[1].trim().split(" ")).mapToLong(Long::parseLong).toArray();
		double[] mdbl = java.util.Arrays.stream(mat[2].trim().split(" ")).mapToDouble(Double::parseDouble).toArray();
		try (NativeDensity nd = new NativeDensity(program)) {
			double[] out = new double[blocks.length / 3];
			for (int root = 0; root < 15; root++) {
				nd.evalPoints(root, blocks, out);
				feed(out);
			}
			int[][] chunks = {{0, 0}, {-1, -1}, {123, -456}, {-62500, 31250}, {1874999, -1874999}, {2000, -2000}};
			int vol = 16 * 16 * 384;
			for (int[] c : chunks) {
				int[] params = {c[0] * 16, c[1] * 16, -8, 48, 4, 4, 8};
				double[] density = new double[vol];
				double[][] veins = new double[3][vol];
				for (boolean lazy : new boolean[] {false, true}) {
					nd.fillChunk(11, params, null, density, new int[] {12, 13, 14}, veins, lazy);
					feed(density);
					for (double[] v : veins) feed(v);
				}
				byte[] codes = new byte[vol];
				nd.fillChunkStates(11, params, null, mi, ml, mdbl, codes, density);
				md.update(codes);
				values += codes.length;
				feed(density);
			}
		}
		System.out.println("platform " + System.getProperty("os.name") + "/" + System.getProperty("os.arch") + ", java "
				+ System.getProperty("java.version"));
		System.out.println("values " + values);
		System.out.println("digest " + HexFormat.of().formatHex(md.digest()));
	}

	private static void feed(double v) {
		feed(new double[] {v});
	}

	private static void feed(double[] a) {
		ByteBuffer b = ByteBuffer.allocate(a.length * 8).order(ByteOrder.LITTLE_ENDIAN);
		for (double d : a) b.putLong(Double.doubleToRawLongBits(d));
		md.update(b.array());
		values += a.length;
	}

	private static void feed(long[] a) {
		ByteBuffer b = ByteBuffer.allocate(a.length * 8).order(ByteOrder.LITTLE_ENDIAN);
		for (long v : a) b.putLong(v);
		md.update(b.array());
		values += a.length;
	}

	private static void feed(int[] a) {
		ByteBuffer b = ByteBuffer.allocate(a.length * 4).order(ByteOrder.LITTLE_ENDIAN);
		for (int v : a) b.putInt(v);
		md.update(b.array());
		values += a.length;
	}
}
