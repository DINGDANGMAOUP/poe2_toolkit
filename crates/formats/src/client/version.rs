//! Read version resources without loading or executing the client program.
use std::path::Path;

#[cfg(not(windows))]
pub(super) fn major(_path: &Path) -> Option<u32> {
    None
}

#[cfg(windows)]
pub(super) fn major(path: &Path) -> Option<u32> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let name = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // All API pointers refer to owned, bounded buffers alive for this call.
    unsafe {
        let size = GetFileVersionInfoSizeW(name.as_ptr(), ptr::null_mut());
        if size == 0 || size > 1024 * 1024 {
            return None;
        }
        let mut bytes = vec![0u8; size as usize];
        if GetFileVersionInfoW(name.as_ptr(), 0, size, bytes.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut value: *mut c_void = ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            bytes.as_ptr().cast(),
            [b'\\' as u16, 0].as_ptr(),
            &mut value,
            &mut length,
        ) == 0
            || value.is_null()
            || (length as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let start = value as usize;
        let lower = bytes.as_ptr() as usize;
        if start < lower || start.checked_add(length as usize)? > lower + bytes.len() {
            return None;
        }
        let info = ptr::read_unaligned(value.cast::<VS_FIXEDFILEINFO>());
        (info.dwSignature == 0xfeef04bd).then_some(info.dwFileVersionMS >> 16)
    }
}
