package rustgen;

/** Implemented by NoiseBasedChunkGenerator (via mixin): what this generator uses for new chunks. */
public interface RustgenGenerator {
	/** Human-readable status, e.g. for the F3 screen. */
	String rustgen$status();
}
