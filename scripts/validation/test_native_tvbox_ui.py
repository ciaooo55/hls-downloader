"""Unit controls for the native media-push acceptance interaction."""
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from native_tvbox_ui import select_expected_device


class NativeTvboxUiTests(unittest.TestCase):
    def setUp(self):
        self.desktop = MagicMock()
        self.window = self.desktop.return_value.window.return_value
        self.receiver = MagicMock()
        self.confirm = MagicMock()
        self.window.child_window.side_effect = [self.receiver, self.confirm]
        self.module = SimpleNamespace(Desktop=self.desktop)
        self.device = {"label": "Living room", "service_type": "TVBox"}

    def select(self, device=None):
        with patch.dict(sys.modules, {"pywinauto": self.module}):
            return select_expected_device(
                expected_device=self.device if device is None else device,
                expected_host="192.0.2.1",
            )

    def test_receiver_and_confirmation_use_real_uia_actions(self):
        report = self.select()
        self.desktop.assert_called_once_with(backend="uia")
        self.desktop.return_value.window.assert_called_once_with(title="投屏与媒体推送")
        self.assertEqual(self.window.child_window.call_args_list[0].kwargs,
                         {"title": "Living room · TVBox", "control_type": "Button"})
        self.assertEqual(self.window.child_window.call_args_list[1].kwargs,
                         {"title": "发送到所选设备", "control_type": "Button"})
        self.receiver.wrapper_object.return_value.invoke.assert_called_once_with()
        self.confirm.wrapper_object.return_value.invoke.assert_called_once_with()
        self.assertEqual(report["receiver_host"], "192.0.2.1")

    def test_empty_identity_fails_before_touching_desktop(self):
        for device in ({}, {"label": "Living room"}, {"service_type": "TVBox"}):
            with self.subTest(device=device), self.assertRaisesRegex(RuntimeError, "no UI identity"):
                self.select(device)
        self.desktop.assert_not_called()

    def test_missing_or_ambiguous_receiver_never_confirms(self):
        self.receiver.wait.side_effect = RuntimeError("receiver unavailable or ambiguous")
        with self.assertRaisesRegex(RuntimeError, "receiver unavailable"):
            self.select()
        self.receiver.wrapper_object.assert_not_called()
        self.confirm.wrapper_object.assert_not_called()

    def test_confirmation_error_is_not_reported_as_success(self):
        self.confirm.wrapper_object.return_value.invoke.side_effect = RuntimeError("input rejected")
        with self.assertRaisesRegex(RuntimeError, "input rejected"):
            self.select()


if __name__ == "__main__":
    unittest.main()
