use sha2::{Digest, Sha256};
#[cfg(not(all(windows, target_arch = "x86_64")))]
compile_error!("Subspace Toolkit requires 64-bit Windows.");
use std::{fs::File, io::Read, mem::size_of, path::Path};

#[derive(Debug)]
pub struct GameError {
    pub key: &'static str,
    pub detail: Option<String>,
}
impl GameError {
    pub(crate) fn detail(key: &'static str, detail: impl ToString) -> Self {
        Self {
            key,
            detail: Some(detail.to_string()),
        }
    }
}
impl From<&'static str> for GameError {
    fn from(key: &'static str) -> Self {
        Self { key, detail: None }
    }
}
impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::{ReadProcessMemory, WriteProcessMemory},
            ToolHelp::*,
        },
        Threading::{
            CreateMutexW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION,
            PROCESS_VM_READ, PROCESS_VM_WRITE,
        },
    },
};

const GAME: &str = "StarTrekOutpostsUnknown.exe";
const ASSEMBLY_HASH: &str = "3f3cc9b0ffa2921dbe9e3c770aa6d79c9929522d57550f01a46112c46b6314fb";
const METADATA_HASH: &str = "43812b70d4033f7cfd1366ec68549e1c84b8fc0b28c22c0f02859363219f1300";
// Filled only from this build's independently inspected get_Instance implementation.
const CAMERA_CLASS_RVA: usize = 0x88589b0;
const STATIC_FIELDS: usize = 0xb8;
const GET_INSTANCE_RVA: usize = 0x43801b0;
const GET_INSTANCE_BYTES: [u8; 59] = [
    0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 0xdf, 0xc7, 0x8c, 0x04, 0x00, 0x75, 0x18, 0x48, 0x8d, 0x0d,
    0xec, 0x87, 0x4d, 0x04, 0xe8, 0x17, 0x96, 0x63, 0xfc, 0xf0, 0x83, 0x0c, 0x24, 0x00, 0xc6, 0x05,
    0xc5, 0xc7, 0x8c, 0x04, 0x01, 0x48, 0x8b, 0x05, 0xd4, 0x87, 0x4d, 0x04, 0x48, 0x8b, 0x80, 0xb8,
    0x00, 0x00, 0x00, 0x48, 0x8b, 0x00, 0x48, 0x83, 0xc4, 0x28, 0xc3,
];
const MINIMUM: usize = 0xb8;
const MAXIMUM: usize = 0xbc;
const CURRENT: usize = 0x1a4;

pub(crate) struct Handle(HANDLE);
// Windows process handles can be transferred between threads; ownership stays singular.
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Handle {
    pub(crate) fn new(handle: HANDLE) -> Result<Self, GameError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(GameError::detail(
                "error.system_access",
                std::io::Error::last_os_error(),
            ))
        } else {
            Ok(Self(handle))
        }
    }
    fn bytes<const N: usize>(&self, address: usize) -> Result<[u8; N], GameError> {
        let mut value = [0; N];
        let mut count = 0;
        if unsafe {
            ReadProcessMemory(
                self.0,
                address as _,
                value.as_mut_ptr().cast(),
                N,
                &mut count,
            )
        } == 0
            || count != N
        {
            return Err("error.game_access_lost".into());
        }
        Ok(value)
    }
    fn pointer(&self, address: usize) -> Result<usize, GameError> {
        let value = usize::from_le_bytes(self.bytes(address)?);
        if value != 0 && (!(0x10000..0x0000_8000_0000_0000).contains(&value) || value % 8 != 0) {
            return Err("error.invalid_pointer".into());
        }
        Ok(value)
    }
    fn float(&self, address: usize) -> Result<f32, GameError> {
        Ok(f32::from_le_bytes(self.bytes(address)?))
    }
    fn write_float(&self, address: usize, value: f32) -> Result<(), GameError> {
        if !value.is_finite() || !address.is_multiple_of(4) {
            return Err("error.invalid_camera_value".into());
        }
        let bytes = value.to_le_bytes();
        let mut written = 0;
        if unsafe {
            WriteProcessMemory(
                self.0,
                address as _,
                bytes.as_ptr().cast(),
                bytes.len(),
                &mut written,
            )
        } == 0
            || written != bytes.len()
        {
            return Err("error.write_failed".into());
        }
        if self.float(address)? != value {
            return Err("error.write_unconfirmed".into());
        }
        Ok(())
    }
}

