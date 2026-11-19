//! Walk validated local Windows paths one name at a time from pinned handles.

use crate::error::{AppError, AppResult};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, ErrorKind},
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
    ptr::{null, null_mut},
    sync::Arc,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::{
            RtlNtStatusToDosError, INVALID_HANDLE_VALUE, OBJ_DONT_REPARSE,
            STATUS_REPARSE_POINT_ENCOUNTERED, UNICODE_STRING,
        },
        Storage::FileSystem::{
            FileAttributeTagInfo, GetFileInformationByHandleEx, GetFileType,
            GetFinalPathNameByHandleW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK,
            SYNCHRONIZE, VOLUME_NAME_DOS, VOLUME_NAME_GUID, VOLUME_NAME_NONE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

// FILE_LIST_DIRECTORY on these handles can conflict with the kernel's internal
// destination open during a handle-relative rename. Enumeration opens separately.
const DIRECTORY_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const DIRECTORY_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const SEPARATOR: u16 = b'\\' as u16;
const MAX_COMPONENTS: usize = 256;

#[derive(Clone)]
pub(super) struct Directory(Arc<DirectoryHandle>);

struct DirectoryHandle {
    file: File,
    _parent: Option<Directory>,
}

impl Directory {
    pub(super) fn open(path: &Path, create: bool) -> AppResult<Self> {
        let (drive, names) = absolute_names(path)?;
        Self::open_root(drive)?.walk(names, create)
    }

    pub(super) fn for_listing(path: &Path) -> AppResult<Self> {
        let (drive, mut names) = absolute_names(path)?;
        let access = DIRECTORY_ACCESS | FILE_LIST_DIRECTORY;
        match names.pop() {
            Some(leaf) => Self::open_root(drive)?
                .walk(names, false)?
                .child_with_access(leaf, access, FILE_OPEN, FILE_OPEN_REPARSE_POINT),
            None => Self::open_root_with_access(drive, access),
        }
    }

    pub(super) fn descend(&self, relative: &Path, create: bool) -> AppResult<Self> {
        self.clone().walk(names(relative, 0, true)?, create)
    }

    pub(super) fn create_child(&self, name: &OsStr) -> AppResult<Self> {
        self.child(name, FILE_CREATE, FILE_DIRECTORY_FILE)
    }

    pub(super) fn file(&self) -> &File {
        &self.0.file
    }

    pub(super) fn open_leaf(
        &self,
        name: &OsStr,
        access: u32,
        disposition: u32,
        options: u32,
        share: u32,
    ) -> AppResult<File> {
        // Never truncate, overwrite, or delete a leaf before checking its handle.
        if !matches!(disposition, FILE_OPEN | FILE_CREATE)
            || options & !FILE_NON_DIRECTORY_FILE != 0
        {
            return Err(AppError::Denied("不安全的 Windows 文件打开选项".into()));
        }
        self.open_relative(
            name,
            access,
            disposition,
            options | FILE_OPEN_REPARSE_POINT,
            share,
        )
        .map(|(file, _)| file)
    }

    fn open_root(drive: u8) -> AppResult<Self> {
        Self::open_root_with_access(drive, DIRECTORY_ACCESS)
    }

    fn open_root_with_access(drive: u8, access: u32) -> AppResult<Self> {
        let root = format!("\\\\?\\{}:\\", char::from(drive));
        let file = OpenOptions::new()
            .access_mode(access)
            .share_mode(DIRECTORY_SHARE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&root)?;
        require_directory(disk_attributes(&file)?)?;
        // A volume GUID is unavailable on network shares. The DOS name must
        // also match this drive root, rather than a substituted directory.
        let volume_name = normalized_path(&file, VOLUME_NAME_GUID)
            .map_err(|err| AppError::Denied(format!("无法确认 Windows 盘符是本地卷: {err}")))?;
        let volume_name = volume_name
            .strip_suffix(&[SEPARATOR])
            .unwrap_or(&volume_name);
        let prefix: Vec<_> = r"\\?\Volume{".encode_utf16().collect();
        if volume_name.len() != prefix.len() + 37
            || !volume_name.starts_with(&prefix)
            || !volume_name.ends_with(&[u16::from(b'}')])
        {
            return Err(AppError::Denied("Windows 盘符未指向本地卷根目录".into()));
        }
        let resolved = normalized_path(&file, VOLUME_NAME_DOS)?;
        if !String::from_utf16(&resolved).is_ok_and(|s| {
            s.strip_suffix('\\')
                .unwrap_or(&s)
                .eq_ignore_ascii_case(&root[..root.len() - 1])
        }) {
            return Err(AppError::Denied("Windows 盘符未指向本地卷根目录".into()));
        }
        Ok(Self(Arc::new(DirectoryHandle {
            file,
            _parent: None,
        })))
    }

    fn walk<'a>(
        mut self,
        names: impl IntoIterator<Item = &'a OsStr>,
        create: bool,
    ) -> AppResult<Self> {
        for name in names {
            self = match self.child(name, FILE_OPEN, FILE_OPEN_REPARSE_POINT) {
                Err(AppError::Io(err)) if create && err.kind() == ErrorKind::NotFound => {
                    match self.create_child(name) {
                        Err(AppError::Io(err)) if err.kind() == ErrorKind::AlreadyExists => {
                            self.child(name, FILE_OPEN, FILE_OPEN_REPARSE_POINT)?
                        }
                        result => result?,
                    }
                }
                result => result?,
            };
        }
        Ok(self)
    }

    fn child(&self, name: &OsStr, disposition: u32, options: u32) -> AppResult<Self> {
        self.child_with_access(name, DIRECTORY_ACCESS, disposition, options)
    }

    fn child_with_access(
        &self,
        name: &OsStr,
        access: u32,
        disposition: u32,
        options: u32,
    ) -> AppResult<Self> {
        // FILE_DIRECTORY_FILE is used only for exclusive creation; opening an
        // existing entry uses FILE_OPEN_REPARSE_POINT and checks its type below.
        let (file, attributes) =
            self.open_relative(name, access, disposition, options, DIRECTORY_SHARE)?;
        require_directory(attributes)?;
        Ok(Self(Arc::new(DirectoryHandle {
            file,
            _parent: Some(self.clone()),
        })))
    }

    fn open_relative(
        &self,
        name: &OsStr,
        access: u32,
        disposition: u32,
        options: u32,
        share: u32,
    ) -> AppResult<(File, u32)> {
        let mut wide = leaf_name(name)?;
        let length = (wide.len() * size_of::<u16>()) as u16;
        let name = UNICODE_STRING {
            Length: length,
            MaximumLength: length,
            Buffer: wide.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: self.file().as_raw_handle(),
            ObjectName: &name,
            Attributes: OBJ_DONT_REPARSE,
            ..OBJECT_ATTRIBUTES::default()
        };
        let mut handle = null_mut();
        let mut status_block = IO_STATUS_BLOCK::default();
        // The name buffer, structures, and borrowed parent handle live through
        // this synchronous call. OBJ_INHERIT is intentionally not set.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                access | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                &attributes,
                &mut status_block,
                null(),
                FILE_ATTRIBUTE_NORMAL,
                share,
                disposition,
                options | FILE_SYNCHRONOUS_IO_NONALERT,
                null(),
                0,
            )
        };
        if status < 0 {
            return Err(nt_error(status));
        }
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::other("Windows 未返回有效的文件句柄").into());
        }
        // A successful NtCreateFile returns a new owned handle.
        let file = unsafe { File::from_raw_handle(handle) };
        let attributes = disk_attributes(&file)?;
        // A short name can resolve without a reparse. Compare against the long
        // spelling reported by the handle before allowing it to be used.
        let resolved = normalized_path(&file, VOLUME_NAME_NONE)?;
        if resolved.rsplit(|&c| c == SEPARATOR).next() != Some(wide.as_slice()) {
            return Err(AppError::Denied("不允许使用 Windows 路径别名".into()));
        }
        Ok((file, attributes))
    }
}

