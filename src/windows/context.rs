use super::wintrust_sys::{
    CertGetNameStringW, WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust, CERT_NAME_ATTR_TYPE, CERT_NAME_ISSUER_FLAG,
    DWORD, HANDLE, INVALID_HANDLE_VALUE, PCCERT_CONTEXT, TRUST_E_NO_SIGNER_CERT,
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_UICONTEXT_EXECUTE, WTD_UI_NONE,
};
use crate::Name;
use windows_sys::Win32::Foundation::WIN32_ERROR;

#[allow(non_camel_case_types)]
#[repr(C)]
struct CRYPT_PROVIDER_CERT_HDR {
    cbStruct: DWORD,
    pCert: PCCERT_CONTEXT,
}

pub(crate) struct Context {
    data: HANDLE,
    leaf_cert_ptr: PCCERT_CONTEXT,
}

impl Drop for Context {
    fn drop(&mut self) {
        close_data(self.data);
    }
}

#[allow(clippy::cast_possible_truncation)]
fn close_data(handle: HANDLE) {
    // Initialize the WINTRUST_DATA structure
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = 0;
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    data.dwUIContext = WTD_UICONTEXT_EXECUTE;
    data.hWVTStateData = handle;

    let mut guid = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    unsafe {
        WinVerifyTrust(
            INVALID_HANDLE_VALUE as _,
            &mut guid,
            std::ptr::from_mut(&mut data).cast(),
        )
    };
}

impl Context {
    #[allow(clippy::cast_sign_loss)]
    pub fn new(state_data: HANDLE) -> Result<Self, WIN32_ERROR> {
        let mut ret = Context {
            data: state_data,
            leaf_cert_ptr: std::ptr::null(),
        };

        unsafe {
            let crypt_prov_data = match WTHelperProvDataFromStateData(state_data) {
                data if data.is_null() => return Err(TRUST_E_NO_SIGNER_CERT as u32),
                data => data,
            };

            let crypt_prov_sgnr = match WTHelperGetProvSignerFromChain(crypt_prov_data, 0, 0, 0) {
                sgnr if sgnr.is_null() => return Err(TRUST_E_NO_SIGNER_CERT as u32),
                sgnr => sgnr,
            };

            let crypt_prov_cert = match WTHelperGetProvCertFromChain(crypt_prov_sgnr, 0) {
                cert if cert.is_null() => return Err(TRUST_E_NO_SIGNER_CERT as u32),
                cert => cert.cast::<CRYPT_PROVIDER_CERT_HDR>(),
            };

            ret.leaf_cert_ptr = crypt_prov_cert.as_ref().unwrap().pCert as PCCERT_CONTEXT;
        }

        Ok(ret)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn get_oid_name(&self, issuer: bool, oid: &str) -> Option<String> {
        use std::os::windows::ffi::OsStringExt;
        let key = std::ffi::CString::new(oid).unwrap();
        let flag = if issuer { CERT_NAME_ISSUER_FLAG } else { 0 };

        // Determine string size:
        let len = unsafe {
            CertGetNameStringW(
                self.leaf_cert_ptr,
                CERT_NAME_ATTR_TYPE,
                flag,
                key.as_bytes_with_nul().as_ptr().cast(),
                std::ptr::null_mut(),
                0,
            )
        };

        if len == 1 {
            return None;
        }

        let mut buf = vec![0; len as usize];

        let len = unsafe {
            CertGetNameStringW(
                self.leaf_cert_ptr,
                CERT_NAME_ATTR_TYPE,
                flag,
                key.as_ptr().cast(),
                buf.as_mut_ptr(),
                buf.len() as _,
            )
        };

        Some(
            std::ffi::OsString::from_wide(&buf[..len as usize - 1])
                .into_string()
                .unwrap(),
        )
    }

    pub fn serial(&self) -> String {
        let serial_blob = unsafe {
            self.leaf_cert_ptr
                .as_ref()
                .unwrap()
                .pCertInfo
                .as_ref()
                .unwrap()
                .SerialNumber
        };

        let blob =
            unsafe { std::slice::from_raw_parts(serial_blob.pbData, serial_blob.cbData as usize) };

        // For some reason windows stores the serial number in reverse order
        blob.iter()
            .fold(String::new(), |v, s| format!("{s:02x}{v}"))
    }

    pub fn subject_name(&self) -> Name {
        Name {
            common_name: self.get_oid_name(false, "2.5.4.3"),
            organization: self.get_oid_name(false, "2.5.4.10"),
            organization_unit: self.get_oid_name(false, "2.5.4.11"),
            country: self.get_oid_name(false, "2.5.4.6"),
        }
    }

    pub fn issuer_name(&self) -> Name {
        Name {
            common_name: self.get_oid_name(true, "2.5.4.3"),
            organization: self.get_oid_name(true, "2.5.4.10"),
            organization_unit: self.get_oid_name(true, "2.5.4.11"),
            country: self.get_oid_name(true, "2.5.4.6"),
        }
    }

    #[allow(clippy::items_after_statements)]
    pub fn sha1_thumbprint(&self) -> String {
        let cert_ref = unsafe { self.leaf_cert_ptr.as_ref().unwrap() };
        let cert_data = unsafe {
            std::slice::from_raw_parts(cert_ref.pbCertEncoded, cert_ref.cbCertEncoded as _)
        };

        use sha1::Digest;
        let hash = sha1::Sha1::digest(cert_data);

        hash.as_slice()
            .iter()
            .fold(String::new(), |s, byte| s + &format!("{byte:02x}"))
    }

    #[allow(clippy::items_after_statements)]
    pub fn sha256_thumbprint(&self) -> String {
        let cert_ref = unsafe { self.leaf_cert_ptr.as_ref().unwrap() };
        let cert_data = unsafe {
            std::slice::from_raw_parts(cert_ref.pbCertEncoded, cert_ref.cbCertEncoded as _)
        };

        use sha2::Digest;
        let hash = sha2::Sha256::digest(cert_data);

        hash.as_slice()
            .iter()
            .fold(String::new(), |s, byte| s + &format!("{byte:02x}"))
    }
}
