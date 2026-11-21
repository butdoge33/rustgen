package rustgen;

import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.StructureFeatureManager;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.DensityFunctions;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.NoiseSettings;
import net.minecraft.world.level.levelgen.blending.Blender;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import rustgen.mixin.BeardifierAccessor;
import rustgen.mixin.NoiseChunkAccessor;

/**
 * Drop-in replacement for NoiseBasedChunkGenerator.doFill. Density, aquifer and ore veins
 * are computed natively (one call per chunk, returning a block-state code per block); the
 * placement loop (sections, heightmaps, light sources, fluid post-processing) is vanilla's,
 * in vanilla's iteration order.
 */
public final class NativeFill {
	private static final Logger LOG = LoggerFactory.getLogger("rustgen");
	private static final BlockState AIR = Blocks.AIR.defaultBlockState();

	public static final AtomicLong NATIVE_CHUNKS = new AtomicLong();
	public static final AtomicLong VANILLA_CHUNKS = new AtomicLong();
	private static final AtomicBoolean NATIVE_ERROR_LOGGED = new AtomicBoolean();

	private static final ThreadLocal<Buffers> BUFFERS = ThreadLocal.withInitial(Buffers::new);

	private NativeFill() {}

	/** @return true if the chunk was filled natively; false means "run vanilla doFill". */
	public static boolean fill(NativeRouter nr, NoiseRouter router, NoiseGeneratorSettings settings, Aquifer.FluidPicker fluidPicker,
			BlockState defaultBlock, Blender blender, StructureFeatureManager structures, ChunkAccess chunk, int cellNoiseMinY, int cellCountY) {
		// Same call as vanilla doFill. Normally the NoiseChunk already exists (biome stage),
		// carrying the blender and beardifier chosen then.
		NoiseChunk nc = chunk.getOrCreateNoiseChunk(router, () -> BeardifierAccessor.rustgen$create(structures, chunk), settings, fluidPicker, blender);
		if (nc.getBlender() != Blender.empty()) {
			VANILLA_CHUNKS.incrementAndGet(); // blending next to pre-1.18 chunks: not supported natively
			return false;
		}

		NoiseSettings ns = settings.noiseSettings();
		int cellW = ns.getCellWidth(), cellH = ns.getCellHeight();
		int minX = chunk.getPos().getMinBlockX(), minZ = chunk.getPos().getMinBlockZ();
		int minY = cellNoiseMinY * cellH;
		int height = cellCountY * cellH;
		Buffers buf = BUFFERS.get().ensure(16 * 16 * height);
		Ctx ctx = buf.ctx;
		ctx.layout(minX, minY, minZ);

		long tBeard = System.nanoTime();
		double[] beard = beardifier(nc, buf, ctx, height);
		long tNative = System.nanoTime();
		Stats.BEARD_NANOS.add(tNative - tBeard);
		byte[] codes = buf.codes;
		try {
			int[] params = {minX, minZ, cellNoiseMinY, cellCountY, 16 / cellW, cellW, cellH};
			nr.density.fillChunkStates(NativeRouter.FINAL, params, beard, nr.matInts, nr.matLongs, nr.matDoubles, codes, null);
		} catch (RuntimeException e) {
			if (NATIVE_ERROR_LOGGED.compareAndSet(false, true)) {
				LOG.error("[rustgen] native noise fill failed for chunk {}; using vanilla (further errors not logged)", chunk.getPos(), e);
			}
			VANILLA_CHUNKS.incrementAndGet();
			return false;
		}
		long tPlace = System.nanoTime();
		Stats.NATIVE_NANOS.add(tPlace - tNative);

		// --- vanilla doFill placement loop; block states come from the native codes ---
		BlockState[] table = nr.codeStates;
		Heightmap oceanFloor = chunk.getOrCreateHeightmapUnprimed(Heightmap.Types.OCEAN_FLOOR_WG);
		Heightmap worldSurface = chunk.getOrCreateHeightmapUnprimed(Heightmap.Types.WORLD_SURFACE_WG);
		BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();
		boolean voidTerrain = SharedConstants.debugVoidTerrain(chunk.getPos());
		int cellsXZ = 16 / cellW;
		for (int q = 0; q < cellsXZ; q++) {
			for (int r = 0; r < cellsXZ; r++) {
				LevelChunkSection section = chunk.getSection(chunk.getSectionsCount() - 1);
				for (int s = cellCountY - 1; s >= 0; s--) {
					for (int t = cellH - 1; t >= 0; t--) {
						int y = (cellNoiseMinY + s) * cellH + t;
						int ly = y & 15;
						int sectionIndex = chunk.getSectionIndex(y);
						if (chunk.getSectionIndex(section.bottomBlockY()) != sectionIndex) {
							section = chunk.getSection(sectionIndex);
						}
						for (int dx = 0; dx < cellW; dx++) {
							int x = minX + q * cellW + dx;
							int lx = x & 15;
							for (int dz = 0; dz < cellW; dz++) {
								int z = minZ + r * cellW + dz;
								int lz = z & 15;
								int code = codes[((y - minY) * 16 + (z - minZ)) * 16 + (x - minX)];
								BlockState state = table[code & 0x7f];
								if (state != AIR && !voidTerrain) {
									if (state.getLightEmission() != 0 && chunk instanceof ProtoChunk proto) {
										pos.set(x, y, z);
										proto.addLight(pos);
									}
									section.setBlockState(lx, ly, lz, state, false);
									oceanFloor.update(lx, y, lz, state);
									worldSurface.update(lx, y, lz, state);
									if ((code & 0x80) != 0 && !state.getFluidState().isEmpty()) {
										pos.set(x, y, z);
										chunk.markPosForPostprocessing(pos);
									}
								}
							}
						}
					}
				}
			}
		}
		Stats.PLACE_NANOS.add(System.nanoTime() - tPlace);
		NATIVE_CHUNKS.incrementAndGet();
		return true;
	}

