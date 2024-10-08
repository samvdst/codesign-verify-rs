#[allow(non_snake_case)]
mod context;
mod wintrust_sys;

use super::Error;
use windows_sys::Win32::Foundation::WIN32_ERROR;
use wintrust_sys::{
    CloseHandle, CreateFileW, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2,
    CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
    CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext, GetLastError, OpenProcess,
    QueryFullProcessImageNameW, WinVerifyTrust, BCRYPT_SHA256_ALGORITHM, BYTE, CATALOG_INFO, DWORD,
    ERROR_INVALID_PARAMETER, FALSE, FILE_SHARE_READ, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    OPEN_EXISTING, PROCESS_QUERY_LIMITED_INFORMATION, TRUST_E_NOSIGNATURE,
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4,
    WTD_NO_IE4_CHAIN_FLAG, WTD_REVOCATION_CHECK_END_CERT, WTD_REVOKE_NONE, WTD_STATEACTION_VERIFY,
    WTD_UICONTEXT_EXECUTE, WTD_UI_NONE, WTD_USE_DEFAULT_OSVER_CHECK,
};

pub(crate) struct Verifier(Vec<u16>);
pub(crate) use context::Context;

#[allow(clippy::struct_field_names)]
struct CleanupContext {
    h_file: HANDLE,
    h_cat_admin: HANDLE,
    h_cat_info: HANDLE,
}

impl CleanupContext {
    pub fn new(h_file: HANDLE) -> Self {
        CleanupContext {
            h_file,
            h_cat_admin: 0,
            h_cat_info: 0,
        }
    }
}

impl Drop for CleanupContext {
    fn drop(&mut self) {
        if self.h_file != 0 {
            unsafe { CloseHandle(self.h_file) };
        }

        if self.h_cat_info != 0 {
            unsafe { CryptCATAdminReleaseCatalogContext(self.h_cat_admin, self.h_cat_info, 0) };
        }

        if self.h_cat_admin != 0 {
            unsafe { CryptCATAdminReleaseContext(self.h_cat_admin, 0) };
        }
    }
}

impl Verifier {
    pub fn for_file<P: AsRef<std::path::Path>>(path: P) -> Self {
        use std::os::windows::ffi::OsStrExt;

        let mut path_vec: Vec<u16> = path.as_ref().as_os_str().encode_wide().collect();
        path_vec.push(0); // Make sure path is null terminated

        Self(path_vec)
    }