fn wide_text(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.iter().position(|&c| c == 0).unwrap_or(text.len())])
}

pub(crate) fn verified_hash(path: &Path, expected: &str) -> Result<(), GameError> {
    let mut file = File::open(path)
        .map_err(|e| GameError::detail("error.file_read", format!("{}: {e}", path.display())))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|e| GameError::detail("error.file_read", e))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let actual: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(GameError::detail(
            "error.unsupported_build",
            path.file_name().unwrap_or_default().to_string_lossy(),
        ));
    }
    Ok(())
}

fn game_processes() -> Result<Vec<u32>, GameError> {
    let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = Vec::new();
    let mut valid = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while valid {
        if wide_text(&entry.szExeFile).eq_ignore_ascii_case(GAME) {
            found.push(entry.th32ProcessID);
        }
        valid = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Ok(found)
}

pub(crate) fn ensure_game_closed() -> Result<(), GameError> {
    if game_processes()?.is_empty() {
        Ok(())
    } else {
        Err("error.game_must_close".into())
    }
}

fn find_game() -> Result<(u32, usize, std::path::PathBuf), GameError> {
    let found = game_processes()?;
    let pid = match found.as_slice() {
        [] => return Err("error.game_not_found".into()),
        [pid] => *pid,
        _ => return Err("error.multiple_games".into()),
    };
    let modules = Handle::new(unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
    })?;
    let mut module = MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    let mut valid = unsafe { Module32FirstW(modules.0, &mut module) } != 0;
    while valid {
        if wide_text(&module.szModule).eq_ignore_ascii_case("GameAssembly.dll") {
            return Ok((
                pid,
                module.modBaseAddr as usize,
                wide_text(&module.szExePath).into(),
            ));
        }
        valid = unsafe { Module32NextW(modules.0, &mut module) } != 0;
    }
    Err("error.game_loading".into())
}

pub(crate) fn verify_installation(dir: &Path) -> Result<(), GameError> {
    verified_hash(&dir.join("GameAssembly.dll"), ASSEMBLY_HASH)?;
    verified_hash(
        &dir.join("StarTrekOutpostsUnknown_Data/il2cpp_data/Metadata/global-metadata.dat"),
        METADATA_HASH,
    )?;
    verified_hash(
        &dir.join("StarTrekOutpostsUnknown_Data/StreamingAssets/aa/catalog.bin"),
        "5d425a60dc121eb5a4e41d7962ed4d639cc498e78fb3107f874047c319c111ec",
    )
}

#[derive(Clone, Copy, Debug)]
pub struct CameraState {
    pub original: f32,
    pub maximum: f32,
    pub current: f32,
}

#[derive(Clone, Copy)]
struct Camera {
    address: usize,
    native: usize,
    original: f32,
    last_written: f32,
}

pub struct GameSession {
    pub game_dir: std::path::PathBuf,
    process: Handle,
    _connection_lock: Handle,
    class_slot: usize,
    camera: Option<Camera>,
}

pub const MAX_LIMIT: f32 = 1000.0;

fn validated_limit(original: f32, limit: f32) -> Result<f32, GameError> {
    if !original.is_finite()
        || original <= 0.0
        || !limit.is_finite()
        || !(original..=MAX_LIMIT).contains(&limit)
    {
        return Err("error.invalid_limit".into());
    }
    Ok(limit)
}

