package rustgen.translate;

import java.io.ByteArrayOutputStream;
import java.lang.reflect.Method;
import java.lang.reflect.RecordComponent;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.util.CubicSpline;
import net.minecraft.util.StringRepresentable;
import net.minecraft.world.level.biome.TerrainShaper;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.DensityFunctions;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.NoiseSettings;
import net.minecraft.world.level.levelgen.NoiseSlider;
import net.minecraft.world.level.levelgen.synth.BlendedNoise;
import net.minecraft.world.level.levelgen.synth.ImprovedNoise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;

import rustgen.mixin.BlendedNoiseAccessor;
import rustgen.mixin.ImprovedNoiseAccessor;
import rustgen.mixin.NormalNoiseAccessor;
import rustgen.mixin.PerlinNoiseAccessor;

/**
 * Serializes a seeded vanilla {@link NoiseRouter} into the native program format
 * documented in native/src/density.rs.
 *
 * <p>Node types are identified by their {@code DENSITY_FUNCTION_TYPES} registry name,
 * and record fields are read by component <em>index</em>. Both are stable across
 * obfuscated (intermediary) and development (Mojang) names, so this works in production.
 *
 * <p>Any type this translator does not understand (e.g. one added by a mod) makes the
 * affected root "unsupported"; the caller must then use vanilla for that root.
 */
public final class DensityTranslator {
	public static final int FORMAT_VERSION = 1;

	/** Program root order. Index into this array == root index on the native side. */
	public static final String[] ROOT_NAMES = {
		"barrier", "fluid_level_floodedness", "fluid_level_spread", "lava",
		"temperature", "vegetation", "continents", "erosion", "depth", "ridges",
		"initial_density_without_jaggedness", "final_density",
		"vein_toggle", "vein_ridged", "vein_gap",
	};

	public static DensityFunction[] roots(NoiseRouter r) {
		return new DensityFunction[] {
			r.barrierNoise(), r.fluidLevelFloodednessNoise(), r.fluidLevelSpreadNoise(), r.lavaNoise(),
			r.temperature(), r.humidity(), r.continents(), r.erosion(), r.depth(), r.ridges(),
			r.initialDensityWithoutJaggedness(), r.finalDensity(),
			r.veinToggle(), r.veinRidged(), r.veinGap(),
		};
	}

	/**
	 * @param program     serialized program for {@code NativeDensity}
	 * @param unsupported per root: {@code null} if translated, otherwise the reason it was not
	 */
	public record Result(byte[] program, String[] unsupported) {
		public boolean isSupported(int root) {
			return unsupported[root] == null;
		}
	}

	public static final class UnsupportedException extends RuntimeException {
		public UnsupportedException(String message) {
			super(message, null, false, false);
		}
	}

	// Opcodes — must match `fn node` in density.rs.
	private static final int OP_CONSTANT = 0, OP_ADD = 1, OP_MUL = 2, OP_MIN = 3, OP_MAX = 4,
			OP_ADD_CONST = 5, OP_MUL_CONST = 6, OP_MAPPED = 7, OP_CLAMP = 8, OP_MARKER = 9,
			OP_NOISE = 10, OP_SHIFT_A = 11, OP_SHIFT_B = 12, OP_SHIFT = 13, OP_SHIFTED_NOISE = 14,
			OP_RANGE_CHOICE = 15, OP_SLIDE = 16, OP_SPLINE = 17, OP_TERRAIN_SHAPER_SPLINE = 18,
			OP_WEIRD_SCALED = 19, OP_Y_CLAMPED_GRADIENT = 20, OP_BLEND_ALPHA = 21, OP_BLEND_OFFSET = 22,
			OP_BEARDIFIER = 23, OP_BLEND_DENSITY = 24, OP_OLD_BLENDED_NOISE = 25;

	private static final List<String> MAPPED = List.of("abs", "square", "cube", "half_negative", "quarter_negative", "squeeze");
	private static final List<String> MARKERS = List.of("interpolated", "flat_cache", "cache_2d", "cache_once", "cache_all_in_cell");