    #[allow(clippy::cast_sign_loss)]
    // Extract the path of a pid, then call for file
    pub fn for_pid(pid: i32) -> Result<Self, Error> {
        let path = get_process_path(pid as _)?;
        Ok(Self::for_file(path))
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    pub fn verify(&self) -> Result<Context, Error> {
        unsafe {
            let mut file_info: WINTRUST_FILE_INFO = std::mem::zeroed();
            file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
            file_info.pcwszFilePath = self.0.as_ptr();

            match Self::verify_internal(Some(&mut file_info), None) {
                Ok(context) => Ok(context),
                Err(err) => {
                    if err == TRUST_E_NOSIGNATURE as u32 {
                        self.verify_catalog_signed()
                    } else {
                        Err(Error::OsError(err as i32))
                    }
                }
            }
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    unsafe fn verify_catalog_signed(&self) -> Result<Context, Error> {
        let h_file = CreateFileW(
            self.0.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        );

        if h_file == INVALID_HANDLE_VALUE {
            let err = GetLastError() as i32;
            return Err(Error::OsError(err));
        }

        let mut ctx = CleanupContext::new(h_file);

        let mut h_cat_admin: HANDLE = 0;
        let result = CryptCATAdminAcquireContext2(
            &mut h_cat_admin,
            std::ptr::null(),
            BCRYPT_SHA256_ALGORITHM,
            std::ptr::null(),
            0,
        );
        if result == 0 {
            let err = GetLastError() as i32;
            return Err(Error::OsError(err));
        }

        ctx.h_cat_admin = h_cat_admin;

        let mut hash_size: DWORD = 32;
        let mut hash_buffer: Vec<BYTE> = vec![0; hash_size as usize];
        let result = CryptCATAdminCalcHashFromFileHandle2(
            h_cat_admin,
            h_file,
            &mut hash_size,
            hash_buffer.as_mut_ptr(),
            0,
        );
        if result == 0 {
            let err = GetLastError() as i32;
            return Err(Error::OsError(err));
        }

        let h_cat_info = CryptCATAdminEnumCatalogFromHash(
            h_cat_admin,
            hash_buffer.as_ptr(),
            hash_size,
            0,
            std::ptr::null_mut(),
        );
        if h_cat_info == 0 {
            return Err(Error::Unsigned);
        }

        ctx.h_cat_info = h_cat_info;

        let mut ci: CATALOG_INFO = std::mem::zeroed();
        ci.cbStruct = std::mem::size_of::<CATALOG_INFO>() as u32;

        let result = CryptCATCatalogInfoFromContext(h_cat_info, &mut ci, 0);
        if result == 0 {
            let err = GetLastError() as i32;
            return Err(Error::OsError(err));
        }

        let hash_str = hash_buffer.iter().fold(String::new(), |mut acc, &val| {
            acc.push_str(&format!("{val:02x}"));
            acc
        });
        let mut hash: Vec<u16> = hash_str.encode_utf16().collect();
        hash.push(0); // Make sure hash is null terminated

        let mut wci: WINTRUST_CATALOG_INFO = std::mem::zeroed();
        wci.cbStruct = std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32;
        wci.pcwszCatalogFilePath = ci.wszCatalogFile.as_ptr();
        wci.pcwszMemberFilePath = self.0.as_ptr();
        wci.pcwszMemberTag = hash.as_ptr();

        match Self::verify_internal(None, Some(&mut wci)) {
            Ok(context) => Ok(context),
            Err(err) => Err(Error::OsError(err as i32)),
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    unsafe fn verify_internal(
        file_info: Option<*mut WINTRUST_FILE_INFO>,
        catalog_info: Option<*mut WINTRUST_CATALOG_INFO>,
    ) -> Result<Context, WIN32_ERROR> {
        // Initialize the WINTRUST_DATA structure
        let mut data: WINTRUST_DATA = std::mem::zeroed();
        data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        data.dwUIContext = WTD_UICONTEXT_EXECUTE;

        if let Some(fi) = file_info {
            data.dwUnionChoice = WTD_CHOICE_FILE;
            data.Anonymous.pFile = fi;
            data.dwProvFlags =
                WTD_DISABLE_MD2_MD4 | WTD_REVOCATION_CHECK_END_CERT | WTD_NO_IE4_CHAIN_FLAG;
        } else if let Some(ci) = catalog_info {
            data.dwUnionChoice = WTD_CHOICE_CATALOG;
            data.Anonymous.pCatalog = ci;
            data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_USE_DEFAULT_OSVER_CHECK;
        } else {
            return Err(ERROR_INVALID_PARAMETER);
        }

        let mut guid = WINTRUST_ACTION_GENERIC_VERIFY_V2;

        // Verify that the signature is actually valid
        if WinVerifyTrust(
            INVALID_HANDLE_VALUE as _,
            &mut guid,
            std::ptr::from_mut(&mut data).cast(),
        ) == 0
        {
            Context::new(data.hWVTStateData)
        } else {
            let _ = Context::new(data.hWVTStateData); // So close gets called on the data
            Err(GetLastError())
        }
    }
}

/// Attempts to get the full system path for a given proccess id
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn get_process_path(proc_id: u32) -> Result<String, Error> {
    let mut buf = [0u16; 2048];

    unsafe {
        let proc_handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, proc_id) {
            0 => return Err(Error::OsError(GetLastError() as i32)),
            handle => handle,
        };

        let mut path_len = buf.len() as _;

        match QueryFullProcessImageNameW(proc_handle, 0, buf.as_mut_ptr(), &mut path_len) {
            0 => Err(Error::OsError(GetLastError() as i32)),
            _ => Ok(String::from_utf16_lossy(&buf[..path_len as usize])),
        }
    }
}

#[cfg(test)]
mod tests {
    // This imports all the items from the parent module.
    use super::*;
    extern crate std;

    fn verify_file(process_path: &str, expected_issuer: &str) {
        match Verifier::for_file(process_path).verify() {
            Ok(context) => {
                assert_eq!(context.issuer_name().organization.unwrap(), expected_issuer);
            }
            Err(err) => {
                panic!("failed to verify signature. {:?}", err);
            }
        }
    }

    #[test]
    fn test_embeded_signed_file() {
        verify_file(
            "c:\\windows\\system32\\svchost.exe",
            "Microsoft Corporation",
        );
    }

    #[test]
    fn test_catalog_signed_file() {
        verify_file("c:\\windows\\system32\\cmd.exe", "Microsoft Corporation");
    }
}
