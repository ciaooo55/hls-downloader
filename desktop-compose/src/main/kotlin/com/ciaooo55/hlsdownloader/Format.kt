package com.ciaooo55.hlsdownloader

import java.net.URI
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import kotlin.math.ln
import kotlin.math.pow

fun formatBytes(value: Long): String {
    if (value <= 0) return "--"
    val units = listOf("B", "KB", "MB", "GB", "TB")
    val index = (ln(value.toDouble()) / ln(1024.0)).toInt().coerceIn(0, units.lastIndex)
    val amount = value / 1024.0.pow(index)
    return if (index == 0) "$value B" else "%.1f %s".format(amount, units[index])
}

fun formatSpeed(value: Double): String = if (value <= 0) "--" else "${formatBytes(value.toLong())}/s"

fun formatEta(value: Double): String {
    if (value <= 0 || !value.isFinite()) return "--"
    val seconds = value.toLong()
    return when {
        seconds < 60 -> "${seconds}s"
        seconds < 3600 -> "%dm %02ds".format(seconds / 60, seconds % 60)
        else -> "%dh %02dm".format(seconds / 3600, (seconds % 3600) / 60)
    }
}

fun formatDate(value: String): String = runCatching {
    DateTimeFormatter.ofPattern("MM-dd HH:mm").withZone(ZoneId.systemDefault()).format(Instant.parse(value))
}.getOrDefault(value.take(16).replace('T', ' '))

fun hostOf(value: String): String = runCatching { URI.create(value).host ?: value }.getOrDefault(value)

fun statusLabel(value: String) = when (value) {
    "queued" -> "等待中"
    "checking" -> "检查中"
    "fetching_metadata" -> "读取信息"
    "awaiting_selection" -> "选择文件"
    "downloading", "downloading_m3u8", "downloading_segments" -> "下载中"
    "parsing" -> "解析中"
    "merging" -> "合并中"
    "remuxing" -> "转封装"
    "pausing" -> "正在暂停"
    "paused" -> "已暂停"
    "done" -> "已完成"
    "failed" -> "失败"
    "canceled" -> "已取消"
    "unsupported" -> "不支持"
    else -> value.ifBlank { "未知" }
}

fun stageLabel(value: String) = when (value) {
    "downloading" -> "下载文件"
    "downloading_segments" -> "下载分片"
    "merging" -> "合并分片"
    "remuxing" -> "转封装"
    "parsing" -> "解析清单"
    else -> value.ifBlank { "--" }
}

fun downloadCategory(filename: String, mimeType: String): String {
    val value = filename.lowercase()
    val mime = mimeType.lowercase()
    return when {
        mime.startsWith("video/") || mime.startsWith("audio/") ||
            "mpegurl" in mime || "dash+xml" in mime ||
            value.matches(Regex(".*\\.(mp4|mkv|webm|mov|mp3|m4a|flac|m3u8|mpd)$")) -> "media"
        value.matches(Regex(".*\\.(exe|msi|apk|dmg|pkg)$")) -> "program"
        value.matches(Regex(".*\\.(zip|7z|rar|tar|gz|iso)$")) -> "archive"
        else -> "other"
    }
}

fun categoryLabel(value: String) = when (value) {
    "media" -> "媒体"
    "program" -> "程序"
    "archive" -> "压缩包"
    else -> "其他"
}
