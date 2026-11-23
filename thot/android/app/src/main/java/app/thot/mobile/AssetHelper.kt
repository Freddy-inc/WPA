package app.thot.mobile

import android.content.Context
import android.content.res.AssetManager
import java.io.File
import java.io.FileOutputStream

object AssetHelper {
    private const val ASSET_ROOT = "thot-static"
    private const val MARKER = ".thot_static_v3"

    /** Copy packaged UI from APK assets into app files (axum ServeDir needs real paths). */
    fun ensureStaticDir(context: Context): File {
        val dest = File(context.filesDir, "static")
        val marker = File(dest, MARKER)
        if (marker.isFile && File(dest, "index.html").isFile) {
            return dest
        }
        if (dest.exists()) {
            dest.deleteRecursively()
        }
        dest.mkdirs()
        copyAssetDir(context.assets, ASSET_ROOT, dest)
        marker.writeText("ok")
        return dest
    }

    fun dataDir(context: Context): File {
        val d = File(context.filesDir, "thot-data")
        d.mkdirs()
        File(d, "docs").mkdirs()
        File(d, "podcasts").mkdirs()
        return d
    }

    private fun copyAssetDir(assets: AssetManager, path: String, dest: File) {
        val kids = assets.list(path) ?: return
        if (kids.isEmpty()) {
            // file
            assets.open(path).use { input ->
                FileOutputStream(dest).use { output -> input.copyTo(output) }
            }
            return
        }
        dest.mkdirs()
        for (name in kids) {
            val childAsset = if (path.isEmpty()) name else "$path/$name"
            val childDest = File(dest, name)
            val sub = assets.list(childAsset)
            if (sub != null && sub.isNotEmpty()) {
                copyAssetDir(assets, childAsset, childDest)
            } else {
                assets.open(childAsset).use { input ->
                    FileOutputStream(childDest).use { output -> input.copyTo(output) }
                }
            }
        }
    }
}
