//! 坚果云应用专用密码的本机保护（AGENTS.md 不可逆决定 #1、DESIGN §4.1）。
//!
//! 密码用 Windows DPAPI 按**当前用户**加封，只落在 `%LOCALAPPDATA%\Perch\credential.bin`。
//! 换个用户、或把文件拷到别的机器上，DPAPI 自己就解不开——这不是我们实现的密码学。
//!
//! 明文只在发起请求前的那一瞬出现：不参与同步、不进 `settings.json`、不写日志、不导出。
//! 同步内容本身是明文（用户明确决定），所以界面上也不说「已加密」。

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

const CREDENTIAL_FILE: &str = "credential.bin";

pub fn path_of(root: &Path) -> PathBuf {
    root.join(CREDENTIAL_FILE)
}

/// 加封。返回的密文只有同一用户在同一台机器上能解。
pub fn protect(plain: &str) -> Result<Vec<u8>, String> {
    seal(plain.as_bytes(), Protector::Protect)
}

/// 解封。密文被截断、篡改或来自别的用户/机器时返回错误，不会 panic。
pub fn unprotect(sealed: &[u8]) -> Result<String, String> {
    if sealed.is_empty() {
        return Err("凭据文件是空的，请回设置页重新填应用专用密码".to_string());
    }
    let plain = seal(sealed, Protector::Unprotect)?;
    String::from_utf8(plain).map_err(|_| "凭据读出来了，但不是一段文字".to_string())
}

#[derive(Clone, Copy)]
enum Protector {
    Protect,
    Unprotect,
}

fn seal(input: &[u8], direction: Protector) -> Result<Vec<u8>, String> {
    let data_in = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut data_out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // 一律禁止 DPAPI 弹任何界面：同步跑在后台线程上，一个看不见的对话框会把它挂死。
    let flags = CRYPTPROTECT_UI_FORBIDDEN;
    let ok = unsafe {
        match direction {
            Protector::Protect => CryptProtectData(
                &data_in,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                flags,
                &mut data_out,
            ),
            Protector::Unprotect => CryptUnprotectData(
                &data_in,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                flags,
                &mut data_out,
            ),
        }
    };
    if ok == 0 {
        // DPAPI 的最后一个错误码只说「不给」或「解不开」，不含任何明文。
        return Err(format!(
            "DPAPI {}失败：{}",
            match direction {
                Protector::Protect => "加封",
                Protector::Unprotect => "解封",
            },
            std::io::Error::last_os_error()
        ));
    }
    // 输出缓冲区由 DPAPI 用 LocalAlloc 分配，必须还给系统；cbData 为 0 时指针可能是空的。
    let bytes = if data_out.cbData == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data_out.pbData, data_out.cbData as usize).to_vec() }
    };
    unsafe { LocalFree(data_out.pbData as *mut c_void) };
    Ok(bytes)
}

/// 存密码：先写临时文件再一步换上去，写到一半断电旧的那份还在。
pub fn save(root: &Path, password: &str) -> Result<(), String> {
    let sealed = protect(password)?;
    fs::create_dir_all(root).map_err(|err| format!("创建 {} 失败: {err}", root.display()))?;
    let path = path_of(root);
    let tmp = root.join(format!("{CREDENTIAL_FILE}.tmp"));
    fs::write(&tmp, &sealed).map_err(|err| format!("写 {} 失败: {err}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("替换 {} 失败: {err}", path.display())
    })
}

/// 读密码。还没存过返回 `Ok(None)`：那是「没配同步」，不是故障。
pub fn load(root: &Path) -> Result<Option<String>, String> {
    match fs::read(path_of(root)) {
        Ok(bytes) => unprotect(&bytes).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("读凭据文件失败: {err}")),
    }
}

/// 忘掉密码（设置页里清空口令时用）。本来就没有也算成功。
pub fn forget(root: &Path) -> Result<(), String> {
    match fs::remove_file(path_of(root)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("删凭据文件失败: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("perch-cred-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_password_survives_the_seal_and_unseal_round_trip() {
        let sealed = protect("abcd-efgh-ijkl-mnop").unwrap();
        assert_eq!(unprotect(&sealed).unwrap(), "abcd-efgh-ijkl-mnop");
    }

    /// 中文口令也要能往返：坚果云应用专用密码是 ASCII，但别在这里埋一个「只有 ASCII 能存」的坑。
    #[test]
    fn a_non_ascii_secret_round_trips_too() {
        let sealed = protect("口令 with spaces").unwrap();
        assert_eq!(unprotect(&sealed).unwrap(), "口令 with spaces");
    }

    /// 密文被改一个字节就必须解不开，而不是解出一段乱码继续发请求。
    #[test]
    fn a_tampered_blob_is_refused() {
        let mut sealed = protect("secret").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(unprotect(&sealed).is_err());
    }

    /// 空文件是「没配」之外的一种坏文件，要说清楚而不是当成空密码。
    #[test]
    fn an_empty_file_is_an_error_not_an_empty_password() {
        assert!(unprotect(&[]).is_err());
    }

    /// 落盘的密文里不许出现口令原文——这是「只在本机保护」的最低要求。
    #[test]
    fn the_file_on_disk_never_contains_the_password() {
        let dir = temp_dir("disk");
        save(&dir, "plain-text-should-not-appear").unwrap();
        let bytes = std::fs::read(path_of(&dir)).unwrap();
        let needle = b"plain-text-should-not-appear";
        assert!(
            !bytes.windows(needle.len()).any(|window| window == needle),
            "凭据文件里出现了口令原文"
        );
        assert_eq!(
            load(&dir).unwrap().as_deref(),
            Some("plain-text-should-not-appear")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_means_not_configured() {
        let dir = temp_dir("missing");
        assert_eq!(load(&dir).unwrap(), None);
        // 删一个本来就没有的文件也算成功，设置页里「清空口令」不该报错。
        forget(&dir).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_again_replaces_the_old_password() {
        let dir = temp_dir("replace");
        save(&dir, "first").unwrap();
        save(&dir, "second").unwrap();
        assert_eq!(load(&dir).unwrap().as_deref(), Some("second"));
        assert!(!dir.join("credential.bin.tmp").exists(), "临时文件没清掉");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forgetting_removes_the_password() {
        let dir = temp_dir("forget");
        save(&dir, "secret").unwrap();
        forget(&dir).unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
