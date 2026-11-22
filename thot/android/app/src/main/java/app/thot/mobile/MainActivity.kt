package app.thot.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.Bundle
import android.view.inputmethod.EditorInfo
import android.webkit.PermissionRequest
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.EditText
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import app.thot.mobile.databinding.ActivityMainBinding

class MainActivity : AppCompatActivity() {
    private lateinit var binding: ActivityMainBinding
    private val prefs by lazy { getSharedPreferences(PREFS, Context.MODE_PRIVATE) }

    private val micPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* Web Speech may still work after grant on next mic tap */ }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            micPermission.launch(Manifest.permission.RECORD_AUDIO)
        }

        val web = binding.webView
        val settings = web.settings
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.databaseEnabled = true
        settings.mediaPlaybackRequiresUserGesture = false
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        settings.cacheMode = WebSettings.LOAD_DEFAULT
        settings.allowFileAccess = false
        WebView.setWebContentsDebuggingEnabled(true)

        web.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest?) {
                // Mic for Web Speech live notes
                request?.grant(request.resources)
            }
        }
        web.webViewClient = object : WebViewClient() {
            override fun onPageStarted(view: WebView?, url: String?, favicon: Bitmap?) {
                binding.titleView.text = "Thot"
            }

            override fun onReceivedError(
                view: WebView?,
                request: WebResourceRequest?,
                error: WebResourceError?
            ) {
                if (request?.isForMainFrame == true) {
                    Toast.makeText(
                        this@MainActivity,
                        getString(R.string.load_error),
                        Toast.LENGTH_LONG
                    ).show()
                }
            }
        }

        binding.btnReload.setOnClickListener { web.reload() }
        binding.btnServer.setOnClickListener { promptServerUrl(force = true) }

        val saved = prefs.getString(KEY_URL, null)
        if (saved.isNullOrBlank()) {
            promptServerUrl(force = true)
        } else {
            loadServer(saved)
        }
    }

    private fun promptServerUrl(force: Boolean) {
        val input = EditText(this).apply {
            hint = getString(R.string.server_url_hint)
            setText(prefs.getString(KEY_URL, DEFAULT_URL))
            inputType = EditorInfo.TYPE_TEXT_VARIATION_URI
            setSelection(text?.length ?: 0)
            setPadding(48, 32, 48, 32)
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.server_url_title)
            .setMessage(R.string.server_url_message)
            .setView(input)
            .setCancelable(!force)
            .setPositiveButton(R.string.action_save) { _, _ ->
                val raw = input.text?.toString()?.trim().orEmpty()
                val url = normalizeUrl(raw)
                prefs.edit().putString(KEY_URL, url).apply()
                loadServer(url)
            }
            .setNegativeButton(R.string.action_cancel) { d, _ ->
                if (!force) d.dismiss()
            }
            .show()
    }

    private fun normalizeUrl(raw: String): String {
        var u = raw.trim()
        if (u.isEmpty()) u = DEFAULT_URL
        if (!u.startsWith("http://") && !u.startsWith("https://")) {
            u = "http://$u"
        }
        return u.trimEnd('/')
    }

    private fun loadServer(url: String) {
        binding.titleView.text = url.removePrefix("http://").removePrefix("https://")
        binding.webView.loadUrl(url)
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (binding.webView.canGoBack()) {
            binding.webView.goBack()
        } else {
            @Suppress("DEPRECATION")
            super.onBackPressed()
        }
    }

    companion object {
        private const val PREFS = "thot"
        private const val KEY_URL = "server_url"
        /** Placeholder — Dan must replace with PC LAN IP or tunnel. */
        private const val DEFAULT_URL = "http://192.168.1.10:8787"
    }
}