	/** Per-block beardifier values, or null when the chunk has no structure pieces nearby. */
	private static double[] beardifier(NoiseChunk nc, Buffers buf, Ctx ctx, int height) {
		DensityFunctions.BeardifierOrMarker b = ((NoiseChunkAccessor) nc).rustgen$beardifier();
		if (!(b instanceof Beardifier bf)) return null; // the marker always yields 0.0
		BeardifierAccessor acc = (BeardifierAccessor) bf;
		if (acc.rustgen$rigids().isEmpty() && acc.rustgen$junctions().isEmpty()) return null; // compute() == 0.0
		double[] out = buf.beard;
		for (int y = ctx.minY; y < ctx.minY + height; y++) {
			for (int z = ctx.minZ; z < ctx.minZ + 16; z++) {
				for (int x = ctx.minX; x < ctx.minX + 16; x++) {
					ctx.set(x, y, z);
					out[ctx.idx] = bf.compute(ctx);
				}
			}
		}
		return out;
	}

	/** Mutable block context; {@link #idx} is the index into the native arrays. */
	static final class Ctx implements DensityFunction.FunctionContext {
		int minX, minY, minZ, x, y, z, idx;

		void layout(int minX, int minY, int minZ) {
			this.minX = minX;
			this.minY = minY;
			this.minZ = minZ;
		}

		void set(int x, int y, int z) {
			this.x = x;
			this.y = y;
			this.z = z;
			this.idx = ((y - minY) * 16 + (z - minZ)) * 16 + (x - minX);
		}

		@Override public int blockX() { return x; }
		@Override public int blockY() { return y; }
		@Override public int blockZ() { return z; }
	}

	/** Per-thread scratch arrays, reused across chunks (worldgen runs on a fixed thread pool). */
	static final class Buffers {
		final Ctx ctx = new Ctx();
		double[] beard = new double[0];
		byte[] codes = new byte[0];

		Buffers ensure(int volume) {
			if (codes.length != volume) {
				beard = new double[volume];
				codes = new byte[volume];
			}
			return this;
		}
	}
}
