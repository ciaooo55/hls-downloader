package com.hlsdownloader.desktop

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.FilterInputStream
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class CoreFrameChannelTest {
    @Test
    fun writesLittleEndianFramesAndReadsFragmentedConsecutiveFrames() {
        val encoded = ByteArrayOutputStream()
        val writer = CoreFrameChannel(DataInputStream(ByteArrayInputStream(byteArrayOf())), DataOutputStream(encoded), encoded)
        val payload = "中文".encodeToByteArray()
        writer.writeFrame(payload)
        writer.writeFrame(byteArrayOf(42))
        assertContentEquals(byteArrayOf(6, 0, 0, 0) + payload + byteArrayOf(1, 0, 0, 0, 42), encoded.toByteArray())

        val fragmented = object : FilterInputStream(ByteArrayInputStream(encoded.toByteArray())) {
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int = super.read(buffer, offset, minOf(length, 1))
        }
        val reader = CoreFrameChannel(DataInputStream(fragmented), DataOutputStream(ByteArrayOutputStream()), fragmented)
        assertContentEquals(payload, reader.readFrame())
        assertContentEquals(byteArrayOf(42), reader.readFrame())
        assertEquals("下载引擎连接已关闭", assertFailsWith<EOFException> { reader.readFrame() }.message)
    }

    @Test
    fun rejectsInvalidLengthBeforeReadingBody() {
        for (size in listOf(0, -1, CoreFrameChannel.MAX_FRAME + 1)) {
            val header = ByteArrayOutputStream().also { DataOutputStream(it).writeInt(Integer.reverseBytes(size)) }
            val input = DataInputStream(ByteArrayInputStream(header.toByteArray()))
            val channel = CoreFrameChannel(input, DataOutputStream(ByteArrayOutputStream()), input)
            assertFailsWith<IllegalArgumentException> { channel.readFrame() }
        }
    }

    @Test
    fun distinguishesTruncatedHeaderAndBody() {
        for ((bytes, message) in listOf(
            byteArrayOf(1, 0) to "下载引擎连接已关闭",
            byteArrayOf(2, 0, 0, 0, 42) to "下载引擎响应不完整",
        )) {
            val input = DataInputStream(ByteArrayInputStream(bytes))
            val channel = CoreFrameChannel(input, DataOutputStream(ByteArrayOutputStream()), input)
            assertEquals(message, assertFailsWith<EOFException> { channel.readFrame() }.message)
        }
    }

    @Test
    fun invalidRequestDoesNotWritePartialHeaderAndCloseReleasesTransport() {
        val output = ByteArrayOutputStream()
        var closed = false
        val channel = CoreFrameChannel(DataInputStream(ByteArrayInputStream(byteArrayOf())), DataOutputStream(output), Closeable { closed = true })
        assertFailsWith<IllegalArgumentException> { channel.writeFrame(byteArrayOf()) }
        assertFailsWith<IllegalArgumentException> { channel.writeFrame(ByteArray(CoreFrameChannel.MAX_FRAME + 1)) }
        assertEquals(0, output.size())
        channel.close()
        assertTrue(closed)
    }
}
