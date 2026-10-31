package com.hlsdownloader.desktop

import androidx.compose.foundation.background
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.hoverable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import com.hlsdownloader.desktop.icons.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.io.File

@Composable internal fun MediaSourcePickerDialog(mode: String, onDismiss: () -> Unit, onChoose: (MediaSourceSelection) -> Unit) {
    var source by remember { mutableStateOf("local") }
    var url by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    val verb = if (mode == "tvbox") "TVBox 推送" else "投屏"
    val validUrl = url.trim().let { it.startsWith("http://", true) || it.startsWith("https://", true) }
    WorkbenchDialog(onDismiss, "选择${verb}内容", "先选择媒体来源，再确认接收设备", 600.dp, content = {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            listOf("local" to Triple(Icons.Outlined.VideoFile, "本机文件", "共享电脑中的视频或音频"), "url" to Triple(Icons.Outlined.Link, "媒体链接", "发送设备可直接访问的链接")).forEach { (id, item) ->
                val active = source == id
                // 卡片约 290dp 宽、64dp 高，缩放会让内部图标+两行文字一起位移，所以只走底色。
                val feedback = rememberPressFeedback(
                    restColor = if (active) selectedSurface else surface2,
                    hoverColor = if (active) selectedSurface else surface3,
                    pressedColor = if (active) selectedSurface else pressedSurface,
                    pressScale = 1f,
                )
                Row(
                    Modifier.weight(1f).height(64.dp)
                        .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
                        .background(feedback.background, RoundedCornerShape(Radius.md))
                        .border(1.dp, if (active) blue else border, RoundedCornerShape(Radius.md))
                        .clip(RoundedCornerShape(Radius.md))
                        .hoverable(feedback.interaction)
                        .clickable(interactionSource = feedback.interaction, indication = null) { source = id; error = "" }
                        .padding(11.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(item.first, null, tint = if (active) blue else muted, modifier = Modifier.size(20.dp)); Spacer(Modifier.width(10.dp))
                    Column { Text(item.second, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold); Text(item.third, color = muted, fontSize = TypeScale.micro, maxLines = 1) }
                }
            }
        }
        Spacer(Modifier.height(14.dp))
        if (source == "local") {
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(12.dp), verticalAlignment = Alignment.Top) {
                Icon(Icons.Outlined.Info, null, tint = blue, modifier = Modifier.size(17.dp)); Spacer(Modifier.width(8.dp))
                Text("文件由下载引擎临时共享到局域网，支持电视端 Range 拖动；关闭主界面不会中断播放地址。", color = muted, fontSize = TypeScale.micro, lineHeight = TypeScale.lineMicro)
            }
        } else {
            OutlinedTextField(url, { url = it; error = "" }, Modifier.fillMaxWidth(), label = { Text("媒体链接") }, placeholder = { Text("https://example.com/video.mp4") }, singleLine = true, shape = RoundedCornerShape(Radius.md))
            Spacer(Modifier.height(8.dp))
            OutlinedTextField(title, { title = it }, Modifier.fillMaxWidth(), label = { Text("显示名称（可选）") }, singleLine = true, shape = RoundedCornerShape(Radius.md))
            Text("需要登录、Cookie 或即将过期的链接可能无法被电视直接访问。", color = muted, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 7.dp))
        }
        if (error.isNotEmpty()) Text(error, color = errorStrong, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 9.dp))
    }, actions = {
        DialogSecondary("取消", onClick = onDismiss)
        if (source == "local") DialogPrimary("选择本机文件") {
            val path = chooseMediaPath()
            if (path != null) onChoose(MediaSourceSelection(path = path, title = java.io.File(path).name))
        } else DialogPrimary("继续选择设备", validUrl) {
            if (validUrl) onChoose(MediaSourceSelection(url = url.trim(), title = title.trim())) else error = "请输入有效的 HTTP(S) 媒体链接"
        }
    })
}

