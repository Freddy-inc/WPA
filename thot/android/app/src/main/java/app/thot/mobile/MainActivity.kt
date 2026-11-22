package app.thot.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.view.View
import android.view.inputmethod.EditorInfo
import android.webkit.JavascriptInterface
import android.webkit.PermissionRequest
import android.webkit.ValueCallback
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
import java.util.Locale
import java.util.concurrent.Executors

/**
 * Embedded mode: JNI axum on 127.0.0.1 + WebView.
 * Critical WebView bridges: file chooser (PDF import) + native speech (notes).
 */
class MainActivity : AppCompatActivity() {
    private lateinit var binding: ActivityMainBinding
    private val prefs by lazy { getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    private val io = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private var serverUrl: String = ""

    private var filePathCallback: ValueCallback<Array<Uri>>? = null
    private var speechRecognizer: SpeechRecognizer? = null
    private var dictationActive = false
    private var pendingWebPermission: PermissionRequest? = null

    private val micPermission = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { granted ->
        val ok = granted[Manifest.permission.RECORD_AUDIO] == true
        pendingWebPermission?.let { req ->
            if (ok) req.grant(req.resources) else req.deny()
            pendingWebPermission = null
        }
        if (!ok) {
            Toast.makeText(this, R.string.mic_denied, Toast.LENGTH_LONG).show()
        }
    }

    private val fileChooserLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val cb = filePathCallback
        filePathCallback = null
        if (cb == null) return@registerForActivityResult
        if (result.resultCode != Activity.RESULT_OK || result.data == null) {
            cb.onReceiveValue(null)
            return@registerForActivityResult
        }
        val data = result.data
        val uris = mutableListOf<Uri>()
        data?.clipData?.let { clip ->
            for (i in 0 until clip.itemCount) {
                clip.getItemAt(i)?.uri?.let { uris.add(it) }
            }
        }
        data?.data?.let { uris.add(it) }
        // Persist read permission for content:// during upload
        uris.forEach { uri ->
            try {
                contentResolver.takePersistableUriPermission(
                    uri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION
                )
            } catch (_: SecurityException) {
                // Not all providers support persistable grants — transient grant from chooser is enough.
            }
        }
        cb.onReceiveValue(if (uris.isEmpty()) null else uris.toTypedArray())
    }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        ensureMicPermission()

        setupWebView()
        binding.btnReload.setOnClickListener { reloadWeb() }
        binding.btnServer.setOnClickListener { promptGroqKey() }
        binding.btnRetry.setOnClickListener { startEmbeddedAndLoad() }
        binding.btnChangeServer.setOnClickListener { promptGroqKey() }

