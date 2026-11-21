package rustgen.mixin;

import net.minecraft.world.level.levelgen.synth.BlendedNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(BlendedNoise.class)
public interface BlendedNoiseAccessor {
	@Accessor("minLimitNoise")
	PerlinNoise rustgen$minLimitNoise();

	@Accessor("maxLimitNoise")
	PerlinNoise rustgen$maxLimitNoise();

	@Accessor("mainNoise")
	PerlinNoise rustgen$mainNoise();

	@Accessor("xzScale")
	double rustgen$xzScale();

	@Accessor("yScale")
	double rustgen$yScale();

	@Accessor("xzMainScale")
	double rustgen$xzMainScale();

	@Accessor("yMainScale")
	double rustgen$yMainScale();

	@Accessor("cellWidth")
	int rustgen$cellWidth();

	@Accessor("cellHeight")
	int rustgen$cellHeight();
}
