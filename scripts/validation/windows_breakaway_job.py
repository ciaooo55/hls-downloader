"""Run validation inside a nested Windows job that permits explicit breakaway.

The outer runner job remains in effect. Production children must still request
CREATE_BREAKAWAY_FROM_JOB to escape the validation/browser lifetime.
"""
from contextlib import contextmanager
import ctypes
from ctypes import wintypes
import os
import subprocess
import sys


class BasicLimits(ctypes.Structure):
    _fields_ = [
        ('PerProcessUserTimeLimit', ctypes.c_int64),
        ('PerJobUserTimeLimit', ctypes.c_int64),
        ('LimitFlags', wintypes.DWORD),
        ('MinimumWorkingSetSize', ctypes.c_size_t),
        ('MaximumWorkingSetSize', ctypes.c_size_t),
        ('ActiveProcessLimit', wintypes.DWORD),
        ('Affinity', ctypes.c_size_t),
        ('PriorityClass', wintypes.DWORD),
        ('SchedulingClass', wintypes.DWORD),
    ]


class ExtendedLimits(ctypes.Structure):
    _fields_ = [
        ('BasicLimitInformation', BasicLimits),
        ('IoInfo', ctypes.c_uint64 * 6),
        ('ProcessMemoryLimit', ctypes.c_size_t),
        ('JobMemoryLimit', ctypes.c_size_t),
        ('PeakProcessMemoryUsed', ctypes.c_size_t),
        ('PeakJobMemoryUsed', ctypes.c_size_t),
    ]


def job_api():
    if os.name != 'nt':
        raise RuntimeError('Windows job validation requires Windows')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    declarations = {
        'CreateJobObjectW': ([ctypes.c_void_p, wintypes.LPCWSTR], wintypes.HANDLE),
        'OpenJobObjectW': ([wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR], wintypes.HANDLE),
        'SetInformationJobObject': ([wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD], wintypes.BOOL),
        'QueryInformationJobObject': ([wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p], wintypes.BOOL),
        'AssignProcessToJobObject': ([wintypes.HANDLE, wintypes.HANDLE], wintypes.BOOL),
        'IsProcessInJob': ([wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)], wintypes.BOOL),
        'TerminateJobObject': ([wintypes.HANDLE, wintypes.UINT], wintypes.BOOL),
        'OpenProcess': ([wintypes.DWORD, wintypes.BOOL, wintypes.DWORD], wintypes.HANDLE),
        'WaitForSingleObject': ([wintypes.HANDLE, wintypes.DWORD], wintypes.DWORD),
        'CloseHandle': ([wintypes.HANDLE], wintypes.BOOL),
        'GetCurrentProcess': ([], wintypes.HANDLE),
    }
    for name, (arguments, result) in declarations.items():
        function = getattr(kernel, name)
        function.argtypes = arguments
        function.restype = result
    return kernel


@contextmanager
def breakaway_job(name=None, *, kill_on_close=False):
    kernel = job_api()
    ctypes.set_last_error(0)
    handle = kernel.CreateJobObjectW(None, name)
    error = ctypes.get_last_error()
    if not handle:
        raise ctypes.WinError(error)
    try:
        if name and error == 183:  # Never change a pre-existing named job.
            raise RuntimeError('Validation job already exists')
        limits = ExtendedLimits()
        limits.BasicLimitInformation.LimitFlags = 0x0800  # BREAKAWAY_OK
        if kill_on_close:
            limits.BasicLimitInformation.LimitFlags |= 0x2000
        if not kernel.SetInformationJobObject(handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)):
            raise ctypes.WinError(ctypes.get_last_error())
        if not kernel.AssignProcessToJobObject(handle, kernel.GetCurrentProcess()):
            raise ctypes.WinError(ctypes.get_last_error())
        yield handle
    finally:
        kernel.CloseHandle(handle)


def main():
    if len(sys.argv) < 2:
        raise SystemExit('Usage: windows_breakaway_job.py COMMAND [ARG ...]')
    # This wrapper is not assigned KILL_ON_JOB_CLOSE: closing its own handle
    # must not kill the wrapper before it returns the validation exit code.
    with breakaway_job():
        return subprocess.call(sys.argv[1:])


if __name__ == '__main__':
    raise SystemExit(main())