impl GameSession {
    pub fn connect() -> Result<Self, GameError> {
        let (pid, base, path) = find_game()?;
        let mutex_name: Vec<u16> = format!("Local\\SubspaceToolkit-{pid}\0")
            .encode_utf16()
            .collect();
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        let already_connected = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let connection_lock = Handle::new(mutex)?;
        if already_connected {
            return Err("error.already_connected".into());
        }
        verified_hash(&path, ASSEMBLY_HASH)?;
        verified_hash(
            &path
                .parent()
                .ok_or("error.invalid_game_path")?
                .join("StarTrekOutpostsUnknown_Data/il2cpp_data/Metadata/global-metadata.dat"),
            METADATA_HASH,
        )?;
        let process = Handle::new(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE
                    | PROCESS_VM_OPERATION,
                0,
                pid,
            )
        })?;
        if process.bytes::<59>(base + GET_INSTANCE_RVA)? != GET_INSTANCE_BYTES {
            return Err("error.loaded_code_mismatch".into());
        }
        let mut session = Self {
            game_dir: path
                .parent()
                .ok_or("error.invalid_game_path")?
                .to_path_buf(),
            process,
            _connection_lock: connection_lock,
            class_slot: base + CAMERA_CLASS_RVA,
            camera: None,
        };
        session.poll()?;
        Ok(session)
    }

    fn active_address(&self) -> Result<(usize, usize), GameError> {
        let class = self.process.pointer(self.class_slot)?;
        if class == 0 {
            return Ok((0, 0));
        }
        let fields = self.process.pointer(class + STATIC_FIELDS)?;
        if fields == 0 {
            return Ok((class, 0));
        }
        let object = self.process.pointer(fields)?;
        if object != 0 && self.process.pointer(object)? != class {
            return Err("error.camera_type_mismatch".into());
        }
        Ok((class, object))
    }

    pub fn poll(&mut self) -> Result<Option<CameraState>, GameError> {
        let (_, address) = self.active_address()?;
        if address == 0 {
            return Ok(None);
        }
        let native = self.process.pointer(address + 0x10)?;
        if native == 0 {
            return Ok(None);
        }
        if self
            .camera
            .is_some_and(|c| c.address != address || c.native != native)
        {
            // Scene changes require a fresh connection; never write into a retired Unity object.
            return Err("error.camera_changed".into());
        }
        let minimum = self.process.float(address + MINIMUM)?;
        let maximum = self.process.float(address + MAXIMUM)?;
        let current = self.process.float(address + CURRENT)?;
        if !minimum.is_finite()
            || !maximum.is_finite()
            || !current.is_finite()
            || minimum < 0.0
            || maximum <= minimum
            || maximum > 1000000.0
            || current < 0.0
            || current > 1000000.0
        {
            return Err("error.camera_bounds".into());
        }
        let camera = self.camera.get_or_insert(Camera {
            address,
            native,
            original: maximum,
            last_written: maximum,
        });
        if maximum != camera.last_written {
            return Err("error.external_change".into());
        }
        Ok(Some(CameraState {
            original: camera.original,
            maximum,
            current,
        }))
    }

    pub fn set_limit(&mut self, limit: f32) -> Result<(), GameError> {
        let state = self.poll()?.ok_or("error.camera_unavailable")?;
        let maximum = validated_limit(state.original, limit)?;
        let camera = self.camera.as_mut().ok_or("error.camera_unavailable")?;
        self.process
            .write_float(camera.address + MAXIMUM, maximum)?;
        camera.last_written = maximum;
        Ok(())
    }

    pub fn restore(&mut self) -> Result<(), GameError> {
        if let Some(camera) = self.camera {
            if camera.last_written == camera.original {
                return Ok(());
            }
            let (_, address) = self.active_address()?;
            if address != camera.address || self.process.pointer(address + 0x10)? != camera.native {
                return Err("error.restore_camera_gone".into());
            }
            let value = self.process.float(address + MAXIMUM)?;
            if value != camera.last_written {
                return Err("error.restore_external_change".into());
            }
            self.process
                .write_float(address + MAXIMUM, camera.original)?;
            if let Some(ref mut c) = self.camera {
                c.last_written = c.original;
            }
        }
        Ok(())
    }
}

