package rustgen;

/** Stand-in for the mod's NativeLib in the cross-check harness: the harness loads the library itself. */
public final class NativeLib {
	public static final int EXPECTED_ABI = 6;

	private NativeLib() {}

	public static boolean isAvailable() {
		return true;
	}

	public static String platform() {
		return System.getProperty("os.name") + "/" + System.getProperty("os.arch");
	}
}
