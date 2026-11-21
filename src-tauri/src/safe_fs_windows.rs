use super::{wrong_identity, FileIdentity};
use crate::error::{AppError, AppResult};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    mem::{offset_of, size_of},
    os::windows::{fs::MetadataExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows_sys::{
    Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_CREATE, FILE_NON_DIRECTORY_FILE,
        FILE_OPEN, FILE_RENAME_INFORMATION,
    },
    Win32::{
        Storage::FileSystem::{
            FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx, GetFileType, DELETE,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_STANDARD_INFO, FILE_TYPE_DISK,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

#[path = "safe_fs_windows_directory.rs"]
mod enumeration;
#[path = "safe_fs_windows_path.rs"]
mod path;

use path::{leaf_name, nt_error, parent, relative_parent, Directory};

const SHARE_FILE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

pub(crate) fn create_dir_all(path: &Path) -> AppResult<()> {
    Directory::open(path, true)?;
    Ok(())
}

pub(crate) fn create_dir(path: &Path) -> AppResult<()> {
    let (parent, name) = parent(path, true)?;
    parent.create_child(name)?;
    Ok(())
}

fn create_new_at(parent: &Directory, name: &OsStr) -> AppResult<File> {
    parent.open_leaf(
        name,
        FILE_GENERIC_WRITE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
        SHARE_FILE,
    )
}

pub(crate) fn create_new(path: &Path) -> AppResult<File> {
    let (parent, name) = parent(path, true)?;
    create_new_at(&parent, name)
}

pub(crate) fn open_regular(path: &Path) -> AppResult<File> {
    let (parent, name) = parent(path, false)?;
    parent.open_leaf(
        name,
        FILE_GENERIC_READ,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
        SHARE_FILE,
    )
}

fn identity_by_handle(file: &File) -> AppResult<(FileIdentity, FILE_STANDARD_INFO)> {
    let mut id = FILE_ID_INFO::default();
    let mut standard = FILE_STANDARD_INFO::default();
    // The live handle and correctly sized output buffers match their information classes.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        ) == 0
            || GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileStandardInfo,
                (&mut standard as *mut FILE_STANDARD_INFO).cast(),
                size_of::<FILE_STANDARD_INFO>() as u32,
            ) == 0
    } {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((
        FileIdentity {
            volume: id.VolumeSerialNumber,
            object: id.FileId.Identifier,
            birth: None,
        },
        standard,
    ))
}

fn observed_identity(path: &Path) -> AppResult<Option<FileIdentity>> {
    if path.file_name().is_none() {
        let dir = Directory::open(path, false)?;
        let (id, info) = identity_by_handle(dir.file())?;
        return Ok((info.Directory && !info.DeletePending).then_some(id));
    }
    let (parent, name) = parent(path, false)?;
    let file = parent.open_leaf(name, FILE_READ_ATTRIBUTES, FILE_OPEN, 0, SHARE_FILE)?;
    let (id, info) = identity_by_handle(&file)?;
    Ok((!info.DeletePending && (info.Directory || info.NumberOfLinks == 1)).then_some(id))
}

pub(crate) fn identity(path: &Path) -> AppResult<Option<FileIdentity>> {
    match observed_identity(path) {
        Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        other => other,
    }
}

pub(crate) fn verify_regular_at(file: &File, path: &Path) -> AppResult<()> {
    // The opened File remains live during verification; identity(path) opens
    // the current name through its own non-reparse directory handles.
    let meta = file.metadata()?;
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        || !meta.is_file()
        || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(wrong_identity(path));
    }
    let (opened, standard) = identity_by_handle(file)?;
    if standard.Directory
        || standard.DeletePending
        || standard.NumberOfLinks != 1
        || identity(path)?.as_ref() != Some(&opened)
    {
        return Err(wrong_identity(path));
    }
    Ok(())
}

pub(crate) fn directory_entries(
    path: &Path,
    expected: &FileIdentity,
) -> AppResult<impl Iterator<Item = AppResult<(OsString, bool)>>> {
    let parent = Directory::for_listing(path)?;
    let (actual, standard) = identity_by_handle(parent.file())?;
    if &actual != expected || !standard.Directory || standard.DeletePending {
        return Err(wrong_identity(path));
    }
    let mut entries = enumeration::entries(parent.file().try_clone()?);
    Ok(std::iter::from_fn(move || {
        let _pinned = &parent;
        entries.next()
    }))
}

pub(crate) fn rename_exclusive(src: &Path, dst: &Path) -> AppResult<()> {
    rename(src, dst, None)
}

pub(crate) fn rename_checked(src: &Path, dst: &Path, expected: &FileIdentity) -> AppResult<()> {
    rename(src, dst, Some(expected))
}

fn rename(src: &Path, dst: &Path, expected: Option<&FileIdentity>) -> AppResult<()> {
    let (from, old) = parent(src, false)?;
    let (to, new) = parent(dst, expected.is_none())?;
    let source = from.open_leaf(
        old,
        DELETE | FILE_READ_ATTRIBUTES,
        FILE_OPEN,
        0,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )?;
    let (id, standard) = identity_by_handle(&source)?;
    if standard.DeletePending
        || (!standard.Directory && standard.NumberOfLinks != 1)
        || expected.is_some_and(|expected| expected != &id)
    {
        return Err(wrong_identity(src));
    }
    let name = leaf_name(new)?;
    let (old_parent, _) = identity_by_handle(from.file())?;
    let (new_parent, _) = identity_by_handle(to.file())?;
    // A simple name with a NULL destination handle is a same-directory rename.
    let destination = (old_parent != new_parent).then_some(to.file());
    match rename_to(&source, destination, &name) {
        Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(
            AppError::Conflict(format!("目标已存在: {}", crate::paths::display_path(dst))),
        ),
        result => result,
    }
}

fn rename_to(source: &File, destination: Option<&File>, name: &[u16]) -> AppResult<()> {
    let offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
    let name_bytes = std::mem::size_of_val(name);
    let length = size_of::<FILE_RENAME_INFORMATION>() + name_bytes;
    let mut storage = vec![0usize; length.div_ceil(size_of::<usize>())];
    let start = storage.as_mut_ptr().cast::<u8>();
    let info = start.cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    // Aligned storage covers the fixed header and the entire UTF-16 name.
    // The zeroed flags request no replacement; both handles stay live through
    // this synchronous rename. Only the single new name is resolved by NT.
    let result = unsafe {
        (*info).RootDirectory =
            destination.map_or(std::ptr::null_mut(), AsRawHandle::as_raw_handle);
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), start.add(offset).cast::<u16>(), name.len());
        NtSetInformationFile(
            source.as_raw_handle(),
            &mut status,
            info.cast(),
            length as u32,
            FileRenameInformation,
        )
    };
    if result < 0 {
        return Err(nt_error(result));
    }
    Ok(())
}

pub(crate) struct StagingDirectory {
    path: PathBuf,
    root: Directory,
}

impl StagingDirectory {
    pub(crate) fn create(path: &Path) -> AppResult<Self> {
        let (parent, name) = parent(path, false)?;
        Ok(Self {
            path: path.into(),
            root: parent.create_child(name)?,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn create_dir_all(&self, relative: &Path) -> AppResult<()> {
        self.verify_path()?;
        self.root.descend(relative, true)?;
        Ok(())
    }

    pub(crate) fn create_file(&self, relative: &Path) -> AppResult<File> {
        let (base, name) = relative_parent(relative)?;
        self.verify_path()?;
        create_new_at(&self.root.descend(base, true)?, name)
    }

    pub(crate) fn verify_path(&self) -> AppResult<()> {
        let (original, standard) = identity_by_handle(self.root.file())?;
        if !standard.Directory
            || standard.DeletePending
            || identity(&self.path)?.as_ref() != Some(&original)
        {
            return Err(wrong_identity(&self.path));
        }
        Ok(())
    }
}
