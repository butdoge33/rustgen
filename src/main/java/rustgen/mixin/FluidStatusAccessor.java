package rustgen.mixin;

import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.Aquifer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(Aquifer.FluidStatus.class)
public interface FluidStatusAccessor {
	@Accessor("fluidLevel")
	int rustgen$fluidLevel();

	@Accessor("fluidType")
	BlockState rustgen$fluidType();
}
