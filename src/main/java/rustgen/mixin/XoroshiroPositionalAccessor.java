package rustgen.mixin;

import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(XoroshiroRandomSource.XoroshiroPositionalRandomFactory.class)
public interface XoroshiroPositionalAccessor {
	@Accessor("seedLo")
	long rustgen$seedLo();

	@Accessor("seedHi")
	long rustgen$seedHi();
}
