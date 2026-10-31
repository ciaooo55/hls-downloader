package com.hlsdownloader.desktop

import java.nio.file.Files
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class BrowserExtensionInstallerTest {
    @Test
    fun unpack_accepts_regular_directories_and_unicode_files() {
        val root = Files.createTempDirectory("hls-extension-valid")
        try {
            val archive = root.resolve("extension.zip")
            ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
                zip.putNextEntry(ZipEntry("目录/"))
                zip.closeEntry()
                zip.putNextEntry(ZipEntry("目录/文件.txt"))
                zip.write("extension".toByteArray())
                zip.closeEntry()
            }
            unpackChromiumExtension(archive, root.resolve("target"))
            assertEquals("extension", Files.readString(root.resolve("target/目录/文件.txt")))
        } finally {
            root.toFile().deleteRecursively()
        }
    }

    @Test
    fun unpack_rejects_an_unbounded_expanded_body() {
        val root = Files.createTempDirectory("hls-extension-limit")
        try {
            val archive = root.resolve("extension.zip")
            for ((index, name) in listOf("oversized.bin", "directory/").withIndex()) {
                ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
                    zip.putNextEntry(ZipEntry(name))
                    val buffer = ByteArray(64 * 1024)
                    repeat(512) { zip.write(buffer) }
                    zip.write(0)
                    zip.closeEntry()
                }
                assertFailsWith<IllegalArgumentException>(name) { unpackChromiumExtension(archive, root.resolve("target-$index")) }
            }
        } finally {
            root.toFile().deleteRecursively()
        }
    }

    @Test
    fun unpack_applies_the_byte_limit_across_entries_and_accepts_the_exact_limit() {
        val root = Files.createTempDirectory("hls-extension-total")
        try {
            val archive = root.resolve("extension.zip")
            for (excess in listOf(false, true)) {
                ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
                    val buffer = ByteArray(64 * 1024)
                    for (name in listOf("first.bin", "second.bin")) {
                        zip.putNextEntry(ZipEntry(name))
                        repeat(256) { zip.write(buffer) }
                        zip.closeEntry()
                    }
                    if (excess) {
                        zip.putNextEntry(ZipEntry("excess.bin"))
                        zip.write(0)
                        zip.closeEntry()
                    }
                }
                val target = root.resolve("target-$excess")
                if (excess) {
                    assertFailsWith<IllegalArgumentException> { unpackChromiumExtension(archive, target) }
                } else {
                    unpackChromiumExtension(archive, target)
                    assertEquals(32L * 1024 * 1024, Files.size(target.resolve("first.bin")) + Files.size(target.resolve("second.bin")))
                }
            }
        } finally {
            root.toFile().deleteRecursively()
        }
    }

    @Test
    fun unpack_rejects_excessive_entries_and_windows_aliases() {
        val root = Files.createTempDirectory("hls-extension-entries")
        try {
            val archive = root.resolve("extension.zip")
            ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
                repeat(2049) { index ->
                    zip.putNextEntry(ZipEntry("$index.txt"))
                    zip.closeEntry()
                }
            }
            assertFailsWith<IllegalArgumentException> { unpackChromiumExtension(archive, root.resolve("many")) }
            for ((index, name) in listOf("dir/.. /outside.txt", "file:stream", "CON.txt", "COM¹.txt", "/absolute.txt", "dir//file.txt").withIndex()) {
                ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
                    zip.putNextEntry(ZipEntry(name))
                    zip.write(1)
                    zip.closeEntry()
                }
                assertFailsWith<IllegalArgumentException>(name) { unpackChromiumExtension(archive, root.resolve("alias-$index")) }
            }
        } finally {
            root.toFile().deleteRecursively()
        }
    }

    @Test
    fun unpack_rejects_case_insensitive_duplicate_paths() {
        val root = Files.createTempDirectory("hls-extension-duplicate")
        try {
            val archive = root.resolve("extension.zip")
            ZipOutputStream(Files.newOutputStream(archive)).use { zip ->
                for (name in listOf("file.txt", "FILE.txt")) {
                    zip.putNextEntry(ZipEntry(name))
                    zip.write(name.toByteArray())
                    zip.closeEntry()
                }
            }
            assertFailsWith<IllegalArgumentException> { unpackChromiumExtension(archive, root.resolve("target")) }
            assertEquals("file.txt", Files.readString(root.resolve("target/file.txt")))
        } finally {
            root.toFile().deleteRecursively()
        }
    }

    @Test
    fun unpack_rejects_paths_outside_the_extension_directory() {
        val root = Files.createTempDirectory("hls-extension-unpack")
        try {
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
        } finally {
            root.toFile().deleteRecursively()
        }
    }
}
