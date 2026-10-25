package com.hlsdownloader.desktop

import java.awt.EventQueue
import java.awt.Frame
import java.awt.Graphics
import java.awt.Panel
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@EnabledIfEnvironmentVariable(named = "HLS_VM_ACCEPTANCE", matches = "1")
class WorkbenchWindowTest {
    @Test
    fun single_instance_wake_restores_java_visibility_and_painting() {
        val instance = checkNotNull(WorkbenchInstanceLock.acquire())
        val frame = Frame("Workbench wake regression")
        var paintLatch = CountDownLatch(1)
        try {
            EventQueue.invokeAndWait {
                frame.add(object : Panel() {
                    override fun paint(graphics: Graphics) {
                        graphics.drawString("Workbench awake", 20, 30)
                        paintLatch.countDown()
                    }
                })
                frame.setSize(400, 240)
                WorkbenchWindow.awtWindow = frame
                frame.isVisible = true
            }
            assertTrue(paintLatch.await(10, TimeUnit.SECONDS), "首次显示应绘制")
            instance.listenForWake()
            repeat(3) {
                EventQueue.invokeAndWait {
                    WorkbenchWindow.hideToTray()
                    assertFalse(frame.isVisible)
                    paintLatch = CountDownLatch(1)
                }
                wakeRunningWorkbench()
                EventQueue.invokeAndWait { assertTrue(frame.isVisible, "唤醒须同步 Java 可见状态") }
                assertTrue(paintLatch.await(10, TimeUnit.SECONDS), "恢复后应再次绘制")
            }
        } finally {
            EventQueue.invokeAndWait {
                WorkbenchWindow.awtWindow = null
                frame.dispose()
            }
            instance.close()
        }
    }
}
