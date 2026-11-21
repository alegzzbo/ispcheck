package online.point.ispcheck

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.TrafficStats
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import com.google.androidgamesdk.GameActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONArray
import org.json.JSONObject
import java.net.Inet4Address

/**
 * Окно создаёт Rust (egui). Методы isp* вызываются из Rust через JNI (sys/android.rs):
 * не переименовывать и не включать minify.
 */
class MainActivity : GameActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // окно рисуется под системными панелями, отступы Rust берёт из ispInsets()
        WindowCompat.setDecorFitsSystemWindows(window, false)
        window.statusBarColor = 0
        window.navigationBarColor = 0
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = false
            isAppearanceLightNavigationBars = false
        }
        if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(
                arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION), 1
            )
        }
    }

    /** left,top,right,bottom в пикселях: системные панели, вырез и клавиатура. */
    /** GameActivity отдаёт «Назад» в нативный код, где его никто не обрабатывает: сворачиваем приложение. */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_BACK) {
            if (event.action == KeyEvent.ACTION_UP && !event.isCanceled) moveTaskToBack(true)
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    fun ispInsets(): String {
        val ins = ViewCompat.getRootWindowInsets(window.decorView) ?: return "0,0,0,0"
        val b = ins.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val ime = ins.getInsets(WindowInsetsCompat.Type.ime())
        return "${b.left},${b.top},${b.right},${maxOf(b.bottom, ime.bottom)}"
    }

    fun ispFilesDir(): String = filesDir.absolutePath

    fun ispCounters(): String = "${TrafficStats.getTotalRxBytes()},${TrafficStats.getTotalTxBytes()}"

    fun ispCopy(text: String): String {
        runOnUiThread {
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            cm.setPrimaryClip(ClipData.newPlainText("ispcheck", text))
        }
        return "ok"
    }

    private fun transportOf(c: NetworkCapabilities): String = when {
        c.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
        c.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
        c.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
        c.hasTransport(NetworkCapabilities.TRANSPORT_VPN) -> "vpn"
        else -> "other"
    }

    private fun hasInternet(c: NetworkCapabilities) =
        c.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) &&
            !c.hasCapability(NetworkCapabilities.NET_CAPABILITY_CAPTIVE_PORTAL)

    private fun physical(cm: ConnectivityManager): List<Pair<Network, NetworkCapabilities>> =
        cm.allNetworks
            .mapNotNull { n -> cm.getNetworkCapabilities(n)?.let { n to it } }
            .filter { (_, c) ->
                !c.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
                    c.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            }

    /**
     * Сеть, по которой реально идёт трафик: не VPN и с рабочим интернетом.
     * Wi-Fi без авторизации (captive portal) или без интернета не выбирается, пока есть другая рабочая сеть.
     */
    private fun pickNetwork(cm: ConnectivityManager): Network? {
        val active = cm.activeNetwork
        val rank = mapOf("ethernet" to 0, "wifi" to 1, "cellular" to 2)
        val cands = physical(cm)
        cands.firstOrNull { it.first == active && hasInternet(it.second) }?.let { return it.first }
        cands.filter { hasInternet(it.second) }.minByOrNull { rank[transportOf(it.second)] ?: 9 }?.let { return it.first }
        cands.firstOrNull { it.first == active }?.let { return it.first }
        return cands.minByOrNull { rank[transportOf(it.second)] ?: 9 }?.first ?: active
    }

    fun ispSnapshot(): String {
        val o = JSONObject()
        try {
            val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val net = pickNetwork(cm)
            val caps = net?.let { cm.getNetworkCapabilities(it) }
            val lp = net?.let { cm.getLinkProperties(it) }
            val transport = if (caps == null) "none" else transportOf(caps)
            o.put("transport", transport)
            o.put("iface", lp?.interfaceName ?: "")
            o.put("validated", caps != null && hasInternet(caps))
            o.put("wifi_no_inet", physical(cm).any { transportOf(it.second) == "wifi" && !hasInternet(it.second) })
            if (caps != null) {
                o.put("down_kbps", caps.linkDownstreamBandwidthKbps)
                o.put("up_kbps", caps.linkUpstreamBandwidthKbps)
            }
            if (lp != null) {
                val gw = lp.routes.firstOrNull { it.isDefaultRoute && it.gateway is Inet4Address }
                o.put("gateway", gw?.gateway?.hostAddress ?: "")
                val all = lp.dnsServers
                val v4 = all.filterIsInstance<Inet4Address>()
                val arr = JSONArray()
                (if (v4.isNotEmpty()) v4 else all).forEach { arr.put(it.hostAddress?.substringBefore('%') ?: "") }
                o.put("dns", arr)
                lp.httpProxy?.let { p ->
                    if (p.host.isNullOrEmpty().not()) o.put("proxy", "http://${p.host}:${p.port}")
                    p.pacFileUrl?.toString()?.takeIf { it.isNotEmpty() }?.let { o.put("pac", it) }
                }
            }
            if (transport == "wifi") {
                val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
                @Suppress("DEPRECATION")
                val wi = wm.connectionInfo
                if (wi != null) {
                    val ssid = (wi.ssid ?: "").trim('"')
                    o.put("ssid", if (ssid == "<unknown ssid>") "" else ssid)
                    if (wi.rssi > -127) o.put("rssi", wi.rssi)
                    o.put("freq", wi.frequency)
                    var rx = wi.linkSpeed
                    var tx = wi.linkSpeed
                    if (Build.VERSION.SDK_INT >= 29) {
                        if (wi.rxLinkSpeedMbps > 0) rx = wi.rxLinkSpeedMbps
                        if (wi.txLinkSpeedMbps > 0) tx = wi.txLinkSpeedMbps
                    }
                    o.put("rx", maxOf(rx, 0))
                    o.put("tx", maxOf(tx, 0))
                    if (Build.VERSION.SDK_INT >= 30) {
                        o.put(
                            "phy", when (wi.wifiStandard) {
                                4 -> "Wi-Fi 4 (802.11n)"
                                5 -> "Wi-Fi 5 (802.11ac)"
                                6 -> "Wi-Fi 6 (802.11ax)"
                                7 -> "802.11ad"
                                8 -> "Wi-Fi 7 (802.11be)"
                                else -> ""
                            }
                        )
                    }
                }
            }
        } catch (_: Exception) {
            // отдаём то, что успели собрать
        }
        return o.toString()
    }
}