	private final Out noises = new Out();
	private final Out blended = new Out();
	private final Out items = new Out();
	private int noiseCount, blendedCount, itemCount, nodeCount, splineCount;
	private final Map<Object, Integer> noiseIds = new IdentityHashMap<>();
	private final Map<Object, Integer> blendedIds = new IdentityHashMap<>();
	private final Map<Object, Integer> nodeIds = new IdentityHashMap<>();
	private final Map<Object, Integer> splineIds = new IdentityHashMap<>();

	private DensityTranslator() {}

	public static Result translate(NoiseRouter router) {
		DensityTranslator t = new DensityTranslator();
		DensityFunction[] roots = roots(router);
		int[] rootIds = new int[roots.length];
		String[] unsupported = new String[roots.length];
		for (int i = 0; i < roots.length; i++) {
			try {
				rootIds[i] = t.node(roots[i]);
			} catch (UnsupportedException e) {
				// Anything emitted before the failure is still a valid (unreferenced) item.
				rootIds[i] = -1;
				unsupported[i] = e.getMessage();
			}
		}

		Out out = new Out();
		out.i32(Integer.reverseBytes(0x52474446)); // "RGDF" read as LE u32
		out.i32(FORMAT_VERSION);
		out.i32(t.noiseCount).bytes(t.noises);
		out.i32(t.blendedCount).bytes(t.blended);
		out.i32(t.itemCount).bytes(t.items);
		out.i32(roots.length);
		for (int id : rootIds) out.i32(id);
		return new Result(out.toByteArray(), unsupported);
	}

	// ------------------------------------------------------------------ density functions

	private int node(DensityFunction f) {
		Integer id = nodeIds.get(f);
		if (id != null) return id;
		int r = translate(f);
		nodeIds.put(f, r);
		return r;
	}

