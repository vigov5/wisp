package dev.vigov5.wisp

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat

/// Foreground service that keeps an active iroh transfer alive when the screen
/// locks or the app is backgrounded. Holds a partial wake lock + Wi-Fi
/// high-performance lock for the lifetime of the service. Idempotent on
/// repeated start commands; updating notification text does not re-acquire
/// the locks.
class TransferKeepaliveService : Service() {

    companion object {
        const val NOTIFICATION_ID = 4711
        const val CHANNEL_ID = "wisp_transfer"
        const val ACTION_STOP = "dev.vigov5.wisp.TRANSFER_STOP"
        const val EXTRA_TITLE = "title"
        const val EXTRA_BODY = "body"

        // True between startForeground() and onDestroy().  Read from the
        // main thread only (service callbacks and the method-channel
        // handler both run there), but marked volatile so a stale value can
        // never survive a thread hand-off.
        @Volatile
        private var foreground: Boolean = false

        /// Re-posts the ongoing notification with new text for an
        /// already-running service, WITHOUT going through
        /// `startForegroundService()`.
        ///
        /// This is the whole point of the method: every
        /// `startForegroundService()` opens a fresh ~5s window in which the
        /// framework demands a matching `startForeground()`, and punishes a
        /// miss with `ForegroundServiceDidNotStartInTimeException` — a hard
        /// process kill (seen in the field, v2.1.0).  Progress ticks fire
        /// about once a second for the life of a transfer, so routing them
        /// through the service start armed one of those deadlines per second,
        /// and a long send is exactly when the main thread is least able to
        /// meet one.  Notifying directly keeps the text live and arms nothing.
        ///
        /// Returns false when the service is not running, so the caller can
        /// fall back to actually starting it.
        fun postUpdate(context: Context, title: String, body: String): Boolean {
            if (!foreground) return false
            return try {
                ensureNotificationChannel(context)
                val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
                    as NotificationManager
                nm.notify(NOTIFICATION_ID, buildNotification(context, title, body))
                true
            } catch (_: SecurityException) {
                // POST_NOTIFICATIONS denied — the service keeps running with
                // whatever text it last showed; only the label goes stale.
                true
            }
        }

        private fun ensureNotificationChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
                as NotificationManager
            if (nm.getNotificationChannel(CHANNEL_ID) != null) return
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Wisp transfer",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "Keeps active file transfers running while the screen is off."
                setShowBadge(false)
            }
            nm.createNotificationChannel(channel)
        }

        private fun buildNotification(
            context: Context,
            title: String,
            body: String,
        ): Notification {
            val stopIntent = Intent(context, TransferKeepaliveService::class.java)
                .setAction(ACTION_STOP)
            val stopPi = PendingIntent.getService(
                context,
                0,
                stopIntent,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

            val launchIntent = context.packageManager
                .getLaunchIntentForPackage(context.packageName)
            val launchPi = launchIntent?.let {
                PendingIntent.getActivity(
                    context,
                    1,
                    it,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
            }

            val builder = NotificationCompat.Builder(context, CHANNEL_ID)
                .setContentTitle(title)
                .setContentText(body)
                .setSmallIcon(R.mipmap.launcher_icon)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setPriority(NotificationCompat.PRIORITY_LOW)
                .addAction(0, "Stop", stopPi)
            if (launchPi != null) {
                builder.setContentIntent(launchPi)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                builder.setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            }
            return builder.build()
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var locksHeld: Boolean = false

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel(this)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Android 12+ kills the process with ForegroundServiceDidNotStartInTime
        // if `startForegroundService()` is followed by anything other than
        // `startForeground()` within ~5s.  Even when handling ACTION_STOP,
        // the framework counts the service as "started as FGS" the moment
        // it was launched via startForegroundService(), so we MUST call
        // startForeground() before stopping — otherwise the system kills us.
        val title = intent?.getStringExtra(EXTRA_TITLE) ?: "Wisp"
        val body = intent?.getStringExtra(EXTRA_BODY) ?: ""
        val notif = buildNotification(this, title, body)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            @Suppress("DEPRECATION")
            startForeground(NOTIFICATION_ID, notif)
        }
        foreground = true

        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }

        if (!locksHeld) {
            acquireLocks()
            locksHeld = true
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        foreground = false
        releaseLocks()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
        } catch (_: IllegalStateException) {
            // Framework already tore the FGS down; nothing to clean up.
        }
        super.onDestroy()
    }

    private fun acquireLocks() {
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "wisp:transfer-cpu").apply {
            setReferenceCounted(false)
            acquire()
        }

        val appContext = applicationContext
        val wm = appContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        val wifiMode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            WifiManager.WIFI_MODE_FULL_HIGH_PERF
        } else {
            @Suppress("DEPRECATION")
            WifiManager.WIFI_MODE_FULL_HIGH_PERF
        }
        wifiLock = wm.createWifiLock(wifiMode, "wisp:transfer-wifi").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseLocks() {
        if (!locksHeld) return
        try {
            wakeLock?.takeIf { it.isHeld }?.release()
        } catch (_: RuntimeException) {
            // ignore double release
        }
        try {
            wifiLock?.takeIf { it.isHeld }?.release()
        } catch (_: RuntimeException) {
            // ignore double release
        }
        wakeLock = null
        wifiLock = null
        locksHeld = false
    }
}
