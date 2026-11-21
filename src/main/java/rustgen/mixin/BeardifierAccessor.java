package rustgen.mixin;

import it.unimi.dsi.fastutil.objects.ObjectList;
import net.minecraft.world.level.StructureFeatureManager;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.structure.pools.JigsawJunction;
import net.minecraft.world.level.levelgen.structure.StructurePiece;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;
import org.spongepowered.asm.mixin.gen.Invoker;

@Mixin(Beardifier.class)
public interface BeardifierAccessor {
	@Accessor("rigids")
	ObjectList<StructurePiece> rustgen$rigids();

	@Accessor("junctions")
	ObjectList<JigsawJunction> rustgen$junctions();

	/** Same constructor vanilla's doFill supplier uses. */
	@Invoker("<init>")
	static Beardifier rustgen$create(StructureFeatureManager structures, ChunkAccess chunk) {
		throw new AssertionError("mixin not applied");
	}
}
