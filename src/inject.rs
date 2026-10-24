use crate::game::GameError;
use std::{
    ffi::c_void,
    fs::{self, File},
    io::Read,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::FILE_SHARE_READ,
    System::{
        Diagnostics::{
            Debug::{ReadProcessMemory, WriteProcessMemory},
            ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                TH32CS_SNAPMODULE,
            },
        },
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress,
        },
        Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
        },
        SystemInformation::IMAGE_FILE_MACHINE_AMD64,
        Threading::{
            CreateRemoteThread, GetCurrentProcess, GetExitCodeProcess, GetExitCodeThread,
            IsWow64Process2, OpenProcess, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION,
            PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE, QueryFullProcessImageNameW,
            WaitForSingleObject,
        },
    },
};

const HOST: &str = "SubspaceLiveHost.dll";
const GAME: &str = "StarTrekOutpostsUnknown.exe";
static PENDING: Mutex<Option<RemoteCall>> = Mutex::new(None);

fn error(detail: impl ToString) -> GameError {
    GameError::detail("error.live_attach", detail)
}
fn win_error() -> GameError {
    error(std::io::Error::last_os_error())
}
fn owned(raw: HANDLE) -> Result<OwnedHandle, GameError> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err(win_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn text(wide: &[u16]) -> String {
    String::from_utf16_lossy(&wide[..wide.iter().position(|&c| c == 0).unwrap_or(wide.len())])
}
fn same_path(a: &Path, b: &Path) -> Result<bool, GameError> {
    Ok(fs::canonicalize(a)
        .map_err(error)?
        .to_string_lossy()
        .eq_ignore_ascii_case(&fs::canonicalize(b).map_err(error)?.to_string_lossy()))
}

struct Module {
    base: usize,
    size: usize,
    path: PathBuf,
}
fn modules(pid: u32) -> Result<Vec<Module>, GameError> {
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) })?;
    let mut item = MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Module32FirstW(snapshot.as_raw_handle(), &mut item) } == 0 {
        return Err(win_error());
    }
    let mut result = Vec::new();
    loop {
        result.push(Module {
            base: item.modBaseAddr as usize,
            size: item.modBaseSize as usize,
            path: PathBuf::from(text(&item.szExePath)),
        });
        if unsafe { Module32NextW(snapshot.as_raw_handle(), &mut item) } == 0 {
            break;
        }
    }
    Ok(result)
}
fn read_memory(process: HANDLE, address: usize, length: usize) -> Result<Vec<u8>, GameError> {
    let mut bytes = vec![0u8; length];
    let mut read = 0;
    if unsafe {
        ReadProcessMemory(
            process,
            address as *const c_void,
            bytes.as_mut_ptr().cast(),
            length,
            &mut read,
        )
    } == 0
        || read != length
    {
        return Err(win_error());
    }
    Ok(bytes)
}
fn live(process: HANDLE) -> Result<(), GameError> {
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process, &mut code) } == 0 || code != 259 {
        return Err("error.game_access_lost".into());
    }
    Ok(())
}