pub(super) fn parent(path: &Path, create: bool) -> AppResult<(Directory, &OsStr)> {
    let (drive, mut names) = absolute_names(path)?;
    let name = names.pop().ok_or_else(|| invalid_path(path))?;
    Ok((Directory::open_root(drive)?.walk(names, create)?, name))
}

pub(super) fn relative_parent(path: &Path) -> AppResult<(&Path, &OsStr)> {
    let names = names(path, 0, true)?;
    path.parent()
        .zip(names.last().copied())
        .ok_or_else(|| invalid_path(path))
}

fn absolute_names(path: &Path) -> AppResult<(u8, Vec<&OsStr>)> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let verbatim = bytes.starts_with(br"\\?\");
    let start = if verbatim { 4 } else { 0 };
    let [drive, b':', separator, ..] = bytes.get(start..).unwrap_or_default() else {
        return Err(invalid_path(path));
    };
    if !drive.is_ascii_alphabetic() || !is_separator(*separator, !verbatim) {
        return Err(invalid_path(path));
    }
    Ok((
        drive.to_ascii_uppercase(),
        names(path, start + 3, !verbatim)?,
    ))
}

fn is_separator(byte: u8, forward_slash: bool) -> bool {
    byte == b'\\' || (forward_slash && byte == b'/')
}

