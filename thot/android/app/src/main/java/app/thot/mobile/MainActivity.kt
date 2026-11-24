package app.thot.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Color
import android.net.Uri
import android.os.Build
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
import java.io.DataOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.Locale
import java.util.concurrent.Executors

/**
 * Embedded mode: JNI axum on 127.0.0.1 + WebView.
 * PDF import: native OpenDocument → multipart POST to local axum (bypasses flaky WebView file input).
 * Notes mic: Android SpeechRecognizer bridge (bypasses missing Web Speech in WebView).
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
    private var dictationLang: String = "en-US"
    private var pendingWebPermission: PermissionRequest? = null
    private var restartSpeechRunnable: Runnable? = null

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

    private val notifyPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        if (!granted) {
            Toast.makeText(this, R.string.notify_denied, Toast.LENGTH_LONG).show()
        }
    }

    /** Fallback for plain &lt;input type=file&gt; (desktop-parity / older paths). */
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
        uris.forEach { uri ->
            try {
                contentResolver.takePersistableUriPermission(
                    uri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION
                )
            } catch (_: SecurityException) {
            }
        }
        cb.onReceiveValue(if (uris.isEmpty()) null else uris.toTypedArray())
    }

    /** Primary APK path: OpenDocument → upload bytes to embedded axum. */
    private val openPdfLauncher = registerForActivityResult(
        ActivityResultContracts.OpenDocument()
    ) { uri ->
        if (uri == null) {
            notifyJsPdfError("Sélection annulée")
            return@registerForActivityResult
        }
        try {
            contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION
            )
        } catch (_: SecurityException) {
        }
        uploadPdfUri(uri)
    }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        // Portrait by default; reader unlocks SENSOR via JS bridge
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
        StudyReminderReceiver.ensureChannel(this)

        ensureMicPermission()

        setupWebView()
        binding.btnReload.setOnClickListener { reloadWeb() }
        binding.btnServer.setOnClickListener { promptGroqKey() }
        binding.btnRetry.setOnClickListener { startEmbeddedAndLoad() }
        binding.btnChangeServer.setOnClickListener { promptGroqKey() }

        startEmbeddedAndLoad()
    }

    private fun ensureMicPermission() {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            micPermission.launch(arrayOf(Manifest.permission.RECORD_AUDIO))
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
                main.post { request.grant(request.resources) }
            }

            override fun onShowFileChooser(
                webView: WebView?,
                filePathCallback: ValueCallback<Array<Uri>>?,
                fileChooserParams: FileChooserParams?
            ): Boolean {
                this@MainActivity.filePathCallback?.onReceiveValue(null)
                this@MainActivity.filePathCallback = filePathCallback

                val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = "application/pdf"
                    putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("application/pdf", "application/*"))
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    addFlags(Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
                }

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
                if (binding.errorPanel.visibility == View.VISIBLE) return
                binding.titleView.text = "Thot"
                injectAndroidSpeechBridge()
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

    /** Replace missing/flaky WebView Web Speech with Android SpeechRecognizer. */
    private fun injectAndroidSpeechBridge() {
        val js = """
            (function(){
              if (window.__thotAndroidSpeech) return;
              window.__thotAndroidSpeech = true;
              function NativeRec(){
                this.continuous = true;
                this.interimResults = true;
                this.lang = 'en-US';
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
                ThotAndroid.startDictation(this.lang || 'en-US');
              };
              NativeRec.prototype.stop = function(){
                if (window.ThotAndroid && ThotAndroid.stopDictation) ThotAndroid.stopDictation();
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

        @JavascriptInterface
        fun pickAndUploadPdf() {
            main.post {
                try {
                    openPdfLauncher.launch(arrayOf("application/pdf", "*/*"))
                } catch (e: Exception) {
                    notifyJsPdfError(e.message ?: "Sélecteur indisponible")
                }
            }
        }

        @JavascriptInterface
        fun hasNativePicker(): Boolean = true

        @JavascriptInterface
        fun setOrientation(mode: String) {
            main.post {
                requestedOrientation = when (mode.lowercase()) {
                    "sensor", "full", "landscape", "unspecified" ->
                        ActivityInfo.SCREEN_ORIENTATION_SENSOR
                    else -> ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
                }
            }
        }

        @JavascriptInterface
        fun requestNotifyPermission() {
            main.post {
                if (Build.VERSION.SDK_INT >= 33) {
                    if (ContextCompat.checkSelfPermission(
                            this@MainActivity,
                            Manifest.permission.POST_NOTIFICATIONS
                        ) != PackageManager.PERMISSION_GRANTED
                    ) {
                        notifyPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
                    }
                }
            }
        }

        @JavascriptInterface
        fun scheduleReminders(prefsJson: String) {
            main.post {
                ReminderScheduler.schedule(this@MainActivity, prefsJson)
                Toast.makeText(
                    this@MainActivity,
                    R.string.reminders_scheduled,
                    Toast.LENGTH_SHORT
                ).show()
            }
        }

        @JavascriptInterface
        fun cancelReminders() {
            main.post {
                ReminderScheduler.cancel(this@MainActivity)
            }
        }
    }

    private fun uploadPdfUri(uri: Uri) {
        val base = serverUrl.ifBlank {
            if (NativeServer.isRunning() == 1 && NativeServer.port() > 0) {
                "http://127.0.0.1:${NativeServer.port()}"
            } else {
                ""
            }
        }
        if (base.isBlank()) {
            notifyJsPdfError("Serveur local pas prêt")
            return
        }
        io.execute {
            try {
                var name = "cours.pdf"
                contentResolver.query(uri, null, null, null, null)?.use { c ->
                    val idx = c.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
                    if (idx >= 0 && c.moveToFirst()) {
                        val n = c.getString(idx)
                        if (!n.isNullOrBlank()) name = n
                    }
                }
                val bytes = contentResolver.openInputStream(uri)?.use { it.readBytes() }
                    ?: throw IllegalStateException("Impossible de lire le PDF")
                if (bytes.isEmpty()) throw IllegalStateException("PDF vide")
                if (bytes.size > 15 * 1024 * 1024) {
                    throw IllegalStateException("PDF trop lourd (max 15 Mo)")
                }

                val boundary = "----ThotBoundary${System.currentTimeMillis()}"
                val url = URL("$base/api/import")
                val conn = (url.openConnection() as HttpURLConnection).apply {
                    requestMethod = "POST"
                    doOutput = true
                    connectTimeout = 30000
                    readTimeout = 120000
                    setRequestProperty("Content-Type", "multipart/form-data; boundary=$boundary")
                }
                DataOutputStream(conn.outputStream).use { out ->
                    out.writeBytes("--$boundary\r\n")
                    out.writeBytes(
                        "Content-Disposition: form-data; name=\"file\"; filename=\"${
                            name.replace("\"", "")
                        }\"\r\n"
                    )
                    out.writeBytes("Content-Type: application/pdf\r\n\r\n")
                    out.write(bytes)
                    out.writeBytes("\r\n--$boundary--\r\n")
                    out.flush()
                }
                val code = conn.responseCode
                val body = try {
                    (if (code in 200..299) conn.inputStream else conn.errorStream)
                        ?.bufferedReader()?.readText().orEmpty()
                } catch (_: Exception) {
                    ""
                }
                conn.disconnect()
                if (code !in 200..299) {
                    val err = Regex(""""error"\s*:\s*"([^"]+)"""").find(body)?.groupValues?.getOrNull(1)
                        ?: "HTTP $code"
                    throw IllegalStateException(err)
                }
                main.post {
                    binding.webView.evaluateJavascript(
                        "window.__thotPdfImported && window.__thotPdfImported();",
                        null
                    )
                }
            } catch (t: Throwable) {
                main.post {
                    notifyJsPdfError(t.message ?: t.javaClass.simpleName)
                }
            }
        }
    }

    private fun notifyJsPdfError(msg: String) {
        val q = org.json.JSONObject.quote(msg)
        binding.webView.evaluateJavascript(
            "window.__thotPdfImportError && window.__thotPdfImportError($q);",
            null
        )
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
        dictationLang = lang.ifBlank { Locale.getDefault().toLanguageTag() }
        val recognizer = SpeechRecognizer.createSpeechRecognizer(this)
        speechRecognizer = recognizer
        dictationActive = true
        recognizer.setRecognitionListener(object : RecognitionListener {
            override fun onReadyForSpeech(params: Bundle?) {}
            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() {
                // Let onResults / onError schedule the next listen — avoid double-start.
            }
            override fun onError(error: Int) {
                if (!dictationActive) return
                if (error == SpeechRecognizer.ERROR_NO_MATCH ||
                    error == SpeechRecognizer.ERROR_SPEECH_TIMEOUT ||
                    error == SpeechRecognizer.ERROR_CLIENT ||
                    error == SpeechRecognizer.ERROR_RECOGNIZER_BUSY
                ) {
                    scheduleSpeechRestart(400)
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
                if (dictationActive) scheduleSpeechRestart(350)
            }
            override fun onPartialResults(partialResults: Bundle?) {
                val texts = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                val text = texts?.firstOrNull()?.trim().orEmpty()
                if (text.isNotEmpty()) deliverSpeech(text, false)
            }
            override fun onEvent(eventType: Int, params: Bundle?) {}
        })
        launchRecognizerIntent()
    }

    private fun scheduleSpeechRestart(delayMs: Long) {
        restartSpeechRunnable?.let { main.removeCallbacks(it) }
        val r = Runnable {
            if (dictationActive) launchRecognizerIntent()
        }
        restartSpeechRunnable = r
        main.postDelayed(r, delayMs)
    }

    private fun launchRecognizerIntent() {
        val recognizer = speechRecognizer ?: return
        if (!dictationActive) return
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(
                RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                RecognizerIntent.LANGUAGE_MODEL_FREE_FORM
            )
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, dictationLang)
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 3)
            putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, packageName)
            putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_COMPLETE_SILENCE_LENGTH_MILLIS, 1500L)
            putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_POSSIBLY_COMPLETE_SILENCE_LENGTH_MILLIS, 1000L)
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
        restartSpeechRunnable?.let { main.removeCallbacks(it) }
        restartSpeechRunnable = null
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
