use std::{ffi::c_void, path::PathBuf};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

fn main() {
    use std::os::windows::ffi::OsStrExt;
    let path = PathBuf::from(std::env::args_os().nth(1).expect("host DLL path required"));
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let module = LoadLibraryW(wide.as_ptr());
        assert!(!module.is_null(), "host DLL load failed");
        let start: extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(
            GetProcAddress(module, c"Start".as_ptr().cast()).expect("Start export missing"),
        );
        // An owned test process must be rejected without entering any game code.
        assert_eq!(start(std::ptr::null_mut()), 1);
        assert_eq!(start(std::ptr::null_mut()), 1);
        let log = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
            .join("SubspaceToolkit")
            .join(format!("live-bootstrap-{}.log", std::process::id()));
        let text = std::fs::read_to_string(log).unwrap();
        assert_eq!(
            text.matches("Wrong target process").count(),
            2,
            "native host did not reach managed process guard: {text}"
        );
        println!(
            "Native host initialized private CLR, entered Default context and rejected wrong target twice"
        );
    }
}
