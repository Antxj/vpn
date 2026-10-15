//! Windows: protection of the saved accounts with DPAPI (CryptProtectData,
//! user scope): only the same Windows account can decrypt them.

use std::ffi::c_void;
use std::ptr;

/// DPAPI is part of Windows: it is never "locked" or unavailable.
pub const PODE_FICAR_INDISPONIVEL: bool = false;

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        descr: *const u16,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
    fn CryptUnprotectData(
        data_in: *const DataBlob,
        descr: *mut *mut u16,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    let blob_in = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut blob_out = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &blob_in,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut blob_out,
            )
        } else {
            CryptUnprotectData(
                &blob_in,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut blob_out,
            )
        }
    };
    if ok == 0 || blob_out.pb_data.is_null() {
        return None;
    }
    let out =
        unsafe { std::slice::from_raw_parts(blob_out.pb_data, blob_out.cb_data as usize).to_vec() };
    unsafe { LocalFree(blob_out.pb_data as *mut c_void) };
    Some(out)
}

pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}
