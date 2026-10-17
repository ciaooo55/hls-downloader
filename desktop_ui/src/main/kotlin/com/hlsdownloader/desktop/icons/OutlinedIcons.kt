/*
 * Copyright 2024 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// Selected unchanged vectors from Compose Material Icons 1.7.3; only package/imports are adapted.
// Source archives and SHA256 are recorded in docs/package-slimming.md.

package com.hlsdownloader.desktop.icons

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.materialIcon
import androidx.compose.material.icons.materialPath
import androidx.compose.ui.graphics.vector.ImageVector

public val Icons.Outlined.Add: ImageVector
    get() {
        if (_add != null) {
            return _add!!
        }
        _add = materialIcon(name = "Outlined.Add") {
            materialPath {
                moveTo(19.0f, 13.0f)
                horizontalLineToRelative(-6.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(-6.0f)
                horizontalLineTo(5.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(6.0f)
                verticalLineTo(5.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(6.0f)
                verticalLineToRelative(2.0f)
                close()
            }
        }
        return _add!!
    }

private var _add: ImageVector? = null

public val Icons.Outlined.Apps: ImageVector
    get() {
        if (_apps != null) {
            return _apps!!
        }
        _apps = materialIcon(name = "Outlined.Apps") {
            materialPath {
                moveTo(4.0f, 8.0f)
                horizontalLineToRelative(4.0f)
                lineTo(8.0f, 4.0f)
                lineTo(4.0f, 4.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(10.0f, 20.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-4.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(4.0f, 20.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-4.0f)
                lineTo(4.0f, 16.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(4.0f, 14.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-4.0f)
                lineTo(4.0f, 10.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(10.0f, 14.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-4.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(16.0f, 4.0f)
                verticalLineToRelative(4.0f)
                horizontalLineToRelative(4.0f)
                lineTo(20.0f, 4.0f)
                horizontalLineToRelative(-4.0f)
                close()
                moveTo(10.0f, 8.0f)
                horizontalLineToRelative(4.0f)
                lineTo(14.0f, 4.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(16.0f, 14.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-4.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(4.0f)
                close()
                moveTo(16.0f, 20.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-4.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(4.0f)
                close()
            }
        }
        return _apps!!
    }

private var _apps: ImageVector? = null

public val Icons.Outlined.ArrowDownward: ImageVector
    get() {
        if (_arrowDownward != null) {
            return _arrowDownward!!
        }
        _arrowDownward = materialIcon(name = "Outlined.ArrowDownward") {
            materialPath {
                moveTo(20.0f, 12.0f)
                lineToRelative(-1.41f, -1.41f)
                lineTo(13.0f, 16.17f)
                verticalLineTo(4.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(12.17f)
                lineToRelative(-5.58f, -5.59f)
                lineTo(4.0f, 12.0f)
                lineToRelative(8.0f, 8.0f)
                lineToRelative(8.0f, -8.0f)
                close()
            }
        }
        return _arrowDownward!!
    }

private var _arrowDownward: ImageVector? = null

public val Icons.Outlined.ArrowUpward: ImageVector
    get() {
        if (_arrowUpward != null) {
            return _arrowUpward!!
        }
        _arrowUpward = materialIcon(name = "Outlined.ArrowUpward") {
            materialPath {
                moveTo(4.0f, 12.0f)
                lineToRelative(1.41f, 1.41f)
                lineTo(11.0f, 7.83f)
                verticalLineTo(20.0f)
                horizontalLineToRelative(2.0f)
                verticalLineTo(7.83f)
                lineToRelative(5.58f, 5.59f)
                lineTo(20.0f, 12.0f)
                lineToRelative(-8.0f, -8.0f)
                lineToRelative(-8.0f, 8.0f)
                close()
            }
        }
        return _arrowUpward!!
    }

private var _arrowUpward: ImageVector? = null

public val Icons.Outlined.Cast: ImageVector
    get() {
        if (_cast != null) {
            return _cast!!
        }
        _cast = materialIcon(name = "Outlined.Cast") {
            materialPath {
                moveTo(21.0f, 3.0f)
                lineTo(3.0f, 3.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(3.0f)
                horizontalLineToRelative(2.0f)
                lineTo(3.0f, 5.0f)
                horizontalLineToRelative(18.0f)
                verticalLineToRelative(14.0f)
                horizontalLineToRelative(-7.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(7.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(23.0f, 5.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(1.0f, 18.0f)
                verticalLineToRelative(3.0f)
                horizontalLineToRelative(3.0f)
                curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
                close()
                moveTo(1.0f, 14.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(2.76f, 0.0f, 5.0f, 2.24f, 5.0f, 5.0f)
                horizontalLineToRelative(2.0f)
                curveToRelative(0.0f, -3.87f, -3.13f, -7.0f, -7.0f, -7.0f)
                close()
                moveTo(1.0f, 10.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(4.97f, 0.0f, 9.0f, 4.03f, 9.0f, 9.0f)
                horizontalLineToRelative(2.0f)
                curveToRelative(0.0f, -6.08f, -4.93f, -11.0f, -11.0f, -11.0f)
                close()
            }
        }
        return _cast!!
    }

private var _cast: ImageVector? = null

public val Icons.Outlined.CastConnected: ImageVector
    get() {
        if (_castConnected != null) {
            return _castConnected!!
        }
        _castConnected = materialIcon(name = "Outlined.CastConnected") {
            materialPath {
                moveTo(1.0f, 18.0f)
                verticalLineToRelative(3.0f)
                horizontalLineToRelative(3.0f)
                curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
                close()
                moveTo(1.0f, 14.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(2.76f, 0.0f, 5.0f, 2.24f, 5.0f, 5.0f)
                horizontalLineToRelative(2.0f)
                curveToRelative(0.0f, -3.87f, -3.13f, -7.0f, -7.0f, -7.0f)
                close()
                moveTo(1.0f, 10.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(4.97f, 0.0f, 9.0f, 4.03f, 9.0f, 9.0f)
                horizontalLineToRelative(2.0f)
                curveToRelative(0.0f, -6.08f, -4.93f, -11.0f, -11.0f, -11.0f)
                close()
                moveTo(21.0f, 3.0f)
                lineTo(3.0f, 3.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(3.0f)
                horizontalLineToRelative(2.0f)
                lineTo(3.0f, 5.0f)
                horizontalLineToRelative(18.0f)
                verticalLineToRelative(14.0f)
                horizontalLineToRelative(-7.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(7.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(23.0f, 5.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(5.0f, 7.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(12.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(-3.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(5.0f)
                lineTo(19.0f, 7.0f)
                close()
            }
        }
        return _castConnected!!
    }

private var _castConnected: ImageVector? = null

public val Icons.Outlined.CheckCircle: ImageVector
    get() {
        if (_checkCircle != null) {
            return _checkCircle!!
        }
        _checkCircle = materialIcon(name = "Outlined.CheckCircle") {
            materialPath {
                moveTo(12.0f, 2.0f)
                curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
                reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
                reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, -3.59f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.59f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.59f, 8.0f, 8.0f)
                reflectiveCurveToRelative(-3.59f, 8.0f, -8.0f, 8.0f)
                close()
                moveTo(16.59f, 7.58f)
                lineTo(10.0f, 14.17f)
                lineToRelative(-2.59f, -2.58f)
                lineTo(6.0f, 13.0f)
                lineToRelative(4.0f, 4.0f)
                lineToRelative(8.0f, -8.0f)
                close()
            }
        }
        return _checkCircle!!
    }

private var _checkCircle: ImageVector? = null

public val Icons.Outlined.Close: ImageVector
    get() {
        if (_close != null) {
            return _close!!
        }
        _close = materialIcon(name = "Outlined.Close") {
            materialPath {
                moveTo(19.0f, 6.41f)
                lineTo(17.59f, 5.0f)
                lineTo(12.0f, 10.59f)
                lineTo(6.41f, 5.0f)
                lineTo(5.0f, 6.41f)
                lineTo(10.59f, 12.0f)
                lineTo(5.0f, 17.59f)
                lineTo(6.41f, 19.0f)
                lineTo(12.0f, 13.41f)
                lineTo(17.59f, 19.0f)
                lineTo(19.0f, 17.59f)
                lineTo(13.41f, 12.0f)
                lineTo(19.0f, 6.41f)
                close()
            }
        }
        return _close!!
    }

private var _close: ImageVector? = null

public val Icons.Outlined.ContentCopy: ImageVector
    get() {
        if (_contentCopy != null) {
            return _contentCopy!!
        }
        _contentCopy = materialIcon(name = "Outlined.ContentCopy") {
            materialPath {
                moveTo(16.0f, 1.0f)
                lineTo(4.0f, 1.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(14.0f)
                horizontalLineToRelative(2.0f)
                lineTo(4.0f, 3.0f)
                horizontalLineToRelative(12.0f)
                lineTo(16.0f, 1.0f)
                close()
                moveTo(19.0f, 5.0f)
                lineTo(8.0f, 5.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(14.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(11.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(21.0f, 7.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(19.0f, 21.0f)
                lineTo(8.0f, 21.0f)
                lineTo(8.0f, 7.0f)
                horizontalLineToRelative(11.0f)
                verticalLineToRelative(14.0f)
                close()
            }
        }
        return _contentCopy!!
    }

private var _contentCopy: ImageVector? = null

public val Icons.Outlined.ContentPaste: ImageVector
    get() {
        if (_contentPaste != null) {
            return _contentPaste!!
        }
        _contentPaste = materialIcon(name = "Outlined.ContentPaste") {
            materialPath {
                moveTo(19.0f, 2.0f)
                horizontalLineToRelative(-4.18f)
                curveTo(14.4f, 0.84f, 13.3f, 0.0f, 12.0f, 0.0f)
                reflectiveCurveTo(9.6f, 0.84f, 9.18f, 2.0f)
                lineTo(5.0f, 2.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(16.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(14.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(21.0f, 4.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(12.0f, 2.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, 0.45f, 1.0f, 1.0f)
                reflectiveCurveToRelative(-0.45f, 1.0f, -1.0f, 1.0f)
                reflectiveCurveToRelative(-1.0f, -0.45f, -1.0f, -1.0f)
                reflectiveCurveToRelative(0.45f, -1.0f, 1.0f, -1.0f)
                close()
                moveTo(19.0f, 20.0f)
                lineTo(5.0f, 20.0f)
                lineTo(5.0f, 4.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(3.0f)
                horizontalLineToRelative(10.0f)
                lineTo(17.0f, 4.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(16.0f)
                close()
            }
        }
        return _contentPaste!!
    }

private var _contentPaste: ImageVector? = null

public val Icons.Outlined.CropSquare: ImageVector
    get() {
        if (_cropSquare != null) {
            return _cropSquare!!
        }
        _cropSquare = materialIcon(name = "Outlined.CropSquare") {
            materialPath {
                moveTo(18.0f, 4.0f)
                lineTo(6.0f, 4.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(12.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(12.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(20.0f, 6.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(18.0f, 18.0f)
                lineTo(6.0f, 18.0f)
                lineTo(6.0f, 6.0f)
                horizontalLineToRelative(12.0f)
                verticalLineToRelative(12.0f)
                close()
            }
        }
        return _cropSquare!!
    }

private var _cropSquare: ImageVector? = null

public val Icons.Outlined.DarkMode: ImageVector
    get() {
        if (_darkMode != null) {
            return _darkMode!!
        }
        _darkMode = materialIcon(name = "Outlined.DarkMode") {
            materialPath {
                moveTo(9.37f, 5.51f)
                curveTo(9.19f, 6.15f, 9.1f, 6.82f, 9.1f, 7.5f)
                curveToRelative(0.0f, 4.08f, 3.32f, 7.4f, 7.4f, 7.4f)
                curveToRelative(0.68f, 0.0f, 1.35f, -0.09f, 1.99f, -0.27f)
                curveTo(17.45f, 17.19f, 14.93f, 19.0f, 12.0f, 19.0f)
                curveToRelative(-3.86f, 0.0f, -7.0f, -3.14f, -7.0f, -7.0f)
                curveTo(5.0f, 9.07f, 6.81f, 6.55f, 9.37f, 5.51f)
                close()
                moveTo(12.0f, 3.0f)
                curveToRelative(-4.97f, 0.0f, -9.0f, 4.03f, -9.0f, 9.0f)
                reflectiveCurveToRelative(4.03f, 9.0f, 9.0f, 9.0f)
                reflectiveCurveToRelative(9.0f, -4.03f, 9.0f, -9.0f)
                curveToRelative(0.0f, -0.46f, -0.04f, -0.92f, -0.1f, -1.36f)
                curveToRelative(-0.98f, 1.37f, -2.58f, 2.26f, -4.4f, 2.26f)
                curveToRelative(-2.98f, 0.0f, -5.4f, -2.42f, -5.4f, -5.4f)
                curveToRelative(0.0f, -1.81f, 0.89f, -3.42f, 2.26f, -4.4f)
                curveTo(12.92f, 3.04f, 12.46f, 3.0f, 12.0f, 3.0f)
                lineTo(12.0f, 3.0f)
                close()
            }
        }
        return _darkMode!!
    }

private var _darkMode: ImageVector? = null

public val Icons.Outlined.DeleteOutline: ImageVector
    get() {
        if (_deleteOutline != null) {
            return _deleteOutline!!
        }
        _deleteOutline = materialIcon(name = "Outlined.DeleteOutline") {
            materialPath {
                moveTo(6.0f, 19.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(8.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(18.0f, 7.0f)
                lineTo(6.0f, 7.0f)
                verticalLineToRelative(12.0f)
                close()
                moveTo(8.0f, 9.0f)
                horizontalLineToRelative(8.0f)
                verticalLineToRelative(10.0f)
                lineTo(8.0f, 19.0f)
                lineTo(8.0f, 9.0f)
                close()
                moveTo(15.5f, 4.0f)
                lineToRelative(-1.0f, -1.0f)
                horizontalLineToRelative(-5.0f)
                lineToRelative(-1.0f, 1.0f)
                lineTo(5.0f, 4.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(14.0f)
                lineTo(19.0f, 4.0f)
                horizontalLineToRelative(-3.5f)
                close()
            }
        }
        return _deleteOutline!!
    }

private var _deleteOutline: ImageVector? = null

public val Icons.Outlined.DeleteSweep: ImageVector
    get() {
        if (_deleteSweep != null) {
            return _deleteSweep!!
        }
        _deleteSweep = materialIcon(name = "Outlined.DeleteSweep") {
            materialPath {
                moveTo(15.0f, 16.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(-4.0f)
                close()
                moveTo(15.0f, 8.0f)
                horizontalLineToRelative(7.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(-7.0f)
                close()
                moveTo(15.0f, 12.0f)
                horizontalLineToRelative(6.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(-6.0f)
                close()
                moveTo(3.0f, 18.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(6.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(13.0f, 8.0f)
                lineTo(3.0f, 8.0f)
                verticalLineToRelative(10.0f)
                close()
                moveTo(5.0f, 10.0f)
                horizontalLineToRelative(6.0f)
                verticalLineToRelative(8.0f)
                lineTo(5.0f, 18.0f)
                verticalLineToRelative(-8.0f)
                close()
                moveTo(10.0f, 4.0f)
                lineTo(6.0f, 4.0f)
                lineTo(5.0f, 5.0f)
                lineTo(2.0f, 5.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(12.0f)
                lineTo(14.0f, 5.0f)
                horizontalLineToRelative(-3.0f)
                close()
            }
        }
        return _deleteSweep!!
    }

private var _deleteSweep: ImageVector? = null

public val Icons.Outlined.Devices: ImageVector
    get() {
        if (_devices != null) {
            return _devices!!
        }
        _devices = materialIcon(name = "Outlined.Devices") {
            materialPath {
                moveTo(4.0f, 6.0f)
                horizontalLineToRelative(18.0f)
                lineTo(22.0f, 4.0f)
                lineTo(4.0f, 4.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(11.0f)
                lineTo(0.0f, 17.0f)
                verticalLineToRelative(3.0f)
                horizontalLineToRelative(14.0f)
                verticalLineToRelative(-3.0f)
                lineTo(4.0f, 17.0f)
                lineTo(4.0f, 6.0f)
                close()
                moveTo(23.0f, 8.0f)
                horizontalLineToRelative(-6.0f)
                curveToRelative(-0.55f, 0.0f, -1.0f, 0.45f, -1.0f, 1.0f)
                verticalLineToRelative(10.0f)
                curveToRelative(0.0f, 0.55f, 0.45f, 1.0f, 1.0f, 1.0f)
                horizontalLineToRelative(6.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, -0.45f, 1.0f, -1.0f)
                lineTo(24.0f, 9.0f)
                curveToRelative(0.0f, -0.55f, -0.45f, -1.0f, -1.0f, -1.0f)
                close()
                moveTo(22.0f, 17.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(-7.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(7.0f)
                close()
            }
        }
        return _devices!!
    }

private var _devices: ImageVector? = null

public val Icons.Outlined.Downloading: ImageVector
    get() {
        if (_downloading != null) {
            return _downloading!!
        }
        _downloading = materialIcon(name = "Outlined.Downloading") {
            materialPath {
                moveTo(18.32f, 4.26f)
                curveTo(16.84f, 3.05f, 15.01f, 2.25f, 13.0f, 2.05f)
                verticalLineToRelative(2.02f)
                curveToRelative(1.46f, 0.18f, 2.79f, 0.76f, 3.9f, 1.62f)
                lineTo(18.32f, 4.26f)
                close()
                moveTo(19.93f, 11.0f)
                horizontalLineToRelative(2.02f)
                curveToRelative(-0.2f, -2.01f, -1.0f, -3.84f, -2.21f, -5.32f)
                lineTo(18.31f, 7.1f)
                curveTo(19.17f, 8.21f, 19.75f, 9.54f, 19.93f, 11.0f)
                close()
                moveTo(18.31f, 16.9f)
                lineToRelative(1.43f, 1.43f)
                curveToRelative(1.21f, -1.48f, 2.01f, -3.32f, 2.21f, -5.32f)
                horizontalLineToRelative(-2.02f)
                curveTo(19.75f, 14.46f, 19.17f, 15.79f, 18.31f, 16.9f)
                close()
                moveTo(13.0f, 19.93f)
                verticalLineToRelative(2.02f)
                curveToRelative(2.01f, -0.2f, 3.84f, -1.0f, 5.32f, -2.21f)
                lineToRelative(-1.43f, -1.43f)
                curveTo(15.79f, 19.17f, 14.46f, 19.75f, 13.0f, 19.93f)
                close()
                moveTo(15.59f, 10.59f)
                lineTo(13.0f, 13.17f)
                verticalLineTo(7.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(6.17f)
                lineToRelative(-2.59f, -2.59f)
                lineTo(7.0f, 12.0f)
                lineToRelative(5.0f, 5.0f)
                lineToRelative(5.0f, -5.0f)
                lineTo(15.59f, 10.59f)
                close()
                moveTo(11.0f, 19.93f)
                verticalLineToRelative(2.02f)
                curveToRelative(-5.05f, -0.5f, -9.0f, -4.76f, -9.0f, -9.95f)
                reflectiveCurveToRelative(3.95f, -9.45f, 9.0f, -9.95f)
                verticalLineToRelative(2.02f)
                curveTo(7.05f, 4.56f, 4.0f, 7.92f, 4.0f, 12.0f)
                reflectiveCurveTo(7.05f, 19.44f, 11.0f, 19.93f)
                close()
            }
        }
        return _downloading!!
    }

private var _downloading: ImageVector? = null

public val Icons.Outlined.DragHandle: ImageVector
    get() {
        if (_dragHandle != null) {
            return _dragHandle!!
        }
        _dragHandle = materialIcon(name = "Outlined.DragHandle") {
            materialPath {
                moveTo(20.0f, 9.0f)
                horizontalLineTo(4.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(16.0f)
                verticalLineTo(9.0f)
                close()
                moveTo(4.0f, 15.0f)
                horizontalLineToRelative(16.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineTo(4.0f)
                verticalLineToRelative(2.0f)
                close()
            }
        }
        return _dragHandle!!
    }

private var _dragHandle: ImageVector? = null

public val Icons.Outlined.ErrorOutline: ImageVector
    get() {
        if (_errorOutline != null) {
            return _errorOutline!!
        }
        _errorOutline = materialIcon(name = "Outlined.ErrorOutline") {
            materialPath {
                moveTo(11.0f, 15.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(-2.0f)
                close()
                moveTo(11.0f, 7.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(-2.0f)
                lineTo(11.0f, 7.0f)
                close()
                moveTo(11.99f, 2.0f)
                curveTo(6.47f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.47f, 10.0f, 9.99f, 10.0f)
                curveTo(17.52f, 22.0f, 22.0f, 17.52f, 22.0f, 12.0f)
                reflectiveCurveTo(17.52f, 2.0f, 11.99f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.42f, 0.0f, -8.0f, -3.58f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.58f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.58f, 8.0f, 8.0f)
                reflectiveCurveToRelative(-3.58f, 8.0f, -8.0f, 8.0f)
                close()
            }
        }
        return _errorOutline!!
    }

private var _errorOutline: ImageVector? = null

public val Icons.Outlined.ExpandLess: ImageVector
    get() {
        if (_expandLess != null) {
            return _expandLess!!
        }
        _expandLess = materialIcon(name = "Outlined.ExpandLess") {
            materialPath {
                moveTo(12.0f, 8.0f)
                lineToRelative(-6.0f, 6.0f)
                lineToRelative(1.41f, 1.41f)
                lineTo(12.0f, 10.83f)
                lineToRelative(4.59f, 4.58f)
                lineTo(18.0f, 14.0f)
                lineToRelative(-6.0f, -6.0f)
                close()
            }
        }
        return _expandLess!!
    }

private var _expandLess: ImageVector? = null

public val Icons.Outlined.ExpandMore: ImageVector
    get() {
        if (_expandMore != null) {
            return _expandMore!!
        }
        _expandMore = materialIcon(name = "Outlined.ExpandMore") {
            materialPath {
                moveTo(16.59f, 8.59f)
                lineTo(12.0f, 13.17f)
                lineTo(7.41f, 8.59f)
                lineTo(6.0f, 10.0f)
                lineToRelative(6.0f, 6.0f)
                lineToRelative(6.0f, -6.0f)
                lineToRelative(-1.41f, -1.41f)
                close()
            }
        }
        return _expandMore!!
    }

private var _expandMore: ImageVector? = null

public val Icons.Outlined.Extension: ImageVector
    get() {
        if (_extension != null) {
            return _extension!!
        }
        _extension = materialIcon(name = "Outlined.Extension") {
            materialPath {
                moveTo(10.5f, 4.5f)
                curveToRelative(0.28f, 0.0f, 0.5f, 0.22f, 0.5f, 0.5f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(6.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(2.0f)
                curveToRelative(0.28f, 0.0f, 0.5f, 0.22f, 0.5f, 0.5f)
                reflectiveCurveToRelative(-0.22f, 0.5f, -0.5f, 0.5f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(-2.12f)
                curveToRelative(-0.68f, -1.75f, -2.39f, -3.0f, -4.38f, -3.0f)
                reflectiveCurveToRelative(-3.7f, 1.25f, -4.38f, 3.0f)
                horizontalLineTo(4.0f)
                verticalLineToRelative(-2.12f)
                curveToRelative(1.75f, -0.68f, 3.0f, -2.39f, 3.0f, -4.38f)
                curveToRelative(0.0f, -1.99f, -1.24f, -3.7f, -2.99f, -4.38f)
                lineTo(4.0f, 7.0f)
                horizontalLineToRelative(6.0f)
                verticalLineTo(5.0f)
                curveToRelative(0.0f, -0.28f, 0.22f, -0.5f, 0.5f, -0.5f)
                moveToRelative(0.0f, -2.0f)
                curveTo(9.12f, 2.5f, 8.0f, 3.62f, 8.0f, 5.0f)
                horizontalLineTo(4.0f)
                curveToRelative(-1.1f, 0.0f, -1.99f, 0.9f, -1.99f, 2.0f)
                verticalLineToRelative(3.8f)
                horizontalLineToRelative(0.29f)
                curveToRelative(1.49f, 0.0f, 2.7f, 1.21f, 2.7f, 2.7f)
                reflectiveCurveToRelative(-1.21f, 2.7f, -2.7f, 2.7f)
                horizontalLineTo(2.0f)
                verticalLineTo(20.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(3.8f)
                verticalLineToRelative(-0.3f)
                curveToRelative(0.0f, -1.49f, 1.21f, -2.7f, 2.7f, -2.7f)
                reflectiveCurveToRelative(2.7f, 1.21f, 2.7f, 2.7f)
                verticalLineToRelative(0.3f)
                horizontalLineTo(17.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                verticalLineToRelative(-4.0f)
                curveToRelative(1.38f, 0.0f, 2.5f, -1.12f, 2.5f, -2.5f)
                reflectiveCurveTo(20.38f, 11.0f, 19.0f, 11.0f)
                verticalLineTo(7.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                horizontalLineToRelative(-4.0f)
                curveToRelative(0.0f, -1.38f, -1.12f, -2.5f, -2.5f, -2.5f)
                close()
            }
        }
        return _extension!!
    }

private var _extension: ImageVector? = null

public val Icons.Outlined.FileDownload: ImageVector
    get() {
        if (_fileDownload != null) {
            return _fileDownload!!
        }
        _fileDownload = materialIcon(name = "Outlined.FileDownload") {
            materialPath {
                moveTo(18.0f, 15.0f)
                verticalLineToRelative(3.0f)
                horizontalLineTo(6.0f)
                verticalLineToRelative(-3.0f)
                horizontalLineTo(4.0f)
                verticalLineToRelative(3.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(12.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                verticalLineToRelative(-3.0f)
                horizontalLineTo(18.0f)
                close()
                moveTo(17.0f, 11.0f)
                lineToRelative(-1.41f, -1.41f)
                lineTo(13.0f, 12.17f)
                verticalLineTo(4.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(8.17f)
                lineTo(8.41f, 9.59f)
                lineTo(7.0f, 11.0f)
                lineToRelative(5.0f, 5.0f)
                lineTo(17.0f, 11.0f)
                close()
            }
        }
        return _fileDownload!!
    }

private var _fileDownload: ImageVector? = null

public val Icons.Outlined.FilterNone: ImageVector
    get() {
        if (_filterNone != null) {
            return _filterNone!!
        }
        _filterNone = materialIcon(name = "Outlined.FilterNone") {
            materialPath {
                moveTo(3.0f, 5.0f)
                lineTo(1.0f, 5.0f)
                verticalLineToRelative(16.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(16.0f)
                verticalLineToRelative(-2.0f)
                lineTo(3.0f, 21.0f)
                lineTo(3.0f, 5.0f)
                close()
                moveTo(21.0f, 1.0f)
                lineTo(7.0f, 1.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(14.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(14.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(23.0f, 3.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(21.0f, 17.0f)
                lineTo(7.0f, 17.0f)
                lineTo(7.0f, 3.0f)
                horizontalLineToRelative(14.0f)
                verticalLineToRelative(14.0f)
                close()
            }
        }
        return _filterNone!!
    }

private var _filterNone: ImageVector? = null

public val Icons.Outlined.Folder: ImageVector
    get() {
        if (_folder != null) {
            return _folder!!
        }
        _folder = materialIcon(name = "Outlined.Folder") {
            materialPath {
                moveTo(9.17f, 6.0f)
                lineToRelative(2.0f, 2.0f)
                horizontalLineTo(20.0f)
                verticalLineToRelative(10.0f)
                horizontalLineTo(4.0f)
                verticalLineTo(6.0f)
                horizontalLineToRelative(5.17f)
                moveTo(10.0f, 4.0f)
                horizontalLineTo(4.0f)
                curveToRelative(-1.1f, 0.0f, -1.99f, 0.9f, -1.99f, 2.0f)
                lineTo(2.0f, 18.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(16.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                verticalLineTo(8.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                horizontalLineToRelative(-8.0f)
                lineToRelative(-2.0f, -2.0f)
                close()
            }
        }
        return _folder!!
    }

private var _folder: ImageVector? = null

public val Icons.Outlined.FolderOpen: ImageVector
    get() {
        if (_folderOpen != null) {
            return _folderOpen!!
        }
        _folderOpen = materialIcon(name = "Outlined.FolderOpen") {
            materialPath {
                moveTo(20.0f, 6.0f)
                horizontalLineToRelative(-8.0f)
                lineToRelative(-2.0f, -2.0f)
                lineTo(4.0f, 4.0f)
                curveToRelative(-1.1f, 0.0f, -1.99f, 0.9f, -1.99f, 2.0f)
                lineTo(2.0f, 18.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(16.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(22.0f, 8.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(20.0f, 18.0f)
                lineTo(4.0f, 18.0f)
                lineTo(4.0f, 8.0f)
                horizontalLineToRelative(16.0f)
                verticalLineToRelative(10.0f)
                close()
            }
        }
        return _folderOpen!!
    }

private var _folderOpen: ImageVector? = null

public val Icons.Outlined.FolderZip: ImageVector
    get() {
        if (_folderZip != null) {
            return _folderZip!!
        }
        _folderZip = materialIcon(name = "Outlined.FolderZip") {
            materialPath {
                moveTo(20.0f, 6.0f)
                horizontalLineToRelative(-8.0f)
                lineToRelative(-2.0f, -2.0f)
                horizontalLineTo(4.0f)
                curveTo(2.9f, 4.0f, 2.01f, 4.9f, 2.01f, 6.0f)
                lineTo(2.0f, 18.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(16.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                verticalLineTo(8.0f)
                curveTo(22.0f, 6.9f, 21.1f, 6.0f, 20.0f, 6.0f)
                close()
                moveTo(16.0f, 16.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineTo(8.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(10.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineTo(16.0f)
                close()
                moveTo(16.0f, 16.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(2.0f)
                horizontalLineTo(4.0f)
                verticalLineTo(6.0f)
                horizontalLineToRelative(5.17f)
                lineToRelative(2.0f, 2.0f)
                horizontalLineTo(14.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(2.0f)
                verticalLineTo(16.0f)
                close()
            }
        }
        return _folderZip!!
    }

private var _folderZip: ImageVector? = null

public val Icons.Outlined.Forward10: ImageVector
    get() {
        if (_forward10 != null) {
            return _forward10!!
        }
        _forward10 = materialIcon(name = "Outlined.Forward10") {
            materialPath {
                moveTo(18.0f, 13.0f)
                curveToRelative(0.0f, 3.31f, -2.69f, 6.0f, -6.0f, 6.0f)
                reflectiveCurveToRelative(-6.0f, -2.69f, -6.0f, -6.0f)
                reflectiveCurveToRelative(2.69f, -6.0f, 6.0f, -6.0f)
                verticalLineToRelative(4.0f)
                lineToRelative(5.0f, -5.0f)
                lineToRelative(-5.0f, -5.0f)
                verticalLineToRelative(4.0f)
                curveToRelative(-4.42f, 0.0f, -8.0f, 3.58f, -8.0f, 8.0f)
                curveToRelative(0.0f, 4.42f, 3.58f, 8.0f, 8.0f, 8.0f)
                reflectiveCurveToRelative(8.0f, -3.58f, 8.0f, -8.0f)
                horizontalLineTo(18.0f)
                close()
            }
            materialPath {
                moveTo(10.9f, 16.0f)
                lineToRelative(0.0f, -4.27f)
                lineToRelative(-0.09f, 0.0f)
                lineToRelative(-1.77f, 0.63f)
                lineToRelative(0.0f, 0.69f)
                lineToRelative(1.01f, -0.31f)
                lineToRelative(0.0f, 3.26f)
                close()
            }
            materialPath {
                moveTo(14.32f, 11.78f)
                curveToRelative(-0.18f, -0.07f, -0.37f, -0.1f, -0.59f, -0.1f)
                reflectiveCurveToRelative(-0.41f, 0.03f, -0.59f, 0.1f)
                reflectiveCurveToRelative(-0.33f, 0.18f, -0.45f, 0.33f)
                reflectiveCurveToRelative(-0.23f, 0.34f, -0.29f, 0.57f)
                reflectiveCurveToRelative(-0.1f, 0.5f, -0.1f, 0.82f)
                verticalLineToRelative(0.74f)
                curveToRelative(0.0f, 0.32f, 0.04f, 0.6f, 0.11f, 0.82f)
                reflectiveCurveToRelative(0.17f, 0.42f, 0.3f, 0.57f)
                reflectiveCurveToRelative(0.28f, 0.26f, 0.46f, 0.33f)
                reflectiveCurveToRelative(0.37f, 0.1f, 0.59f, 0.1f)
                reflectiveCurveToRelative(0.41f, -0.03f, 0.59f, -0.1f)
                reflectiveCurveToRelative(0.33f, -0.18f, 0.45f, -0.33f)
                reflectiveCurveToRelative(0.22f, -0.34f, 0.29f, -0.57f)
                reflectiveCurveToRelative(0.1f, -0.5f, 0.1f, -0.82f)
                verticalLineTo(13.5f)
                curveToRelative(0.0f, -0.32f, -0.04f, -0.6f, -0.11f, -0.82f)
                reflectiveCurveToRelative(-0.17f, -0.42f, -0.3f, -0.57f)
                reflectiveCurveTo(14.49f, 11.85f, 14.32f, 11.78f)
                close()
                moveTo(14.33f, 14.35f)
                curveToRelative(0.0f, 0.19f, -0.01f, 0.35f, -0.04f, 0.48f)
                reflectiveCurveToRelative(-0.06f, 0.24f, -0.11f, 0.32f)
                reflectiveCurveToRelative(-0.11f, 0.14f, -0.19f, 0.17f)
                reflectiveCurveToRelative(-0.16f, 0.05f, -0.25f, 0.05f)
                reflectiveCurveToRelative(-0.18f, -0.02f, -0.25f, -0.05f)
                reflectiveCurveToRelative(-0.14f, -0.09f, -0.19f, -0.17f)
                reflectiveCurveToRelative(-0.09f, -0.19f, -0.12f, -0.32f)
                reflectiveCurveToRelative(-0.04f, -0.29f, -0.04f, -0.48f)
                verticalLineToRelative(-0.97f)
                curveToRelative(0.0f, -0.19f, 0.01f, -0.35f, 0.04f, -0.48f)
                reflectiveCurveToRelative(0.06f, -0.23f, 0.12f, -0.31f)
                reflectiveCurveToRelative(0.11f, -0.14f, 0.19f, -0.17f)
                reflectiveCurveToRelative(0.16f, -0.05f, 0.25f, -0.05f)
                reflectiveCurveToRelative(0.18f, 0.02f, 0.25f, 0.05f)
                reflectiveCurveToRelative(0.14f, 0.09f, 0.19f, 0.17f)
                reflectiveCurveToRelative(0.09f, 0.18f, 0.12f, 0.31f)
                reflectiveCurveToRelative(0.04f, 0.29f, 0.04f, 0.48f)
                verticalLineTo(14.35f)
                close()
            }
        }
        return _forward10!!
    }

private var _forward10: ImageVector? = null

public val Icons.Outlined.GppGood: ImageVector
    get() {
        if (_gppGood != null) {
            return _gppGood!!
        }
        _gppGood = materialIcon(name = "Outlined.GppGood") {
            materialPath {
                moveTo(12.0f, 2.0f)
                lineTo(4.0f, 5.0f)
                verticalLineToRelative(6.09f)
                curveToRelative(0.0f, 5.05f, 3.41f, 9.76f, 8.0f, 10.91f)
                curveToRelative(4.59f, -1.15f, 8.0f, -5.86f, 8.0f, -10.91f)
                verticalLineTo(5.0f)
                lineTo(12.0f, 2.0f)
                close()
                moveTo(18.0f, 11.09f)
                curveToRelative(0.0f, 4.0f, -2.55f, 7.7f, -6.0f, 8.83f)
                curveToRelative(-3.45f, -1.13f, -6.0f, -4.82f, -6.0f, -8.83f)
                verticalLineTo(6.31f)
                lineToRelative(6.0f, -2.12f)
                lineToRelative(6.0f, 2.12f)
                verticalLineTo(11.09f)
                close()
                moveTo(8.82f, 10.59f)
                lineTo(7.4f, 12.0f)
                lineToRelative(3.54f, 3.54f)
                lineToRelative(5.66f, -5.66f)
                lineToRelative(-1.41f, -1.41f)
                lineToRelative(-4.24f, 4.24f)
                lineTo(8.82f, 10.59f)
                close()
            }
        }
        return _gppGood!!
    }

private var _gppGood: ImageVector? = null

public val Icons.Outlined.HourglassTop: ImageVector
    get() {
        if (_hourglassTop != null) {
            return _hourglassTop!!
        }
        _hourglassTop = materialIcon(name = "Outlined.HourglassTop") {
            materialPath {
                moveTo(6.0f, 2.0f)
                lineToRelative(0.01f, 6.0f)
                lineTo(10.0f, 12.0f)
                lineToRelative(-3.99f, 4.01f)
                lineTo(6.0f, 22.0f)
                horizontalLineToRelative(12.0f)
                verticalLineToRelative(-6.0f)
                lineToRelative(-4.0f, -4.0f)
                lineToRelative(4.0f, -3.99f)
                verticalLineTo(2.0f)
                horizontalLineTo(6.0f)
                close()
                moveTo(16.0f, 16.5f)
                verticalLineTo(20.0f)
                horizontalLineTo(8.0f)
                verticalLineToRelative(-3.5f)
                lineToRelative(4.0f, -4.0f)
                lineTo(16.0f, 16.5f)
                close()
            }
        }
        return _hourglassTop!!
    }

private var _hourglassTop: ImageVector? = null

public val Icons.Outlined.Hub: ImageVector
    get() {
        if (_hub != null) {
            return _hub!!
        }
        _hub = materialIcon(name = "Outlined.Hub") {
            materialPath {
                moveTo(21.0f, 6.5f)
                curveToRelative(-1.66f, 0.0f, -3.0f, 1.34f, -3.0f, 3.0f)
                curveToRelative(0.0f, 0.07f, 0.0f, 0.14f, 0.01f, 0.21f)
                lineToRelative(-2.03f, 0.68f)
                curveToRelative(-0.64f, -1.21f, -1.82f, -2.09f, -3.22f, -2.32f)
                verticalLineTo(5.91f)
                curveTo(14.04f, 5.57f, 15.0f, 4.4f, 15.0f, 3.0f)
                curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
                reflectiveCurveTo(9.0f, 1.34f, 9.0f, 3.0f)
                curveToRelative(0.0f, 1.4f, 0.96f, 2.57f, 2.25f, 2.91f)
                verticalLineToRelative(2.16f)
                curveToRelative(-1.4f, 0.23f, -2.58f, 1.11f, -3.22f, 2.32f)
                lineTo(5.99f, 9.71f)
                curveTo(6.0f, 9.64f, 6.0f, 9.57f, 6.0f, 9.5f)
                curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
                reflectiveCurveToRelative(-3.0f, 1.34f, -3.0f, 3.0f)
                reflectiveCurveToRelative(1.34f, 3.0f, 3.0f, 3.0f)
                curveToRelative(1.06f, 0.0f, 1.98f, -0.55f, 2.52f, -1.37f)
                lineToRelative(2.03f, 0.68f)
                curveToRelative(-0.2f, 1.29f, 0.17f, 2.66f, 1.09f, 3.69f)
                lineToRelative(-1.41f, 1.77f)
                curveTo(6.85f, 17.09f, 6.44f, 17.0f, 6.0f, 17.0f)
                curveToRelative(-1.66f, 0.0f, -3.0f, 1.34f, -3.0f, 3.0f)
                reflectiveCurveToRelative(1.34f, 3.0f, 3.0f, 3.0f)
                reflectiveCurveToRelative(3.0f, -1.34f, 3.0f, -3.0f)
                curveToRelative(0.0f, -0.68f, -0.22f, -1.3f, -0.6f, -1.8f)
                lineToRelative(1.41f, -1.77f)
                curveToRelative(1.36f, 0.76f, 3.02f, 0.75f, 4.37f, 0.0f)
                lineToRelative(1.41f, 1.77f)
                curveTo(15.22f, 18.7f, 15.0f, 19.32f, 15.0f, 20.0f)
                curveToRelative(0.0f, 1.66f, 1.34f, 3.0f, 3.0f, 3.0f)
                reflectiveCurveToRelative(3.0f, -1.34f, 3.0f, -3.0f)
                reflectiveCurveToRelative(-1.34f, -3.0f, -3.0f, -3.0f)
                curveToRelative(-0.44f, 0.0f, -0.85f, 0.09f, -1.23f, 0.26f)
                lineToRelative(-1.41f, -1.77f)
                curveToRelative(0.93f, -1.04f, 1.29f, -2.4f, 1.09f, -3.69f)
                lineToRelative(2.03f, -0.68f)
                curveToRelative(0.53f, 0.82f, 1.46f, 1.37f, 2.52f, 1.37f)
                curveToRelative(1.66f, 0.0f, 3.0f, -1.34f, 3.0f, -3.0f)
                reflectiveCurveTo(22.66f, 6.5f, 21.0f, 6.5f)
                close()
                moveTo(3.0f, 10.5f)
                curveToRelative(-0.55f, 0.0f, -1.0f, -0.45f, -1.0f, -1.0f)
                curveToRelative(0.0f, -0.55f, 0.45f, -1.0f, 1.0f, -1.0f)
                reflectiveCurveToRelative(1.0f, 0.45f, 1.0f, 1.0f)
                curveTo(4.0f, 10.05f, 3.55f, 10.5f, 3.0f, 10.5f)
                close()
                moveTo(6.0f, 21.0f)
                curveToRelative(-0.55f, 0.0f, -1.0f, -0.45f, -1.0f, -1.0f)
                curveToRelative(0.0f, -0.55f, 0.45f, -1.0f, 1.0f, -1.0f)
                reflectiveCurveToRelative(1.0f, 0.45f, 1.0f, 1.0f)
                curveTo(7.0f, 20.55f, 6.55f, 21.0f, 6.0f, 21.0f)
                close()
                moveTo(11.0f, 3.0f)
                curveToRelative(0.0f, -0.55f, 0.45f, -1.0f, 1.0f, -1.0f)
                reflectiveCurveToRelative(1.0f, 0.45f, 1.0f, 1.0f)
                curveToRelative(0.0f, 0.55f, -0.45f, 1.0f, -1.0f, 1.0f)
                reflectiveCurveTo(11.0f, 3.55f, 11.0f, 3.0f)
                close()
                moveTo(12.0f, 15.0f)
                curveToRelative(-1.38f, 0.0f, -2.5f, -1.12f, -2.5f, -2.5f)
                curveToRelative(0.0f, -1.38f, 1.12f, -2.5f, 2.5f, -2.5f)
                reflectiveCurveToRelative(2.5f, 1.12f, 2.5f, 2.5f)
                curveTo(14.5f, 13.88f, 13.38f, 15.0f, 12.0f, 15.0f)
                close()
                moveTo(18.0f, 19.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, 0.45f, 1.0f, 1.0f)
                curveToRelative(0.0f, 0.55f, -0.45f, 1.0f, -1.0f, 1.0f)
                reflectiveCurveToRelative(-1.0f, -0.45f, -1.0f, -1.0f)
                curveTo(17.0f, 19.45f, 17.45f, 19.0f, 18.0f, 19.0f)
                close()
                moveTo(21.0f, 10.5f)
                curveToRelative(-0.55f, 0.0f, -1.0f, -0.45f, -1.0f, -1.0f)
                curveToRelative(0.0f, -0.55f, 0.45f, -1.0f, 1.0f, -1.0f)
                reflectiveCurveToRelative(1.0f, 0.45f, 1.0f, 1.0f)
                curveTo(22.0f, 10.05f, 21.55f, 10.5f, 21.0f, 10.5f)
                close()
            }
        }
        return _hub!!
    }

private var _hub: ImageVector? = null

public val Icons.Outlined.Image: ImageVector
    get() {
        if (_image != null) {
            return _image!!
        }
        _image = materialIcon(name = "Outlined.Image") {
            materialPath {
                moveTo(19.0f, 5.0f)
                verticalLineToRelative(14.0f)
                lineTo(5.0f, 19.0f)
                lineTo(5.0f, 5.0f)
                horizontalLineToRelative(14.0f)
                moveToRelative(0.0f, -2.0f)
                lineTo(5.0f, 3.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(14.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(14.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                lineTo(21.0f, 5.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(14.14f, 11.86f)
                lineToRelative(-3.0f, 3.87f)
                lineTo(9.0f, 13.14f)
                lineTo(6.0f, 17.0f)
                horizontalLineToRelative(12.0f)
                lineToRelative(-3.86f, -5.14f)
                close()
            }
        }
        return _image!!
    }

private var _image: ImageVector? = null

public val Icons.Outlined.Info: ImageVector
    get() {
        if (_info != null) {
            return _info!!
        }
        _info = materialIcon(name = "Outlined.Info") {
            materialPath {
                moveTo(11.0f, 7.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(-2.0f)
                close()
                moveTo(11.0f, 11.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(-2.0f)
                close()
                moveTo(12.0f, 2.0f)
                curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
                reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
                reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, -3.59f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.59f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.59f, 8.0f, 8.0f)
                reflectiveCurveToRelative(-3.59f, 8.0f, -8.0f, 8.0f)
                close()
            }
        }
        return _info!!
    }

private var _info: ImageVector? = null

public val Icons.Outlined.KeyboardArrowDown: ImageVector
    get() {
        if (_keyboardArrowDown != null) {
            return _keyboardArrowDown!!
        }
        _keyboardArrowDown = materialIcon(name = "Outlined.KeyboardArrowDown") {
            materialPath {
                moveTo(7.41f, 8.59f)
                lineTo(12.0f, 13.17f)
                lineToRelative(4.59f, -4.58f)
                lineTo(18.0f, 10.0f)
                lineToRelative(-6.0f, 6.0f)
                lineToRelative(-6.0f, -6.0f)
                lineToRelative(1.41f, -1.41f)
                close()
            }
        }
        return _keyboardArrowDown!!
    }

private var _keyboardArrowDown: ImageVector? = null

public val Icons.Outlined.KeyboardArrowUp: ImageVector
    get() {
        if (_keyboardArrowUp != null) {
            return _keyboardArrowUp!!
        }
        _keyboardArrowUp = materialIcon(name = "Outlined.KeyboardArrowUp") {
            materialPath {
                moveTo(7.41f, 15.41f)
                lineTo(12.0f, 10.83f)
                lineToRelative(4.59f, 4.58f)
                lineTo(18.0f, 14.0f)
                lineToRelative(-6.0f, -6.0f)
                lineToRelative(-6.0f, 6.0f)
                lineToRelative(1.41f, 1.41f)
                close()
            }
        }
        return _keyboardArrowUp!!
    }

private var _keyboardArrowUp: ImageVector? = null

public val Icons.Outlined.Language: ImageVector
    get() {
        if (_language != null) {
            return _language!!
        }
        _language = materialIcon(name = "Outlined.Language") {
            materialPath {
                moveTo(11.99f, 2.0f)
                curveTo(6.47f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.47f, 10.0f, 9.99f, 10.0f)
                curveTo(17.52f, 22.0f, 22.0f, 17.52f, 22.0f, 12.0f)
                reflectiveCurveTo(17.52f, 2.0f, 11.99f, 2.0f)
                close()
                moveTo(18.92f, 8.0f)
                horizontalLineToRelative(-2.95f)
                curveToRelative(-0.32f, -1.25f, -0.78f, -2.45f, -1.38f, -3.56f)
                curveToRelative(1.84f, 0.63f, 3.37f, 1.91f, 4.33f, 3.56f)
                close()
                moveTo(12.0f, 4.04f)
                curveToRelative(0.83f, 1.2f, 1.48f, 2.53f, 1.91f, 3.96f)
                horizontalLineToRelative(-3.82f)
                curveToRelative(0.43f, -1.43f, 1.08f, -2.76f, 1.91f, -3.96f)
                close()
                moveTo(4.26f, 14.0f)
                curveTo(4.1f, 13.36f, 4.0f, 12.69f, 4.0f, 12.0f)
                reflectiveCurveToRelative(0.1f, -1.36f, 0.26f, -2.0f)
                horizontalLineToRelative(3.38f)
                curveToRelative(-0.08f, 0.66f, -0.14f, 1.32f, -0.14f, 2.0f)
                reflectiveCurveToRelative(0.06f, 1.34f, 0.14f, 2.0f)
                lineTo(4.26f, 14.0f)
                close()
                moveTo(5.08f, 16.0f)
                horizontalLineToRelative(2.95f)
                curveToRelative(0.32f, 1.25f, 0.78f, 2.45f, 1.38f, 3.56f)
                curveToRelative(-1.84f, -0.63f, -3.37f, -1.9f, -4.33f, -3.56f)
                close()
                moveTo(8.03f, 8.0f)
                lineTo(5.08f, 8.0f)
                curveToRelative(0.96f, -1.66f, 2.49f, -2.93f, 4.33f, -3.56f)
                curveTo(8.81f, 5.55f, 8.35f, 6.75f, 8.03f, 8.0f)
                close()
                moveTo(12.0f, 19.96f)
                curveToRelative(-0.83f, -1.2f, -1.48f, -2.53f, -1.91f, -3.96f)
                horizontalLineToRelative(3.82f)
                curveToRelative(-0.43f, 1.43f, -1.08f, 2.76f, -1.91f, 3.96f)
                close()
                moveTo(14.34f, 14.0f)
                lineTo(9.66f, 14.0f)
                curveToRelative(-0.09f, -0.66f, -0.16f, -1.32f, -0.16f, -2.0f)
                reflectiveCurveToRelative(0.07f, -1.35f, 0.16f, -2.0f)
                horizontalLineToRelative(4.68f)
                curveToRelative(0.09f, 0.65f, 0.16f, 1.32f, 0.16f, 2.0f)
                reflectiveCurveToRelative(-0.07f, 1.34f, -0.16f, 2.0f)
                close()
                moveTo(14.59f, 19.56f)
                curveToRelative(0.6f, -1.11f, 1.06f, -2.31f, 1.38f, -3.56f)
                horizontalLineToRelative(2.95f)
                curveToRelative(-0.96f, 1.65f, -2.49f, 2.93f, -4.33f, 3.56f)
                close()
                moveTo(16.36f, 14.0f)
                curveToRelative(0.08f, -0.66f, 0.14f, -1.32f, 0.14f, -2.0f)
                reflectiveCurveToRelative(-0.06f, -1.34f, -0.14f, -2.0f)
                horizontalLineToRelative(3.38f)
                curveToRelative(0.16f, 0.64f, 0.26f, 1.31f, 0.26f, 2.0f)
                reflectiveCurveToRelative(-0.1f, 1.36f, -0.26f, 2.0f)
                horizontalLineToRelative(-3.38f)
                close()
            }
        }
        return _language!!
    }

private var _language: ImageVector? = null

public val Icons.Outlined.LightMode: ImageVector
    get() {
        if (_lightMode != null) {
            return _lightMode!!
        }
        _lightMode = materialIcon(name = "Outlined.LightMode") {
            materialPath {
                moveTo(12.0f, 9.0f)
                curveToRelative(1.65f, 0.0f, 3.0f, 1.35f, 3.0f, 3.0f)
                reflectiveCurveToRelative(-1.35f, 3.0f, -3.0f, 3.0f)
                reflectiveCurveToRelative(-3.0f, -1.35f, -3.0f, -3.0f)
                reflectiveCurveTo(10.35f, 9.0f, 12.0f, 9.0f)
                moveTo(12.0f, 7.0f)
                curveToRelative(-2.76f, 0.0f, -5.0f, 2.24f, -5.0f, 5.0f)
                reflectiveCurveToRelative(2.24f, 5.0f, 5.0f, 5.0f)
                reflectiveCurveToRelative(5.0f, -2.24f, 5.0f, -5.0f)
                reflectiveCurveTo(14.76f, 7.0f, 12.0f, 7.0f)
                lineTo(12.0f, 7.0f)
                close()
                moveTo(2.0f, 13.0f)
                lineToRelative(2.0f, 0.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, -0.45f, 1.0f, -1.0f)
                reflectiveCurveToRelative(-0.45f, -1.0f, -1.0f, -1.0f)
                lineToRelative(-2.0f, 0.0f)
                curveToRelative(-0.55f, 0.0f, -1.0f, 0.45f, -1.0f, 1.0f)
                reflectiveCurveTo(1.45f, 13.0f, 2.0f, 13.0f)
                close()
                moveTo(20.0f, 13.0f)
                lineToRelative(2.0f, 0.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, -0.45f, 1.0f, -1.0f)
                reflectiveCurveToRelative(-0.45f, -1.0f, -1.0f, -1.0f)
                lineToRelative(-2.0f, 0.0f)
                curveToRelative(-0.55f, 0.0f, -1.0f, 0.45f, -1.0f, 1.0f)
                reflectiveCurveTo(19.45f, 13.0f, 20.0f, 13.0f)
                close()
                moveTo(11.0f, 2.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(0.0f, 0.55f, 0.45f, 1.0f, 1.0f, 1.0f)
                reflectiveCurveToRelative(1.0f, -0.45f, 1.0f, -1.0f)
                verticalLineTo(2.0f)
                curveToRelative(0.0f, -0.55f, -0.45f, -1.0f, -1.0f, -1.0f)
                reflectiveCurveTo(11.0f, 1.45f, 11.0f, 2.0f)
                close()
                moveTo(11.0f, 20.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(0.0f, 0.55f, 0.45f, 1.0f, 1.0f, 1.0f)
                reflectiveCurveToRelative(1.0f, -0.45f, 1.0f, -1.0f)
                verticalLineToRelative(-2.0f)
                curveToRelative(0.0f, -0.55f, -0.45f, -1.0f, -1.0f, -1.0f)
                curveTo(11.45f, 19.0f, 11.0f, 19.45f, 11.0f, 20.0f)
                close()
                moveTo(5.99f, 4.58f)
                curveToRelative(-0.39f, -0.39f, -1.03f, -0.39f, -1.41f, 0.0f)
                curveToRelative(-0.39f, 0.39f, -0.39f, 1.03f, 0.0f, 1.41f)
                lineToRelative(1.06f, 1.06f)
                curveToRelative(0.39f, 0.39f, 1.03f, 0.39f, 1.41f, 0.0f)
                reflectiveCurveToRelative(0.39f, -1.03f, 0.0f, -1.41f)
                lineTo(5.99f, 4.58f)
                close()
                moveTo(18.36f, 16.95f)
                curveToRelative(-0.39f, -0.39f, -1.03f, -0.39f, -1.41f, 0.0f)
                curveToRelative(-0.39f, 0.39f, -0.39f, 1.03f, 0.0f, 1.41f)
                lineToRelative(1.06f, 1.06f)
                curveToRelative(0.39f, 0.39f, 1.03f, 0.39f, 1.41f, 0.0f)
                curveToRelative(0.39f, -0.39f, 0.39f, -1.03f, 0.0f, -1.41f)
                lineTo(18.36f, 16.95f)
                close()
                moveTo(19.42f, 5.99f)
                curveToRelative(0.39f, -0.39f, 0.39f, -1.03f, 0.0f, -1.41f)
                curveToRelative(-0.39f, -0.39f, -1.03f, -0.39f, -1.41f, 0.0f)
                lineToRelative(-1.06f, 1.06f)
                curveToRelative(-0.39f, 0.39f, -0.39f, 1.03f, 0.0f, 1.41f)
                reflectiveCurveToRelative(1.03f, 0.39f, 1.41f, 0.0f)
                lineTo(19.42f, 5.99f)
                close()
                moveTo(7.05f, 18.36f)
                curveToRelative(0.39f, -0.39f, 0.39f, -1.03f, 0.0f, -1.41f)
                curveToRelative(-0.39f, -0.39f, -1.03f, -0.39f, -1.41f, 0.0f)
                lineToRelative(-1.06f, 1.06f)
                curveToRelative(-0.39f, 0.39f, -0.39f, 1.03f, 0.0f, 1.41f)
                reflectiveCurveToRelative(1.03f, 0.39f, 1.41f, 0.0f)
                lineTo(7.05f, 18.36f)
                close()
            }
        }
        return _lightMode!!
    }

private var _lightMode: ImageVector? = null

public val Icons.Outlined.Link: ImageVector
    get() {
        if (_link != null) {
            return _link!!
        }
        _link = materialIcon(name = "Outlined.Link") {
            materialPath {
                moveTo(17.0f, 7.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(4.0f)
                curveToRelative(1.65f, 0.0f, 3.0f, 1.35f, 3.0f, 3.0f)
                reflectiveCurveToRelative(-1.35f, 3.0f, -3.0f, 3.0f)
                horizontalLineToRelative(-4.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(4.0f)
                curveToRelative(2.76f, 0.0f, 5.0f, -2.24f, 5.0f, -5.0f)
                reflectiveCurveToRelative(-2.24f, -5.0f, -5.0f, -5.0f)
                close()
                moveTo(11.0f, 15.0f)
                lineTo(7.0f, 15.0f)
                curveToRelative(-1.65f, 0.0f, -3.0f, -1.35f, -3.0f, -3.0f)
                reflectiveCurveToRelative(1.35f, -3.0f, 3.0f, -3.0f)
                horizontalLineToRelative(4.0f)
                lineTo(11.0f, 7.0f)
                lineTo(7.0f, 7.0f)
                curveToRelative(-2.76f, 0.0f, -5.0f, 2.24f, -5.0f, 5.0f)
                reflectiveCurveToRelative(2.24f, 5.0f, 5.0f, 5.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(-2.0f)
                close()
                moveTo(8.0f, 11.0f)
                horizontalLineToRelative(8.0f)
                verticalLineToRelative(2.0f)
                lineTo(8.0f, 13.0f)
                close()
            }
        }
        return _link!!
    }

private var _link: ImageVector? = null

public val Icons.Outlined.Minimize: ImageVector
    get() {
        if (_minimize != null) {
            return _minimize!!
        }
        _minimize = materialIcon(name = "Outlined.Minimize") {
            materialPath {
                moveTo(6.0f, 19.0f)
                horizontalLineToRelative(12.0f)
                verticalLineToRelative(2.0f)
                horizontalLineTo(6.0f)
                verticalLineToRelative(-2.0f)
                close()
            }
        }
        return _minimize!!
    }

private var _minimize: ImageVector? = null

public val Icons.Outlined.MoreHoriz: ImageVector
    get() {
        if (_moreHoriz != null) {
            return _moreHoriz!!
        }
        _moreHoriz = materialIcon(name = "Outlined.MoreHoriz") {
            materialPath {
                moveTo(6.0f, 10.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
                reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
                reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(18.0f, 10.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
                reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
                reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(12.0f, 10.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
                reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
                reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
                close()
            }
        }
        return _moreHoriz!!
    }

private var _moreHoriz: ImageVector? = null

public val Icons.Outlined.MoreVert: ImageVector
    get() {
        if (_moreVert != null) {
            return _moreVert!!
        }
        _moreVert = materialIcon(name = "Outlined.MoreVert") {
            materialPath {
                moveTo(12.0f, 8.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
                reflectiveCurveToRelative(-2.0f, 0.9f, -2.0f, 2.0f)
                reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
                close()
                moveTo(12.0f, 10.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
                reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
                reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(12.0f, 16.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
                reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
                reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
                close()
            }
        }
        return _moreVert!!
    }

private var _moreVert: ImageVector? = null

public val Icons.Outlined.Movie: ImageVector
    get() {
        if (_movie != null) {
            return _movie!!
        }
        _movie = materialIcon(name = "Outlined.Movie") {
            materialPath {
                moveTo(4.0f, 6.47f)
                lineTo(5.76f, 10.0f)
                horizontalLineTo(20.0f)
                verticalLineToRelative(8.0f)
                horizontalLineTo(4.0f)
                verticalLineTo(6.47f)
                moveTo(22.0f, 4.0f)
                horizontalLineToRelative(-4.0f)
                lineToRelative(2.0f, 4.0f)
                horizontalLineToRelative(-3.0f)
                lineToRelative(-2.0f, -4.0f)
                horizontalLineToRelative(-2.0f)
                lineToRelative(2.0f, 4.0f)
                horizontalLineToRelative(-3.0f)
                lineToRelative(-2.0f, -4.0f)
                horizontalLineTo(8.0f)
                lineToRelative(2.0f, 4.0f)
                horizontalLineTo(7.0f)
                lineTo(5.0f, 4.0f)
                horizontalLineTo(4.0f)
                curveToRelative(-1.1f, 0.0f, -1.99f, 0.9f, -1.99f, 2.0f)
                lineTo(2.0f, 18.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(16.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                verticalLineTo(4.0f)
                close()
            }
        }
        return _movie!!
    }

private var _movie: ImageVector? = null

public val Icons.Outlined.Palette: ImageVector
    get() {
        if (_palette != null) {
            return _palette!!
        }
        _palette = materialIcon(name = "Outlined.Palette") {
            materialPath {
                moveTo(12.0f, 22.0f)
                curveTo(6.49f, 22.0f, 2.0f, 17.51f, 2.0f, 12.0f)
                reflectiveCurveTo(6.49f, 2.0f, 12.0f, 2.0f)
                reflectiveCurveToRelative(10.0f, 4.04f, 10.0f, 9.0f)
                curveToRelative(0.0f, 3.31f, -2.69f, 6.0f, -6.0f, 6.0f)
                horizontalLineToRelative(-1.77f)
                curveToRelative(-0.28f, 0.0f, -0.5f, 0.22f, -0.5f, 0.5f)
                curveToRelative(0.0f, 0.12f, 0.05f, 0.23f, 0.13f, 0.33f)
                curveToRelative(0.41f, 0.47f, 0.64f, 1.06f, 0.64f, 1.67f)
                curveTo(14.5f, 20.88f, 13.38f, 22.0f, 12.0f, 22.0f)
                close()
                moveTo(12.0f, 4.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, 3.59f, -8.0f, 8.0f)
                reflectiveCurveToRelative(3.59f, 8.0f, 8.0f, 8.0f)
                curveToRelative(0.28f, 0.0f, 0.5f, -0.22f, 0.5f, -0.5f)
                curveToRelative(0.0f, -0.16f, -0.08f, -0.28f, -0.14f, -0.35f)
                curveToRelative(-0.41f, -0.46f, -0.63f, -1.05f, -0.63f, -1.65f)
                curveToRelative(0.0f, -1.38f, 1.12f, -2.5f, 2.5f, -2.5f)
                horizontalLineTo(16.0f)
                curveToRelative(2.21f, 0.0f, 4.0f, -1.79f, 4.0f, -4.0f)
                curveTo(20.0f, 7.14f, 16.41f, 4.0f, 12.0f, 4.0f)
                close()
            }
            materialPath {
                moveTo(6.5f, 11.5f)
                moveToRelative(-1.5f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, 3.0f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, -3.0f, 0.0f)
            }
            materialPath {
                moveTo(9.5f, 7.5f)
                moveToRelative(-1.5f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, 3.0f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, -3.0f, 0.0f)
            }
            materialPath {
                moveTo(14.5f, 7.5f)
                moveToRelative(-1.5f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, 3.0f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, -3.0f, 0.0f)
            }
            materialPath {
                moveTo(17.5f, 11.5f)
                moveToRelative(-1.5f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, 3.0f, 0.0f)
                arcToRelative(1.5f, 1.5f, 0.0f, true, true, -3.0f, 0.0f)
            }
        }
        return _palette!!
    }

private var _palette: ImageVector? = null

public val Icons.Outlined.Pause: ImageVector
    get() {
        if (_pause != null) {
            return _pause!!
        }
        _pause = materialIcon(name = "Outlined.Pause") {
            materialPath {
                moveTo(6.0f, 19.0f)
                horizontalLineToRelative(4.0f)
                lineTo(10.0f, 5.0f)
                lineTo(6.0f, 5.0f)
                verticalLineToRelative(14.0f)
                close()
                moveTo(14.0f, 5.0f)
                verticalLineToRelative(14.0f)
                horizontalLineToRelative(4.0f)
                lineTo(18.0f, 5.0f)
                horizontalLineToRelative(-4.0f)
                close()
            }
        }
        return _pause!!
    }

private var _pause: ImageVector? = null

public val Icons.Outlined.PauseCircle: ImageVector
    get() {
        if (_pauseCircle != null) {
            return _pauseCircle!!
        }
        _pauseCircle = materialIcon(name = "Outlined.PauseCircle") {
            materialPath {
                moveTo(9.0f, 16.0f)
                horizontalLineToRelative(2.0f)
                verticalLineTo(8.0f)
                horizontalLineTo(9.0f)
                verticalLineTo(16.0f)
                close()
                moveTo(12.0f, 2.0f)
                curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
                reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
                reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, -3.59f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.59f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.59f, 8.0f, 8.0f)
                reflectiveCurveTo(16.41f, 20.0f, 12.0f, 20.0f)
                close()
                moveTo(13.0f, 16.0f)
                horizontalLineToRelative(2.0f)
                verticalLineTo(8.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineTo(16.0f)
                close()
            }
        }
        return _pauseCircle!!
    }

private var _pauseCircle: ImageVector? = null

public val Icons.Outlined.PauseCircleOutline: ImageVector
    get() {
        if (_pauseCircleOutline != null) {
            return _pauseCircleOutline!!
        }
        _pauseCircleOutline = materialIcon(name = "Outlined.PauseCircleOutline") {
            materialPath {
                moveTo(9.0f, 16.0f)
                horizontalLineToRelative(2.0f)
                lineTo(11.0f, 8.0f)
                lineTo(9.0f, 8.0f)
                verticalLineToRelative(8.0f)
                close()
                moveTo(12.0f, 2.0f)
                curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
                reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
                reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, -3.59f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.59f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.59f, 8.0f, 8.0f)
                reflectiveCurveToRelative(-3.59f, 8.0f, -8.0f, 8.0f)
                close()
                moveTo(13.0f, 16.0f)
                horizontalLineToRelative(2.0f)
                lineTo(15.0f, 8.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(8.0f)
                close()
            }
        }
        return _pauseCircleOutline!!
    }

private var _pauseCircleOutline: ImageVector? = null

public val Icons.Outlined.PlayArrow: ImageVector
    get() {
        if (_playArrow != null) {
            return _playArrow!!
        }
        _playArrow = materialIcon(name = "Outlined.PlayArrow") {
            materialPath {
                moveTo(10.0f, 8.64f)
                lineTo(15.27f, 12.0f)
                lineTo(10.0f, 15.36f)
                verticalLineTo(8.64f)
                moveTo(8.0f, 5.0f)
                verticalLineToRelative(14.0f)
                lineToRelative(11.0f, -7.0f)
                lineTo(8.0f, 5.0f)
                close()
            }
        }
        return _playArrow!!
    }

private var _playArrow: ImageVector? = null

public val Icons.Outlined.PlayCircle: ImageVector
    get() {
        if (_playCircle != null) {
            return _playCircle!!
        }
        _playCircle = materialIcon(name = "Outlined.PlayCircle") {
            materialPath {
                moveTo(12.0f, 2.0f)
                curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
                reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
                reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, -3.59f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.59f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.59f, 8.0f, 8.0f)
                reflectiveCurveTo(16.41f, 20.0f, 12.0f, 20.0f)
                close()
                moveTo(9.5f, 16.5f)
                lineToRelative(7.0f, -4.5f)
                lineToRelative(-7.0f, -4.5f)
                verticalLineTo(16.5f)
                close()
            }
        }
        return _playCircle!!
    }

private var _playCircle: ImageVector? = null

public val Icons.Outlined.PlayCircleOutline: ImageVector
    get() {
        if (_playCircleOutline != null) {
            return _playCircleOutline!!
        }
        _playCircleOutline = materialIcon(name = "Outlined.PlayCircleOutline") {
            materialPath {
                moveTo(10.0f, 16.5f)
                lineToRelative(6.0f, -4.5f)
                lineToRelative(-6.0f, -4.5f)
                close()
                moveTo(12.0f, 2.0f)
                curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
                reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
                reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.41f, 0.0f, -8.0f, -3.59f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.59f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.59f, 8.0f, 8.0f)
                reflectiveCurveToRelative(-3.59f, 8.0f, -8.0f, 8.0f)
                close()
            }
        }
        return _playCircleOutline!!
    }

private var _playCircleOutline: ImageVector? = null

public val Icons.Outlined.Refresh: ImageVector
    get() {
        if (_refresh != null) {
            return _refresh!!
        }
        _refresh = materialIcon(name = "Outlined.Refresh") {
            materialPath {
                moveTo(17.65f, 6.35f)
                curveTo(16.2f, 4.9f, 14.21f, 4.0f, 12.0f, 4.0f)
                curveToRelative(-4.42f, 0.0f, -7.99f, 3.58f, -7.99f, 8.0f)
                reflectiveCurveToRelative(3.57f, 8.0f, 7.99f, 8.0f)
                curveToRelative(3.73f, 0.0f, 6.84f, -2.55f, 7.73f, -6.0f)
                horizontalLineToRelative(-2.08f)
                curveToRelative(-0.82f, 2.33f, -3.04f, 4.0f, -5.65f, 4.0f)
                curveToRelative(-3.31f, 0.0f, -6.0f, -2.69f, -6.0f, -6.0f)
                reflectiveCurveToRelative(2.69f, -6.0f, 6.0f, -6.0f)
                curveToRelative(1.66f, 0.0f, 3.14f, 0.69f, 4.22f, 1.78f)
                lineTo(13.0f, 11.0f)
                horizontalLineToRelative(7.0f)
                verticalLineTo(4.0f)
                lineToRelative(-2.35f, 2.35f)
                close()
            }
        }
        return _refresh!!
    }

private var _refresh: ImageVector? = null

public val Icons.Outlined.Replay10: ImageVector
    get() {
        if (_replay10 != null) {
            return _replay10!!
        }
        _replay10 = materialIcon(name = "Outlined.Replay10") {
            materialPath {
                moveTo(11.99f, 5.0f)
                lineTo(11.99f, 1.0f)
                lineToRelative(-5.0f, 5.0f)
                lineToRelative(5.0f, 5.0f)
                lineTo(11.99f, 7.0f)
                curveToRelative(3.31f, 0.0f, 6.0f, 2.69f, 6.0f, 6.0f)
                reflectiveCurveToRelative(-2.69f, 6.0f, -6.0f, 6.0f)
                reflectiveCurveToRelative(-6.0f, -2.69f, -6.0f, -6.0f)
                horizontalLineToRelative(-2.0f)
                curveToRelative(0.0f, 4.42f, 3.58f, 8.0f, 8.0f, 8.0f)
                reflectiveCurveToRelative(8.0f, -3.58f, 8.0f, -8.0f)
                reflectiveCurveToRelative(-3.58f, -8.0f, -8.0f, -8.0f)
                close()
                moveTo(10.89f, 16.0f)
                horizontalLineToRelative(-0.85f)
                verticalLineToRelative(-3.26f)
                lineToRelative(-1.01f, 0.31f)
                verticalLineToRelative(-0.69f)
                lineToRelative(1.77f, -0.63f)
                horizontalLineToRelative(0.09f)
                lineTo(10.89f, 16.0f)
                close()
                moveTo(15.17f, 14.24f)
                curveToRelative(0.0f, 0.32f, -0.03f, 0.6f, -0.1f, 0.82f)
                reflectiveCurveToRelative(-0.17f, 0.42f, -0.29f, 0.57f)
                reflectiveCurveToRelative(-0.28f, 0.26f, -0.45f, 0.33f)
                reflectiveCurveToRelative(-0.37f, 0.1f, -0.59f, 0.1f)
                reflectiveCurveToRelative(-0.41f, -0.03f, -0.59f, -0.1f)
                reflectiveCurveToRelative(-0.33f, -0.18f, -0.46f, -0.33f)
                reflectiveCurveToRelative(-0.23f, -0.34f, -0.3f, -0.57f)
                reflectiveCurveToRelative(-0.11f, -0.5f, -0.11f, -0.82f)
                verticalLineToRelative(-0.74f)
                curveToRelative(0.0f, -0.32f, 0.03f, -0.6f, 0.1f, -0.82f)
                reflectiveCurveToRelative(0.17f, -0.42f, 0.29f, -0.57f)
                reflectiveCurveToRelative(0.28f, -0.26f, 0.45f, -0.33f)
                reflectiveCurveToRelative(0.37f, -0.1f, 0.59f, -0.1f)
                reflectiveCurveToRelative(0.41f, 0.03f, 0.59f, 0.1f)
                reflectiveCurveToRelative(0.33f, 0.18f, 0.46f, 0.33f)
                reflectiveCurveToRelative(0.23f, 0.34f, 0.3f, 0.57f)
                reflectiveCurveToRelative(0.11f, 0.5f, 0.11f, 0.82f)
                verticalLineToRelative(0.74f)
                close()
                moveTo(14.32f, 13.38f)
                curveToRelative(0.0f, -0.19f, -0.01f, -0.35f, -0.04f, -0.48f)
                reflectiveCurveToRelative(-0.07f, -0.23f, -0.12f, -0.31f)
                reflectiveCurveToRelative(-0.11f, -0.14f, -0.19f, -0.17f)
                reflectiveCurveToRelative(-0.16f, -0.05f, -0.25f, -0.05f)
                reflectiveCurveToRelative(-0.18f, 0.02f, -0.25f, 0.05f)
                reflectiveCurveToRelative(-0.14f, 0.09f, -0.19f, 0.17f)
                reflectiveCurveToRelative(-0.09f, 0.18f, -0.12f, 0.31f)
                reflectiveCurveToRelative(-0.04f, 0.29f, -0.04f, 0.48f)
                verticalLineToRelative(0.97f)
                curveToRelative(0.0f, 0.19f, 0.01f, 0.35f, 0.04f, 0.48f)
                reflectiveCurveToRelative(0.07f, 0.24f, 0.12f, 0.32f)
                reflectiveCurveToRelative(0.11f, 0.14f, 0.19f, 0.17f)
                reflectiveCurveToRelative(0.16f, 0.05f, 0.25f, 0.05f)
                reflectiveCurveToRelative(0.18f, -0.02f, 0.25f, -0.05f)
                reflectiveCurveToRelative(0.14f, -0.09f, 0.19f, -0.17f)
                reflectiveCurveToRelative(0.09f, -0.19f, 0.11f, -0.32f)
                reflectiveCurveToRelative(0.04f, -0.29f, 0.04f, -0.48f)
                verticalLineToRelative(-0.97f)
                close()
            }
        }
        return _replay10!!
    }

private var _replay10: ImageVector? = null

public val Icons.Outlined.RestartAlt: ImageVector
    get() {
        if (_restartAlt != null) {
            return _restartAlt!!
        }
        _restartAlt = materialIcon(name = "Outlined.RestartAlt") {
            materialPath {
                moveTo(6.0f, 13.0f)
                curveToRelative(0.0f, -1.65f, 0.67f, -3.15f, 1.76f, -4.24f)
                lineTo(6.34f, 7.34f)
                curveTo(4.9f, 8.79f, 4.0f, 10.79f, 4.0f, 13.0f)
                curveToRelative(0.0f, 4.08f, 3.05f, 7.44f, 7.0f, 7.93f)
                verticalLineToRelative(-2.02f)
                curveTo(8.17f, 18.43f, 6.0f, 15.97f, 6.0f, 13.0f)
                close()
                moveTo(20.0f, 13.0f)
                curveToRelative(0.0f, -4.42f, -3.58f, -8.0f, -8.0f, -8.0f)
                curveToRelative(-0.06f, 0.0f, -0.12f, 0.01f, -0.18f, 0.01f)
                lineToRelative(1.09f, -1.09f)
                lineTo(11.5f, 2.5f)
                lineTo(8.0f, 6.0f)
                lineToRelative(3.5f, 3.5f)
                lineToRelative(1.41f, -1.41f)
                lineToRelative(-1.08f, -1.08f)
                curveTo(11.89f, 7.01f, 11.95f, 7.0f, 12.0f, 7.0f)
                curveToRelative(3.31f, 0.0f, 6.0f, 2.69f, 6.0f, 6.0f)
                curveToRelative(0.0f, 2.97f, -2.17f, 5.43f, -5.0f, 5.91f)
                verticalLineToRelative(2.02f)
                curveTo(16.95f, 20.44f, 20.0f, 17.08f, 20.0f, 13.0f)
                close()
            }
        }
        return _restartAlt!!
    }

private var _restartAlt: ImageVector? = null

public val Icons.Outlined.Schedule: ImageVector
    get() {
        if (_schedule != null) {
            return _schedule!!
        }
        _schedule = materialIcon(name = "Outlined.Schedule") {
            materialPath {
                moveTo(11.99f, 2.0f)
                curveTo(6.47f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
                reflectiveCurveToRelative(4.47f, 10.0f, 9.99f, 10.0f)
                curveTo(17.52f, 22.0f, 22.0f, 17.52f, 22.0f, 12.0f)
                reflectiveCurveTo(17.52f, 2.0f, 11.99f, 2.0f)
                close()
                moveTo(12.0f, 20.0f)
                curveToRelative(-4.42f, 0.0f, -8.0f, -3.58f, -8.0f, -8.0f)
                reflectiveCurveToRelative(3.58f, -8.0f, 8.0f, -8.0f)
                reflectiveCurveToRelative(8.0f, 3.58f, 8.0f, 8.0f)
                reflectiveCurveToRelative(-3.58f, 8.0f, -8.0f, 8.0f)
                close()
                moveTo(12.5f, 7.0f)
                lineTo(11.0f, 7.0f)
                verticalLineToRelative(6.0f)
                lineToRelative(5.25f, 3.15f)
                lineToRelative(0.75f, -1.23f)
                lineToRelative(-4.5f, -2.67f)
                close()
            }
        }
        return _schedule!!
    }

private var _schedule: ImageVector? = null

public val Icons.Outlined.Search: ImageVector
    get() {
        if (_search != null) {
            return _search!!
        }
        _search = materialIcon(name = "Outlined.Search") {
            materialPath {
                moveTo(15.5f, 14.0f)
                horizontalLineToRelative(-0.79f)
                lineToRelative(-0.28f, -0.27f)
                curveTo(15.41f, 12.59f, 16.0f, 11.11f, 16.0f, 9.5f)
                curveTo(16.0f, 5.91f, 13.09f, 3.0f, 9.5f, 3.0f)
                reflectiveCurveTo(3.0f, 5.91f, 3.0f, 9.5f)
                reflectiveCurveTo(5.91f, 16.0f, 9.5f, 16.0f)
                curveToRelative(1.61f, 0.0f, 3.09f, -0.59f, 4.23f, -1.57f)
                lineToRelative(0.27f, 0.28f)
                verticalLineToRelative(0.79f)
                lineToRelative(5.0f, 4.99f)
                lineTo(20.49f, 19.0f)
                lineToRelative(-4.99f, -5.0f)
                close()
                moveTo(9.5f, 14.0f)
                curveTo(7.01f, 14.0f, 5.0f, 11.99f, 5.0f, 9.5f)
                reflectiveCurveTo(7.01f, 5.0f, 9.5f, 5.0f)
                reflectiveCurveTo(14.0f, 7.01f, 14.0f, 9.5f)
                reflectiveCurveTo(11.99f, 14.0f, 9.5f, 14.0f)
                close()
            }
        }
        return _search!!
    }

private var _search: ImageVector? = null

public val Icons.Outlined.Security: ImageVector
    get() {
        if (_security != null) {
            return _security!!
        }
        _security = materialIcon(name = "Outlined.Security") {
            materialPath {
                moveTo(12.0f, 1.0f)
                lineTo(3.0f, 5.0f)
                verticalLineToRelative(6.0f)
                curveToRelative(0.0f, 5.55f, 3.84f, 10.74f, 9.0f, 12.0f)
                curveToRelative(5.16f, -1.26f, 9.0f, -6.45f, 9.0f, -12.0f)
                lineTo(21.0f, 5.0f)
                lineToRelative(-9.0f, -4.0f)
                close()
                moveTo(12.0f, 11.99f)
                horizontalLineToRelative(7.0f)
                curveToRelative(-0.53f, 4.12f, -3.28f, 7.79f, -7.0f, 8.94f)
                lineTo(12.0f, 12.0f)
                lineTo(5.0f, 12.0f)
                lineTo(5.0f, 6.3f)
                lineToRelative(7.0f, -3.11f)
                verticalLineToRelative(8.8f)
                close()
            }
        }
        return _security!!
    }

private var _security: ImageVector? = null

public val Icons.Outlined.Settings: ImageVector
    get() {
        if (_settings != null) {
            return _settings!!
        }
        _settings = materialIcon(name = "Outlined.Settings") {
            materialPath {
                moveTo(19.43f, 12.98f)
                curveToRelative(0.04f, -0.32f, 0.07f, -0.64f, 0.07f, -0.98f)
                curveToRelative(0.0f, -0.34f, -0.03f, -0.66f, -0.07f, -0.98f)
                lineToRelative(2.11f, -1.65f)
                curveToRelative(0.19f, -0.15f, 0.24f, -0.42f, 0.12f, -0.64f)
                lineToRelative(-2.0f, -3.46f)
                curveToRelative(-0.09f, -0.16f, -0.26f, -0.25f, -0.44f, -0.25f)
                curveToRelative(-0.06f, 0.0f, -0.12f, 0.01f, -0.17f, 0.03f)
                lineToRelative(-2.49f, 1.0f)
                curveToRelative(-0.52f, -0.4f, -1.08f, -0.73f, -1.69f, -0.98f)
                lineToRelative(-0.38f, -2.65f)
                curveTo(14.46f, 2.18f, 14.25f, 2.0f, 14.0f, 2.0f)
                horizontalLineToRelative(-4.0f)
                curveToRelative(-0.25f, 0.0f, -0.46f, 0.18f, -0.49f, 0.42f)
                lineToRelative(-0.38f, 2.65f)
                curveToRelative(-0.61f, 0.25f, -1.17f, 0.59f, -1.69f, 0.98f)
                lineToRelative(-2.49f, -1.0f)
                curveToRelative(-0.06f, -0.02f, -0.12f, -0.03f, -0.18f, -0.03f)
                curveToRelative(-0.17f, 0.0f, -0.34f, 0.09f, -0.43f, 0.25f)
                lineToRelative(-2.0f, 3.46f)
                curveToRelative(-0.13f, 0.22f, -0.07f, 0.49f, 0.12f, 0.64f)
                lineToRelative(2.11f, 1.65f)
                curveToRelative(-0.04f, 0.32f, -0.07f, 0.65f, -0.07f, 0.98f)
                curveToRelative(0.0f, 0.33f, 0.03f, 0.66f, 0.07f, 0.98f)
                lineToRelative(-2.11f, 1.65f)
                curveToRelative(-0.19f, 0.15f, -0.24f, 0.42f, -0.12f, 0.64f)
                lineToRelative(2.0f, 3.46f)
                curveToRelative(0.09f, 0.16f, 0.26f, 0.25f, 0.44f, 0.25f)
                curveToRelative(0.06f, 0.0f, 0.12f, -0.01f, 0.17f, -0.03f)
                lineToRelative(2.49f, -1.0f)
                curveToRelative(0.52f, 0.4f, 1.08f, 0.73f, 1.69f, 0.98f)
                lineToRelative(0.38f, 2.65f)
                curveToRelative(0.03f, 0.24f, 0.24f, 0.42f, 0.49f, 0.42f)
                horizontalLineToRelative(4.0f)
                curveToRelative(0.25f, 0.0f, 0.46f, -0.18f, 0.49f, -0.42f)
                lineToRelative(0.38f, -2.65f)
                curveToRelative(0.61f, -0.25f, 1.17f, -0.59f, 1.69f, -0.98f)
                lineToRelative(2.49f, 1.0f)
                curveToRelative(0.06f, 0.02f, 0.12f, 0.03f, 0.18f, 0.03f)
                curveToRelative(0.17f, 0.0f, 0.34f, -0.09f, 0.43f, -0.25f)
                lineToRelative(2.0f, -3.46f)
                curveToRelative(0.12f, -0.22f, 0.07f, -0.49f, -0.12f, -0.64f)
                lineToRelative(-2.11f, -1.65f)
                close()
                moveTo(17.45f, 11.27f)
                curveToRelative(0.04f, 0.31f, 0.05f, 0.52f, 0.05f, 0.73f)
                curveToRelative(0.0f, 0.21f, -0.02f, 0.43f, -0.05f, 0.73f)
                lineToRelative(-0.14f, 1.13f)
                lineToRelative(0.89f, 0.7f)
                lineToRelative(1.08f, 0.84f)
                lineToRelative(-0.7f, 1.21f)
                lineToRelative(-1.27f, -0.51f)
                lineToRelative(-1.04f, -0.42f)
                lineToRelative(-0.9f, 0.68f)
                curveToRelative(-0.43f, 0.32f, -0.84f, 0.56f, -1.25f, 0.73f)
                lineToRelative(-1.06f, 0.43f)
                lineToRelative(-0.16f, 1.13f)
                lineToRelative(-0.2f, 1.35f)
                horizontalLineToRelative(-1.4f)
                lineToRelative(-0.19f, -1.35f)
                lineToRelative(-0.16f, -1.13f)
                lineToRelative(-1.06f, -0.43f)
                curveToRelative(-0.43f, -0.18f, -0.83f, -0.41f, -1.23f, -0.71f)
                lineToRelative(-0.91f, -0.7f)
                lineToRelative(-1.06f, 0.43f)
                lineToRelative(-1.27f, 0.51f)
                lineToRelative(-0.7f, -1.21f)
                lineToRelative(1.08f, -0.84f)
                lineToRelative(0.89f, -0.7f)
                lineToRelative(-0.14f, -1.13f)
                curveToRelative(-0.03f, -0.31f, -0.05f, -0.54f, -0.05f, -0.74f)
                reflectiveCurveToRelative(0.02f, -0.43f, 0.05f, -0.73f)
                lineToRelative(0.14f, -1.13f)
                lineToRelative(-0.89f, -0.7f)
                lineToRelative(-1.08f, -0.84f)
                lineToRelative(0.7f, -1.21f)
                lineToRelative(1.27f, 0.51f)
                lineToRelative(1.04f, 0.42f)
                lineToRelative(0.9f, -0.68f)
                curveToRelative(0.43f, -0.32f, 0.84f, -0.56f, 1.25f, -0.73f)
                lineToRelative(1.06f, -0.43f)
                lineToRelative(0.16f, -1.13f)
                lineToRelative(0.2f, -1.35f)
                horizontalLineToRelative(1.39f)
                lineToRelative(0.19f, 1.35f)
                lineToRelative(0.16f, 1.13f)
                lineToRelative(1.06f, 0.43f)
                curveToRelative(0.43f, 0.18f, 0.83f, 0.41f, 1.23f, 0.71f)
                lineToRelative(0.91f, 0.7f)
                lineToRelative(1.06f, -0.43f)
                lineToRelative(1.27f, -0.51f)
                lineToRelative(0.7f, 1.21f)
                lineToRelative(-1.07f, 0.85f)
                lineToRelative(-0.89f, 0.7f)
                lineToRelative(0.14f, 1.13f)
                close()
                moveTo(12.0f, 8.0f)
                curveToRelative(-2.21f, 0.0f, -4.0f, 1.79f, -4.0f, 4.0f)
                reflectiveCurveToRelative(1.79f, 4.0f, 4.0f, 4.0f)
                reflectiveCurveToRelative(4.0f, -1.79f, 4.0f, -4.0f)
                reflectiveCurveToRelative(-1.79f, -4.0f, -4.0f, -4.0f)
                close()
                moveTo(12.0f, 14.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, -0.9f, -2.0f, -2.0f)
                reflectiveCurveToRelative(0.9f, -2.0f, 2.0f, -2.0f)
                reflectiveCurveToRelative(2.0f, 0.9f, 2.0f, 2.0f)
                reflectiveCurveToRelative(-0.9f, 2.0f, -2.0f, 2.0f)
                close()
            }
        }
        return _settings!!
    }

private var _settings: ImageVector? = null

public val Icons.Outlined.SettingsSuggest: ImageVector
    get() {
        if (_settingsSuggest != null) {
            return _settingsSuggest!!
        }
        _settingsSuggest = materialIcon(name = "Outlined.SettingsSuggest") {
            materialPath {
                moveTo(10.0f, 13.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, 0.45f, 1.0f, 1.0f)
                reflectiveCurveToRelative(-0.45f, 1.0f, -1.0f, 1.0f)
                reflectiveCurveToRelative(-1.0f, -0.45f, -1.0f, -1.0f)
                reflectiveCurveTo(9.45f, 13.0f, 10.0f, 13.0f)
                moveTo(10.0f, 11.0f)
                curveToRelative(-1.66f, 0.0f, -3.0f, 1.34f, -3.0f, 3.0f)
                reflectiveCurveToRelative(1.34f, 3.0f, 3.0f, 3.0f)
                reflectiveCurveToRelative(3.0f, -1.34f, 3.0f, -3.0f)
                reflectiveCurveTo(11.66f, 11.0f, 10.0f, 11.0f)
                lineTo(10.0f, 11.0f)
                close()
                moveTo(18.5f, 9.0f)
                lineToRelative(1.09f, -2.41f)
                lineTo(22.0f, 5.5f)
                lineToRelative(-2.41f, -1.09f)
                lineTo(18.5f, 2.0f)
                lineToRelative(-1.09f, 2.41f)
                lineTo(15.0f, 5.5f)
                lineToRelative(2.41f, 1.09f)
                lineTo(18.5f, 9.0f)
                close()
                moveTo(21.28f, 12.72f)
                lineTo(20.5f, 11.0f)
                lineToRelative(-0.78f, 1.72f)
                lineTo(18.0f, 13.5f)
                lineToRelative(1.72f, 0.78f)
                lineTo(20.5f, 16.0f)
                lineToRelative(0.78f, -1.72f)
                lineTo(23.0f, 13.5f)
                lineTo(21.28f, 12.72f)
                close()
                moveTo(16.25f, 14.0f)
                curveToRelative(0.0f, -0.12f, 0.0f, -0.25f, -0.01f, -0.37f)
                lineToRelative(1.94f, -1.47f)
                lineToRelative(-2.5f, -4.33f)
                lineToRelative(-2.24f, 0.94f)
                curveToRelative(-0.2f, -0.13f, -0.42f, -0.26f, -0.64f, -0.37f)
                lineTo(12.5f, 6.0f)
                horizontalLineToRelative(-5.0f)
                lineTo(7.2f, 8.41f)
                curveTo(6.98f, 8.52f, 6.77f, 8.65f, 6.56f, 8.78f)
                lineTo(4.32f, 7.83f)
                lineToRelative(-2.5f, 4.33f)
                lineToRelative(1.94f, 1.47f)
                curveTo(3.75f, 13.75f, 3.75f, 13.88f, 3.75f, 14.0f)
                reflectiveCurveToRelative(0.0f, 0.25f, 0.01f, 0.37f)
                lineToRelative(-1.94f, 1.47f)
                lineToRelative(2.5f, 4.33f)
                lineToRelative(2.24f, -0.94f)
                curveToRelative(0.2f, 0.13f, 0.42f, 0.26f, 0.64f, 0.37f)
                lineTo(7.5f, 22.0f)
                horizontalLineToRelative(5.0f)
                lineToRelative(0.3f, -2.41f)
                curveToRelative(0.22f, -0.11f, 0.43f, -0.23f, 0.64f, -0.37f)
                lineToRelative(2.24f, 0.94f)
                lineToRelative(2.5f, -4.33f)
                lineToRelative(-1.94f, -1.47f)
                curveTo(16.25f, 14.25f, 16.25f, 14.12f, 16.25f, 14.0f)
                close()
                moveTo(14.83f, 17.64f)
                lineToRelative(-1.73f, -0.73f)
                curveToRelative(-0.56f, 0.6f, -1.3f, 1.04f, -2.13f, 1.23f)
                lineTo(10.73f, 20.0f)
                horizontalLineTo(9.27f)
                lineToRelative(-0.23f, -1.86f)
                curveToRelative(-0.83f, -0.19f, -1.57f, -0.63f, -2.13f, -1.23f)
                lineToRelative(-1.73f, 0.73f)
                lineToRelative(-0.73f, -1.27f)
                lineToRelative(1.49f, -1.13f)
                curveToRelative(-0.12f, -0.39f, -0.18f, -0.8f, -0.18f, -1.23f)
                curveToRelative(0.0f, -0.43f, 0.06f, -0.84f, 0.18f, -1.23f)
                lineToRelative(-1.49f, -1.13f)
                lineToRelative(0.73f, -1.27f)
                lineToRelative(1.73f, 0.73f)
                curveToRelative(0.56f, -0.6f, 1.3f, -1.04f, 2.13f, -1.23f)
                lineTo(9.27f, 8.0f)
                horizontalLineToRelative(1.47f)
                lineToRelative(0.23f, 1.86f)
                curveToRelative(0.83f, 0.19f, 1.57f, 0.63f, 2.13f, 1.23f)
                lineToRelative(1.73f, -0.73f)
                lineToRelative(0.73f, 1.27f)
                lineToRelative(-1.49f, 1.13f)
                curveToRelative(0.12f, 0.39f, 0.18f, 0.8f, 0.18f, 1.23f)
                curveToRelative(0.0f, 0.43f, -0.06f, 0.84f, -0.18f, 1.23f)
                lineToRelative(1.49f, 1.13f)
                lineTo(14.83f, 17.64f)
                close()
            }
        }
        return _settingsSuggest!!
    }

private var _settingsSuggest: ImageVector? = null

public val Icons.Outlined.Stop: ImageVector
    get() {
        if (_stop != null) {
            return _stop!!
        }
        _stop = materialIcon(name = "Outlined.Stop") {
            materialPath {
                moveTo(16.0f, 8.0f)
                verticalLineToRelative(8.0f)
                horizontalLineTo(8.0f)
                verticalLineTo(8.0f)
                horizontalLineToRelative(8.0f)
                moveToRelative(2.0f, -2.0f)
                horizontalLineTo(6.0f)
                verticalLineToRelative(12.0f)
                horizontalLineToRelative(12.0f)
                verticalLineTo(6.0f)
                close()
            }
        }
        return _stop!!
    }

private var _stop: ImageVector? = null

public val Icons.Outlined.TravelExplore: ImageVector
    get() {
        if (_travelExplore != null) {
            return _travelExplore!!
        }
        _travelExplore = materialIcon(name = "Outlined.TravelExplore") {
            materialPath {
                moveTo(19.3f, 16.9f)
                curveToRelative(0.4f, -0.7f, 0.7f, -1.5f, 0.7f, -2.4f)
                curveToRelative(0.0f, -2.5f, -2.0f, -4.5f, -4.5f, -4.5f)
                reflectiveCurveTo(11.0f, 12.0f, 11.0f, 14.5f)
                reflectiveCurveToRelative(2.0f, 4.5f, 4.5f, 4.5f)
                curveToRelative(0.9f, 0.0f, 1.7f, -0.3f, 2.4f, -0.7f)
                lineToRelative(3.2f, 3.2f)
                lineToRelative(1.4f, -1.4f)
                lineTo(19.3f, 16.9f)
                close()
                moveTo(15.5f, 17.0f)
                curveToRelative(-1.4f, 0.0f, -2.5f, -1.1f, -2.5f, -2.5f)
                reflectiveCurveToRelative(1.1f, -2.5f, 2.5f, -2.5f)
                reflectiveCurveToRelative(2.5f, 1.1f, 2.5f, 2.5f)
                reflectiveCurveTo(16.9f, 17.0f, 15.5f, 17.0f)
                close()
                moveTo(12.0f, 20.0f)
                verticalLineToRelative(2.0f)
                curveTo(6.48f, 22.0f, 2.0f, 17.52f, 2.0f, 12.0f)
                curveTo(2.0f, 6.48f, 6.48f, 2.0f, 12.0f, 2.0f)
                curveToRelative(4.84f, 0.0f, 8.87f, 3.44f, 9.8f, 8.0f)
                horizontalLineToRelative(-2.07f)
                curveToRelative(-0.64f, -2.46f, -2.4f, -4.47f, -4.73f, -5.41f)
                verticalLineTo(5.0f)
                curveToRelative(0.0f, 1.1f, -0.9f, 2.0f, -2.0f, 2.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(2.0f)
                curveToRelative(0.0f, 0.55f, -0.45f, 1.0f, -1.0f, 1.0f)
                horizontalLineTo(8.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(3.0f)
                horizontalLineTo(9.0f)
                lineToRelative(-4.79f, -4.79f)
                curveTo(4.08f, 10.79f, 4.0f, 11.38f, 4.0f, 12.0f)
                curveTo(4.0f, 16.41f, 7.59f, 20.0f, 12.0f, 20.0f)
                close()
            }
        }
        return _travelExplore!!
    }

private var _travelExplore: ImageVector? = null

public val Icons.Outlined.Tune: ImageVector
    get() {
        if (_tune != null) {
            return _tune!!
        }
        _tune = materialIcon(name = "Outlined.Tune") {
            materialPath {
                moveTo(3.0f, 17.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(6.0f)
                verticalLineToRelative(-2.0f)
                lineTo(3.0f, 17.0f)
                close()
                moveTo(3.0f, 5.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(10.0f)
                lineTo(13.0f, 5.0f)
                lineTo(3.0f, 5.0f)
                close()
                moveTo(13.0f, 21.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(8.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(-8.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(6.0f)
                horizontalLineToRelative(2.0f)
                close()
                moveTo(7.0f, 9.0f)
                verticalLineToRelative(2.0f)
                lineTo(3.0f, 11.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(4.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(2.0f)
                lineTo(9.0f, 9.0f)
                lineTo(7.0f, 9.0f)
                close()
                moveTo(21.0f, 13.0f)
                verticalLineToRelative(-2.0f)
                lineTo(11.0f, 11.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(10.0f)
                close()
                moveTo(15.0f, 9.0f)
                horizontalLineToRelative(2.0f)
                lineTo(17.0f, 7.0f)
                horizontalLineToRelative(4.0f)
                lineTo(21.0f, 5.0f)
                horizontalLineToRelative(-4.0f)
                lineTo(17.0f, 3.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(6.0f)
                close()
            }
        }
        return _tune!!
    }

private var _tune: ImageVector? = null

public val Icons.Outlined.Tv: ImageVector
    get() {
        if (_tv != null) {
            return _tv!!
        }
        _tv = materialIcon(name = "Outlined.Tv") {
            materialPath {
                moveTo(21.0f, 3.0f)
                lineTo(3.0f, 3.0f)
                curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
                verticalLineToRelative(12.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(5.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(8.0f)
                verticalLineToRelative(-2.0f)
                horizontalLineToRelative(5.0f)
                curveToRelative(1.1f, 0.0f, 1.99f, -0.9f, 1.99f, -2.0f)
                lineTo(23.0f, 5.0f)
                curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
                close()
                moveTo(21.0f, 17.0f)
                lineTo(3.0f, 17.0f)
                lineTo(3.0f, 5.0f)
                horizontalLineToRelative(18.0f)
                verticalLineToRelative(12.0f)
                close()
            }
        }
        return _tv!!
    }

private var _tv: ImageVector? = null

public val Icons.Outlined.VerifiedUser: ImageVector
    get() {
        if (_verifiedUser != null) {
            return _verifiedUser!!
        }
        _verifiedUser = materialIcon(name = "Outlined.VerifiedUser") {
            materialPath {
                moveTo(12.0f, 1.0f)
                lineTo(3.0f, 5.0f)
                verticalLineToRelative(6.0f)
                curveToRelative(0.0f, 5.55f, 3.84f, 10.74f, 9.0f, 12.0f)
                curveToRelative(5.16f, -1.26f, 9.0f, -6.45f, 9.0f, -12.0f)
                lineTo(21.0f, 5.0f)
                lineToRelative(-9.0f, -4.0f)
                close()
                moveTo(19.0f, 11.0f)
                curveToRelative(0.0f, 4.52f, -2.98f, 8.69f, -7.0f, 9.93f)
                curveToRelative(-4.02f, -1.24f, -7.0f, -5.41f, -7.0f, -9.93f)
                lineTo(5.0f, 6.3f)
                lineToRelative(7.0f, -3.11f)
                lineToRelative(7.0f, 3.11f)
                lineTo(19.0f, 11.0f)
                close()
                moveTo(7.41f, 11.59f)
                lineTo(6.0f, 13.0f)
                lineToRelative(4.0f, 4.0f)
                lineToRelative(8.0f, -8.0f)
                lineToRelative(-1.41f, -1.42f)
                lineTo(10.0f, 14.17f)
                close()
            }
        }
        return _verifiedUser!!
    }

private var _verifiedUser: ImageVector? = null

public val Icons.Outlined.VideoFile: ImageVector
    get() {
        if (_videoFile != null) {
            return _videoFile!!
        }
        _videoFile = materialIcon(name = "Outlined.VideoFile") {
            materialPath {
                moveTo(14.0f, 2.0f)
                horizontalLineTo(6.0f)
                curveTo(4.9f, 2.0f, 4.0f, 2.9f, 4.0f, 4.0f)
                verticalLineToRelative(16.0f)
                curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
                horizontalLineToRelative(12.0f)
                curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
                verticalLineTo(8.0f)
                lineTo(14.0f, 2.0f)
                close()
                moveTo(6.0f, 20.0f)
                verticalLineTo(4.0f)
                horizontalLineToRelative(7.0f)
                verticalLineToRelative(5.0f)
                horizontalLineToRelative(5.0f)
                verticalLineToRelative(11.0f)
                horizontalLineTo(6.0f)
                close()
                moveTo(14.0f, 14.0f)
                lineToRelative(2.0f, -1.06f)
                verticalLineToRelative(4.12f)
                lineTo(14.0f, 16.0f)
                verticalLineToRelative(1.0f)
                curveToRelative(0.0f, 0.55f, -0.45f, 1.0f, -1.0f, 1.0f)
                horizontalLineTo(9.0f)
                curveToRelative(-0.55f, 0.0f, -1.0f, -0.45f, -1.0f, -1.0f)
                verticalLineToRelative(-4.0f)
                curveToRelative(0.0f, -0.55f, 0.45f, -1.0f, 1.0f, -1.0f)
                horizontalLineToRelative(4.0f)
                curveToRelative(0.55f, 0.0f, 1.0f, 0.45f, 1.0f, 1.0f)
                verticalLineTo(14.0f)
                close()
            }
        }
        return _videoFile!!
    }

private var _videoFile: ImageVector? = null

public val Icons.Outlined.WarningAmber: ImageVector
    get() {
        if (_warningAmber != null) {
            return _warningAmber!!
        }
        _warningAmber = materialIcon(name = "Outlined.WarningAmber") {
            materialPath {
                moveTo(12.0f, 5.99f)
                lineTo(19.53f, 19.0f)
                lineTo(4.47f, 19.0f)
                lineTo(12.0f, 5.99f)
                moveTo(12.0f, 2.0f)
                lineTo(1.0f, 21.0f)
                horizontalLineToRelative(22.0f)
                lineTo(12.0f, 2.0f)
                close()
                moveTo(13.0f, 16.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(2.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(-2.0f)
                close()
                moveTo(13.0f, 10.0f)
                horizontalLineToRelative(-2.0f)
                verticalLineToRelative(4.0f)
                horizontalLineToRelative(2.0f)
                verticalLineToRelative(-4.0f)
                close()
            }
        }
        return _warningAmber!!
    }

private var _warningAmber: ImageVector? = null
