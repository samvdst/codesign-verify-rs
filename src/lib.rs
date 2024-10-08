#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos::{Context, Verifier};
#[cfg(windows)]
use windows::{Context, Verifier};

///
/// Used to verify the validity of a code signature
///
pub struct CodeSignVerifier(Verifier);

///
/// Used to extract additional information from the signing leaf certificate
///
pub struct SignatureContext(Context);

///
/// Represents an Issuer or Subject name with the following fields:
///
/// # Fields
///
/// `common_name`: OID 2.5.4.3
///
/// `organization`: OID 2.5.4.10
///
/// `organization_unit`: OID 2.5.4.11
///
/// `country`: OID 2.5.4.6
///
#[derive(Debug, PartialEq)]
pub struct Name {
    pub common_name: Option<String>,       // 2.5.4.3
    pub organization: Option<String>,      // 2.5.4.10
    pub organization_unit: Option<String>, // 2.5.4.11
    pub country: Option<String>,           // 2.5.4.6
}

#[derive(Debug)]
pub enum Error {
    Unsigned,         // The binary file didn't have any singature
    OsError(i32),     // Warps an inner provider error code
    InvalidPath,      // The provided path was malformed
    LeafCertNotFound, // Unable to fetch certificate information
    #[cfg(target_os = "macos")]
    CFError(String),
    #[cfg(windows)]
    IoError(std::io::Error),
}

impl CodeSignVerifier {
    /// Create a verifier for a binary at a given path.
    /// On macOS it can be either a binary or an application package.
    pub fn for_file<P: AsRef<std::path::Path>>(path: P) -> Self {
        CodeSignVerifier(Verifier::for_file(path))
    }

    /// Create a verifier for a running application by PID.
    /// On Windows it will get the full path to the running application first.
    /// This can be used for e.g. verifying the app on the other end of a pipe.
    pub fn for_pid(pid: i32) -> Result<Self, Error> {
        Verifier::for_pid(pid).map(CodeSignVerifier)
    }

    /// Perform the verification itself.
    /// On macOS the verification uses the Security framework with "anchor trusted" as the requirement.
    /// On Windows the verification uses `WinTrust` and the `WINTRUST_ACTION_GENERIC_VERIFY_V2` action.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use codesign_verify::CodeSignVerifier;
    ///
    /// CodeSignVerifier::for_file("C:/Windows/explorer.exe").verify().unwrap();
    /// ```
    pub fn verify(self) -> Result<SignatureContext, Error> {
        self.0.verify().map(SignatureContext)
    }
}

impl SignatureContext {
    /// Retrieve the subject name on the leaf certificate
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use codesign_verify::CodeSignVerifier;
    ///
    /// let ctx = CodeSignVerifier::for_file("C:/Windows/explorer.exe").verify().unwrap();
    /// assert_eq!(
    ///    ctx.subject_name().organization.as_deref(),
    ///    Some("Microsoft Corporation")
    /// );
    ///
    /// ```
    #[must_use]
    pub fn subject_name(&self) -> Name {
        self.0.subject_name()
    }

    /// Retrieve the issuer name on the leaf certificate
    #[must_use]
    pub fn issuer_name(&self) -> Name {
        self.0.issuer_name()
    }

    /// Compute the sha1 thumbprint of the leaf certificate
    #[must_use]
    pub fn sha1_thumbprint(&self) -> String {
        self.0.sha1_thumbprint()
    }

    /// Compute the sha256 thumbprint of the leaf certificate
    #[must_use]
    pub fn sha256_thumbprint(&self) -> String {
        self.0.sha256_thumbprint()
    }

    /// Retrieve the leaf certificate serial number
    #[must_use]
    pub fn serial(&self) -> String {
        self.0.serial()
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_signed() {
        let verifier = super::CodeSignVerifier::for_file("/sbin/ping").unwrap(); // Should always be present on macOS
        let ctx = verifier.verify().unwrap(); // Should always be signed

        // If those values begin to fail, Apple probably changed their certficate
        assert_eq!(
            ctx.subject_name().organization.as_deref(),
            Some("Apple Inc.")
        );

        assert_eq!(
            ctx.issuer_name().organization_unit.as_deref(),
            Some("Apple Certification Authority")
        );

        assert_eq!(
            ctx.sha1_thumbprint(),
            "013e2787748a74103d62d2cdbf77a1345517c482"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_signed() {
        let path = format!("{}/explorer.exe", std::env::var("windir").unwrap()); // Should always be present on Windows
        let verifier = super::CodeSignVerifier::for_file(path);
        let ctx = verifier.verify().unwrap(); // Should always be signed

        // If those values begin to fail, Microsoft probably changed their certficate
        assert_eq!(
            ctx.subject_name().organization.as_deref(),
            Some("Microsoft Corporation")
        );

        assert_eq!(
            ctx.issuer_name().common_name.as_deref(),
            Some("Microsoft Windows Production PCA 2011")
        );

        assert_eq!(
            ctx.sha1_thumbprint(),
            "d8fb0cc66a08061b42d46d03546f0d42cbc49b7c"
        );

        assert_eq!(ctx.serial(), "3300000460cf42a912315f6fb3000000000460");
    }

    #[test]
    fn test_unsigned() {
        let path = std::env::args().next().unwrap(); // own path, always unsigned and present

        assert!(matches!(
            super::CodeSignVerifier::for_file(path).verify(),
            Err(Error::Unsigned)
        ));
    }
}
