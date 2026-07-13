import time

class ProgressTracker:
    def __init__(self):
        self.total = 0
        self.completed = 0
        self.downloaded_bytes = 0
        self._start_time = 0
        self._history = []  # (timestamp, bytes)

    def start(self, total: int):
        self.total = total
        self.completed = 0
        self.downloaded_bytes = 0
        self._start_time = time.monotonic()
        self._history = []

    def add_completed(self, size: int):
        self.completed += 1
        self.downloaded_bytes += size
        now = time.monotonic()
        self._history.append((now, size))
        # Keep only last 15 seconds OR max 150 entries
        if len(self._history) > 150:
            cutoff = now - 15
            self._history = [(t, b) for t, b in self._history if t > cutoff]
            # If still too many (burst), keep only last 100
            if len(self._history) > 100:
                self._history = self._history[-100:]

    def snapshot(self) -> dict:
        elapsed = time.monotonic() - self._start_time if self._start_time else 0

        now = time.monotonic()
        cutoff = now - 10
        window_bytes = 0
        window_start = now
        for t, b in self._history:
            if t > cutoff:
                window_bytes += b
                if t < window_start:
                    window_start = t
        if window_bytes > 0:
            dt = max(now - window_start, 0.1)
            speed = window_bytes / dt
        else:
            speed = self.downloaded_bytes / max(elapsed, 0.1)

        remaining = self.total - self.completed
        avg_per_seg = self.downloaded_bytes / max(self.completed, 1)
        remaining_bytes = avg_per_seg * remaining
        eta = remaining_bytes / speed if speed > 0 else 0

        return {
            "total": self.total,
            "completed": self.completed,
            "downloaded_bytes": self.downloaded_bytes,
            "total_bytes": self.downloaded_bytes,
            "speed": speed,
            "eta": eta,
        }