struct RemoteCall {
    process: OwnedHandle,
    thread: OwnedHandle,
    allocation: usize,
}
impl RemoteCall {
    fn wait(&self, milliseconds: u32) -> Result<Option<u32>, GameError> {
        match unsafe { WaitForSingleObject(self.thread.as_raw_handle(), milliseconds) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeThread(self.thread.as_raw_handle(), &mut code) } == 0 {
                    return Err(win_error());
                }
                Ok(Some(code))
            }
            _ => Err(win_error()),
        }
    }
}
impl Drop for RemoteCall {
    fn drop(&mut self) {
        // A pending loader can still be reading its path; never free that memory early.
        if self.allocation != 0
            && unsafe { WaitForSingleObject(self.thread.as_raw_handle(), 0) } == WAIT_OBJECT_0
        {
            unsafe {
                VirtualFreeEx(
                    self.process.as_raw_handle(),
                    self.allocation as *mut c_void,
                    0,
                    MEM_RELEASE,
                );
            }
        }
    }
}
fn call(process: &OwnedHandle, address: usize, allocation: usize) -> Result<u32, GameError> {
    let copied =
        match live(process.as_raw_handle()).and_then(|()| process.try_clone().map_err(error)) {
            Ok(handle) => handle,
            Err(e) => {
                if allocation != 0 {
                    unsafe {
                        VirtualFreeEx(
                            process.as_raw_handle(),
                            allocation as *mut c_void,
                            0,
                            MEM_RELEASE,
                        );
                    }
                }
                return Err(e);
            }
        };
    let start = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(*mut c_void) -> u32>(address)
    };
    let thread = match owned(unsafe {
        CreateRemoteThread(
            process.as_raw_handle(),
            std::ptr::null(),
            0,
            Some(start),
            allocation as *const c_void,
            0,
            std::ptr::null_mut(),
        )
    }) {
        Ok(thread) => thread,
        Err(e) => {
            if allocation != 0 {
                unsafe {
                    VirtualFreeEx(
                        process.as_raw_handle(),
                        allocation as *mut c_void,
                        0,
                        MEM_RELEASE,
                    );
                }
            }
            return Err(e);
        }
    };
    let call = RemoteCall {
        process: copied,
        thread,
        allocation,
    };
    match call.wait(10_000) {
        Ok(Some(code)) => Ok(code),
        outcome => {
            *PENDING
                .lock()
                .map_err(|_| error("Attachment lock poisoned"))? = Some(call);
            outcome?;
            Err("error.live_pending".into())
        }
    }
}