	private int translate(DensityFunction f) {
		// HolderHolder: transparent reference to a registered function. Its codec() throws,
		// so detect it structurally: a record whose only component is a Holder.
		if (f instanceof Record && components(f.getClass()).length == 1
				&& Holder.class.isAssignableFrom(components(f.getClass())[0].getType())) {
			return node((DensityFunction) ((Holder<?>) comp(f, 0)).value());
		}
		if (f instanceof BlendedNoise b) {
			int id = blendedNoise(b);
			return emit(f, OP_OLD_BLENDED_NOISE, o -> o.i32(id));
		}

		ResourceLocation key = Registry.DENSITY_FUNCTION_TYPES.getKey(f.codec());
		if (key == null || !key.getNamespace().equals("minecraft")) {
			throw new UnsupportedException("unsupported density function type " + key + " (" + f.getClass().getName() + ")");
		}
		String type = key.getPath();

		switch (type) {
			case "constant" -> {
				double v = (double) comp(f, 0);
				return emit(f, OP_CONSTANT, o -> o.f64(v));
			}
			case "add", "mul" -> {
				boolean add = type.equals("add");
				if (components(f.getClass())[2].getType() == double.class) {
					// MulOrAdd(specificType, input, minValue, maxValue, argument)
					int input = node((DensityFunction) comp(f, 1));
					double arg = (double) comp(f, 4);
					return emit(f, add ? OP_ADD_CONST : OP_MUL_CONST, o -> o.i32(input).f64(arg));
				}
				// Ap2(type, argument1, argument2, minValue, maxValue)
				int a = node((DensityFunction) comp(f, 1));
				int b = node((DensityFunction) comp(f, 2));
				return emit(f, add ? OP_ADD : OP_MUL, o -> o.i32(a).i32(b));
			}
			case "min", "max" -> {
				int a = node((DensityFunction) comp(f, 1));
				int b = node((DensityFunction) comp(f, 2));
				return emit(f, type.equals("min") ? OP_MIN : OP_MAX, o -> o.i32(a).i32(b));
			}
			case "abs", "square", "cube", "half_negative", "quarter_negative", "squeeze" -> {
				// Mapped(type, input, minValue, maxValue)
				int kind = MAPPED.indexOf(type);
				int input = node((DensityFunction) comp(f, 1));
				return emit(f, OP_MAPPED, o -> o.u8(kind).i32(input));
			}
			case "interpolated", "flat_cache", "cache_2d", "cache_once", "cache_all_in_cell" -> {
				// Marker(type, wrapped)
				int kind = MARKERS.indexOf(type);
				int input = node((DensityFunction) comp(f, 1));
				return emit(f, OP_MARKER, o -> o.u8(kind).i32(input));
			}
			case "clamp" -> {
				int input = node((DensityFunction) comp(f, 0));
				double lo = (double) comp(f, 1), hi = (double) comp(f, 2);
				return emit(f, OP_CLAMP, o -> o.i32(input).f64(lo).f64(hi));
			}
			case "noise" -> {
				// Noise(noiseData, noise, xzScale, yScale)
				int n = normalNoise((NormalNoise) comp(f, 1));
				double xz = (double) comp(f, 2), y = (double) comp(f, 3);
				return emit(f, OP_NOISE, o -> o.i32(n).f64(xz).f64(y));
			}
			case "shift_a", "shift_b", "shift" -> {
				// Shift*(noiseData, offsetNoise)
				int n = normalNoise((NormalNoise) comp(f, 1));
				int op = type.equals("shift_a") ? OP_SHIFT_A : type.equals("shift_b") ? OP_SHIFT_B : OP_SHIFT;
				return emit(f, op, o -> o.i32(n));
			}
			case "shifted_noise" -> {
				// ShiftedNoise(shiftX, shiftY, shiftZ, xzScale, yScale, noiseData, noise)
				int sx = node((DensityFunction) comp(f, 0));
				int sy = node((DensityFunction) comp(f, 1));
				int sz = node((DensityFunction) comp(f, 2));
				double xz = (double) comp(f, 3), y = (double) comp(f, 4);
				int n = normalNoise((NormalNoise) comp(f, 6));
				return emit(f, OP_SHIFTED_NOISE, o -> o.i32(sx).i32(sy).i32(sz).f64(xz).f64(y).i32(n));
			}
			case "range_choice" -> {
				// RangeChoice(input, minInclusive, maxExclusive, whenInRange, whenOutOfRange)
				int input = node((DensityFunction) comp(f, 0));
				double lo = (double) comp(f, 1), hi = (double) comp(f, 2);
				int in = node((DensityFunction) comp(f, 3));
				int out = node((DensityFunction) comp(f, 4));
				return emit(f, OP_RANGE_CHOICE, o -> o.i32(input).f64(lo).f64(hi).i32(in).i32(out));
			}
			case "slide" -> {
				// Slide(settings, input)
				NoiseSettings s = (NoiseSettings) comp(f, 0);
				int input = node((DensityFunction) comp(f, 1));
				return emit(f, OP_SLIDE, o -> {
					o.i32(input);
					if (s == null) {
						o.u8(0);
					} else {
						o.u8(1).i32(s.getCellHeight()).i32(s.getMinCellY()).i32(s.getCellCountY());
						slider(o, s.topSlideSettings());
						slider(o, s.bottomSlideSettings());
					}
				});
			}
			case "spline" -> {
				DensityFunctions.Spline sp = (DensityFunctions.Spline) f;
				int s = spline(sp.spline());
				double lo = sp.minValue(), hi = sp.maxValue();
				return emit(f, OP_SPLINE, o -> o.i32(s).f64(lo).f64(hi));
			}
			case "terrain_shaper_spline" -> {
				DensityFunctions.TerrainShaperSpline ts = (DensityFunctions.TerrainShaperSpline) f;
				int c = node(ts.continentalness());
				int e = node(ts.erosion());
				int w = node(ts.weirdness());
				TerrainShaper shaper = ts.shaper();
				String kind = ts.spline().getSerializedName();
				int s;
				if (shaper == null) {
					s = -1;
				} else {
					s = switch (kind) {
						case "offset" -> spline(shaper.offsetSampler());
						case "factor" -> spline(shaper.factorSampler());
						case "jaggedness" -> spline(shaper.jaggednessSampler());
						default -> throw new UnsupportedException("unknown terrain shaper spline " + kind);
					};
				}
				boolean addOffset = kind.equals("offset");
				double lo = ts.minValue(), hi = ts.maxValue();
				return emit(f, OP_TERRAIN_SHAPER_SPLINE, o -> {
					o.i32(c).i32(e).i32(w);
					if (s < 0) o.u8(0); else o.u8(1).i32(s);
					o.u8(addOffset ? 1 : 0).f64(lo).f64(hi);
				});
			}
			case "weird_scaled_sampler" -> {
				// WeirdScaledSampler(input, noiseData, noise, rarityValueMapper)
				int input = node((DensityFunction) comp(f, 0));
				int n = normalNoise((NormalNoise) comp(f, 2));
				String mapper = ((StringRepresentable) comp(f, 3)).getSerializedName();
				int m = switch (mapper) {
					case "type_1" -> 1;
					case "type_2" -> 2;
					default -> throw new UnsupportedException("unknown rarity mapper " + mapper);
				};
				return emit(f, OP_WEIRD_SCALED, o -> o.i32(input).i32(n).u8(m));
			}
			case "y_clamped_gradient" -> {
				int fromY = (int) comp(f, 0), toY = (int) comp(f, 1);
				double fromV = (double) comp(f, 2), toV = (double) comp(f, 3);
				return emit(f, OP_Y_CLAMPED_GRADIENT, o -> o.i32(fromY).i32(toY).f64(fromV).f64(toV));
			}
			case "blend_alpha" -> {
				return emit(f, OP_BLEND_ALPHA, o -> {});
			}
			case "blend_offset" -> {
				return emit(f, OP_BLEND_OFFSET, o -> {});
			}
			case "beardifier" -> {
				return emit(f, OP_BEARDIFIER, o -> {});
			}
			case "blend_density" -> {
				int input = node((DensityFunction) comp(f, 0));
				return emit(f, OP_BLEND_DENSITY, o -> o.i32(input));
			}
			default -> throw new UnsupportedException("unsupported density function type minecraft:" + type);
		}
	}

