package app.thot.mobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

class StudyReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        when (intent?.action) {
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED -> {
                ReminderScheduler.rescheduleNext(context)
            }
            ReminderScheduler.ACTION -> {
                showNotification(context)
                ReminderScheduler.rescheduleNext(context)
            }
        }
    }

    private fun showNotification(context: Context) {
        ensureChannel(context)
        val prefs = context.getSharedPreferences(ReminderScheduler.PREFS, Context.MODE_PRIVATE)
        val target = prefs.getString("target", "both") ?: "both"
        val body = when (target) {
            "fiches" -> "Il est temps de réviser vos fiches."
            "quiz" -> "Il est temps de faire un quiz."
            else -> "Il est temps de réviser (fiches ou quiz)."
        }
        val open = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val pi = PendingIntent.getActivity(
            context,
            0,
            open,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
        val notif = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_edit)
            .setContentTitle("Thot — rappel d’étude")
            .setContentText(body)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .setContentIntent(pi)
            .setAutoCancel(true)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(NOTIF_ID, notif)
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS denied
        }
    }

    companion object {
        private const val CHANNEL = "thot_study"
        private const val NOTIF_ID = 4201

        fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val mgr = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            val ch = NotificationChannel(
                CHANNEL,
                "Rappels d’étude",
                NotificationManager.IMPORTANCE_DEFAULT
            ).apply {
                description = "Rappels fiches / quiz — aucun audio"
            }
            mgr.createNotificationChannel(ch)
        }
    }
}
