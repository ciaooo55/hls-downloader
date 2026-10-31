package com.hlsdownloader.desktop

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.VerticalScrollbar
import androidx.compose.foundation.rememberScrollbarAdapter
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import com.hlsdownloader.desktop.icons.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

@Composable
internal fun QueueAssignDialog(
    count: Int,
    profiles: List<QueueProfileDto>,
    onDismiss: () -> Unit,
    onAssign: (String) -> Unit,
) {
    val available = profiles.filter { it.enabled }
    var selectedId by remember(profiles) { mutableStateOf(available.firstOrNull()?.id.orEmpty()) }
    WorkbenchDialog(
        onDismiss,
        "移动到队列",
        "为已选择的 $count 个任务指定调度队列",
        500.dp,
        content = {
            if (available.isEmpty()) {
                Surface(Modifier.fillMaxWidth(), color = surface2, shape = RoundedCornerShape(Radius.md)) {
                    Text("当前没有启用的下载队列，请先在队列管理中启用一个队列。", color = muted, fontSize = TypeScale.body, modifier = Modifier.padding(14.dp))
                }
            } else {
                available.forEach { profile ->
                    val selected = selectedId == profile.id
                    val feedback = rememberPressFeedback(
                        restColor = if (selected) selectedSurface else Color.Transparent,
                        hoverColor = if (selected) selectedSurface else surface3,
                        pressScale = .99f,
                    )
                    Row(
                        Modifier.fillMaxWidth().pressFeedback(feedback, RoundedCornerShape(Radius.md))
                            .clickable(interactionSource = feedback.interaction, indication = null) { selectedId = profile.id }
                            .padding(horizontal = 11.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(selected, { selectedId = profile.id }, accessibilityLabel = "选择队列 ${profile.name}")
                        Spacer(Modifier.width(8.dp))
                        Column(Modifier.weight(1f)) {
                            Text(profile.name, color = ink, fontSize = TypeScale.body, fontWeight = FontWeight.SemiBold)
                            val speed = if (profile.speedLimitKib > 0) " · ${profile.speedLimitKib} KiB/s" else " · 不限速"
                            Text("优先级 ${profile.priority} · 最多 ${profile.maxActive} 个活动任务$speed", color = faint, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 2.dp))
                        }
                    }
                }
            }
        },
        actions = {
            DialogSecondary("取消", onClick = onDismiss)
            DialogPrimary("移动", selectedId.isNotBlank()) { onAssign(selectedId) }
        },
    )
}