fn names(path: &Path, offset: usize, forward_slash: bool) -> AppResult<Vec<&OsStr>> {
    let path = path.as_os_str();
    let tail = &path.as_encoded_bytes()[offset..];
    if tail.is_empty() {
        return Ok(Vec::new());
    }
    // Path::components normalizes away empty and dot components. Validate the
    // original spelling instead.
    tail.split(|&byte| is_separator(byte, forward_slash))
        .enumerate()
        .map(|(index, part)| {
            if index >= MAX_COMPONENTS {
                return Err(invalid_path(Path::new(path)));
            }
            // SAFETY: part came from as_encoded_bytes in this process. The
            // offset is zero or just after an ASCII root separator, and all
            // subsequent splits are at ASCII separators.
            let name = unsafe { OsStr::from_encoded_bytes_unchecked(part) };
            validate_leaf(name)?;
            Ok(name)
        })
        .collect()
}

pub(super) fn leaf_name(name: &OsStr) -> AppResult<Vec<u16>> {
    validate_leaf(name)?;
    Ok(name.encode_wide().collect())
}

fn validate_leaf(name: &OsStr) -> AppResult<()> {
    let bytes = name.as_encoded_bytes();
    if bytes.is_empty()
        || matches!(bytes.last(), Some(b'.' | b' '))
        || bytes
            .iter()
            .any(|&b| b.is_ascii_control() || br#"<>:"/\|?*"#.contains(&b))
        || is_device_name(bytes)
        || name.encode_wide().count() > u16::MAX as usize / size_of::<u16>()
    {
        return Err(invalid_path(Path::new(name)));
    }
    Ok(())
}

fn is_device_name(bytes: &[u8]) -> bool {
    let stem = bytes.split(|&b| b == b'.').next().unwrap_or_default();
    let end = stem.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    let stem = &stem[..end];
    [
        b"CON".as_slice(),
        b"PRN",
        b"AUX",
        b"NUL",
        b"CONIN$",
        b"CONOUT$",
        b"CLOCK$",
    ]
    .iter()
    .any(|device| stem.eq_ignore_ascii_case(device))
        || stem.split_at_checked(3).is_some_and(|(port, number)| {
            (port.eq_ignore_ascii_case(b"COM") || port.eq_ignore_ascii_case(b"LPT"))
                && matches!(number, [b'1'..=b'9'] | [0xc2, 0xb9 | 0xb2 | 0xb3])
        })
}

fn invalid_path(path: &Path) -> AppError {
    AppError::Denied(format!("无效或不安全的 Windows 路径: {}", path.display()))
}

pub(super) fn nt_error(status: i32) -> AppError {
    if status == STATUS_REPARSE_POINT_ENCOUNTERED {
        return AppError::Denied("Windows 路径包含重解析点".into());
    }
    // This conversion preserves std::io::ErrorKind for name misses and collisions.
    io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32).into()
}

fn require_directory(attributes: u32) -> AppResult<()> {
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(AppError::Denied("Windows 路径组件不是目录".into()));
    }
    Ok(())
}

fn disk_attributes(file: &File) -> AppResult<u32> {
    let handle = file.as_raw_handle();
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // The handle stays live and the output buffer is valid for both queries.
    if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
        return Err(AppError::Denied(
            "只允许打开 Windows 磁盘上的文件或目录".into(),
        ));
    }
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.ReparseTag != 0 {
        return Err(AppError::Denied("Windows 路径包含重解析点".into()));
    }
    Ok(info.FileAttributes)
}