fn load_library_address(process: HANDLE, modules: &[Module]) -> Result<usize, GameError> {
    let kernel = unsafe { GetModuleHandleW(wide(Path::new("kernel32.dll")).as_ptr()) };
    let address = unsafe { GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()) }
        .ok_or_else(win_error)? as usize;
    let mut owner = std::ptr::null_mut();
    if unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut owner,
        )
    } == 0
    {
        return Err(win_error());
    }
    let mut name = vec![0u16; 32768];
    let length =
        unsafe { GetModuleFileNameW(owner, name.as_mut_ptr(), name.len() as u32) } as usize;
    if length == 0 || length >= name.len() {
        return Err(win_error());
    }
    let path = PathBuf::from(String::from_utf16_lossy(&name[..length]));
    let mut matches = Vec::new();
    for module in modules {
        if module.path.file_name().is_some_and(|n| {
            path.file_name().is_some_and(|other| {
                n.to_string_lossy()
                    .eq_ignore_ascii_case(&other.to_string_lossy())
            })
        }) && same_path(&module.path, &path)?
        {
            matches.push(module);
        }
    }
    let [target] = matches.as_slice() else {
        return Err(error(
            "LoadLibraryW owning system module is not uniquely loaded",
        ));
    };
    let offset = address
        .checked_sub(owner as usize)
        .filter(|&n| n + 16 <= target.size)
        .ok_or_else(|| error("LoadLibraryW outside system image"))?;
    let local = unsafe { GetCurrentProcess() };
    if identity(&read_memory(local, owner as usize, 4096)?)?
        != identity(&read_memory(process, target.base, 4096)?)?
        || read_memory(local, address, 16)? != read_memory(process, target.base + offset, 16)?
    {
        return Err(error(
            "System loader image differs between trainer and game",
        ));
    }
    Ok(target.base + offset)
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, GameError> {
    Ok(u16::from_le_bytes(
        data.get(
            offset
                ..offset
                    .checked_add(2)
                    .ok_or_else(|| error("PE offset overflow"))?,
        )
        .ok_or_else(|| error("Truncated PE"))?
        .try_into()
        .unwrap(),
    ))
}
fn u32_at(data: &[u8], offset: usize) -> Result<u32, GameError> {
    Ok(u32::from_le_bytes(
        data.get(
            offset
                ..offset
                    .checked_add(4)
                    .ok_or_else(|| error("PE offset overflow"))?,
        )
        .ok_or_else(|| error("Truncated PE"))?
        .try_into()
        .unwrap(),
    ))
}
fn pe_offset(data: &[u8]) -> Result<usize, GameError> {
    let pe = u32_at(data, 0x3c)? as usize;
    if data.get(..2) != Some(b"MZ")
        || pe > 65536
        || data.get(pe..pe + 4) != Some(b"PE\0\0")
        || u16_at(data, pe + 4)? != IMAGE_FILE_MACHINE_AMD64
        || u16_at(data, pe + 24)? != 0x20b
    {
        return Err(error("Expected x64 PE image"));
    }
    Ok(pe)
}
fn identity(data: &[u8]) -> Result<(u32, u32), GameError> {
    let pe = pe_offset(data)?;
    Ok((u32_at(data, pe + 8)?, u32_at(data, pe + 24 + 56)?))
}
fn start_export(data: &[u8]) -> Result<usize, GameError> {
    let pe = pe_offset(data)?;
    if u16_at(data, pe + 22)? & 0x2000 == 0 {
        return Err(error("Live host is not a DLL"));
    }
    let sections = u16_at(data, pe + 6)? as usize;
    let optional_size = u16_at(data, pe + 20)? as usize;
    if sections == 0 || sections > 96 || optional_size < 120 || u32_at(data, pe + 24 + 108)? == 0 {
        return Err(error("Invalid PE directories"));
    }
    let section_table = pe + 24 + optional_size;
    let header_size = u32_at(data, pe + 24 + 60)? as usize;
    let locate = |rva: u32, length: usize| -> Result<usize, GameError> {
        let rva = rva as usize;
        let end = rva
            .checked_add(length)
            .ok_or_else(|| error("PE range overflow"))?;
        if end <= header_size && end <= data.len() {
            return Ok(rva);
        }
        for index in 0..sections {
            let section = section_table + index * 40;
            let base = u32_at(data, section + 12)? as usize;
            let size = u32_at(data, section + 16)? as usize;
            if rva >= base && end <= base + size {
                let offset = u32_at(data, section + 20)? as usize + rva - base;
                if offset.checked_add(length).is_some_and(|n| n <= data.len()) {
                    return Ok(offset);
                }
            }
        }
        Err(error("PE export outside file"))
    };
    let export_rva = u32_at(data, pe + 24 + 112)?;
    let export_size = u32_at(data, pe + 24 + 116)?;
    if export_rva == 0 || export_size < 40 {
        return Err(error("Missing export directory"));
    }
    let exports = locate(export_rva, 40)?;
    let functions = u32_at(data, exports + 20)? as usize;
    let names = u32_at(data, exports + 24)? as usize;
    if names == 0 || names > 65536 || functions > 65536 {
        return Err(error("Invalid export count"));
    }
    let addresses = locate(u32_at(data, exports + 28)?, functions * 4)?;
    let name_table = locate(u32_at(data, exports + 32)?, names * 4)?;
    let ordinals = locate(u32_at(data, exports + 36)?, names * 2)?;
    for index in 0..names {
        let name = locate(u32_at(data, name_table + index * 4)?, 6)?;
        if data.get(name..name + 6) != Some(b"Start\0") {
            continue;
        }
        let ordinal = u16_at(data, ordinals + index * 2)? as usize;
        if ordinal >= functions {
            return Err(error("Invalid export ordinal"));
        }
        let rva = u32_at(data, addresses + ordinal * 4)? as usize;
        if rva >= export_rva as usize && rva < export_rva as usize + export_size as usize {
            return Err(error("Forwarded Start export rejected"));
        }
        for index in 0..sections {
            let section = section_table + index * 40;
            let base = u32_at(data, section + 12)? as usize;
            let size = u32_at(data, section + 8)? as usize;
            if rva >= base && rva < base + size && u32_at(data, section + 36)? & 0x20000000 != 0 {
                locate(rva as u32, 1)?;
                return Ok(rva);
            }
        }
        return Err(error("Start export is not executable"));
    }
    Err(error("Start export missing"))
}

