package rustgen.mixin;

import it.unimi.dsi.fastutil.doubles.DoubleList;
import net.minecraft.world.level.levelgen.synth.ImprovedNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(PerlinNoise.class)
public interface PerlinNoiseAccessor {
	@Accessor("noiseLevels")
	ImprovedNoise[] rustgen$noiseLevels();

	@Accessor("amplitudes")
	DoubleList rustgen$amplitudes();

	@Accessor("lowestFreqInputFactor")
	double rustgen$lowestFreqInputFactor();

	@Accessor("lowestFreqValueFactor")
	double rustgen$lowestFreqValueFactor();

	@Accessor("maxValue")
	double rustgen$maxValue();
}
