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
import rustgen.Stats;

/**
 * Replaces the noise sampling in doFill with the native fill when possible. If anything
 * is unsupported for this chunk, the injection returns without cancelling and vanilla runs.
 */
@Mixin(NoiseBasedChunkGenerator.class)
public abstract class NoiseBasedChunkGeneratorMixin {
	@Shadow @Final protected BlockState defaultBlock;
	@Shadow @Final protected Holder<NoiseGeneratorSettings> settings;
	@Shadow @Final private NoiseRouter router;
	@Shadow @Final private Aquifer.FluidPicker globalFluidPicker;

	@Unique private volatile boolean rustgen$initialized;
	@Unique private NativeRouter rustgen$native;

	@Unique private static final ThreadLocal<long[]> rustgen$start = ThreadLocal.withInitial(() -> new long[1]);

	@Inject(method = "doFill", at = @At("HEAD"), cancellable = true)
	private void rustgen$nativeFill(Blender blender, StructureFeatureManager structures, ChunkAccess chunk, int cellNoiseMinY, int cellCountY,
			CallbackInfoReturnable<ChunkAccess> cir) {
		long t0 = System.nanoTime();
		NativeRouter nr = rustgen$router();
		if (nr != null && NativeFill.fill(nr, router, settings.value(), globalFluidPicker, defaultBlock, blender, structures, chunk, cellNoiseMinY, cellCountY)) {
			Stats.DOFILL_NANOS.add(System.nanoTime() - t0);
			Stats.DOFILL_COUNT.increment();
			cir.setReturnValue(chunk);
		} else {
			rustgen$start.get()[0] = t0; // vanilla path; timed at RETURN
		}
	}

	@Inject(method = "doFill", at = @At("RETURN"))
	private void rustgen$timeVanilla(Blender blender, StructureFeatureManager structures, ChunkAccess chunk, int cellNoiseMinY, int cellCountY,
			CallbackInfoReturnable<ChunkAccess> cir) {
		Stats.DOFILL_NANOS.add(System.nanoTime() - rustgen$start.get()[0]);
		Stats.DOFILL_COUNT.increment();
	}

	@Unique
	private NativeRouter rustgen$router() {
		if (!rustgen$initialized) {
			synchronized (this) {
				if (!rustgen$initialized) {
					String name = settings.unwrapKey().map(k -> k.location().toString()).orElse("<inline settings>");
					rustgen$native = NativeRouter.create(this, router, settings.value(), globalFluidPicker, name);
					rustgen$initialized = true; // volatile write publishes rustgen$native
				}
			}
		}
		return rustgen$native;
	}
}