	/** Writes one node item. Children must already be emitted (callers compute them first). */
	private int emit(DensityFunction f, int op, java.util.function.Consumer<Out> payload) {
		Out o = new Out();
		o.u8(0).u8(op).f64(f.minValue()).f64(f.maxValue());
		payload.accept(o);
		items.bytes(o);
		itemCount++;
		return nodeCount++;
	}

	private static void slider(Out o, NoiseSlider s) {
		o.f64(s.target()).i32(s.size()).i32(s.offset());
	}

	// ------------------------------------------------------------------ splines

	private int spline(CubicSpline<?> s) {
		Integer id = splineIds.get(s);
		if (id != null) return id;
		Out o = new Out();
		if (s instanceof CubicSpline.Constant<?> c) {
			o.u8(1).u8(0).f32(c.value());
		} else if (s instanceof CubicSpline.Multipoint<?> m) {
			Object coord = m.coordinate();
			int coordKind;
			int coordNode = -1;
			if (coord instanceof TerrainShaper.CoordinateCustom cc) {
				coordKind = 4;
				coordNode = node(cc.function().value());
			} else if (coord instanceof StringRepresentable sr) {
				coordKind = switch (sr.getSerializedName()) {
					case "continents" -> 0;
					case "erosion" -> 1;
					case "ridges" -> 2;
					case "weirdness" -> 3;
					default -> throw new UnsupportedException("unknown spline coordinate " + sr.getSerializedName());
				};
			} else {
				throw new UnsupportedException("unsupported spline coordinate " + coord.getClass().getName());
			}
			int n = m.locations().length;
			int[] children = new int[n];
			for (int i = 0; i < n; i++) children[i] = spline(m.values().get(i));

			o.u8(1).u8(1).u8(coordKind);
			if (coordKind == 4) o.i32(coordNode);
			o.i32(n);
			for (float v : m.locations()) o.f32(v);
			for (float v : m.derivatives()) o.f32(v);
			for (int c : children) o.i32(c);
		} else {
			throw new UnsupportedException("unsupported spline type " + s.getClass().getName());
		}
		items.bytes(o);
		itemCount++;
		int r = splineCount++;
		splineIds.put(s, r);
		return r;
	}

	// ------------------------------------------------------------------ noise snapshots