impl Drop for GameSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        session: GameSession,
        _class: Box<[usize; 64]>,
        fields: Box<usize>,
        object: Box<[usize; 80]>,
        _slot: Box<usize>,
    }
    fn fixture() -> Fixture {
        use windows_sys::Win32::System::Threading::GetCurrentProcessId;
        let open = || {
            Handle::new(unsafe {
                OpenProcess(
                    PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION,
                    0,
                    GetCurrentProcessId(),
                )
            })
            .unwrap()
        };
        let mut class = Box::new([0usize; 64]);
        let mut object = Box::new([0usize; 80]);
        object[0] = class.as_mut_ptr() as usize;
        object[2] = 0x10000;
        let mut fields = Box::new(object.as_mut_ptr() as usize);
        class[STATIC_FIELDS / 8] = (&mut *fields) as *mut usize as usize;
        let mut slot = Box::new(class.as_mut_ptr() as usize);
        let session = GameSession {
            game_dir: Default::default(),
            process: open(),
            _connection_lock: open(),
            class_slot: (&mut *slot) as *mut usize as usize,
            camera: None,
        };
        for (offset, value) in [(MINIMUM, 15.0), (MAXIMUM, 300.0), (CURRENT, 300.0)] {
            session
                .process
                .write_float(*fields + offset, value)
                .unwrap();
        }
        Fixture {
            session,
            _class: class,
            fields,
            object,
            _slot: slot,
        }
    }
    #[test]
    fn camera_chain_apply_reset_and_foreign_change_guard() {
        let mut fixture = fixture();
        assert_eq!(fixture.session.poll().unwrap().unwrap().maximum, 300.0);
        fixture.session.set_limit(1000.0).unwrap();
        assert_eq!(fixture.session.poll().unwrap().unwrap().maximum, 1000.0);
        fixture.session.restore().unwrap();
        assert_eq!(fixture.session.poll().unwrap().unwrap().maximum, 300.0);
        fixture.session.set_limit(900.0).unwrap();
        fixture
            .session
            .process
            .write_float(*fixture.fields + MAXIMUM, 777.0)
            .unwrap();
        assert!(fixture.session.set_limit(1000.0).is_err());
        assert!(fixture.session.restore().is_err());
        assert_eq!(
            fixture
                .session
                .process
                .float(*fixture.fields + MAXIMUM)
                .unwrap(),
            777.0
        );
    }
    #[test]
    fn missing_destroyed_or_wrong_class_camera_never_written() {
        let mut fixture = fixture();
        let original_class = fixture.object[0];
        fixture.object[0] = 0x10000;
        assert!(fixture.session.set_limit(1000.0).is_err());
        fixture.object[0] = original_class;
        fixture.object[2] = 0;
        assert!(fixture.session.set_limit(1000.0).is_err());
        assert_eq!(
            fixture
                .session
                .process
                .float(*fixture.fields + MAXIMUM)
                .unwrap(),
            300.0
        );
        *fixture.fields = 0;
        assert!(fixture.session.poll().unwrap().is_none());
    }
    #[test]
    fn absolute_limit_boundaries_and_invalid_floats() {
        assert_eq!(validated_limit(150.0, 150.0).unwrap(), 150.0);
        assert_eq!(validated_limit(150.0, 1000.0).unwrap(), 1000.0);
        for factor in [0.0, 149.9, 1000.1, f32::NAN, f32::INFINITY] {
            assert!(validated_limit(150.0, factor).is_err());
        }
        for original in [-10.0, 0.0, f32::NAN, f32::INFINITY] {
            assert!(validated_limit(original, 300.0).is_err());
        }
    }
    #[test]
    fn reads_and_writes_exactly_one_float() {
        use windows_sys::Win32::System::Threading::GetCurrentProcessId;
        let process = Handle::new(unsafe {
            OpenProcess(
                PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION,
                0,
                GetCurrentProcessId(),
            )
        })
        .unwrap();
        let mut values = Box::new([17.0f32, 150.0, 29.0]);
        let address = (&mut values[1]) as *mut f32 as usize;
        assert_eq!(process.float(address).unwrap(), 150.0);
        process.write_float(address, 1500.0).unwrap();
        assert_eq!(*values, [17.0, 1500.0, 29.0]);
        process.write_float(address, 150.0).unwrap();
        assert_eq!(*values, [17.0, 150.0, 29.0]);
        assert!(process.write_float(address, f32::NAN).is_err());
        assert!(process.float(0).is_err());
    }
}
