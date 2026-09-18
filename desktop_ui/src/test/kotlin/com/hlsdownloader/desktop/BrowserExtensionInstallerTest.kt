package com.hlsdownloader.desktop

import java.nio.file.Files
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class BrowserExtensionInstallerTest {
    @Test
    fun unpack_rejects_paths_outside_the_extension_directory() {
        val root = Files.createTempDirectory("hls-extension-unpack")
        val archive = root.resolve("extension.zip")
        ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
            zip.putNextEntry(ZipEntry("../escaped.txt"))
            zip.write("escaped".toByteArray())
            zip.closeEntry()
        }

        assertFailsWith<IllegalArgumentException> {
            unpackChromiumExtension(archive, root.resolve("target"))
        }
        assertEquals(false, Files.exists(root.resolve("escaped.txt")))
    }
}
