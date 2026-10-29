use anyhow::{Result, bail, ensure};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::{
        io::{BufRead, Write},
        mem::{size_of, zeroed},
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::Cryptography::*,
        System::{
            Diagnostics::{Debug::ReadProcessMemory, ToolHelp::*},
            LibraryLoader::*,
            Memory::*,
            Threading::*,
        },
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn text(s: &[u16]) -> String {
        String::from_utf16_lossy(&s[..s.iter().position(|x| *x == 0).unwrap_or(s.len())])
    }
    pub struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                    CloseHandle(self.0);
                }
            }
        }
    }
    pub struct MappedFile {
        _mapping: Handle,
        _file: std::fs::File,
        pub len: usize,
    }
    impl MappedFile {
        pub fn open(path: &Path) -> Result<Self> {
            use std::os::windows::io::AsRawHandle;
            let file = std::fs::File::open(path)?;
            let len = file.metadata()?.len() as usize;
            ensure!(len > 0, "EMPTY_SOURCE");
            unsafe {
                let mapping = Handle(CreateFileMappingW(
                    file.as_raw_handle() as _,
                    std::ptr::null(),
                    PAGE_READONLY,
                    0,
                    0,
                    std::ptr::null(),
                ));
                ensure!(
                    !mapping.0.is_null(),
                    "FILE_MAPPING_FAILED: {}",
                    GetLastError()
                );
                Ok(Self {
                    _mapping: mapping,
                    _file: file,
                    len,
                })
            }
        }
        pub fn copy_into(&self, offset: usize, out: &mut [u8]) -> Result<()> {
            ensure!(offset + out.len() <= self.len, "FILE_MAPPING_BOUNDS");
            let aligned = offset / 65536 * 65536;
            let delta = offset - aligned;
            unsafe {
                let view = MapViewOfFile(
                    self._mapping.0,
                    FILE_MAP_READ,
                    (aligned as u64 >> 32) as u32,
                    aligned as u32,
                    delta + out.len(),
                );
                ensure!(
                    !view.Value.is_null(),
                    "FILE_MAPPING_FAILED: {}",
                    GetLastError()
                );
                std::ptr::copy_nonoverlapping(
                    (view.Value as *const u8).add(delta),
                    out.as_mut_ptr(),
                    out.len(),
                );
                UnmapViewOfFile(view);
            }
            Ok(())
        }
    }
    pub fn processes(name: &str) -> Vec<(u32, PathBuf)> {
        unsafe {
            let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0));
            if snap.0 == INVALID_HANDLE_VALUE {
                return vec![];
            }
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap.0, &mut entry);
            let mut out = vec![];
            while ok != 0 {
                if text(&entry.szExeFile).eq_ignore_ascii_case(name) {
                    let p = Handle(OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        entry.th32ProcessID,
                    ));
                    let mut buf = vec![0u16; 32768];
                    let mut len = buf.len() as u32;
                    let path = if !p.0.is_null()
                        && QueryFullProcessImageNameW(p.0, 0, buf.as_mut_ptr(), &mut len) != 0
                    {
                        PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
                    } else {
                        PathBuf::new()
                    };
                    out.push((entry.th32ProcessID, path));
                }
                ok = Process32NextW(snap.0, &mut entry);
            }
            out
        }
    }
    pub fn module_base(pid: u32, name: &str) -> Option<(usize, PathBuf)> {
        unsafe {
            let snap = Handle(CreateToolhelp32Snapshot(
                TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32,
                pid,
            ));
            if snap.0 == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: MODULEENTRY32W = zeroed();
            entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
            let mut ok = Module32FirstW(snap.0, &mut entry);
            while ok != 0 {
                if text(&entry.szModule).eq_ignore_ascii_case(name) {
                    return Some((
                        entry.modBaseAddr as usize,
                        PathBuf::from(text(&entry.szExePath)),
                    ));
                }
                ok = Module32NextW(snap.0, &mut entry);
            }
            None
        }
    }
    pub struct Reader(Handle);
    impl Reader {
        pub fn open(pid: u32) -> Result<Self> {
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
                ensure!(
                    !h.is_null(),
                    "PROCESS_OPEN_FAILED: 无法读取客户端进程，WinError {}",
                    GetLastError()
                );
                Ok(Self(Handle(h)))
            }
        }
        pub fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            unsafe {
                let mut out = vec![0; len];
                let mut n = 0;
                if ReadProcessMemory(self.0.0, address as _, out.as_mut_ptr() as _, len, &mut n)
                    == 0
                    || n != len
                {
                    return None;
                }
                Some(out)
            }
        }
        pub fn u32(&self, address: usize) -> Option<usize> {
            Some(u32::from_le_bytes(self.read(address, 4)?.try_into().ok()?) as usize)
        }
        pub fn cstr(&self, address: usize) -> Option<String> {
            let mut bytes = vec![];
            for n in 0..2048 {
                let b = *self.read(address + n, 1)?.first()?;
                if b == 0 {
                    if let Ok(value) = std::str::from_utf8(&bytes) {
                        return Some(value.to_owned());
                    }
                    use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
                    let length = unsafe {
                        MultiByteToWideChar(
                            CP_ACP,
                            0,
                            bytes.as_ptr(),
                            bytes.len() as i32,
                            std::ptr::null_mut(),
                            0,
                        )
                    };
                    if length <= 0 {
                        return None;
                    }
                    let mut wide = vec![0u16; length as usize];
                    unsafe {
                        MultiByteToWideChar(
                            CP_ACP,
                            0,
                            bytes.as_ptr(),
                            bytes.len() as i32,
                            wide.as_mut_ptr(),
                            length,
                        );
                    }
                    return Some(String::from_utf16_lossy(&wide));
                }
                bytes.push(b);
            }
            None
        }
        pub fn scan<F: FnMut(usize, &[u8]) -> bool>(&self, private_only: bool, mut visit: F) {
            unsafe {
                let mut address = 0usize;
                let mut mbi: MEMORY_BASIC_INFORMATION = zeroed();
                while VirtualQueryEx(
                    self.0.0,
                    address as _,
                    &mut mbi,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                ) != 0
                {
                    let base = mbi.BaseAddress as usize;
                    let size = mbi.RegionSize;
                    if mbi.State == MEM_COMMIT
                        && mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0
                        && (!private_only || mbi.Type == MEM_PRIVATE)
                    {
                        let mut offset = 0;
                        while offset < size {
                            let n = (4 * 1024 * 1024).min(size - offset);
                            if let Some(bytes) = self.read(base + offset, n) {
                                if visit(base + offset, &bytes) {
                                    return;
                                }
                            }
                            if n <= 512 {
                                break;
                            }
                            offset += n - 512;
                        }
                    }
                    let Some(next) = base.checked_add(size) else {
                        break;
                    };
                    if next <= address {
                        break;
                    }
                    address = next;
                }
            }
        }
    }
    pub fn recover_qq(source: &Path) -> Result<(Zeroizing<Vec<u8>>, u32)> {
        let expected_exe = "e20e6670f31472e4545af1b6115f1a5cc553bbd0da2894b8def41cc3280e2347";
        let expected_dll = "b39f79c6cf43a5665dabb587f31784c713fbfda55ef0ebdd4a51ac1d0a0ec4e9";
        let mut candidates = vec![];
        let mut supported = false;
        let procs = processes("QQ.exe");
        let running = !procs.is_empty();
        let mut inaccessible = false;
        for (pid, exe) in procs {
            let Some((base, dll)) = module_base(pid, "KernelUtil.dll") else {
                inaccessible = true;
                continue;
            };
            if exe.as_os_str().is_empty() {
                inaccessible = true;
                continue;
            }
            if crate::crypto::hash_file(&exe)? != expected_exe
                || crate::crypto::hash_file(&dll)? != expected_dll
            {
                continue;
            }
            supported = true;
            let reader = Reader::open(pid)?;
            let pattern = ((base + 0x15882c) as u32).to_le_bytes();
            reader.scan(true, |region, data| {
                for (i, window) in data.windows(4).enumerate() {
                    if window != pattern {
                        continue;
                    }
                    let object = region + i;
                    let inspect = || -> Option<(String, Zeroizing<Vec<u8>>)> {
                        if reader.u32(object + 8)? != base + 0x181dd8
                            || reader.u32(object + 0x10)? != 0x2bf20
                            || reader.u32(object + 0x14)? != 16
                            || reader.u32(object + 0x40)? != object
                        {
                            return None;
                        }
                        let db = reader.u32(object + 0xc)?;
                        let array = reader.u32(db + 0x14)?;
                        let tree = reader.u32(array + 4)?;
                        let shared = reader.u32(tree + 4)?;
                        let pager = reader.u32(shared)?;
                        if reader.u32(pager + 0xc8)? != base + 0x64107
                            || reader.u32(pager + 0xcc)? != base + 0x249d
                            || reader.u32(pager + 0xd0)? != base + 0x642cc
                        {
                            return None;
                        }
                        let file = reader.cstr(reader.u32(pager + 0xa8)?)?;
                        let codec = reader.u32(pager + 0xd4)?;
                        if reader.u32(codec + 4)? != 272 || reader.u32(codec + 0x10)? != 8192 {
                            return None;
                        }
                        let blob = Zeroizing::new(reader.read(reader.u32(codec)?, 272)?);
                        let key = Zeroizing::new(
                            (0..16)
                                .map(|n| blob[17 * n + (blob[17 * n + 16] & 15) as usize])
                                .collect(),
                        );
                        Some((file, key))
                    };
                    if let Some((file, key)) = inspect() {
                        if Path::new(&file)
                            .file_name()
                            .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("Msg3.0.db"))
                        {
                            candidates.push((file, key, pid));
                        }
                    }
                }
                false
            });
        }
        let normalized = |p: &Path| {
            p.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .replace('/', "\\")
                .to_lowercase()
        };
        let target = normalized(source);
        let mut exact: Vec<_> = candidates
            .into_iter()
            .filter(|(file, _, _)| normalized(Path::new(file)) == target)
            .collect();
        exact.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0) && a.1 == b.1);
        if exact.len() == 1 {
            let (_, key, pid) = exact.remove(0);
            return Ok((key, pid));
        }
        if exact.len() > 1 {
            bail!("ACCOUNT_AMBIGUOUS: 多个 QQ 密钥上下文匹配，请保留目标账号实例");
        }
        bail!(if supported {
            "KEY_NOT_FOUND: 未找到目标 Msg3.0.db 的密钥上下文"
        } else if inaccessible {
            "PROCESS_ACCESS_DENIED: QQ 正在运行，但当前环境无法读取进程，请在可访问该进程的终端运行"
        } else if running {
            "UNSUPPORTED_QQ_BUILD: 当前 QQ 版本与 9.7.25.29417 不匹配"
        } else {
            "QQ_NOT_RUNNING: 请运行并登录 Classic QQ 9.7.25.29417"
        })
    }
    pub fn recover_wechat(
        pages: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(BTreeMap<String, String>, Option<u32>)> {
        let re = regex::bytes::Regex::new(r"x'([0-9a-fA-F]{64,192})'")?;
        let mut keys = BTreeMap::new();
        let mut matched_pid = None;
        for (pid, _) in processes("Weixin.exe")
            .into_iter()
            .chain(processes("WeChat.exe"))
        {
            let Ok(reader) = Reader::open(pid) else {
                continue;
            };
            reader.scan(false, |_, bytes| {
                for caps in re.captures_iter(bytes) {
                    let raw = &caps[1];
                    if raw.len() < 64 {
                        continue;
                    }
                    let Ok(key) = hex::decode(&raw[..64]) else {
                        continue;
                    };
                    let key = Zeroizing::new(key);
                    for (salt, page) in pages {
                        if !keys.contains_key(salt) && crate::crypto::verify_wx_key(&key, page) {
                            keys.insert(salt.clone(), hex::encode(&*key));
                            matched_pid = Some(pid);
                        }
                    }
                }
                keys.len() == pages.len()
            });
            if keys.len() == pages.len() {
                break;
            }
        }
        ensure!(
            keys.len() == pages.len(),
            "KEY_NOT_FOUND: 部分微信数据库密钥未找到，请登录对应账号后刷新"
        );
        Ok((keys, matched_pid))
    }
    pub fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: bytes.len() as u32,
                pbData: bytes.as_ptr() as _,
            };
            let mut output: CRYPT_INTEGER_BLOB = zeroed();
            ensure!(
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output
                ) != 0,
                "VAULT_WRITE_FAILED"
            );
            let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData as _);
            Ok(result)
        }
    }
    pub fn unprotect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: bytes.len() as u32,
                pbData: bytes.as_ptr() as _,
            };
            let mut output: CRYPT_INTEGER_BLOB = zeroed();
            ensure!(
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output
                ) != 0,
                "VAULT_READ_FAILED: 密钥缓存无法由当前 Windows 用户解开"
            );
            let result = Zeroizing::new(
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec(),
            );
            LocalFree(output.pbData as _);
            Ok(result)
        }
    }
    type NtProcess = unsafe extern "system" fn(HANDLE) -> i32;
    unsafe fn nt(name: &[u8]) -> Result<NtProcess> {
        unsafe {
            let m = GetModuleHandleW(wide("ntdll.dll").as_ptr());
            let f = GetProcAddress(m, name.as_ptr());
            ensure!(f.is_some(), "PROCESS_CONTROL_FAILED");
            Ok(std::mem::transmute(f.unwrap()))
        }
    }
    fn creation(handle: HANDLE) -> Result<u64> {
        unsafe {
            let mut c = zeroed();
            let mut e = zeroed();
            let mut k = zeroed();
            let mut u = zeroed();
            ensure!(
                GetProcessTimes(handle, &mut c, &mut e, &mut k, &mut u) != 0,
                "PROCESS_QUERY_FAILED"
            );
            Ok(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
        }
    }
    pub fn identity(pid: u32) -> Option<crate::model::ClientIdentity> {
        let h = unsafe { Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)) };
        if h.0.is_null() {
            return None;
        }
        Some(crate::model::ClientIdentity {
            pid,
            created: creation(h.0).ok()?,
        })
    }
    pub fn client_alive(client: &crate::model::ClientIdentity) -> bool {
        identity(client.pid).is_some_and(|c| c.created == client.created)
    }
    pub fn watch_parent(pid: u32) -> Result<()> {
        let handle = unsafe { OpenProcess(0x00100000, 0, pid) }; // SYNCHRONIZE
        ensure!(!handle.is_null(), "PARENT_EXITED: 命令行进程已退出");
        let raw = handle as usize;
        std::thread::spawn(move || {
            let handle = Handle(raw as HANDLE);
            if unsafe { WaitForSingleObject(handle.0, INFINITE) } == WAIT_OBJECT_0 {
                std::process::exit(1);
            }
        });
        Ok(())
    }
    pub fn peak_memory_bytes() -> Option<usize> {
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        (unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } != 0)
            .then_some(counters.PeakWorkingSetSize)
    }
    pub struct Pause {
        child: std::process::Child,
        pub started: std::time::Instant,
    }
    impl Pause {
        pub fn acquire(pid: u32, millis: u64) -> Result<Self> {
            let started = std::time::Instant::now();
            let h = unsafe { Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)) };
            ensure!(!h.0.is_null(), "PROCESS_QUERY_FAILED");
            let time = creation(h.0)?;
            let mut child = Command::new(std::env::current_exe()?)
                .args([
                    "--pause-watchdog",
                    &pid.to_string(),
                    &time.to_string(),
                    &millis.min(5000).to_string(),
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(0x08000000)
                .spawn()?;
            let mut reply = String::new();
            let mut read = std::io::BufReader::new(child.stdout.take().unwrap());
            read.read_line(&mut reply)?;
            ensure!(
                reply.trim() == "PAUSED",
                "PROCESS_PAUSE_FAILED: 客户端暂停失败"
            );
            Ok(Self { child, started })
        }
    }
    impl Drop for Pause {
        fn drop(&mut self) {
            if let Some(mut input) = self.child.stdin.take() {
                let _ = input.write_all(b"resume\n");
            }
            let _ = self.child.wait();
        }
    }
    pub fn watchdog(args: &[String]) -> Result<()> {
        ensure!(args.len() == 3, "WATCHDOG_ARGUMENT");
        let pid = args[0].parse::<u32>()?;
        let expected = args[1].parse::<u64>()?;
        let millis = args[2].parse::<u64>()?.min(5000);
        unsafe {
            let h = Handle(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SUSPEND_RESUME,
                0,
                pid,
            ));
            ensure!(
                !h.0.is_null() && creation(h.0)? == expected,
                "PROCESS_CHANGED"
            );
            let resume = nt(b"NtResumeProcess\0")?;
            let started = std::time::Instant::now();
            ensure!(nt(b"NtSuspendProcess\0")?(h.0) >= 0, "PROCESS_PAUSE_FAILED");
            struct Restore(HANDLE, NtProcess);
            impl Drop for Restore {
                fn drop(&mut self) {
                    unsafe {
                        (self.1)(self.0);
                    }
                }
            }
            let restore = Restore(h.0, resume);
            println!("PAUSED");
            std::io::stdout().flush()?;
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(
                std::time::Duration::from_millis(millis).saturating_sub(started.elapsed()),
            );
            drop(restore);
        }
        Ok(())
    }
}
#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub fn processes(_: &str) -> Vec<(u32, PathBuf)> {
    vec![]
}
