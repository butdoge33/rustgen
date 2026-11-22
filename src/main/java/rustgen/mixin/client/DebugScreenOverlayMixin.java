package rustgen.mixin.client;

import java.util.List;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.DebugScreenOverlay;
import net.minecraft.client.server.IntegratedServer;
import net.minecraft.server.level.ServerLevel;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import rustgen.RustgenGenerator;

/**
 * F3: one line telling which world generation creates new chunks in the current dimension.
 * Singleplayer only — on a remote server, generation happens server-side.
 */
@Mixin(DebugScreenOverlay.class)
public abstract class DebugScreenOverlayMixin {
	@Shadow @Final private Minecraft minecraft;

	@Inject(method = "getGameInformation", at = @At("RETURN"))
	private void rustgen$worldgenLine(CallbackInfoReturnable<List<String>> cir) {
		IntegratedServer server = minecraft.getSingleplayerServer();
		if (server == null || minecraft.level == null) return;
		ServerLevel level = server.getLevel(minecraft.level.dimension());
		if (level == null) return;
		String line = level.getChunkSource().getGenerator() instanceof RustgenGenerator g
				? g.rustgen$status()
				: "Generating chunks with Java world-generation (non-noise generator)";
		cir.getReturnValue().add("");
		cir.getReturnValue().add(line);
	}
}
