package app.thot.mobile

/**
 * JNI to `libthot.so` — starts embedded axum on 127.0.0.1.
 */
object NativeServer {
    init {
        System.loadLibrary("thot")
    }

    /**
     * @return bound port (>0) or negative error code
     */
    external fun start(
        dataDir: String,
        staticDir: String,
        preferredPort: Int,
        groqKey: String
    ): Int

    external fun port(): Int

    external fun isRunning(): Int
}
