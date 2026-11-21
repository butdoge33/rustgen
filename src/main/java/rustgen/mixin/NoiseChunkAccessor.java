package rustgen.mixin;

import net.minecraft.world.level.levelgen.DensityFunctions;
import net.minecraft.world.level.levelgen.NoiseChunk;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(NoiseChunk.class)
public interface NoiseChunkAccessor {
	/** The structure beardifier chosen when the NoiseChunk was created (biome stage). */
	@Accessor("beardifier")
	DensityFunctions.BeardifierOrMarker rustgen$beardifier();
}
