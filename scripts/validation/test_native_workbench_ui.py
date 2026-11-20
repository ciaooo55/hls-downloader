"""Controls for native workbench observation used by packaged acceptance."""
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from unittest.mock import MagicMock, patch

try:
    import psutil  # noqa: F401
except ModuleNotFoundError:
    sys.modules["psutil"] = MagicMock()

import smoke_v7_portable_app as portable


class NativeWorkbenchUiTests(unittest.TestCase):
    def setUp(self):
        self.root = Path("/candidate")
        self.desktop = MagicMock()
        self.window = MagicMock()
        self.desktop.return_value.windows.return_value = [self.window]
        self.window.descendants.return_value = [
            SimpleNamespace(element_info=SimpleNamespace(name="下载引擎已连接")),
            SimpleNamespace(element_info=SimpleNamespace(name="portable-validation.bin 已完成")),
        ]
        self.window.rectangle.return_value = SimpleNamespace(left=10, top=20, right=810, bottom=620)
        self.window.is_visible.return_value = True
        self.modules = patch.dict(sys.modules, {"pywinauto": SimpleNamespace(Desktop=self.desktop)})
        self.processes = patch.object(portable, "runtime_processes", return_value=[
            {"pid": 1, "name": "HLSDownloaderEngine.exe"},
            {"pid": 2, "name": "HLSDownloaderPresenter.exe"},
        ])
        self.modules.start()
        self.processes.start()
        self.addCleanup(self.modules.stop)
        self.addCleanup(self.processes.stop)

    def test_observes_native_presenter_window_and_real_connection_label(self):
        state = portable.workbench_state(self.root)
        self.assertTrue(state["showing"])
        self.assertTrue(state["connected"])
        self.assertEqual(state["bounds"], [10, 20, 810, 620])
        self.desktop.assert_called_once_with(backend="uia")
        self.desktop.return_value.windows.assert_called_once_with(
            process=2, title="HLS Downloader", visible_only=True)
        portable.runtime_processes.assert_called_once_with(self.root)

    def test_missing_window_is_not_reported_as_connected(self):
        self.desktop.return_value.windows.return_value = []
        self.assertEqual(portable.workbench_state(self.root),
                         {"showing": False, "connected": False, "names": []})

    def test_disconnected_label_does_not_pass_connection_check(self):
        self.window.descendants.return_value = [
            SimpleNamespace(element_info=SimpleNamespace(name="正在连接下载引擎…"))]
        self.assertFalse(portable.workbench_state(self.root)["connected"])

    def test_ambiguous_windows_fail_closed(self):
        self.desktop.return_value.windows.return_value = [self.window, MagicMock()]
        with self.assertRaisesRegex(RuntimeError, "Multiple native workbench"):
            portable.workbench_state(self.root)

    def test_screenshot_uses_native_window_capture(self):
        def save(output, format):
            self.assertEqual(format, "PNG")
            output.write(b"native screenshot fixture")
        self.window.capture_as_image.return_value.save.side_effect = save
        self.assertEqual(portable.workbench_capture(self.root), b"native screenshot fixture")

    def test_missing_window_cannot_produce_screenshot_evidence(self):
        self.desktop.return_value.windows.return_value = []
        with self.assertRaisesRegex(RuntimeError, "not visible"):
            portable.workbench_capture(self.root)


if __name__ == "__main__":
    unittest.main()
