//! Sandbox-only process fixture for installation-scope and failed-checkpoint checks.
use std::os::windows::fs::OpenOptionsExt;
fn main() {
    assert_eq!(
        std::env::var("USERNAME").as_deref(),
        Ok("WDAGUtilityAccount")
    );
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--shutdown") {
        std::process::exit(1);
    }
    if args.first().is_some_and(|arg| arg == "--pipe-server") {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateNamedPipeW(
                name: *const u16,
                mode: u32,
                pipe_mode: u32,
                instances: u32,
                output: u32,
                input: u32,
                timeout: u32,
                security: *const c_void,
            ) -> *mut c_void;
            fn ConnectNamedPipe(pipe: *mut c_void, overlapped: *mut c_void) -> i32;
            fn DisconnectNamedPipe(pipe: *mut c_void) -> i32;
        }
        let name: Vec<u16> = args[1].encode_wide().chain(Some(0)).collect();
        let pipe =
            unsafe { CreateNamedPipeW(name.as_ptr(), 3, 0, 1, 4096, 4096, 0, std::ptr::null()) };
        assert_ne!(pipe as isize, -1);
        loop {
            unsafe {
                ConnectNamedPipe(pipe, std::ptr::null_mut());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            unsafe {
                DisconnectNamedPipe(pipe);
            }
        }
    }
    let _lock = args.first().map(|path| {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap()
    });
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