fn normalized_path(file: &File, volume: u32) -> AppResult<Vec<u16>> {
    fn query(file: &File, volume: u32, buffer: &mut [u16]) -> AppResult<usize> {
        // The handle and writable buffer are valid for the duration of the call.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                FILE_NAME_NORMALIZED | volume,
            )
        } as usize;
        if length == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(length)
    }

    let mut buffer = vec![0; 256];
    let mut length = query(file, volume, &mut buffer)?;
    if length >= buffer.len() && length <= u16::MAX as usize {
        buffer.resize(length, 0);
        length = query(file, volume, &mut buffer)?;
    }
    if length >= buffer.len() {
        return Err(AppError::Denied("无法核对 Windows 路径的规范名称".into()));
    }
    buffer.truncate(length);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use windows_sys::Win32::Foundation::{STATUS_NOT_SAME_DEVICE, STATUS_OBJECT_NAME_COLLISION};

    #[test]
    fn maps_native_result_codes_used_by_transaction_fallbacks() {
        for (status, kind) in [
            (STATUS_NOT_SAME_DEVICE, ErrorKind::CrossesDevices),
            (STATUS_OBJECT_NAME_COLLISION, ErrorKind::AlreadyExists),
        ] {
            assert!(matches!(nt_error(status), AppError::Io(error) if error.kind() == kind));
        }
    }

    #[test]
    fn parses_drive_and_verbatim_disk_paths_without_normalizing_components() {
        for input in [r"c:/mods\a.txt", r"C:\mods\a.txt", r"\\?\C:\mods\a.txt"] {
            let (drive, names) = absolute_names(Path::new(input)).unwrap();
            assert_eq!(drive, b'C');
            assert_eq!(names, [OsStr::new("mods"), OsStr::new("a.txt")]);
        }
        for input in [
            r"C:mods\a.txt",
            r"\mods\a.txt",
            r"\\server\share\a.txt",
            r"\\?\UNC\server\share\a.txt",
            r"\\?\GLOBALROOT\Device\a.txt",
            r"\\.\C:\mods\a.txt",
            r"\\?\C:\mods/a.txt",
            r"C:\mods\\a.txt",
            r"C:\mods\",
            r"C:\mods\.\a.txt",
            r"C:\mods\..\a.txt",
            r"C:\mods\a.txt::$DATA",
        ] {
            assert!(absolute_names(Path::new(input)).is_err(), "{input:?}");
        }
    }

    #[test]
    fn rejects_excessive_depth_before_any_directory_opens() {
        let path = format!(r"C:\{}end", "a\\".repeat(MAX_COMPONENTS));
        assert!(absolute_names(Path::new(&path)).is_err());
    }

    #[test]
    fn rejects_ambiguous_leaf_names_and_preserves_utf16() {
        for input in [
            "",
            ".",
            "..",
            "file.",
            "file ",
            "a/b",
            "a\\b",
            "a:stream",
            "a\0b",
            "a?",
            "NUL.txt",
            "con .txt",
            "COM\u{b2}.txt",
            "lPt9",
            "CLOCK$",
        ] {
            assert!(leaf_name(OsStr::new(input)).is_err(), "{input:?}");
        }
        let normal = "COM10_\u{6a21}\u{1f4be}.txt";
        assert!(leaf_name(OsStr::new(normal))
            .unwrap()
            .into_iter()
            .eq(normal.encode_utf16()));
    }

    #[test]
    fn rooted_handles_pin_ancestors_and_create_only_new_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = root.join("original");
        let moved = root.join("moved");
        let deepest = Directory::open(&root, false)
            .unwrap()
            .descend(Path::new(r"original\deepest"), true)
            .unwrap();

        assert!(fs::rename(&original, &moved).is_err());
        let name = OsStr::new("entry.bin");
        let exclusive = || {
            deepest.open_leaf(
                name,
                FILE_READ_ATTRIBUTES,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE,
                0,
            )
        };
        drop(exclusive().unwrap());
        assert!(
            matches!(exclusive(), Err(AppError::Io(e)) if e.kind() == ErrorKind::AlreadyExists)
        );

        drop(deepest);
        fs::rename(&original, &moved).unwrap();
        assert!(moved.join("deepest").join(name).is_file());
    }
}
