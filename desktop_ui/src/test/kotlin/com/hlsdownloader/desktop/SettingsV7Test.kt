package com.hlsdownloader.desktop

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class SettingsV7Test {
    @Test
    fun cast_device_description_keeps_protocol_and_address_visible() {
        assertEquals(
            "TVBox · http://192.168.1.45:9978",
            castDeviceDescription(
                CastDeviceDto(
                    id = "tvbox:http://192.168.1.45:9978",
                    label = "TVBox / 影视盒子",
                    location = "http://192.168.1.45:9978",
                    serviceType = "tvbox",
                ),
            ),
        )
        assertEquals(
            "Chromecast · 192.168.1.32:8009",
            castDeviceDescription(
                CastDeviceDto(
                    id = "chromecast:bedroom",
                    label = "卧室显示器",
                    controlUrl = "192.168.1.32:8009",
                    serviceType = "chromecast",
                ),
            ),
        )
    }

    @Test
    fun settings_separates_cast_devices_from_tvbox_push_receivers() {
        val devices = listOf(
            CastDeviceDto(id = "dlna:living", label = "客厅电视", serviceType = "dlna"),
            CastDeviceDto(id = "chromecast:kitchen", label = "厨房音箱", serviceType = "chromecast"),
            CastDeviceDto(id = "tvbox:http://192.168.1.45:9978", label = "影视盒子", serviceType = "tvbox"),
        )
        assertEquals(listOf("dlna:living", "chromecast:kitchen"), settingsDevicesForMode(devices, "cast").map { it.id })
        assertEquals(listOf("tvbox:http://192.168.1.45:9978"), settingsDevicesForMode(devices, "tvbox").map { it.id })
        assertTrue(isTvBoxDevice(devices.last()))
    }
}
