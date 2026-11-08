use crate::config::*;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Clone)]
pub struct UpdFile {
    pub url: String,
    pub sha256: String,
    pub sig: String, // base64(ed25519(sha256 бинарника))
}

#[derive(Deserialize, Clone)]
pub struct UpdInfo {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub files: HashMap<String, UpdFile>, // windows-amd64, linux-amd64, darwin-arm64...
}

fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        a => a,
    };
    format!("{os}-{arch}")
}

/// a новее b
pub fn newer(a: &str, b: &str) -> bool {
    let p = |s: &str| -> Vec<u32> {
        s.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect()
    };
    let (pa, pb) = (p(a), p(b));
    for i in 0..4 {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn client(secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .map_err(|e| e.to_string())
}

pub fn fetch() -> Result<(UpdInfo, UpdFile), String> {
    let r = client(10)?.get(UPDATE_URL).send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    let mut s = String::new();
    r.take(1 << 16).read_to_string(&mut s).map_err(|e| e.to_string())?;
    let info: UpdInfo = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    let f = info.files.get(&platform()).cloned().ok_or("нет сборки для этой платформы")?;
    Ok((info, f))
}

fn download_verify(f: &UpdFile, dst: &std::path::Path) -> Result<(), String> {
    let r = client(300)?.get(&f.url).send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("загрузка: HTTP {}", r.status().as_u16()));
    }
    let mut out = std::fs::File::create(dst).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut r = r.take(200 << 20);
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    drop(out);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o755));
    }
    let sum = h.finalize();
    if !hex::encode(sum).eq_ignore_ascii_case(&f.sha256) {
        return Err("контрольная сумма не совпала".into());
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let sig = b64.decode(&f.sig).map_err(|_| "подпись обновления неверна")?;
    let pubk = b64.decode(UPDATE_PUBKEY).map_err(|_| "подпись обновления неверна")?;
    let pubk: [u8; 32] = pubk.try_into().map_err(|_| "подпись обновления неверна")?;
    let vk = VerifyingKey::from_bytes(&pubk).map_err(|_| "подпись обновления неверна")?;
    let sig = Signature::from_slice(&sig).map_err(|_| "подпись обновления неверна")?;
    vk.verify(sum.as_slice(), &sig).map_err(|_| "подпись обновления неверна".to_string())
}

pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
        let _ = std::fs::remove_file(exe.with_extension("new"));
    }
}

/// Скачать, проверить, подменить exe (старый -> .old) и запустить новый.
pub fn install_and_restart(f: &UpdFile) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let (new_p, old_p) = (exe.with_extension("new"), exe.with_extension("old"));
    if let Err(e) = download_verify(f, &new_p) {
        let _ = std::fs::remove_file(&new_p);
        return Err(e);
    }
    let _ = std::fs::remove_file(&old_p);
    if let Err(e) = std::fs::rename(&exe, &old_p) {
        let _ = std::fs::remove_file(&new_p);
        return Err(format!("нет прав на запись в папку программы: {e}"));
    }
    if let Err(e) = std::fs::rename(&new_p, &exe) {
        let _ = std::fs::rename(&old_p, &exe);
        return Err(e.to_string());
    }
    std::process::Command::new(&exe).args(std::env::args().skip(1)).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn versions() {
        assert!(newer("1.0.1", "1.0.0") && newer("v2.0", "1.9.9") && !newer("1.0.0", "1.0.0"));
    }
}