@Composable internal fun DevicePickerDialog(
    signal: UiSignal.Devices,
    mode: String,
    source: MediaSourceSelection?,
    busy: Boolean,
    connecting: Boolean,
    preferredDeviceId: String,
    operationEpoch: Int,
    onDismiss: () -> Unit,
    onRescan: () -> Unit,
    onPublish: () -> Unit,
    onSelect: (CastDeviceDto) -> Unit,
) {
    val devices = if (mode == "tvbox") signal.devices.filter { it.serviceType.equals("tvbox", true) || it.id.startsWith("tvbox:") } else signal.devices.filterNot { it.serviceType.equals("tvbox", true) || it.id.startsWith("tvbox:") }
    val verb = if (mode == "tvbox") "TVBox 推送" else "投屏"
    var selectedId by remember(operationEpoch) { mutableStateOf(preferredDeviceId) }
    val selected = devices.firstOrNull { it.id == selectedId }
    WorkbenchDialog(onDismiss, "选择${verb}设备", if (mode == "tvbox") "自动搜索同一局域网内的 TVBox，发送前确认目标" else "自动搜索同一局域网内的 DLNA 和 Chromecast", 620.dp, dismissible = !connecting, content = {
        source?.let { media ->
            Surface(Modifier.fillMaxWidth(), color = selectedSurface, shape = RoundedCornerShape(Radius.md), border = BorderStroke(1.dp, blue.copy(alpha = .35f))) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(if (media.path.isNotBlank()) Icons.Outlined.VideoFile else Icons.Outlined.Link, null, tint = blue, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(9.dp))
                    Column(Modifier.weight(1f)) {
                        Text(media.title.ifBlank { "待发送媒体" }, color = ink, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(if (media.path.isNotBlank()) media.path else safeResourceLocation(media.url), color = muted, fontSize = TypeScale.micro, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
            Spacer(Modifier.height(10.dp))
        }
        when {
            busy -> repeat(3) { index ->
                Row(Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(Radius.md)).background(surface2).padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Surface(Modifier.size(34.dp), color = surface3, shape = RoundedCornerShape(Radius.md)) {}
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Surface(Modifier.width((150 + index * 28).dp).height(11.dp), color = surface3, shape = RoundedCornerShape(Radius.sm)) {}
                        Spacer(Modifier.height(8.dp)); Surface(Modifier.width(210.dp).height(8.dp), color = surface3, shape = RoundedCornerShape(Radius.sm)) {}
                    }
                }
            }
            devices.isEmpty() -> Box(Modifier.fillMaxWidth().height(170.dp), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(Icons.Outlined.Cast, null, tint = faint, modifier = Modifier.size(38.dp))
                    Spacer(Modifier.height(11.dp)); Text("没有发现可用设备", color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                    Text("确认电脑和电视连接到同一局域网，然后重新搜索。", color = muted, fontSize = TypeScale.caption, modifier = Modifier.padding(top = 4.dp))
                }
            }
            else -> devices.forEach { device ->
                val active = selected?.id == device.id
                val protocol = when {
                    device.serviceType.equals("chromecast", true) || device.id.startsWith("chromecast:") -> "Chromecast"
                    device.serviceType.equals("tvbox", true) || device.id.startsWith("tvbox:") -> "TVBox"
                    else -> "DLNA"
                }
                val metadata = if (device.label.contains(protocol, ignoreCase = true)) device.location else listOf(protocol, device.location).filter(String::isNotBlank).joinToString(" · ")
                // 设备卡是整行通宽，同上：只走底色档位，不做缩放。
                val feedback = rememberPressFeedback(
                    restColor = if (active) selectedSurface else surface2,
                    hoverColor = if (active) selectedSurface else surface3,
                    pressedColor = if (active) selectedSurface else pressedSurface,
                    pressScale = 1f,
                )
                Row(
                    Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(Radius.md))
                        .reportControlBounds("device.${device.id}")
                        .graphicsLayer { scaleX = feedback.scale; scaleY = feedback.scale }
                        .background(feedback.background)
                        .border(1.dp, if (active) blue else Color.Transparent, RoundedCornerShape(Radius.md))
                        .hoverable(feedback.interaction)
                        .clickable(enabled = !connecting, interactionSource = feedback.interaction, indication = null) { selectedId = device.id }
                        .padding(13.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Surface(Modifier.size(36.dp), color = if (active) blue else surface3, shape = RoundedCornerShape(Radius.md)) { Box(contentAlignment = Alignment.Center) { Icon(Icons.Outlined.Tv, null, tint = if (active) onBlue else muted, modifier = Modifier.size(20.dp)) } }
                    Spacer(Modifier.width(12.dp)); Column(Modifier.weight(1f)) {
                        Text(device.label, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(metadata, color = muted, fontSize = TypeScale.micro, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    if (active) Icon(Icons.Outlined.CheckCircle, "已选择", tint = blue, modifier = Modifier.size(19.dp))
                }
            }
        }
        if (!busy) {
            Spacer(Modifier.height(10.dp)); Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Outlined.Info, null, tint = muted, modifier = Modifier.size(16.dp)); Spacer(Modifier.width(8.dp))
                Text(if (mode == "tvbox") "TVBox 需要开启接收服务；确认推送后会在电视端打开媒体。" else "设备无法接收时，可发布局域网播放地址并在电视播放器中打开。", color = muted, fontSize = TypeScale.micro)
            }
        }
    }, actions = {
        if (!connecting) DialogSecondary("取消", onClick = onDismiss)
        TextButton(onClick = onRescan, enabled = !busy && !connecting, contentPadding = PaddingValues(horizontal = 12.dp), hoverColor = surface3, pressedColor = pressedSurface, modifier = Modifier.reportControlBounds("device.rescan")) { Icon(Icons.Outlined.Refresh, null, Modifier.size(16.dp)); Spacer(Modifier.width(5.dp)); Text("重新搜索", fontSize = TypeScale.body) }
        if (mode != "tvbox") DialogPrimary("局域网播放", !busy && !connecting, onPublish)
        DialogPrimary(if (connecting) "正在连接…" else if (mode == "tvbox") "确认推送" else "连接设备", selected != null && !busy && !connecting) { selected?.let(onSelect) }
    })
}
