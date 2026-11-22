package rustgen.mixin.client;

import java.util.Arrays;
import net.minecraft.client.CycleOption;
import net.minecraft.client.Option;
import net.minecraft.client.gui.screens.VideoSettingsScreen;
import net.minecraft.network.chat.TranslatableComponent;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Unique;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.ModifyArg;

import rustgen.RustgenConfig;

/** Adds "World Generation: Rust / Java" to Video Settings. */
@Mixin(VideoSettingsScreen.class)
public abstract class VideoSettingsScreenMixin {
	@Unique
	private static final CycleOption<Boolean> RUSTGEN_WORLDGEN = CycleOption.createBinaryOption(
			"options.rustgen.worldgen",
			new TranslatableComponent("options.rustgen.worldgen.rust"),
			new TranslatableComponent("options.rustgen.worldgen.java"),
			options -> RustgenConfig.useRust(),
			(options, option, value) -> RustgenConfig.setUseRust(value))
		.setTooltip(minecraft -> value -> minecraft.font.split(
			new TranslatableComponent(value ? "options.rustgen.worldgen.rust.tooltip" : "options.rustgen.worldgen.java.tooltip"), 200));

	@ModifyArg(method = "init", at = @At(value = "INVOKE",
			target = "Lnet/minecraft/client/gui/components/OptionsList;addSmall([Lnet/minecraft/client/Option;)V"))
	private Option[] rustgen$addOption(Option[] options) {
		Option[] extended = Arrays.copyOf(options, options.length + 1);
		extended[options.length] = RUSTGEN_WORLDGEN;
		return extended;
	}
}