@Composable
internal fun QueueManagerDialog(
    current: List<QueueProfileDto>,
    onDismiss: () -> Unit,
    saving: Boolean = false,
    onSave: (List<QueueProfileDto>) -> Unit,
) {
    var profiles by remember(current) { mutableStateOf(current.ifEmpty { listOf(QueueProfileDto()) }) }
    var selectedId by remember(current) { mutableStateOf(profiles.first().id) }
    val selected = profiles.firstOrNull { it.id == selectedId } ?: profiles.first()
    var priorityInput by remember(selected.id) { mutableStateOf(selected.priority.toString()) }
    var maxActiveInput by remember(selected.id) { mutableStateOf(selected.maxActive.toString()) }
    var speedInput by remember(selected.id) { mutableStateOf(selected.speedLimitKib.toString()) }
    val priorityValid = priorityInput.toIntOrNull()?.let { it in -100..100 } == true
    val maxActiveValid = maxActiveInput.toIntOrNull()?.let { it in 1..64 } == true
    val speedValid = speedInput.toLongOrNull()?.let { it in 0..1_048_576 } == true
    val valid = queueProfilesValid(profiles) && priorityValid && maxActiveValid && speedValid
    fun update(transform: (QueueProfileDto) -> QueueProfileDto) {
        if (saving) return
        profiles = profiles.map { if (it.id == selected.id) transform(it) else it }
    }
    fun move(delta: Int) {
        val from = profiles.indexOfFirst { it.id == selected.id }
        val to = (from + delta).coerceIn(0, profiles.lastIndex)
        if (from == to) return
        profiles = profiles.toMutableList().also { list -> list.add(to, list.removeAt(from)) }
    }
    WorkbenchDialog(
        onDismiss,
        "下载队列",
        "不同队列按优先级、并发、限速、时间表和完成动作独立运行",
        780.dp,
        dismissible = !saving,
        scrollable = false,
        content = {
            Row(Modifier.fillMaxWidth().heightIn(min = 360.dp, max = 420.dp)) {
                Column(Modifier.width(218.dp).fillMaxHeight().padding(end = 14.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text("队列", color = muted, fontSize = TypeScale.caption, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        IconButton(onClick = {
                            val id = "queue-${System.currentTimeMillis().toString(36)}"
                            val names = profiles.map { it.name }.toSet()
                            val name = generateSequence(1) { it + 1 }.map { if (it == 1) "新队列" else "新队列 $it" }.first { it !in names }
                            profiles = profiles + QueueProfileDto(id = id, name = name, priority = ((profiles.maxOfOrNull { it.priority } ?: 0) + 1).coerceAtMost(100))
                            selectedId = id
                        }, enabled = !saving && profiles.size < 32, modifier = Modifier.size(30.dp)) { Icon(Icons.Outlined.Add, "新建队列", tint = blue, modifier = Modifier.size(18.dp)) }
                    }
                    Spacer(Modifier.height(5.dp))
                    Box(Modifier.weight(1f).fillMaxWidth()) {
                        val profilesScroll = rememberScrollState()
                        Column(Modifier.fillMaxSize().verticalScroll(profilesScroll).padding(end = 12.dp)) {
                            profiles.forEach { profile ->
                                val active = profile.id == selected.id
                                val feedback = rememberPressFeedback(
                                    restColor = if (active) selectedSurface else Color.Transparent,
                                    hoverColor = if (active) selectedSurface else surface3,
                                    pressScale = .99f,
                                )
                                Row(
                                    Modifier.fillMaxWidth().pressFeedback(feedback, RoundedCornerShape(Radius.md))
                                        .clickable(enabled = !saving, interactionSource = feedback.interaction, indication = null) { selectedId = profile.id }
                                        .padding(horizontal = 10.dp, vertical = 9.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Icon(if (profile.enabled) Icons.Outlined.PlayCircleOutline else Icons.Outlined.PauseCircleOutline, null, tint = if (profile.enabled) blue else faint, modifier = Modifier.size(17.dp))
                                    Spacer(Modifier.width(8.dp))
                                    Column(Modifier.weight(1f)) {
                                        Text(profile.name, color = ink, fontSize = TypeScale.body, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                        val speed = if (profile.speedLimitKib > 0) " · ${profile.speedLimitKib} KiB/s" else " · 不限速"
                                        Text("优先级 ${profile.priority} · 并发 ${profile.maxActive}$speed", color = faint, fontSize = TypeScale.micro, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                }
                            }
                        }
                        VerticalScrollbar(rememberScrollbarAdapter(profilesScroll), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp), style = workbenchScrollbarStyle())
                    }
                    HorizontalDivider(color = border)
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        IconButton(onClick = { move(-1) }, enabled = !saving && profiles.indexOf(selected) > 0) { Icon(Icons.Outlined.KeyboardArrowUp, "上移队列", tint = muted) }
                        IconButton(onClick = { move(1) }, enabled = !saving && profiles.indexOf(selected) < profiles.lastIndex) { Icon(Icons.Outlined.KeyboardArrowDown, "下移队列", tint = muted) }
                        IconButton(onClick = {
                            if (selected.id != "default" && profiles.size > 1) {
                                profiles = profiles.filterNot { it.id == selected.id }
                                selectedId = profiles.first().id
                            }
                        }, enabled = !saving && selected.id != "default" && profiles.size > 1) { Icon(Icons.Outlined.DeleteOutline, "删除队列", tint = if (selected.id == "default") faint else errorStrong) }
                    }
                }
                Box(Modifier.width(1.dp).fillMaxHeight().background(border))
                Box(Modifier.weight(1f).fillMaxHeight().padding(start = 16.dp)) {
                    val editorScroll = rememberScrollState()
                    Column(Modifier.fillMaxSize().verticalScroll(editorScroll).padding(end = 12.dp)) {
                        DialogLabel("队列名称")
                        OutlinedTextField(selected.name, { value -> update { it.copy(name = value.take(40)) } }, Modifier.fillMaxWidth(), enabled = !saving, singleLine = true, isError = selected.name.isBlank(), shape = RoundedCornerShape(Radius.md))
                        Spacer(Modifier.height(8.dp))
                        SettingRow("启用队列", "停用后保留任务，但不会开始新的下载", selected.enabled) { value -> update { it.copy(enabled = value) } }
                        Row(Modifier.fillMaxWidth()) {
                            Column(Modifier.weight(1f)) {
                                DialogLabel("优先级 (-100 至 100)")
                                OutlinedTextField(priorityInput, { value -> priorityInput = value; value.toIntOrNull()?.takeIf { it in -100..100 }?.let { number -> update { it.copy(priority = number) } } }, Modifier.fillMaxWidth(), enabled = !saving, singleLine = true, isError = !priorityValid, shape = RoundedCornerShape(Radius.md))
                            }
                            Spacer(Modifier.width(10.dp))
                            Column(Modifier.weight(1f)) {
                                DialogLabel("最大活动任务")
                                OutlinedTextField(maxActiveInput, { value -> maxActiveInput = value; value.toIntOrNull()?.takeIf { it in 1..64 }?.let { number -> update { it.copy(maxActive = number) } } }, Modifier.fillMaxWidth(), enabled = !saving, singleLine = true, isError = !maxActiveValid, shape = RoundedCornerShape(Radius.md))
                            }
                        }
                        Spacer(Modifier.height(8.dp))
                        DialogLabel("队列总限速（KiB/s）")
                        OutlinedTextField(
                            speedInput,
                            { value -> speedInput = value; value.toLongOrNull()?.takeIf { it in 0..1_048_576 }?.let { number -> update { it.copy(speedLimitKib = number) } } },
                            Modifier.fillMaxWidth(),
                            enabled = !saving,
                            isError = !speedValid,
                            singleLine = true,
                            supportingText = { Text("0 表示不限速；同一队列的活动任务共享该速度", color = faint, fontSize = TypeScale.micro) },
                            shape = RoundedCornerShape(Radius.md),
                        )
                        SettingRow("启用时间表", "只在指定星期和时段启动新任务", selected.scheduleEnabled) { value -> update { it.copy(scheduleEnabled = value) } }
                        if (selected.scheduleEnabled) {
                            Row(Modifier.fillMaxWidth()) {
                                Column(Modifier.weight(1f)) { DialogLabel("开始时间"); OutlinedTextField(selected.startTime, { value -> update { it.copy(startTime = value.take(5)) } }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)) }
                                Spacer(Modifier.width(10.dp))
                                Column(Modifier.weight(1f)) { DialogLabel("停止时间"); OutlinedTextField(selected.stopTime, { value -> update { it.copy(stopTime = value.take(5)) } }, Modifier.fillMaxWidth(), singleLine = true, shape = RoundedCornerShape(Radius.md)) }
                            }
                            Spacer(Modifier.height(8.dp)); DialogLabel("生效星期")
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                listOf("一", "二", "三", "四", "五", "六", "日").forEachIndexed { index, label ->
                                    val day = index + 1
                                    val days = selected.activeDays.split(',').mapNotNull(String::toIntOrNull)
                                    val active = day in days
                                    TextButton(onClick = { update { profile -> profile.copy(activeDays = (if (active) days - day else days + day).distinct().sorted().joinToString(",")) } }, modifier = Modifier.weight(1f), containerColor = segmentBackground(active, selectedSurface, surface2), contentPadding = PaddingValues(0.dp), hoverColor = if (active) selectedSurface else surface3, pressedColor = if (active) selectedSurface else pressedSurface, pressScale = SEGMENT_SCALE) { Text(label, color = if (active) blue else muted, fontSize = TypeScale.caption) }
                                }
                            }
                        }
                        Spacer(Modifier.height(8.dp))
                        DialogLabel("队列全部完成后")
                        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(Radius.md)).background(surface2).padding(3.dp)) {
                            POWER_ACTION_OPTIONS.forEach { (value, label) ->
                                val active = selected.completionAction == value
                                TextButton(
                                    onClick = { update { it.copy(completionAction = value) } },
                                    modifier = Modifier.weight(1f),
                                    containerColor = segmentBackground(active, selectedSurface, Color.Transparent),
                                    contentPadding = PaddingValues(horizontal = 4.dp), hoverColor = if (active) selectedSurface else surface3, pressedColor = if (active) selectedSurface else pressedSurface, pressScale = SEGMENT_SCALE,
                                ) { Text(label, color = if (active) blue else muted, fontSize = TypeScale.micro) }
                            }
                        }
                        Text("只有该队列中的任务全部成功完成后才会触发，执行前可取消。", color = faint, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 4.dp))
                        if (!valid) Text("请检查名称是否重复、时间与星期，以及优先级、并发和限速范围。", color = errorStrong, fontSize = TypeScale.micro, modifier = Modifier.padding(top = 8.dp))
                    }
                    VerticalScrollbar(rememberScrollbarAdapter(editorScroll), Modifier.align(Alignment.CenterEnd).fillMaxHeight().width(6.dp), style = workbenchScrollbarStyle())
                }
            }
        },
        actions = {
            DialogSecondary("取消", onClick = onDismiss, enabled = !saving)
            DialogPrimary(if (saving) "保存中…" else "保存队列", valid && !saving) { onSave(profiles) }
        },
    )
}
