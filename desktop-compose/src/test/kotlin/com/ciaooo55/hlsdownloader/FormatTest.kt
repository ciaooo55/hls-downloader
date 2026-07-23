package com.ciaooo55.hlsdownloader

import kotlin.test.Test
import kotlin.test.assertEquals

class FormatTest {
    @Test fun formatsBytesAndEta() {
        assertEquals("1.0 KB", formatBytes(1024))
        assertEquals("1m 05s", formatEta(65.0))
    }

    @Test fun classifiesDownloads() {
        assertEquals("media", downloadCategory("video.mp4", ""))
        assertEquals("media", downloadCategory("第十二集：重新出发", "application/vnd.apple.mpegurl"))
        assertEquals("media", downloadCategory("manifest.mpd", "application/dash+xml"))
        assertEquals("program", downloadCategory("setup.exe", "application/octet-stream"))
        assertEquals("archive", downloadCategory("files.zip", ""))
    }
}
