package com.hlsdownloader.desktop

import java.io.File

internal fun workbenchSourceFiles(): List<File> =
    File("src/main/kotlin/com/hlsdownloader/desktop").walkTopDown()
        .filter { it.isFile && it.extension == "kt" }.sortedBy { it.path }.toList()

internal fun workbenchSources(): String = workbenchSourceFiles().joinToString("\n") { it.readText() }
