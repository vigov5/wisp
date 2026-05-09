package com.example.drift

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
        const val CHANNEL_ID = "drift_transfer"
        const val ACTION_STOP = "com.example.drift.TRANSFER_STOP"
        const val EXTRA_TITLE = "title"
        const val EXTRA_BODY = "body"
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var locksHeld: Boolean = false

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Android 12+ kills the process with ForegroundServiceDidNotStartInTime
        // if `startForegroundService()` is followed by anything other than
        // `startForeground()` within ~5s.  Even when handling ACTION_STOP,
        // the framework counts the service as "started as FGS" the moment
        // it was launched via startForegroundService(), so we MUST call
        // startForeground() before stopping — otherwise the system kills us.
        val title = intent?.getStringExtra(EXTRA_TITLE) ?: "Drift"
        val body = intent?.getStringExtra(EXTRA_BODY) ?: ""
        val notif = buildNotification(title, body)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            @Suppress("DEPRECATION")
            startForeground(NOTIFICATION_ID, notif)
        }

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

    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (nm.getNotificationChannel(CHANNEL_ID) != null) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Drift transfer",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Keeps active file transfers running while the screen is off."
            setShowBadge(false)
        }
        nm.createNotificationChannel(channel)
    }

    private fun buildNotification(title: String, body: String): Notification {
        val stopIntent = Intent(this, TransferKeepaliveService::class.java)
            .setAction(ACTION_STOP)
        val stopPi = PendingIntent.getService(
            this,
            0,
            stopIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        val launchIntent = packageManager.getLaunchIntentForPackage(packageName)
        val launchPi = launchIntent?.let {
            PendingIntent.getActivity(
                this,
                1,
                it,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        }

        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
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

    private fun acquireLocks() {
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "drift:transfer-cpu").apply {
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
        wifiLock = wm.createWifiLock(wifiMode, "drift:transfer-wifi").apply {
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
