package rustgen.mixin;

import com.mojang.datafixers.util.Either;
import java.util.concurrent.CompletableFuture;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkStatus;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.Unique;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import rustgen.StageStats;

/** Times every chunk generation stage (start until its future completes). */
@Mixin(ChunkStatus.class)
public abstract class ChunkStatusMixin {
	@Shadow
	public abstract String getName();

	@Unique private static final ThreadLocal<long[]> rustgen$start = ThreadLocal.withInitial(() -> new long[1]);

	@Inject(method = "generate", at = @At("HEAD"))
	private void rustgen$startStage(CallbackInfoReturnable<CompletableFuture<Either<ChunkAccess, ChunkHolder.ChunkLoadingFailure>>> cir) {
		rustgen$start.get()[0] = System.nanoTime();
	}

	@Inject(method = "generate", at = @At("RETURN"))
	private void rustgen$endStage(CallbackInfoReturnable<CompletableFuture<Either<ChunkAccess, ChunkHolder.ChunkLoadingFailure>>> cir) {
		long t0 = rustgen$start.get()[0];
		String name = getName();
		cir.getReturnValue().whenComplete((result, error) -> StageStats.stage(name, System.nanoTime() - t0));
	}
}
