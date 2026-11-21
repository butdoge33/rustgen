package rustgen.debug;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.locks.LockSupport;

import com.mojang.datafixers.util.Either;

import it.unimi.dsi.fastutil.shorts.ShortList;
import net.fabricmc.fabric.api.event.lifecycle.v1.ServerLifecycleEvents;
import net.minecraft.core.BlockPos;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ServerChunkCache;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkStatus;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.levelgen.Heightmap;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import rustgen.NativeFill;
import rustgen.NativeLib;
import rustgen.mixin.ServerChunkCacheInvoker;

/**
 * Debug harness: {@code -Drustgen.pregen=R} generates a (2R+1)^2 chunk area of the overworld
 * up to the NOISE status right after server start, hashes everything doFill produces
 * (block states, WG heightmaps, fluid post-processing marks, light sources), writes
 * {@code rustgen-pregen.txt} to the run directory, logs timing, then stops the server.
 *
 * Run once natively and once with {@code -Drustgen.disable=true}; identical files mean
 * the native path produced exactly vanilla's chunks.
 */
public final class PregenHarness {
	private static final Logger LOG = LoggerFactory.getLogger("rustgen");

	private PregenHarness() {}

	public static void register() {
		ServerLifecycleEvents.SERVER_STARTED.register(PregenHarness::run);
	}

	private static void run(MinecraftServer server) {
		int radius = Integer.getInteger("rustgen.pregen");
		// Far from spawn so the already-generated spawn area doesn't skew timing.
		int cx = Integer.getInteger("rustgen.pregen.centerX", 2000);
		int cz = Integer.getInteger("rustgen.pregen.centerZ", -2000);
		ServerLevel level = server.overworld();
		ServerChunkCache cache = level.getChunkSource();
		ServerChunkCacheInvoker inv = (ServerChunkCacheInvoker) cache;
		long native0 = NativeFill.NATIVE_CHUNKS.get(), vanilla0 = NativeFill.VANILLA_CHUNKS.get();

		LOG.info("[rustgen] pregen: {} chunks around {},{} (native library {})", (2 * radius + 1) * (2 * radius + 1), cx, cz,
				NativeLib.isAvailable() ? "ON" : "OFF");
		rustgen.Stats.reset();
		long t0 = System.nanoTime();
		List<int[]> positions = new ArrayList<>();
		List<CompletableFuture<Either<ChunkAccess, ChunkHolder.ChunkLoadingFailure>>> futures = new ArrayList<>();
		for (int x = cx - radius; x <= cx + radius; x++) {
			for (int z = cz - radius; z <= cz + radius; z++) {
				positions.add(new int[] {x, z});
				futures.add(inv.rustgen$getChunkFutureMainThread(x, z, ChunkStatus.NOISE, true));
			}
		}
		CompletableFuture<Void> all = CompletableFuture.allOf(futures.toArray(CompletableFuture[]::new));
		// Like ServerChunkCache.getChunk's managedBlock, but for all chunks at once. No server
		// ticks run meanwhile, so the temporary tickets can't expire.
		while (!all.isDone()) {
			if (!cache.pollTask()) LockSupport.parkNanos(100_000);
		}
		long t1 = System.nanoTime();

		StringBuilder out = new StringBuilder();
		for (int i = 0; i < futures.size(); i++) {
			int[] p = positions.get(i);
			ChunkAccess chunk = futures.get(i).join().left()
					.orElseThrow(() -> new IllegalStateException("chunk " + p[0] + "," + p[1] + " failed to generate"));
			out.append(p[0]).append(' ').append(p[1]).append(' ').append(Long.toHexString(hash(chunk))).append('\n');
		}
		Path file = server.getServerDirectory().toPath().resolve("rustgen-pregen.txt");
		try {
			Files.writeString(file, out);
		} catch (IOException e) {
			throw new RuntimeException(e);
		}

		double secs = (t1 - t0) / 1e9;
		LOG.info("[rustgen] pregen: {} chunks to NOISE in {} s ({} chunks/s); noise fill native={} vanilla={}; hashes -> {}",
				futures.size(), String.format("%.2f", secs), String.format("%.1f", futures.size() / secs),
				NativeFill.NATIVE_CHUNKS.get() - native0, NativeFill.VANILLA_CHUNKS.get() - vanilla0, file.toAbsolutePath());
		LOG.info("[rustgen] pregen: {}", rustgen.Stats.summary());
		server.halt(false);
	}

	/** FNV-1a over everything doFill writes. */
	static long hash(ChunkAccess chunk) {
		long h = 0xcbf29ce484222325L;
		for (LevelChunkSection section : chunk.getSections()) {
			for (int y = 0; y < 16; y++) {
				for (int z = 0; z < 16; z++) {
					for (int x = 0; x < 16; x++) {
						h = mix(h, Block.BLOCK_STATE_REGISTRY.getId(section.getBlockState(x, y, z)));
					}
				}
			}
		}
		for (Heightmap.Types type : new Heightmap.Types[] {Heightmap.Types.OCEAN_FLOOR_WG, Heightmap.Types.WORLD_SURFACE_WG}) {
			for (long v : chunk.getOrCreateHeightmapUnprimed(type).getRawData()) h = mix(h, v);
		}
		ShortList[] post = chunk.getPostProcessing();
		for (int i = 0; i < post.length; i++) {
			if (post[i] == null) continue;
			h = mix(h, 1000 + i);
			for (int j = 0; j < post[i].size(); j++) h = mix(h, post[i].getShort(j));
		}
		for (BlockPos light : (Iterable<BlockPos>) chunk.getLights()::iterator) h = mix(h, light.asLong());
		return h;
	}

	private static long mix(long h, long v) {
		for (int i = 0; i < 8; i++) {
			h ^= (v >>> (i * 8)) & 0xff;
			h *= 0x100000001b3L;
		}
		return h;
	}
}
