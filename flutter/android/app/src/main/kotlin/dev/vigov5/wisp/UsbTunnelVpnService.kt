package dev.vigov5.wisp

import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor

/**
 * Turns the AOA cable into a point-to-point IP link (path A). It only owns the
 * TUN lifecycle + the system VPN consent/notification; the actual packet pump
 * (TUN ⇄ AOA bulk) runs in [UsbAoaChannel], reached via [AoaBridge], so it can
 * share the live [UsbAoaChannel] AOA link.
 *
 * Addressing is a /30: the USB host takes 10.42.0.1, the accessory 10.42.0.2.
 * Only that /30 is routed through the TUN, so the rest of the phone's
 * networking is untouched — and iroh binds/dials those addresses exactly as it
 * does on Wi-Fi, which is why the entire transfer stack is reused unchanged.
 */
class UsbTunnelVpnService : VpnService() {

    private var tun: ParcelFileDescriptor? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val role = intent?.getStringExtra(UsbAoaChannel.EXTRA_ROLE)
        val localIp = UsbAoaChannel.tunnelIpForRole(role)

        val pfd = runCatching {
            Builder()
                .setSession("Wisp USB cable")
                .addAddress(localIp, UsbAoaChannel.TUNNEL_PREFIX)
                .addRoute(UsbAoaChannel.TUNNEL_SUBNET, UsbAoaChannel.TUNNEL_PREFIX)
                .setMtu(UsbAoaChannel.TUNNEL_MTU)
                .setBlocking(true)
                .establish()
        }.getOrNull()

        if (pfd == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        tun = pfd

        val channel = AoaBridge.channel
        if (channel == null) {
            runCatching { pfd.close() }
            stopSelf()
            return START_NOT_STICKY
        }
        // Channel takes ownership of the fd and runs the pump.
        channel.beginTunnel(pfd)
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        AoaBridge.channel?.onTunnelServiceDestroyed()
        tun = null
        super.onDestroy()
    }
}
