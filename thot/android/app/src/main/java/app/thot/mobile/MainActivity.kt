package app.thot.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Color
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.View
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
import java.util.concurrent.Executors

/**
 * Embedded mode: JNI starts Rust axum on 127.0.0.1; WebView loads that URL.
 * No PC / cloud server required for basic offline UI + local SQLite.
 */
class MainActivity : AppCompatActivity() {
    private lateinit var binding: ActivityMainBinding
    private val prefs by lazy { getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    private val io = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private var serverUrl: String = ""

    private val micPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { }

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

        setupWebView()
        binding.btnReload.setOnClickListener { reloadWeb() }
        binding.btnServer.setOnClickListener { promptGroqKey() }
        binding.btnRetry.setOnClickListener { startEmbeddedAndLoad() }
        binding.btnChangeServer.setOnClickListener { promptGroqKey() }

        startEmbeddedAndLoad()
    }

    private fun setupWebView() {
        val web = binding.webView
        web.setBackgroundColor(Color.WHITE)
        val settings = web.settings
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.databaseEnabled = true
        settings.mediaPlaybackRequiresUserGesture = false
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        settings.cacheMode = WebSettings.LOAD_NO_CACHE
        settings.allowFileAccess = false
        WebView.setWebContentsDebuggingEnabled(true)

        web.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest?) {
                request?.grant(request.resources)
            }
        }
        web.webViewClient = object : WebViewClient() {
            override fun onPageStarted(view: WebView?, url: String?, favicon: Bitmap?) {
                showLoading()
                binding.titleView.text = "Thot"
            }

            override fun onPageFinished(view: WebView?, url: String?) {
                binding.loadingBar.visibility = View.GONE
                if (binding.errorPanel.visibility == View.VISIBLE) return
                hideErrorPanel()
            }

            override fun onReceivedError(
                view: WebView?,
                request: WebResourceRequest?,
                error: WebResourceError?
            ) {
                if (request?.isForMainFrame != true) return
                showErrorPanel(
                    serverUrl,
                    "WebView: ${error?.description} (${error?.errorCode})"
                )
            }
        }
    }

    private fun startEmbeddedAndLoad() {
        showLoading()
        binding.titleView.text = "Démarrage…"
        io.execute {
            try {
                val data = AssetHelper.dataDir(this)
                val staticDir = AssetHelper.ensureStaticDir(this)
                val groq = prefs.getString(KEY_GROQ, "")?.trim().orEmpty()
                // Persist optional key for Rust file fallback too
                val keyFile = java.io.File(data, "groq.key")
                if (groq.isNotEmpty()) {
                    keyFile.writeText(groq)
                }

                val code = NativeServer.start(
                    data.absolutePath,
                    staticDir.absolutePath,
                    8787,
                    groq
                )
                main.post {
                    if (code > 0) {
                        serverUrl = "http://127.0.0.1:$code"
                        binding.titleView.text = "Thot · local:$code"
                        hideErrorPanel()
                        // Brief delay so the listener is accepting
                        main.postDelayed({
                            binding.webView.loadUrl(serverUrl)
                        }, 200)
                    } else {
                        showErrorPanel(
                            "embedded",
                            "Échec démarrage natif (code $code). Réessayez ou réinstallez l’APK."
                        )
                    }
                }
            } catch (t: Throwable) {
                main.post {
                    showErrorPanel(
                        "embedded",
                        "Crash natif: ${t.javaClass.simpleName}: ${t.message}"
                    )
                }
            }
        }
    }

    private fun reloadWeb() {
        if (NativeServer.isRunning() == 1 && NativeServer.port() > 0) {
            serverUrl = "http://127.0.0.1:${NativeServer.port()}"
            binding.webView.loadUrl(serverUrl)
        } else {
            startEmbeddedAndLoad()
        }
    }

    private fun promptGroqKey() {
        val input = EditText(this).apply {
            hint = getString(R.string.groq_key_hint)
            setText(prefs.getString(KEY_GROQ, "") ?: "")
            inputType = EditorInfo.TYPE_CLASS_TEXT or EditorInfo.TYPE_TEXT_VARIATION_PASSWORD
            setPadding(48, 32, 48, 32)
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.groq_key_title)
            .setMessage(R.string.groq_key_message)
            .setView(input)
            .setPositiveButton(R.string.action_save) { _, _ ->
                val v = input.text?.toString()?.trim().orEmpty()
                prefs.edit().putString(KEY_GROQ, v).apply()
                Toast.makeText(this, R.string.groq_saved_restart, Toast.LENGTH_LONG).show()
                startEmbeddedAndLoad()
            }
            .setNeutralButton(R.string.action_clear_key) { _, _ ->
                prefs.edit().remove(KEY_GROQ).apply()
                java.io.File(AssetHelper.dataDir(this), "groq.key").delete()
                Toast.makeText(this, R.string.groq_cleared, Toast.LENGTH_SHORT).show()
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    private fun showLoading() {
        binding.loadingBar.visibility = View.VISIBLE
        binding.errorPanel.visibility = View.GONE
        binding.webView.visibility = View.VISIBLE
    }

    private fun hideErrorPanel() {
        binding.errorPanel.visibility = View.GONE
        binding.webView.visibility = View.VISIBLE
    }

    private fun showErrorPanel(url: String, detail: String) {
        binding.loadingBar.visibility = View.GONE
        binding.webView.visibility = View.INVISIBLE
        binding.errorPanel.visibility = View.VISIBLE
        binding.errorUrlView.text = "Mode embarqué · $url"
        binding.errorDetailView.text = buildString {
            append(detail.trim())
            append("\n\n")
            append(getString(R.string.error_checklist_embedded))
        }
        binding.titleView.text = getString(R.string.error_title)
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
        private const val KEY_GROQ = "groq_api_key"
    }
}