        startEmbeddedAndLoad()
    }

    private fun ensureMicPermission() {
        val need = mutableListOf<String>()
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            need.add(Manifest.permission.RECORD_AUDIO)
        }
        if (need.isNotEmpty()) {
            micPermission.launch(need.toTypedArray())
        }
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun setupWebView() {
        val web = binding.webView
        web.setBackgroundColor(Color.WHITE)
        val settings = web.settings
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.databaseEnabled = true
        settings.mediaPlaybackRequiresUserGesture = false
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        settings.cacheMode = WebSettings.LOAD_DEFAULT
        // Required for <input type=file> / blob reads after chooser
        settings.allowFileAccess = true
        settings.allowContentAccess = true
        @Suppress("DEPRECATION")
        settings.allowFileAccessFromFileURLs = true
        @Suppress("DEPRECATION")
        settings.allowUniversalAccessFromFileURLs = true
        WebView.setWebContentsDebuggingEnabled(true)

        web.addJavascriptInterface(ThotBridge(), "ThotAndroid")

        web.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest?) {
                if (request == null) return
                val needsAudio = request.resources.any {
                    it == PermissionRequest.RESOURCE_AUDIO_CAPTURE
                }
                if (needsAudio &&
                    ContextCompat.checkSelfPermission(
                        this@MainActivity,
                        Manifest.permission.RECORD_AUDIO
                    ) != PackageManager.PERMISSION_GRANTED
                ) {
                    pendingWebPermission = request
                    ensureMicPermission()
                    return
                }
                // Grant on UI thread
                main.post { request.grant(request.resources) }
            }

            override fun onShowFileChooser(
                webView: WebView?,
                filePathCallback: ValueCallback<Array<Uri>>?,
                fileChooserParams: FileChooserParams?
            ): Boolean {
                // Cancel any previous callback
                this@MainActivity.filePathCallback?.onReceiveValue(null)
                this@MainActivity.filePathCallback = filePathCallback

                val intent = try {
                    fileChooserParams?.createIntent()
                } catch (_: Exception) {
                    null
                } ?: Intent(Intent.ACTION_GET_CONTENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = "application/pdf"
                    putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("application/pdf", "application/*"))
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }

                // Prefer PDF but allow */* fallback so OEM pickers work
                if (intent.type.isNullOrBlank()) {
                    intent.type = "*/*"
                }
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

                return try {
                    fileChooserLauncher.launch(
                        Intent.createChooser(intent, getString(R.string.choose_pdf))
                    )
                    true
                } catch (e: Exception) {
                    this@MainActivity.filePathCallback = null
                    filePathCallback?.onReceiveValue(null)
                    Toast.makeText(
                        this@MainActivity,
                        "Sélecteur de fichiers indisponible: ${e.message}",
                        Toast.LENGTH_LONG
                    ).show()
                    false
                }
            }
        }

        web.webViewClient = object : WebViewClient() {
            override fun onPageStarted(view: WebView?, url: String?, favicon: Bitmap?) {
                // Don't flip loading overlay on every in-app navigation — only first load
                if (binding.errorPanel.visibility == View.VISIBLE) return
                binding.titleView.text = "Thot"
            }

            override fun onPageFinished(view: WebView?, url: String?) {
                binding.loadingBar.visibility = View.GONE
                if (binding.errorPanel.visibility == View.VISIBLE) return
                hideErrorPanel()
                injectAndroidSpeechBridge()
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

    /** Replace flaky WebView Web Speech with Android SpeechRecognizer. */
    private fun injectAndroidSpeechBridge() {
        val js = """
            (function(){
              if (window.__thotAndroidSpeech) return;
              window.__thotAndroidSpeech = true;
              function NativeRec(){
                this.continuous = true;
                this.interimResults = true;
                this.lang = 'fr-FR';
                this.onresult = null;
                this.onerror = null;
                this.onend = null;
                window.__thotActiveRec = this;
              }
              NativeRec.prototype.start = function(){
                window.__thotActiveRec = this;
                if (!window.ThotAndroid || !ThotAndroid.startDictation) {
                  if (this.onerror) this.onerror({ error: 'not-allowed', message: 'bridge missing' });
                  return;
                }
                ThotAndroid.startDictation(this.lang || 'fr-FR');
              };
              NativeRec.prototype.stop = function(){
                if (window.ThotAndroid && ThotAndroid.stopDictation) ThotAndroid.stopDictation();
                if (this.onend) { try { this.onend(); } catch (e) {} }
              };
              NativeRec.prototype.abort = function(){ this.stop(); };
              window.SpeechRecognition = NativeRec;
              window.webkitSpeechRecognition = NativeRec;
              window.__thotReceiveSpeech = function(text, isFinal){
                var rec = window.__thotActiveRec;
                if (!rec || !rec.onresult) return;
                var alt = { transcript: String(text || '') };
                var item = [alt];
                item[0] = alt;
                item.isFinal = !!isFinal;
                var results = [item];
                results[0] = item;
                results.length = 1;
                rec.onresult({ resultIndex: 0, results: results });
              };
              window.__thotSpeechError = function(msg){
                var rec = window.__thotActiveRec;
                if (rec && rec.onerror) rec.onerror({ error: 'network', message: String(msg || 'speech') });
              };
              console.log('[Thot] Android SpeechRecognizer bridge ready');
            })();
        """.trimIndent()
        binding.webView.evaluateJavascript(js, null)
    }

    private inner class ThotBridge {
        @JavascriptInterface
        fun startDictation(lang: String) {
            main.post { beginNativeDictation(lang) }
        }

        @JavascriptInterface
        fun stopDictation() {
            main.post { endNativeDictation() }
        }

        @JavascriptInterface
        fun hasNativeSpeech(): Boolean = SpeechRecognizer.isRecognitionAvailable(this@MainActivity)
    }

    private fun beginNativeDictation(lang: String) {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            ensureMicPermission()
            binding.webView.evaluateJavascript(
                "window.__thotSpeechError && __thotSpeechError('micro refusé');",
                null
            )
            return
        }
        if (!SpeechRecognizer.isRecognitionAvailable(this)) {
            binding.webView.evaluateJavascript(
                "window.__thotSpeechError && __thotSpeechError('SpeechRecognizer indisponible sur cet appareil');",
                null
            )
            Toast.makeText(this, R.string.speech_unavailable, Toast.LENGTH_LONG).show()
            return
        }
        endNativeDictation()
        val recognizer = SpeechRecognizer.createSpeechRecognizer(this)
        speechRecognizer = recognizer
        dictationActive = true
        recognizer.setRecognitionListener(object : RecognitionListener {
            override fun onReadyForSpeech(params: Bundle?) {}
            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() {
                // Continuous-ish: restart while active
                if (dictationActive) {
                    main.postDelayed({ if (dictationActive) launchRecognizerIntent(lang) }, 250)
                }
            }
            override fun onError(error: Int) {
                if (!dictationActive) return
                // Restart on busy/timeout; surface hard errors
                if (error == SpeechRecognizer.ERROR_NO_MATCH ||
                    error == SpeechRecognizer.ERROR_SPEECH_TIMEOUT ||
                    error == SpeechRecognizer.ERROR_CLIENT
                ) {
                    main.postDelayed({ if (dictationActive) launchRecognizerIntent(lang) }, 300)
                } else {
                    val msg = "speech error $error"
                    binding.webView.evaluateJavascript(
                        "window.__thotSpeechError && __thotSpeechError(${org.json.JSONObject.quote(msg)});",
                        null
                    )
                }
            }
            override fun onResults(results: Bundle?) {
                val texts = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                val text = texts?.firstOrNull()?.trim().orEmpty()
                if (text.isNotEmpty()) {
                    deliverSpeech(text, true)
                }
                if (dictationActive) {
                    main.postDelayed({ if (dictationActive) launchRecognizerIntent(lang) }, 200)
                }
            }
            override fun onPartialResults(partialResults: Bundle?) {
                val texts = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                val text = texts?.firstOrNull()?.trim().orEmpty()
                if (text.isNotEmpty()) deliverSpeech(text, false)
            }
            override fun onEvent(eventType: Int, params: Bundle?) {}
        })
        launchRecognizerIntent(lang)
    }

    private fun launchRecognizerIntent(lang: String) {
        val recognizer = speechRecognizer ?: return
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(
                RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                RecognizerIntent.LANGUAGE_MODEL_FREE_FORM
            )
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, lang.ifBlank { Locale.getDefault().toLanguageTag() })
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 3)
            putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, packageName)
        }
        try {
            recognizer.startListening(intent)
        } catch (e: Exception) {
            binding.webView.evaluateJavascript(
                "window.__thotSpeechError && __thotSpeechError(${org.json.JSONObject.quote(e.message ?: "startListening")});",
                null
            )
        }
    }

    private fun deliverSpeech(text: String, isFinal: Boolean) {
        val q = org.json.JSONObject.quote(text)
        binding.webView.evaluateJavascript(
            "window.__thotReceiveSpeech && __thotReceiveSpeech($q, ${if (isFinal) "true" else "false"});",
            null
        )
    }

    private fun endNativeDictation() {
        dictationActive = false
        try {
            speechRecognizer?.stopListening()
        } catch (_: Exception) {
        }
        try {
            speechRecognizer?.cancel()
            speechRecognizer?.destroy()
        } catch (_: Exception) {
        }
        speechRecognizer = null
    }

    private fun startEmbeddedAndLoad() {
        showLoading()
        binding.titleView.text = "Démarrage…"
        io.execute {
            try {
                val data = AssetHelper.dataDir(this)
                val staticDir = AssetHelper.ensureStaticDir(this)
                val groq = prefs.getString(KEY_GROQ, "")?.trim().orEmpty()
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
                        main.postDelayed({
                            binding.webView.loadUrl(serverUrl)
                        }, 250)
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

    override fun onDestroy() {
        endNativeDictation()
        super.onDestroy()
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
