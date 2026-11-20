"""Native Windows UI Automation for the browser media-push acceptance gate."""

def select_expected_device(
    *,
    expected_device: dict[str, object],
    expected_host: str,
) -> dict[str, object]:
    """Select and confirm the receiver through native Windows UI Automation."""
    from pywinauto import Desktop

    label = str(expected_device.get("label", "")).strip()
    service_type = str(expected_device.get("service_type", "")).strip()
    if not label or not service_type:
        raise RuntimeError(f"Discovered receiver has no UI identity: {expected_device}")
    window = Desktop(backend="uia").window(title="投屏与媒体推送")
    window.wait("visible enabled", timeout=60)
    device_name = f"{label} · {service_type}"
    receiver = window.child_window(title=device_name, control_type="Button")
    receiver.wait("visible enabled", timeout=60)
    receiver.wrapper_object().invoke()
    confirm = window.child_window(title="发送到所选设备", control_type="Button")
    confirm.wait("visible enabled", timeout=60)
    confirm.wrapper_object().invoke()
    return {
        "receiver_label": label,
        "receiver_host": expected_host,
        "receiver_service_type": service_type,
        "action": "发送到所选设备",
        "automation": "Windows UI Automation",
    }
