package rustgen.mixin;

import net.minecraft.core.Holder;
import net.minecraft.world.level.StructureFeatureManager;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.blending.Blender;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.Unique;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import rustgen.NativeFill;
import rustgen.NativeRouter;
import rustgen.RustgenConfig;
import rustgen.RustgenGenerator;
import rustgen.StageStats;
import rustgen.Stats;
import rustgen.debug.FillHashes;

/**
 * Replaces the noise sampling in doFill with the native fill when the user selected Rust
 * world generation and the native path can reproduce vanilla for this generator/chunk.
 * Otherwise the injection returns without cancelling and vanilla runs.
 */
@Mixin(NoiseBasedChunkGenerator.class)
public abstract class NoiseBasedChunkGeneratorMixin implements RustgenGenerator {
	@Shadow @Final protected BlockState defaultBlock;
	@Shadow @Final protected Holder<NoiseGeneratorSettings> settings;
	@Shadow @Final private NoiseRouter router;
	@Shadow @Final private Aquifer.FluidPicker globalFluidPicker;

	@Unique private volatile boolean rustgen$initialized;
	@Unique private NativeRouter rustgen$native;
	/** Why the native path is unavailable for this generator (null if available). */
	@Unique private volatile String rustgen$why;

	@Unique private static final ThreadLocal<long[]> rustgen$start = ThreadLocal.withInitial(() -> new long[1]);

	@Inject(method = "doFill", at = @At("HEAD"), cancellable = true)
	private void rustgen$nativeFill(Blender blender, StructureFeatureManager structures, ChunkAccess chunk, int cellNoiseMinY, int cellCountY,
			CallbackInfoReturnable<ChunkAccess> cir) {
		long t0 = System.nanoTime();
		if (RustgenConfig.useRust()) {
			NativeRouter nr = rustgen$router();
			if (nr != null && NativeFill.fill(nr, router, settings.value(), globalFluidPicker, defaultBlock, blender, structures, chunk, cellNoiseMinY, cellCountY)) {
				long dt = System.nanoTime() - t0;
				Stats.DOFILL_NANOS.add(dt);
				Stats.DOFILL_COUNT.increment();
				StageStats.RUST_FILLS.increment();
				StageStats.RUST_FILL_NANOS.add(dt);
				FillHashes.record(chunk);
				cir.setReturnValue(chunk);
				return;
			}
		}
		rustgen$start.get()[0] = t0; // vanilla path; timed at RETURN
	}

	@Inject(method = "doFill", at = @At("RETURN"))
	private void rustgen$timeVanilla(Blender blender, StructureFeatureManager structures, ChunkAccess chunk, int cellNoiseMinY, int cellCountY,
			CallbackInfoReturnable<ChunkAccess> cir) {
		long dt = System.nanoTime() - rustgen$start.get()[0];
		Stats.DOFILL_NANOS.add(dt);
		Stats.DOFILL_COUNT.increment();
		StageStats.JAVA_FILLS.increment();
		StageStats.JAVA_FILL_NANOS.add(dt);
		FillHashes.record(chunk);
	}

	@Unique
	private NativeRouter rustgen$router() {
		if (!rustgen$initialized) {
			synchronized (this) {
				if (!rustgen$initialized) {
					String name = settings.unwrapKey().map(k -> k.location().toString()).orElse("<inline settings>");
					String[] why = new String[1];
					rustgen$native = NativeRouter.create(this, router, settings.value(), globalFluidPicker, name, why);
					rustgen$why = rustgen$native == null ? why[0] : null;
					rustgen$initialized = true; // volatile write publishes the fields above
				}
			}
		}
		return rustgen$native;
	}

	@Override
	public String rustgen$status() {
		if (!RustgenConfig.useRust()) return "Generating chunks with Java world-generation (selected in Video Settings)";
		if (!rustgen$initialized) return "Rust world-generation selected (starts with the first new chunk)";
		if (rustgen$native == null) return "Generating chunks with Java world-generation (Rust unavailable: " + rustgen$why + ")";
		return "Generating chunks with Rust world-generation";
	}
}
