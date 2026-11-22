package app.thot.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Color
import android.os.Bundle
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

/**
 * Thin WebView shell. Backend is the Rust axum process on LAN/tunnel — not embedded.
 * Blank white screen usually = unreachable URL; we always surface a visible error panel.
 */
class MainActivity : AppCompatActivity() {
    private lateinit var binding: ActivityMainBinding
    private val prefs by lazy { getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    private var currentUrl: String = ""
    private var pageFinishedOk = false

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
        web.setBackgroundColor(Color.WHITE)
        val settings = web.settings
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.databaseEnabled = true
        settings.mediaPlaybackRequiresUserGesture = false
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        // Avoid showing a stale blank cached page when the host is down.
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
                pageFinishedOk = false
                showLoading()
                binding.titleView.text = displayHost(url ?: currentUrl)
            }

            override fun onPageFinished(view: WebView?, url: String?) {
                binding.loadingBar.visibility = View.GONE
                // If we already flipped to the error panel, keep it.
                if (binding.errorPanel.visibility == View.VISIBLE) return
                // Detect Chromium interstitial / empty failure pages.
                view?.evaluateJavascript(
                    "(function(){try{var b=document.body;if(!b)return 'empty';" +
                        "var t=(b.innerText||'').trim();var h=(b.innerHTML||'').length;" +
                        "if(h<40&&t.length<8)return 'empty';return 'ok';}catch(e){return 'err';}})()"
                ) { result ->
                    val r = result?.trim('"') ?: "ok"
                    if (r == "empty" || r == "err") {
                        showErrorPanel(
                            currentUrl.ifBlank { url.orEmpty() },
                            getString(R.string.load_error) + " (page vide)"
                        )
                    } else {
                        pageFinishedOk = true
                        hideErrorPanel()
                    }
                }
            }

            override fun onReceivedError(
                view: WebView?,
                request: WebResourceRequest?,
                error: WebResourceError?
            ) {
                if (request?.isForMainFrame != true) return
                val code = error?.errorCode ?: -1
                val desc = error?.description?.toString().orEmpty()
                showErrorPanel(
                    currentUrl.ifBlank { request.url?.toString().orEmpty() },
                    "${getString(R.string.load_error)}\n($code) $desc"
                )
            }

            @Deprecated("Deprecated in Java")
            override fun onReceivedError(
                view: WebView?,
                errorCode: Int,
                description: String?,
                failingUrl: String?
            ) {
                // Older API path — still used on some devices.
                showErrorPanel(
                    failingUrl ?: currentUrl,
                    "${getString(R.string.load_error)}\n($errorCode) ${description.orEmpty()}"
                )
            }
        }

        binding.btnReload.setOnClickListener { reloadCurrent() }
        binding.btnServer.setOnClickListener { promptServerUrl(force = false) }
        binding.btnRetry.setOnClickListener { reloadCurrent() }
        binding.btnChangeServer.setOnClickListener { promptServerUrl(force = false) }

        val saved = prefs.getString(KEY_URL, null)?.trim().orEmpty()
        if (saved.isEmpty()) {
            showErrorPanel("", getString(R.string.no_server_yet))
            promptServerUrl(force = true)
        } else {
            loadServer(saved)
        }
    }

    private fun promptServerUrl(force: Boolean) {
        val input = EditText(this).apply {
            hint = getString(R.string.server_url_hint)
            setText(prefs.getString(KEY_URL, "") ?: "")
            inputType = EditorInfo.TYPE_TEXT_VARIATION_URI
            setPadding(48, 32, 48, 32)
            setSelection(text?.length ?: 0)
        }
        val dialog = AlertDialog.Builder(this)
            .setTitle(R.string.server_url_title)
            .setMessage(R.string.server_url_message)
            .setView(input)
            .setCancelable(!force)
            .setPositiveButton(R.string.action_save, null)
            .create()

        if (!force) {
            dialog.setButton(AlertDialog.BUTTON_NEGATIVE, getString(android.R.string.cancel)) { d, _ ->
                d.dismiss()
            }
        }

        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val raw = input.text?.toString()?.trim().orEmpty()
                if (raw.isEmpty()) {
                    Toast.makeText(this, R.string.url_required, Toast.LENGTH_SHORT).show()
                    return@setOnClickListener
                }
                if (raw.contains("127.0.0.1") || raw.contains("localhost")) {
                    Toast.makeText(
                        this,
                        "127.0.0.1 = ce téléphone. Utilisez l’IP LAN du PC.",
                        Toast.LENGTH_LONG
                    ).show()
                }
                val url = normalizeUrl(raw)
                prefs.edit().putString(KEY_URL, url).apply()
                dialog.dismiss()
                loadServer(url)
            }
        }
        dialog.show()
    }

    private fun normalizeUrl(raw: String): String {
        var u = raw.trim()
        if (u.isEmpty()) return ""
        if (!u.startsWith("http://") && !u.startsWith("https://")) {
            u = "http://$u"
        }
        // Common: user types only IP
        if (u.matches(Regex("""https?://\d{1,3}(?:\.\d{1,3}){3}"""))) {
            u = "$u:8787"
        }
        return u.trimEnd('/')
    }

    private fun loadServer(url: String) {
        currentUrl = url
        pageFinishedOk = false
        hideErrorPanel()
        showLoading()
        binding.titleView.text = displayHost(url)
        // Stop any prior document so failures don't leave a white blank layer.
        binding.webView.stopLoading()
        binding.webView.loadUrl("about:blank")
        binding.webView.loadUrl(url)
    }

    private fun reloadCurrent() {
        val url = prefs.getString(KEY_URL, null)?.trim().orEmpty().ifBlank { currentUrl }
        if (url.isBlank()) {
            showErrorPanel("", getString(R.string.no_server_yet))
            promptServerUrl(force = true)
            return
        }
        loadServer(url)
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
        binding.webView.stopLoading()
        binding.webView.visibility = View.INVISIBLE
        binding.errorPanel.visibility = View.VISIBLE
        binding.errorUrlView.text = if (url.isBlank()) {
            getString(R.string.no_server_yet)
        } else {
            "URL : $url"
        }
        binding.errorDetailView.text = buildString {
            append(detail.trim())
            append("\n\n")
            append(getString(R.string.error_checklist))
        }
        binding.titleView.text = getString(R.string.error_title)
    }

    private fun displayHost(url: String): String {
        return url.removePrefix("http://").removePrefix("https://").ifBlank { getString(R.string.app_name) }
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (binding.errorPanel.visibility == View.VISIBLE) {
            promptServerUrl(force = false)
            return
        }
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
    }
}
