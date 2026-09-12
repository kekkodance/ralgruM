use std::fmt;

const MAGIC: &[u8; 8] = b"RLGSESS1";
const VERSION: u16 = 1;
const HEADER_LEN: usize = MAGIC.len() + 2 + 2 + 4;
const MAX_PLAINTEXT_LEN: usize = 64 * 1024;
const MAX_PROTECTED_LEN: usize = 256 * 1024;
const APP_ENTROPY: &[u8] = b"ralgruM account session v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProtectionError {
    Unavailable,
    InvalidEnvelope,
    TooLarge,
    Platform,
}

impl fmt::Display for ProtectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "account session protection is unavailable",
            Self::InvalidEnvelope => "account session protection envelope is invalid",
            Self::TooLarge => "account session protection payload is too large",
            Self::Platform => "account session protection failed",
        })
    }
}

impl std::error::Error for ProtectionError {}

pub(crate) fn protect(plaintext: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    if plaintext.is_empty() || plaintext.len() > MAX_PLAINTEXT_LEN {
        return Err(ProtectionError::TooLarge);
    }
    let protected = protect_platform(plaintext)?;
    if protected.is_empty() || protected.len() > MAX_PROTECTED_LEN {
        return Err(ProtectionError::TooLarge);
    }

    let protected_len = u32::try_from(protected.len()).map_err(|_| ProtectionError::TooLarge)?;
    let mut envelope = Vec::with_capacity(HEADER_LEN + protected.len());
    envelope.extend_from_slice(MAGIC);
    envelope.extend_from_slice(&VERSION.to_le_bytes());
    envelope.extend_from_slice(&0_u16.to_le_bytes());
    envelope.extend_from_slice(&protected_len.to_le_bytes());
    envelope.extend_from_slice(&protected);
    Ok(envelope)
}

pub(crate) fn unprotect(envelope: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    let payload = parse_envelope(envelope)?;
    let mut plaintext = unprotect_platform(payload)?;
    if plaintext.is_empty() || plaintext.len() > MAX_PLAINTEXT_LEN {
        wipe(&mut plaintext);
        return Err(ProtectionError::TooLarge);
    }
    Ok(plaintext)
}

pub(crate) fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn parse_envelope(envelope: &[u8]) -> Result<&[u8], ProtectionError> {
    if envelope.len() < HEADER_LEN || envelope.len() > HEADER_LEN + MAX_PROTECTED_LEN {
        return Err(ProtectionError::InvalidEnvelope);
    }
    if &envelope[..MAGIC.len()] != MAGIC {
        return Err(ProtectionError::InvalidEnvelope);
    }
    let version_offset = MAGIC.len();
    let version = u16::from_le_bytes(
        envelope[version_offset..version_offset + 2]
            .try_into()
            .expect("version header has a fixed width"),
    );
    if version != VERSION {
        return Err(ProtectionError::InvalidEnvelope);
    }
    let reserved_offset = version_offset + 2;
    if envelope[reserved_offset..reserved_offset + 2] != [0, 0] {
        return Err(ProtectionError::InvalidEnvelope);
    }
    let length_offset = reserved_offset + 2;
    let length = u32::from_le_bytes(
        envelope[length_offset..length_offset + 4]
            .try_into()
            .expect("payload length header has a fixed width"),
    ) as usize;
    if length == 0 || length > MAX_PROTECTED_LEN || HEADER_LEN + length != envelope.len() {
        return Err(ProtectionError::InvalidEnvelope);
    }
    Ok(&envelope[HEADER_LEN..])
}

#[cfg(windows)]
fn protect_platform(plaintext: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plaintext.len()).map_err(|_| ProtectionError::TooLarge)?,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: APP_ENTROPY.len() as u32,
        pbData: APP_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            Some(&entropy as *const _),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    let _output_guard = unsafe { DpapiBufferGuard::new(output.pbData, output.cbData as usize) };
    if result.is_err() {
        return Err(ProtectionError::Platform);
    }
    if output.pbData.is_null() || output.cbData == 0 || output.cbData as usize > MAX_PROTECTED_LEN {
        return Err(ProtectionError::Platform);
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    Ok(bytes.to_vec())
}

#[cfg(windows)]
fn unprotect_platform(protected: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(protected.len()).map_err(|_| ProtectionError::TooLarge)?,
        pbData: protected.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: APP_ENTROPY.len() as u32,
        pbData: APP_ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy as *const _),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    let _output_guard = unsafe { DpapiBufferGuard::new(output.pbData, output.cbData as usize) };
    if result.is_err() {
        return Err(ProtectionError::Platform);
    }
    if output.pbData.is_null() || output.cbData == 0 || output.cbData as usize > MAX_PLAINTEXT_LEN {
        return Err(ProtectionError::Platform);
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    Ok(bytes.to_vec())
}

