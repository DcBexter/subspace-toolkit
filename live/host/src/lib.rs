use std::{
    ffi::c_void,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};
use windows_sys::Win32::{Foundation::HMODULE, System::LibraryLoader::*};

static STATE: AtomicU32 = AtomicU32::new(0);
type Init = unsafe extern "C" fn(*const u16, *const c_void, *mut *mut c_void) -> i32;
type Delegate = unsafe extern "C" fn(*mut c_void, i32, *mut *mut c_void) -> i32;
type Close = unsafe extern "C" fn(*mut c_void) -> i32;
type Load = unsafe extern "system" fn(
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    *mut c_void,
    *mut *mut c_void,
) -> i32;
type Entry = unsafe extern "system" fn(*mut c_void, i32) -> i32;

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn log(message: &str) {
    use std::io::Write;
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(local).join("SubspaceToolkit");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("live-bootstrap-{}.log", std::process::id())))
        {
            let _ = writeln!(file, "native: {message}");
        }
    }
}
unsafe fn symbol(module: HMODULE, name: &[u8]) -> Result<*mut c_void, String> {
    unsafe { GetProcAddress(module, name.as_ptr()) }
        .map(|p| p as *mut c_void)
        .ok_or_else(|| format!("Missing export {}", String::from_utf8_lossy(name)))
}
fn start() -> Result<(), String> {
    unsafe {
        let mut module = std::ptr::null_mut();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            Start as *const () as *const u16,
            &mut module,
        ) == 0
        {
            return Err("Cannot locate live host".into());
        }
        let mut path = vec![0u16; 32768];
        let length = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
        if length == 0 || length >= path.len() {
            return Err("Invalid live host path".into());
        }
        let folder = PathBuf::from(String::from_utf16_lossy(&path[..length]))
            .parent()
            .ok_or("Missing host folder")?
            .to_owned();
        let mut fxr = GetModuleHandleW(wide("hostfxr.dll").as_ptr());
        if fxr.is_null() {
            fxr = LoadLibraryExW(
                wide(folder.join("runtime/host/fxr/6.0.36/hostfxr.dll")).as_ptr(),
                std::ptr::null_mut(),
                LOAD_WITH_ALTERED_SEARCH_PATH,
            );
        }
        if fxr.is_null() {
            return Err(format!(
                "Cannot load private hostfxr: {}",
                std::io::Error::last_os_error()
            ));
        }
        let init: Init =
            std::mem::transmute(symbol(fxr, b"hostfxr_initialize_for_runtime_config\0")?);
        let delegate: Delegate =
            std::mem::transmute(symbol(fxr, b"hostfxr_get_runtime_delegate\0")?);
        let close: Close = std::mem::transmute(symbol(fxr, b"hostfxr_close\0")?);
        let mut context = std::ptr::null_mut();
        let result = init(
            wide(folder.join("SubspaceLive.runtimeconfig.json")).as_ptr(),
            std::ptr::null(),
            &mut context,
        );
        // Positive hostfxr results mean an already-loaded compatible CLR.
        if result < 0 {
            return Err(format!("CLR initialization failed: {result:#x}"));
        }
        let mut load_pointer = std::ptr::null_mut();
        let result = delegate(context, 5, &mut load_pointer);
        close(context);
        if result < 0 || load_pointer.is_null() {
            return Err(format!("CLR delegate unavailable: {result:#x}"));
        }
        let load: Load = std::mem::transmute(load_pointer);
        let mut entry = std::ptr::null_mut();
        let result = load(
            wide(folder.join("SubspaceLive.dll")).as_ptr(),
            wide("SubspaceToolkit.Live.Bootstrap, SubspaceLive").as_ptr(),
            wide("NativeEntry").as_ptr(),
            -1isize as *const u16,
            std::ptr::null_mut(),
            &mut entry,
        );
        if result < 0 || entry.is_null() {
            return Err(format!("Managed entry unavailable: {result:#x}"));
        }
        let entry: Entry = std::mem::transmute(entry);
        let result = entry(std::ptr::null_mut(), 0);
        if result != 0 {
            return Err(format!("Managed bootstrap failed: {result}"));
        }
        Ok(())
    }
}

/// Called explicitly after LoadLibraryW returns; never initialize CLR under loader lock.
#[unsafe(no_mangle)]
pub extern "system" fn Start(_: *mut c_void) -> u32 {
    match STATE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire) {
        Err(2) => return 0,
        Err(_) => return 170,
        Ok(_) => {}
    }
    let result =
        std::panic::catch_unwind(start).unwrap_or_else(|_| Err("Native bootstrap panicked".into()));
    match result {
        Ok(()) => {
            STATE.store(2, Ordering::Release);
            0
        }
        Err(error) => {
            log(&error);
            STATE.store(0, Ordering::Release);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_utf16_terminated_and_preserve_non_ascii() {
        let path = "C:\\Spiele\\Überblick";
        let encoded = wide(path);
        assert_eq!(encoded.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&encoded[..encoded.len() - 1]).unwrap(),
            path
        );
    }
}
