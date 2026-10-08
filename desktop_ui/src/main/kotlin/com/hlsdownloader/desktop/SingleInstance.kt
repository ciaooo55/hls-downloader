package com.hlsdownloader.desktop

import java.nio.channels.FileChannel
import java.nio.channels.FileLock
import java.nio.channels.OverlappingFileLockException
import java.nio.file.Files
import java.nio.file.StandardOpenOption
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.io.DataInputStream
import java.io.DataOutputStream
import java.util.UUID

internal class WorkbenchInstanceLock private constructor(
    private val channel: FileChannel,
    private val lock: FileLock,
) : AutoCloseable {
    private var wakeServer: ServerSocket? = null

    fun listenForWake() {
        if (wakeServer != null) return
        val server = ServerSocket(0, 4, InetAddress.getLoopbackAddress())
        val token = UUID.randomUUID().toString()
        wakeServer = server
        Files.writeString(WorkbenchPaths.uiDirectory.resolve("workbench.wake"), "${server.localPort}\n$token")
        Thread({
            while (!server.isClosed) {
                runCatching {
                    server.accept().use { client ->
                        client.soTimeout = 1_000
                        val accepted = DataInputStream(client.getInputStream()).readUTF() == token
                        if (accepted) WorkbenchWindow.show()
                        DataOutputStream(client.getOutputStream()).writeBoolean(accepted)
                    }
                }
            }
        }, "workbench-wake").apply { isDaemon = true; start() }
    }

    override fun close() {
        try {
            wakeServer?.close()
            Files.deleteIfExists(WorkbenchPaths.uiDirectory.resolve("workbench.wake"))
            lock.release()
        } finally {
            channel.close()
        }
    }

    companion object {
        fun acquire(): WorkbenchInstanceLock? {
            val lockFile = WorkbenchPaths.uiDirectory.resolve("workbench.lock")
            Files.createDirectories(lockFile.parent)
            val channel = FileChannel.open(lockFile, StandardOpenOption.CREATE, StandardOpenOption.WRITE)
            val lock = try {
                channel.tryLock()
            } catch (_: OverlappingFileLockException) {
                null
            } catch (error: Exception) {
                channel.close()
                throw error
            }
            if (lock == null) channel.close()
            return lock?.let { WorkbenchInstanceLock(channel, it) }
        }
    }
}

internal fun wakeRunningWorkbench() {
    repeat(100) { attempt ->
        // 直接通知持锁工作台，恢复 Java 窗口状态；引擎退出或隐藏窗口暂停重组都不阻塞显示。
        val restored = runCatching {
            val endpoint = Files.readAllLines(WorkbenchPaths.uiDirectory.resolve("workbench.wake"))
            Socket().use { socket ->
                socket.connect(InetSocketAddress(InetAddress.getLoopbackAddress(), endpoint[0].toInt()), 300)
                socket.soTimeout = 1_000
                DataOutputStream(socket.getOutputStream()).writeUTF(endpoint[1])
                DataInputStream(socket.getInputStream()).readBoolean()
            }
        }.getOrDefault(false)
        if (restored) return
        if (attempt < 99) Thread.sleep(100)
    }
}
