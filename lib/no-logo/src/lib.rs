use std::ffi::{c_void, CStr};
use std::mem;
use std::ptr::null_mut;

use libeldenring::prelude::*;
use libeldenring::version;
use once_cell::sync::Lazy;
use u16cstr::u16str;
use widestring::U16CString;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{
    VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

type FnDirectInput8Create = unsafe extern "system" fn(
    _: HINSTANCE,
    _: u32,
    _: *const c_void,
    _: *const *const c_void,
    _: *const c_void,
) -> HRESULT;

type FnHResult = unsafe extern "system" fn() -> HRESULT;
type FnGetClassObject =
    unsafe extern "system" fn(*const c_void, *const c_void, *const c_void) -> HRESULT;

const fn pcstr(s: &'static CStr) -> PCSTR {
    PCSTR(s.as_ptr() as *const u8)
}

static SYMBOLS: Lazy<(FnDirectInput8Create, FnHResult, FnGetClassObject, FnHResult, FnHResult)> =
    Lazy::new(|| unsafe {
        apply_patch();

        let module = LoadLibraryW(PCWSTR(dinput8_path().as_ptr() as _))
            .expect("Failed to load system dinput8.dll");

        (
            mem::transmute::<unsafe extern "system" fn() -> isize, FnDirectInput8Create>(
                GetProcAddress(module, pcstr(c"DirectInput8Create"))
                    .expect("Failed to get DirectInput8Create export"),
            ),
            mem::transmute::<unsafe extern "system" fn() -> isize, FnHResult>(
                GetProcAddress(module, pcstr(c"DllCanUnloadNow"))
                    .expect("Failed to get DllCanUnloadNow export"),
            ),
            mem::transmute::<unsafe extern "system" fn() -> isize, FnGetClassObject>(
                GetProcAddress(module, pcstr(c"DllGetClassObject"))
                    .expect("Failed to get DllGetClassObject export"),
            ),
            mem::transmute::<unsafe extern "system" fn() -> isize, FnHResult>(
                GetProcAddress(module, pcstr(c"DllRegisterServer"))
                    .expect("Failed to get DllRegisterServer export"),
            ),
            mem::transmute::<unsafe extern "system" fn() -> isize, FnHResult>(
                GetProcAddress(module, pcstr(c"DllUnregisterServer"))
                    .expect("Failed to get DllUnregisterServer export"),
            ),
        )
    });

fn dinput8_path() -> U16CString {
    let mut sys_path = vec![0u16; 320];
    let len = unsafe { GetSystemDirectoryW(Some(&mut sys_path)) as usize };

    widestring::U16CString::from_vec_truncate(
        sys_path[..len]
            .iter()
            .chain(u16str!("\\dinput8.dll\0").as_slice().iter())
            .copied()
            .collect::<Vec<_>>(),
    )
}

/// # Safety
///
/// Patches the game's intro screen jump instruction to NOP. Must only be called
/// after the game module is loaded. Modifies 2 bytes at the known offset.
unsafe fn apply_patch() {
    let module_base = GetModuleHandleW(PCWSTR(null_mut())).expect("Failed to get module handle");
    let Ok(version) = version::check_version() else {
        return;
    };

    let offset = base_addresses::BaseAddresses::from(version).func_remove_intro_screens;

    let ptr = (module_base.0 as usize + offset) as *mut [u8; 2];
    let mut old = PAGE_PROTECTION_FLAGS(0);
    if *ptr == [0x74, 0x53] && VirtualProtect(ptr as _, 2, PAGE_EXECUTE_READWRITE, &mut old).is_ok()
    {
        (*ptr) = [0x90, 0x90];
        if let Err(e) = VirtualProtect(ptr as _, 2, old, &mut old) {
            eprintln!("no-logo: VirtualProtect restore failed: {e:?}");
        }
    }
}

/// # Safety
#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    a: HINSTANCE,
    b: u32,
    c: *const c_void,
    d: *const *const c_void,
    e: *const c_void,
) -> HRESULT {
    (SYMBOLS.0)(a, b, c, d, e)
}

#[no_mangle]
unsafe extern "C" fn DllMain(
    _hmodule: windows::Win32::Foundation::HINSTANCE,
    reason: u32,
    _: *mut std::ffi::c_void,
) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        apply_patch();
    }

    BOOL(1)
}
