package app.thot.mobile

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import org.json.JSONObject
import java.util.Calendar

object ReminderScheduler {
    const val PREFS = "thot_reminders"
    const val ACTION = "app.thot.mobile.STUDY_REMINDER"
    private const val REQ = 8742

    fun schedule(context: Context, prefsJson: String) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.edit().putString("json", prefsJson).apply()
        val obj = JSONObject(prefsJson)
        if (!obj.optBoolean("reminders_enabled", false)) {
            cancel(context)
            return
        }
        val hour = obj.optInt("reminder_hour", 18).coerceIn(0, 23)
        val minute = obj.optInt("reminder_minute", 0).coerceIn(0, 59)
        val target = obj.optString("reminder_target", "both")
        prefs.edit()
            .putInt("hour", hour)
            .putInt("minute", minute)
            .putString("target", target)
            .putBoolean("enabled", true)
            .apply()

        val am = context.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        val pi = pending(context)
        val cal = Calendar.getInstance().apply {
            set(Calendar.HOUR_OF_DAY, hour)
            set(Calendar.MINUTE, minute)
            set(Calendar.SECOND, 0)
            set(Calendar.MILLISECOND, 0)
            if (timeInMillis <= System.currentTimeMillis()) {
                add(Calendar.DAY_OF_YEAR, 1)
            }
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, cal.timeInMillis, pi)
            } else {
                @Suppress("DEPRECATION")
                am.set(AlarmManager.RTC_WAKEUP, cal.timeInMillis, pi)
            }
        } catch (_: SecurityException) {
            am.set(AlarmManager.RTC_WAKEUP, cal.timeInMillis, pi)
        }
    }

    fun cancel(context: Context) {
        val am = context.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        am.cancel(pending(context))
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putBoolean("enabled", false)
            .apply()
    }

    fun rescheduleNext(context: Context) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (!prefs.getBoolean("enabled", false)) return
        val json = prefs.getString("json", null) ?: return
        schedule(context, json)
    }

    private fun pending(context: Context): PendingIntent {
        val intent = Intent(context, StudyReminderReceiver::class.java).setAction(ACTION)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getBroadcast(context, REQ, intent, flags)
    }
}
