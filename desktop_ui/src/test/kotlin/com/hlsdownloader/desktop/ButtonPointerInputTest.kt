package com.hlsdownloader.desktop

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performMouseInput
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.v2.runSkikoComposeUiTest
import androidx.compose.ui.unit.dp
import kotlin.test.Test
import kotlin.test.assertEquals

/** Inject into an offscreen Compose scene, never the desktop mouse. */
@OptIn(ExperimentalTestApi::class)
class ButtonPointerInputTest {
    @Test
    fun tooltip_does_not_cancel_the_first_click() = runSkikoComposeUiTest {
        mainClock.autoAdvance = false
        var clicks = 0
        setContent {
            Column(Modifier.padding(64.dp)) {
                WorkbenchTooltip("Icon hint") {
                    IconButton(onClick = { clicks++ }, modifier = Modifier.size(36.dp).testTag("tooltip")) { Text("Icon") }
                }
            }
        }
        onNodeWithTag("tooltip").performMouseInput { moveTo(Offset(.25f, center.y)) }
        mainClock.advanceTimeBy(600)
        waitForIdle()
        onNodeWithTag("tooltip").performMouseInput { press() }
        mainClock.advanceTimeBy(150)
        waitForIdle()
        onNodeWithTag("tooltip").performMouseInput { moveBy(Offset(.1f, 0f)); release() }
        mainClock.advanceTimeBy(250)
        waitForIdle()
        assertEquals(1, clicks)
    }

    @Test
    fun first_mouse_click_survives_focus_and_pressed_state_recomposition() = runSkikoComposeUiTest {
        mainClock.autoAdvance = false
        val clicks = IntArray(3)
        setContent {
            Column {
                Button(
                    onClick = { clicks[0]++ },
                    border = BorderStroke(1.dp, Color.Transparent),
                    modifier = Modifier.size(120.dp, 40.dp).testTag("bordered"),
                ) { Text("Button") }
                TextButton(onClick = { clicks[1]++ }, modifier = Modifier.size(120.dp, 40.dp).testTag("text")) { Text("Text") }
                IconButton(onClick = { clicks[2]++ }, modifier = Modifier.size(40.dp).testTag("icon")) { Text("Icon") }
            }
        }
        for ((index, tag) in listOf("bordered", "text", "icon").withIndex()) {
            onNodeWithTag(tag).performMouseInput { moveTo(center); press() }
            mainClock.advanceTimeBy(250)
            waitForIdle()
            onNodeWithTag(tag).assertIsFocused()
            onNodeWithTag(tag).performMouseInput { release() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals(1, clicks[index], "First click on $tag must execute, not only focus")
            onNodeWithTag(tag).performKeyInput { keyDown(Key.Enter); keyUp(Key.Enter) }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals(2, clicks[index], "$tag must retain keyboard activation")
        }
    }

    @Test
    fun edge_click_survives_pressed_scale() = runSkikoComposeUiTest {
        mainClock.autoAdvance = false
        val clicks = IntArray(7)
        val tags = listOf("button", "text", "icon", "checkbox", "radio", "switch", "shared")
        setContent {
            Column {
                Button(onClick = { clicks[0]++ }, border = BorderStroke(1.dp, Color.Transparent), modifier = Modifier.size(160.dp, 40.dp).testTag(tags[0])) { Text("Button") }
                TextButton(onClick = { clicks[1]++ }, modifier = Modifier.size(160.dp, 40.dp).testTag(tags[1])) { Text("Text") }
                IconButton(onClick = { clicks[2]++ }, modifier = Modifier.size(40.dp).testTag(tags[2])) { Text("Icon") }
                Checkbox(false, { clicks[3]++ }, Modifier.testTag(tags[3]))
                RadioButton(false, { clicks[4]++ }, Modifier.testTag(tags[4]))
                Switch(false, { clicks[5]++ }, Modifier.testTag(tags[5]))
                val feedback = rememberPressFeedback()
                Text("Shared", Modifier.size(160.dp, 40.dp).testTag(tags[6])
                    .pressFeedback(feedback)
                    .clickable(interactionSource = feedback.interaction, indication = null) { clicks[6]++ })
            }
        }
        for ((index, tag) in tags.withIndex()) {
            onNodeWithTag(tag).performMouseInput { moveTo(Offset(.25f, center.y)); press() }
            mainClock.advanceTimeBy(250)
            waitForIdle()
            // A real mouse sends moves during the press, even when the hand barely shifts.
            onNodeWithTag(tag).performMouseInput { moveBy(Offset(.1f, 0f)); release() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals(1, clicks[index], "$tag: press animation must not cancel a click inside the resting control")

            onNodeWithTag(tag).performMouseInput { moveTo(center); press() }
            mainClock.advanceTimeBy(250)
            waitForIdle()
            onNodeWithTag(tag).performMouseInput { moveTo(Offset(-30f, center.y)); release() }
            mainClock.advanceTimeByFrame()
            waitForIdle()
            assertEquals(1, clicks[index], "$tag: dragging outside must still cancel the click")
        }
    }
}
