//! API Key 보관.
//!
//! 원칙
//! - API Key 는 소스 코드, 설정 파일(settings.json), 로그 어디에도 기록하지 않는다.
//! - 기본은 메모리에만 보관하고, 앱을 닫으면 사라진다.
//! - 사용자가 "이 PC에 기억"을 선택한 경우에만 Windows 자격 증명 관리자에 저장한다
//!   (사용자 계정 단위로 OS 가 암호화해 보관, 저장소/프로젝트 폴더와 무관).
//! - 메모리의 키는 Debug/Display 출력에서 가리고, 버려질 때 0 으로 덮어쓴다.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::CloudProvider;

/// 사용자 API Key. 출력 시 항상 가려진다.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ApiKey(String);

impl ApiKey {
    /// 앞뒤 공백을 제거한다. 빈 값은 None.
    pub fn new(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| Self(trimmed.to_owned()))
    }

    /// HTTP Authorization 헤더 등 실제 전송에만 사용한다.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// 화면 표시용: 앞 4자와 뒤 4자만 보여준다 (예: `AIza…9xQz`).
    pub fn masked(&self) -> String {
        let chars: Vec<char> = self.0.chars().collect();
        if chars.len() <= 10 {
            return "••••".to_owned();
        }
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("Windows 자격 증명 관리자 오류: {0}")]
    Os(String),
    #[error("저장된 API Key 형식이 올바르지 않습니다")]
    Corrupted,
    #[cfg_attr(windows, allow(dead_code))]
    #[error("이 운영체제에서는 키 저장을 지원하지 않습니다")]
    Unsupported,
}

/// 자격 증명 관리자에 표시되는 이름 ("Windows 자격 증명" → "일반 자격 증명").
/// 공급자별로 따로 저장한다.
fn credential_target(provider: CloudProvider) -> &'static str {
    match provider {
        CloudProvider::OpenAi => "MediaTranscriber/OpenAI API Key",
        CloudProvider::Gemini => "MediaTranscriber/Gemini API Key",
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
        CredWriteW,
    };
    use windows::core::{HSTRING, PWSTR};
    use zeroize::Zeroize;

    use super::{ApiKey, SecretError};

    fn is_not_found(err: &windows::core::Error) -> bool {
        err.code() == ERROR_NOT_FOUND.to_hresult()
    }

    pub fn save(target: &str, key: &ApiKey) -> Result<(), SecretError> {
        let target_w = HSTRING::from(target);
        let user_w = HSTRING::from("api-key");
        let mut blob = key.expose().as_bytes().to_vec();
        let blob_len = u32::try_from(blob.len()).map_err(|_| SecretError::Corrupted)?;

        let credential = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_w.as_ptr().cast_mut()),
            CredentialBlobSize: blob_len,
            CredentialBlob: blob.as_mut_ptr(),
            // LOCAL_MACHINE: 현재 사용자 계정 + 이 PC 에서만 유효 (로밍 프로필로 복제되지 않음)
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user_w.as_ptr().cast_mut()),
            ..Default::default()
        };
        // SAFETY: credential 의 모든 포인터는 이 함수가 끝날 때까지 살아 있는 버퍼를 가리킨다.
        let result = unsafe { CredWriteW(&credential, 0) };
        blob.zeroize();
        result.map_err(|e| SecretError::Os(e.message()))
    }

    pub fn load(target: &str) -> Result<Option<ApiKey>, SecretError> {
        let target_w = HSTRING::from(target);
        let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: 성공 시 ptr 은 OS 가 할당한 CREDENTIALW 를 가리키며 CredFree 로 해제한다.
        match unsafe { CredReadW(&target_w, CRED_TYPE_GENERIC, None, &mut ptr) } {
            Ok(()) => {}
            Err(e) if is_not_found(&e) => return Ok(None),
            Err(e) => return Err(SecretError::Os(e.message())),
        }
        if ptr.is_null() {
            return Ok(None);
        }
        // SAFETY: CredReadW 성공 시 유효한 포인터. blob 은 CredentialBlobSize 바이트.
        let result = unsafe {
            let cred = &*ptr;
            let bytes = if cred.CredentialBlob.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
            };
            let mut text = String::from_utf8(bytes.to_vec()).map_err(|_| SecretError::Corrupted);
            let key = text.as_ref().ok().and_then(|t| ApiKey::new(t));
            if let Ok(t) = text.as_mut() {
                t.zeroize();
            }
            text.map(|_| key)
        };
        // SAFETY: CredReadW 가 할당한 버퍼 해제
        unsafe { CredFree(ptr.cast()) };
        result
    }

    pub fn delete(target: &str) -> Result<(), SecretError> {
        let target_w = HSTRING::from(target);
        // SAFETY: 단순 Win32 호출
        match unsafe { CredDeleteW(&target_w, CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(SecretError::Os(e.message())),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{ApiKey, SecretError};
    pub fn save(_: &str, _: &ApiKey) -> Result<(), SecretError> {
        Err(SecretError::Unsupported)
    }
    pub fn load(_: &str) -> Result<Option<ApiKey>, SecretError> {
        Ok(None)
    }
    pub fn delete(_: &str) -> Result<(), SecretError> {
        Ok(())
    }
}

/// 사용자가 "이 PC에 기억"을 선택했을 때만 호출한다.
pub fn save_api_key(provider: CloudProvider, key: &ApiKey) -> Result<(), SecretError> {
    imp::save(credential_target(provider), key)
}

pub fn load_api_key(provider: CloudProvider) -> Result<Option<ApiKey>, SecretError> {
    imp::load(credential_target(provider))
}

pub fn delete_api_key(provider: CloudProvider) -> Result<(), SecretError> {
    imp::delete(credential_target(provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_redacted_in_debug_and_masked() {
        let key = ApiKey::new("  fake-key-ABCDEFGHIJKLMNOP1234  ").expect("key");
        assert_eq!(key.expose(), "fake-key-ABCDEFGHIJKLMNOP1234");
        assert_eq!(format!("{key:?}"), "ApiKey(<redacted>)");
        assert_eq!(key.masked(), "fake…1234");
        assert!(ApiKey::new("   ").is_none());
        assert_eq!(ApiKey::new("short").expect("k").masked(), "••••");
    }

    /// 실제 자격 증명 관리자에 테스트 전용 이름으로 쓰고 읽고 지운다.
    #[cfg(windows)]
    #[test]
    fn credential_manager_roundtrip() {
        let target = format!("MediaTranscriber/test-{}", std::process::id());
        let key = ApiKey::new("fake-test-key-not-real-0000").expect("key");
        imp::save(&target, &key).expect("save");
        let loaded = imp::load(&target).expect("load").expect("exists");
        assert_eq!(loaded.expose(), key.expose());
        imp::delete(&target).expect("delete");
        assert!(imp::load(&target).expect("load").is_none());
        // 없는 항목 삭제도 성공으로 처리
        imp::delete(&target).expect("delete again");
    }
}
