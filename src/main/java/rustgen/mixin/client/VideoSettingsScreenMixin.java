package rustgen.mixin.client;

import java.util.Arrays;
import net.minecraft.client.Option;
import net.minecraft.client.gui.screens.VideoSettingsScreen;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.ModifyArg;

import rustgen.client.WorldGenOption;

/** Adds "World Generation: Rust / Java" to Video Settings. */
@Mixin(VideoSettingsScreen.class)
public abstract class VideoSettingsScreenMixin {
	@ModifyArg(method = "init", at = @At(value = "INVOKE",
			target = "Lnet/minecraft/client/gui/components/OptionsList;addSmall([Lnet/minecraft/client/Option;)V"))
	private Option[] rustgen$addOption(Option[] options) {
		Option[] extended = Arrays.copyOf(options, options.length + 1);
		extended[options.length] = WorldGenOption.INSTANCE;
		return extended;
	}
}
