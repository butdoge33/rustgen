package rustgen.mixin;

import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(NormalNoise.class)
public interface NormalNoiseAccessor {
	@Accessor("valueFactor")
	double rustgen$valueFactor();

	@Accessor("first")
	PerlinNoise rustgen$first();

	@Accessor("second")
	PerlinNoise rustgen$second();
}
