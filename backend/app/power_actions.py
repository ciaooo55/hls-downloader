import asyncio
from dataclasses import dataclass
from datetime import datetime, timedelta
import os
import subprocess
import uuid
from collections.abc import Callable


POWER_ACTIONS = {"shutdown", "sleep", "hibernate"}


@dataclass
class PendingPowerAction:
    id: str
    task_id: str
    task_title: str
    action: str
    execute_at: str
    handle: asyncio.Task
    executor: Callable[[str], None]


class PowerActionService:
    def __init__(self, delay_seconds: int = 30) -> None:
        self.delay_seconds = max(5, int(delay_seconds))
        self._pending: dict[str, PendingPowerAction] = {}
        self._fired: set[str] = set()

    def schedule(
        self,
        *,
        task_id: str,
        task_title: str,
        action: str,
        publish: Callable[[dict], None],
        executor: Callable[[str], None] | None = None,
    ) -> str:
        normalized = str(action or "").strip().lower()
        if normalized not in POWER_ACTIONS:
            raise ValueError("不支持的完成后电源动作")
        for pending in list(self._pending.values()):
            if pending.task_id == task_id:
                return pending.id
        action_id = uuid.uuid4().hex
        execute_at = (datetime.now() + timedelta(seconds=self.delay_seconds)).isoformat()
        run = executor or self._execute

        async def wait_and_execute() -> None:
            try:
                publish(self._event(action_id, task_id, task_title, normalized, execute_at))
                await asyncio.sleep(self.delay_seconds)
                if self._fire(action_id, normalized, run):
                    publish({"type": "power_action_executed", "power_action_id": action_id})
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                publish({
                    "type": "power_action_failed",
                    "power_action_id": action_id,
                    "message": str(exc)[:300],
                })
            finally:
                self._pending.pop(action_id, None)

        handle = asyncio.create_task(wait_and_execute(), name=f"power-action-{action_id}")
        self._pending[action_id] = PendingPowerAction(
            id=action_id,
            task_id=task_id,
            task_title=task_title,
            action=normalized,
            execute_at=execute_at,
            handle=handle,
            executor=run,
        )
        return action_id

    def _fire(self, action_id: str, action: str, executor: Callable[[str], None]) -> bool:
        if action_id in self._fired:
            return False
        self._fired.add(action_id)
        if len(self._fired) > 64:
            extra = list(self._fired)[:-32]
            self._fired.difference_update(extra)
        executor(action)
        return True

    def _event(
        self,
        action_id: str,
        task_id: str,
        task_title: str,
        action: str,
        execute_at: str,
    ) -> dict:
        try:
            remaining = max(
                0,
                int((datetime.fromisoformat(execute_at) - datetime.now()).total_seconds() + 0.999),
            )
        except ValueError:
            remaining = self.delay_seconds
        return {
            "type": "power_action_pending",
            "power_action_id": action_id,
            "task_id": task_id,
            "task_title": task_title,
            "action": action,
            "execute_at": execute_at,
            "delay_seconds": remaining,
        }

    def pending(self, action_id: str) -> dict | None:
        item = self._pending.get(action_id)
        if item is None:
            return None
        return self._event(
            item.id, item.task_id, item.task_title, item.action, item.execute_at
        )

    def all_pending(self) -> list[dict]:
        return [
            self._event(item.id, item.task_id, item.task_title, item.action, item.execute_at)
            for item in self._pending.values()
        ]

    def cancel(self, action_id: str) -> bool:
        item = self._pending.pop(action_id, None)
        if item is None:
            return False
        self._fired.add(action_id)
        item.handle.cancel()
        return True

    def confirm(self, action_id: str) -> bool:
        item = self._pending.get(action_id)
        if item is None:
            return False
        fired = self._fire(action_id, item.action, item.executor)
        self._pending.pop(action_id, None)
        item.handle.cancel()
        return fired

    def close(self) -> None:
        for item in list(self._pending.values()):
            item.handle.cancel()
        self._pending.clear()

    @staticmethod
    def _execute(action: str) -> None:
        if os.name != "nt":
            raise OSError("电源动作仅支持 Windows")
        creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        if action == "shutdown":
            command = ["shutdown.exe", "/s", "/t", "0"]
        elif action == "hibernate":
            command = ["shutdown.exe", "/h"]
        elif action == "sleep":
            command = [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; "
                "[System.Windows.Forms.Application]::SetSuspendState('Suspend',$false,$false)",
            ]
        else:
            raise ValueError("不支持的电源动作")
        subprocess.Popen(command, creationflags=creationflags)


power_action_service = PowerActionService()
