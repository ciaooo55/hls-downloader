package com.hlsdownloader.desktop

import java.nio.channels.FileChannel
import java.nio.channels.FileLock
import java.nio.channels.OverlappingFileLockException
import java.nio.file.Files
import java.nio.file.StandardOpenOption

internal class WorkbenchInstanceLock private constructor(
    private val channel: FileChannel,
    private val lock: FileLock,
) : AutoCloseable {
    override fun close() {
        try {
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
        if (runCatching { EnginePipeClient.openMain() }.isSuccess) return
        if (attempt < 99) Thread.sleep(100)
    }
}
