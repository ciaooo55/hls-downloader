//! Singleton Win32 Job Object ensuring spawned helper processes (`ffmpeg`,
//! `ffprobe`, `curl`, antivirus scanners) never outlive `HLSDownloaderEngine.exe`
//! if the engine crashes or is terminated abruptly.

use std::process::Child;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_BREAKAWAY_OK: u32 = 0x0000_0800;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(lp_job_attributes: *const c_void, lp_name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            h_job: *mut c_void,
            job_object_information_class: i32,
            lp_job_object_information: *const c_void,
            cb_job_object_information_length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(h_job: *mut c_void, h_process: *mut c_void) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    pub(super) struct JobHandle(*mut c_void);

    // SAFETY: Win32 job object kernel handles are process-wide kernel objects without
    // thread affinity, so moving `JobHandle` across threads is sound.
    unsafe impl Send for JobHandle {}
    // SAFETY: `AssignProcessToJobObject` is synchronized by the Windows kernel on a live
    // job handle, and the singleton `JobHandle` is never mutated or closed concurrently.
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        pub(super) fn create_kill_on_close() -> Option<Self> {
            // SAFETY: Anonymous Win32 Job Object creation with default security descriptor.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let mut info = JobObjectExtendedLimitInformation::default();
            info.basic_limit_information.limit_flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            // SAFETY: `info` matches `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` layout and lifetime.
            let ok = unsafe {
                SetInformationJobObject(
                    handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&raw const info).cast::<c_void>(),
                    std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
                )
            };
            if ok == 0 {
                // SAFETY: Close the newly created handle when configuration fails.
                unsafe {
                    CloseHandle(handle);
                }
                return None;
            }
            Some(Self(handle))
        }

        pub(super) fn assign(&self, child: &Child) -> bool {
            let process = child.as_raw_handle();
            if process.is_null() {
                return false;
            }
            // SAFETY: Both `self.0` and `process` are live kernel handles. If the current
            // process already runs inside a non-nested job on older Windows configurations,
            // `AssignProcessToJobObject` simply returns 0 without harming the child.
            unsafe { AssignProcessToJobObject(self.0, process) != 0 }
        }

        #[cfg(test)]
        pub(super) fn close_for_test(self) {
            // SAFETY: Owned test job handle closed exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(super) fn singleton() -> Option<&'static JobHandle> {
        static JOB: OnceLock<Option<JobHandle>> = OnceLock::new();
        JOB.get_or_init(JobHandle::create_kill_on_close).as_ref()
    }
}

pub(crate) fn assign_child(child: &Child) {
    #[cfg(windows)]
    {
        if let Some(job) = imp::singleton() {
            let _ = job.assign(child);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = child;
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::imp;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn singleton_job_object_initializes_and_kills_assigned_child_on_close() {
        assert!(
            imp::singleton().is_some(),
            "singleton Win32 job object must initialize"
        );

        let job = imp::JobHandle::create_kill_on_close().expect("create test job object");
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("spawn ping child");
        assert!(
            job.assign(&child),
            "AssignProcessToJobObject should succeed"
        );
        std::thread::sleep(Duration::from_millis(80));
        assert!(
            child
                .try_wait()
                .expect("poll child before job close")
                .is_none(),
            "ping -n 30 child must still be running before job handle is closed"
        );

        let started = Instant::now();
        job.close_for_test();
        let _status = child.wait().expect("wait terminated child");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "closing job object with KILL_ON_JOB_CLOSE must terminate 29s child promptly, took {:?}",
            started.elapsed()
        );
    }
}