	/** Returns the noise index, or -1 for an unseeded (null) noise. */
	private int normalNoise(NormalNoise n) {
		if (n == null) return -1;
		Integer id = noiseIds.get(n);
		if (id != null) return id;
		NormalNoiseAccessor a = (NormalNoiseAccessor) n;
		noises.f64(a.rustgen$valueFactor()).f64(n.maxValue());
		perlin(noises, a.rustgen$first());
		perlin(noises, a.rustgen$second());
		int r = noiseCount++;
		noiseIds.put(n, r);
		return r;
	}

	private int blendedNoise(BlendedNoise b) {
		Integer id = blendedIds.get(b);
		if (id != null) return id;
		BlendedNoiseAccessor a = (BlendedNoiseAccessor) b;
		perlin(blended, a.rustgen$minLimitNoise());
		perlin(blended, a.rustgen$maxLimitNoise());
		perlin(blended, a.rustgen$mainNoise());
		blended.f64(a.rustgen$xzScale()).f64(a.rustgen$yScale()).f64(a.rustgen$xzMainScale()).f64(a.rustgen$yMainScale())
				.i32(a.rustgen$cellWidth()).i32(a.rustgen$cellHeight()).f64(b.maxValue());
		int r = blendedCount++;
		blendedIds.put(b, r);
		return r;
	}

	private static void perlin(Out o, PerlinNoise p) {
		PerlinNoiseAccessor a = (PerlinNoiseAccessor) p;
		ImprovedNoise[] levels = a.rustgen$noiseLevels();
		var amps = a.rustgen$amplitudes();
		o.f64(a.rustgen$lowestFreqInputFactor()).f64(a.rustgen$lowestFreqValueFactor()).f64(a.rustgen$maxValue());
		o.i32(levels.length);
		for (int i = 0; i < levels.length; i++) {
			o.f64(amps.getDouble(i));
			ImprovedNoise n = levels[i];
			if (n == null) {
				o.u8(0);
			} else {
				o.u8(1).f64(n.xo).f64(n.yo).f64(n.zo).raw(((ImprovedNoiseAccessor) (Object) n).rustgen$p());
			}
		}
	}

	// ------------------------------------------------------------------ record reflection

	private static final Map<Class<?>, RecordComponent[]> COMPONENTS = new ConcurrentHashMap<>();
	private static final Map<Class<?>, Method[]> ACCESSORS = new ConcurrentHashMap<>();

	private static RecordComponent[] components(Class<?> c) {
		return COMPONENTS.computeIfAbsent(c, k -> {
			if (!k.isRecord()) throw new UnsupportedException("expected a record: " + k.getName());
			return k.getRecordComponents();
		});
	}

	/** Reads record component {@code i} (declaration order, which is mapping-independent). */
	private static Object comp(Object rec, int i) {
		Method[] acc = ACCESSORS.computeIfAbsent(rec.getClass(), k -> {
			RecordComponent[] cs = components(k);
			Method[] ms = new Method[cs.length];
			for (int j = 0; j < cs.length; j++) {
				ms[j] = cs[j].getAccessor();
				ms[j].setAccessible(true);
			}
			return ms;
		});
		try {
			return acc[i].invoke(rec);
		} catch (ReflectiveOperationException | IndexOutOfBoundsException e) {
			throw new UnsupportedException("cannot read component " + i + " of " + rec.getClass().getName() + ": " + e);
		}
	}

	// ------------------------------------------------------------------ little-endian writer

	static final class Out {
		private final ByteArrayOutputStream buf = new ByteArrayOutputStream();

		Out u8(int v) {
			buf.write(v);
			return this;
		}

		Out i32(int v) {
			buf.write(v);
			buf.write(v >>> 8);
			buf.write(v >>> 16);
			buf.write(v >>> 24);
			return this;
		}

		Out f32(float v) {
			return i32(Float.floatToRawIntBits(v));
		}

		Out f64(double v) {
			long l = Double.doubleToRawLongBits(v);
			i32((int) l);
			return i32((int) (l >>> 32));
		}

		Out raw(byte[] b) {
			buf.writeBytes(b);
			return this;
		}

		Out bytes(Out other) {
			buf.writeBytes(other.buf.toByteArray());
			return this;
		}

		byte[] toByteArray() {
			return buf.toByteArray();
		}
	}
}