fn prepare_wrappers(live_dir: &Path, game_dir: &Path, pid: u32) -> Result<(), GameError> {
    use std::os::windows::process::CommandExt;
    let log_dir = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| error("LOCALAPPDATA is missing"))?,
    )
    .join("SubspaceToolkit");
    fs::create_dir_all(&log_dir).map_err(error)?;
    let log_path = log_dir.join(format!("live-generator-{pid}.log"));
    let log = File::create(&log_path).map_err(error)?;
    let mut child = Command::new(live_dir.join("runtime/dotnet.exe"))
        .arg(live_dir.join("Generator/SubspaceInterop.dll"))
        .arg(game_dir)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(error)?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| GameError::detail("error.live_payload", e))?;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let Some(status) = child.try_wait().map_err(error)? {
            return if status.success() {
                Ok(())
            } else {
                Err(GameError::detail(
                    "error.live_payload",
                    format!(
                        "Wrapper preparation failed ({status}); {}",
                        log_path.display()
                    ),
                ))
            };
        }
        if Instant::now() >= deadline {
            child.kill().map_err(error)?;
            let _ = child.wait();
            return Err(GameError::detail(
                "error.live_payload",
                format!("Wrapper preparation timed out; {}", log_path.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn ensure_attached(pid: u32, game_dir: &Path) -> Result<(), GameError> {
    {
        let mut pending = PENDING
            .lock()
            .map_err(|_| error("Attachment lock poisoned"))?;
        if let Some(call) = pending.as_ref() {
            if call.wait(0)?.is_none() {
                return Err("error.live_pending".into());
            }
            pending.take();
        }
    }
    let path = fs::canonicalize(
        std::env::current_exe()
            .map_err(error)?
            .parent()
            .ok_or_else(|| error("No application directory"))?
            .join("live")
            .join(HOST),
    )
    .map_err(|e| GameError::detail("error.live_payload", e))?;
    let mut file = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(error)?;
    if file.metadata().map_err(error)?.len() > 16 * 1024 * 1024 {
        return Err(error("Live host exceeds size limit"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(error)?;
    let start = start_export(&bytes)?;
    let process = owned(unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION
                | PROCESS_CREATE_THREAD
                | PROCESS_VM_OPERATION
                | PROCESS_VM_READ
                | PROCESS_VM_WRITE,
            0,
            pid,
        )
    })?;
    let raw = process.as_raw_handle();
    let mut process_machine = 0;
    let mut native_machine = 0;
    if unsafe { IsWow64Process2(raw, &mut process_machine, &mut native_machine) } == 0 {
        return Err(win_error());
    }
    if process_machine != 0 || native_machine != IMAGE_FILE_MACHINE_AMD64 {
        return Err(error("Game must be native x64"));
    }
    let mut executable = vec![0u16; 32768];
    let mut length = executable.len() as u32;
    if unsafe { QueryFullProcessImageNameW(raw, 0, executable.as_mut_ptr(), &mut length) } == 0 {
        return Err(win_error());
    }
    if !same_path(
        &PathBuf::from(String::from_utf16_lossy(&executable[..length as usize])),
        &game_dir.join(GAME),
    )? {
        return Err(error("PID does not belong to selected game"));
    }
    live(raw)?;
    let find_host = |items: Vec<Module>| -> Result<Option<Module>, GameError> {
        let hosts: Vec<_> = items
            .into_iter()
            .filter(|m| {
                m.path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(HOST))
            })
            .collect();
        if hosts.len() > 1 {
            return Err(error("Multiple live hosts loaded"));
        }
        if let Some(host) = hosts.into_iter().next() {
            if !same_path(&host.path, &path)?
                || identity(&read_memory(raw, host.base, 4096)?)? != identity(&bytes)?
            {
                return Err(error("Different live host is already loaded; restart game"));
            }
            return Ok(Some(host));
        }
        Ok(None)
    };
    let items = modules(pid)?;
    if find_host(modules(pid)?)?.is_none() {
        prepare_wrappers(
            path.parent().ok_or_else(|| error("Host has no parent"))?,
            game_dir,
            pid,
        )?;
        live(raw)?;
        let loader = load_library_address(raw, &items)?;
        let encoded = wide(&path);
        let size = encoded.len() * size_of::<u16>();
        let allocation = unsafe {
            VirtualAllocEx(
                raw,
                std::ptr::null(),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if allocation.is_null() {
            return Err(win_error());
        }
        let mut written = 0;
        if unsafe {
            WriteProcessMemory(raw, allocation, encoded.as_ptr().cast(), size, &mut written)
        } == 0
            || written != size
        {
            let e = win_error();
            unsafe {
                VirtualFreeEx(raw, allocation, 0, MEM_RELEASE);
            }
            return Err(e);
        }
        call(&process, loader, allocation as usize)?;
    }
    let host =
        find_host(modules(pid)?)?.ok_or_else(|| error("LoadLibraryW did not load live host"))?;
    if start >= host.size {
        return Err(error("Start export outside loaded host"));
    }
    let code = call(&process, host.base + start, 0)?;
    if code == 170 {
        return Err("error.live_pending".into());
    }
    if code != 0 {
        return Err(error(format!(
            "Live bootstrap returned {code:#x}; inspect live-bootstrap-{pid}.log"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn host_fixture() -> Vec<u8> {
        let mut bytes = vec![0; 0x800];
        bytes[..2].copy_from_slice(b"MZ");
        put32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        put16(&mut bytes, 0x84, IMAGE_FILE_MACHINE_AMD64);
        put16(&mut bytes, 0x86, 1);
        put32(&mut bytes, 0x88, 0x12345);
        put16(&mut bytes, 0x94, 240);
        put16(&mut bytes, 0x96, 0x2000);
        put16(&mut bytes, 0x98, 0x20b);
        put32(&mut bytes, 0x98 + 56, 0x2000);
        put32(&mut bytes, 0x98 + 60, 0x200);
        put32(&mut bytes, 0x98 + 108, 16);
        put32(&mut bytes, 0x98 + 112, 0x1000);
        put32(&mut bytes, 0x98 + 116, 0x100);
        put32(&mut bytes, 0x188 + 8, 0x600);
        put32(&mut bytes, 0x188 + 12, 0x1000);
        put32(&mut bytes, 0x188 + 16, 0x600);
        put32(&mut bytes, 0x188 + 20, 0x200);
        put32(&mut bytes, 0x188 + 36, 0x60000020);
        put32(&mut bytes, 0x200 + 20, 1);
        put32(&mut bytes, 0x200 + 24, 1);
        put32(&mut bytes, 0x200 + 28, 0x1040);
        put32(&mut bytes, 0x200 + 32, 0x1044);
        put32(&mut bytes, 0x200 + 36, 0x1048);
        put32(&mut bytes, 0x240, 0x1200);
        put32(&mut bytes, 0x244, 0x1050);
        bytes[0x250..0x256].copy_from_slice(b"Start\0");
        bytes[0x400] = 0xc3;
        bytes
    }

    #[test]
    fn resolves_only_direct_executable_x64_start_and_rejects_malformed_images() {
        let bytes = host_fixture();
        assert_eq!(start_export(&bytes).unwrap(), 0x1200);
        assert_eq!(identity(&bytes).unwrap(), (0x12345, 0x2000));
        for length in 0..0x401 {
            assert!(
                start_export(&bytes[..length]).is_err(),
                "truncated at {length}"
            );
        }
        for (offset, value) in [
            (0x84, 0x14c),
            (0x98, 0x10b),
            (0x96, 0),
            (0x3c, u32::MAX),
            (0x98 + 112, 0),
            (0x218, 65537),
            (0x248, 1),
            (0x240, 0x1060),
            (0x240, 0x7000),
            (0x188 + 36, 0x40000040),
            (0x244, u32::MAX),
            (0x188 + 20, u32::MAX),
        ] {
            let mut malformed = bytes.clone();
            put32(&mut malformed, offset, value);
            assert!(
                start_export(&malformed).is_err(),
                "offset {offset:#x}, value {value:#x}"
            );
        }
    }
}
