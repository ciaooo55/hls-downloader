package com.ciaooo55.hlsdownloader

import kotlinx.coroutines.delay
import java.nio.file.Files
import java.nio.file.Path
import kotlin.io.path.absolutePathString

data class BootstrapResult(
    val root: Path,
    val api: ApiClient,
    val existingInstance: Boolean,
    val coreProcess: Process? = null,
)

object DesktopRuntime {
    fun findRoot(): Path {
        val working = Path.of(System.getProperty("user.dir")).toAbsolutePath().normalize()
        val command = ProcessHandle.current().info().command().orElse("")
        val executable = command.takeIf { it.isNotBlank() }?.let { Path.of(it).toAbsolutePath().normalize().parent }
        val candidates = buildList {
            add(working)
            add(working.parent ?: working)
            if (executable != null) {
                add(executable)
                add(executable.parent ?: executable)
                add(executable.parent?.parent ?: executable)
            }
        }.distinct()
        return candidates.firstOrNull {
            Files.isRegularFile(it.resolve("HLSDownloaderCore.exe")) || Files.isDirectory(it.resolve("backend"))
        } ?: working
    }

    suspend fun bootstrap(): BootstrapResult {
        val root = findRoot()
        val (port, token) = loadApiConfig(root)
        val api = ApiClient(port, token)
        val alive = runCatching { api.health(); true }.getOrDefault(false)
        if (alive) {
            val presenter = runCatching { api.presenter() }.getOrNull()
            if (presenter?.session == true) {
                runCatching { api.activate() }
                return BootstrapResult(root, api, existingInstance = true)
            }
        }

        val process = if (alive) null else startCore(root)
        if (!alive) {
            var ready = false
            for (attempt in 0 until 120) {
                if (runCatching { api.health(); true }.getOrDefault(false)) {
                    ready = true
                    break
                }
                if (process?.isAlive == false) error("下载核心启动失败（退出码 ${process.exitValue()}）")
                delay(250)
            }
            if (!ready) error("下载核心未能在 30 秒内启动，请检查端口 $port 是否被占用")
        }
        api.startSession()
        return BootstrapResult(root, api, existingInstance = false, coreProcess = process)
    }

    private fun startCore(root: Path): Process {
        val packaged = root.resolve("HLSDownloaderCore.exe")
        val command = if (Files.isRegularFile(packaged)) {
            listOf(packaged.absolutePathString())
        } else {
            listOf(resolveDevelopmentPython(), root.resolve("backend/run_core.py").absolutePathString())
        }
        return ProcessBuilder(command)
            .directory(root.toFile())
            .redirectError(root.resolve("core-error.log").toFile())
            .redirectOutput(root.resolve("core.log").toFile())
            .start()
    }

    private fun resolveDevelopmentPython(): String {
        val candidates = buildList {
            System.getenv("PYTHON")?.takeIf { it.isNotBlank() }?.let(::add)
            System.getenv("CONDA_PREFIX")?.takeIf { it.isNotBlank() }
                ?.let { add(Path.of(it, "python.exe").absolutePathString()) }
            add("python.exe")
            add("python")
        }
        return candidates.firstOrNull { candidate ->
            if (candidate == "python.exe" || candidate == "python") {
                runCatching {
                    ProcessBuilder(candidate, "--version").start().waitFor() == 0
                }.getOrDefault(false)
            } else {
                Files.isRegularFile(Path.of(candidate))
            }
        } ?: error("未找到 Python 开发环境；发布版将使用内置下载核心")
    }

    suspend fun shutdown(result: BootstrapResult) {
        runCatching { result.api.stopSession() }
        runCatching { result.api.stopCore() }
        val process = result.coreProcess
        if (process != null && process.isAlive) {
            repeat(100) {
                if (!process.isAlive) return
                delay(100)
            }
            process.destroy()
            repeat(20) {
                if (!process.isAlive) return
                delay(100)
            }
            process.destroyForcibly()
        }
    }
}