#[cfg(windows)]
struct DpapiBufferGuard {
    pointer: *mut u8,
    length: usize,
    #[cfg(test)]
    before_free: Option<Box<dyn FnOnce(&[u8])>>,
}

#[cfg(windows)]
impl DpapiBufferGuard {
    /// Takes ownership of the full allocation returned by DPAPI.
    ///
    /// # Safety
    /// A non-null pointer must own `length` initialized bytes allocated by the
    /// Windows local allocator, with no other owner or outstanding references.
    unsafe fn new(pointer: *mut u8, length: usize) -> Self {
        Self {
            pointer,
            length,
            #[cfg(test)]
            before_free: None,
        }
    }
}

#[cfg(windows)]
impl Drop for DpapiBufferGuard {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            if self.length != 0 {
                let bytes = unsafe { std::slice::from_raw_parts_mut(self.pointer, self.length) };
                wipe(bytes);
                #[cfg(test)]
                if let Some(before_free) = self.before_free.take() {
                    before_free(bytes);
                }
            }
            use windows::Win32::Foundation::{HLOCAL, LocalFree};
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.pointer.cast())));
            }
        }
    }
}

#[cfg(all(not(windows), not(test)))]
fn protect_platform(_plaintext: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    Err(ProtectionError::Unavailable)
}

#[cfg(all(not(windows), not(test)))]
fn unprotect_platform(_protected: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    Err(ProtectionError::Unavailable)
}

#[cfg(all(not(windows), test))]
fn protect_platform(plaintext: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    Ok(plaintext
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ APP_ENTROPY[index % APP_ENTROPY.len()] ^ 0xA5)
        .collect())
}

#[cfg(all(not(windows), test))]
fn unprotect_platform(protected: &[u8]) -> Result<Vec<u8>, ProtectionError> {
    Ok(protected
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ APP_ENTROPY[index % APP_ENTROPY.len()] ^ 0xA5)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_without_plaintext_in_storage() {
        let plaintext = br#"{"murglar":"secret-token"}"#;
        let protected = protect(plaintext).unwrap();
        assert!(
            !protected
                .windows(b"secret-token".len())
                .any(|window| window == b"secret-token")
        );
        assert!(serde_json::from_slice::<serde_json::Value>(&protected).is_err());
        let mut decoded = unprotect(&protected).unwrap();
        assert_eq!(decoded, plaintext);
        wipe(&mut decoded);
    }

    #[test]
    fn envelope_rejects_bad_magic_version_length_and_bounds() {
        let plaintext = b"session";
        let protected = protect(plaintext).unwrap();

        for invalid in [
            {
                let mut bytes = protected.clone();
                bytes[0] ^= 1;
                bytes
            },
            {
                let mut bytes = protected.clone();
                bytes[MAGIC.len()] = 2;
                bytes
            },
            {
                let mut bytes = protected.clone();
                bytes.pop();
                bytes
            },
        ] {
            assert_eq!(unprotect(&invalid), Err(ProtectionError::InvalidEnvelope));
        }
        assert_eq!(
            protect(&vec![0; MAX_PLAINTEXT_LEN + 1]),
            Err(ProtectionError::TooLarge)
        );
        let payload_len = MAX_PROTECTED_LEN + 1;
        let mut oversized = Vec::with_capacity(HEADER_LEN + payload_len);
        oversized.extend_from_slice(MAGIC);
        oversized.extend_from_slice(&VERSION.to_le_bytes());
        oversized.extend_from_slice(&0_u16.to_le_bytes());
        oversized.extend_from_slice(&(payload_len as u32).to_le_bytes());
        oversized.resize(HEADER_LEN + payload_len, 0xA5);
        assert_eq!(unprotect(&oversized), Err(ProtectionError::InvalidEnvelope));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_guard_wipes_full_oversized_allocation_before_free() {
        use std::{cell::Cell, rc::Rc};
        use windows::Win32::System::Memory::{LMEM_FIXED, LocalAlloc};

        let length = MAX_PROTECTED_LEN + 257;
        let allocation = unsafe { LocalAlloc(LMEM_FIXED, length) }.unwrap();
        // The guard immediately becomes the sole owner of this LocalAlloc block.
        let mut guard = unsafe { DpapiBufferGuard::new(allocation.0.cast(), length) };
        unsafe { std::slice::from_raw_parts_mut(guard.pointer, length) }.fill(0xA5);
        let observed = Rc::new(Cell::new(false));
        let result = Rc::clone(&observed);
        guard.before_free = Some(Box::new(move |bytes| {
            // Record the observation without panicking before LocalFree runs.
            result.set(bytes.len() == length && bytes.iter().all(|byte| *byte == 0));
        }));

        drop(guard);

        assert!(
            observed.get(),
            "the full returned allocation must be wiped before freeing"
        );
    }
}
