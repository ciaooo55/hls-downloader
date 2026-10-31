package com.hlsdownloader.desktop

import java.io.Closeable
import java.io.DataInput
import java.io.DataOutput
import java.io.EOFException
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** The same bounded little-endian frame format over a pipe or a socket. */
internal class CoreFrameChannel(
    private val input: DataInput,
    private val output: DataOutput,
    private val transport: Closeable,
) : AutoCloseable {
    fun writeFrame(payload: ByteArray) {
        require(payload.size in 1..MAX_FRAME) { "下载引擎请求长度无效" }
        output.write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(payload.size).array())
        output.write(payload)
    }

    fun readFrame(): ByteArray {
        val header = ByteArray(4)
        try {
            input.readFully(header)
        } catch (_: EOFException) {
            throw EOFException("下载引擎连接已关闭")
        }
        val size = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN).int
        require(size in 1..MAX_FRAME) { "下载引擎响应长度无效" }
        val body = ByteArray(size)
        try { input.readFully(body) } catch (_: EOFException) { throw EOFException("下载引擎响应不完整") }
        return body
    }

    override fun close() { transport.close() }

    companion object {
        const val MAX_FRAME = 4 * 1024 * 1024
    }
}
