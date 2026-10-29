use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use anyhow::{Result, ensure};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

pub const SQLITE: &[u8; 16] = b"SQLite format 3\0";
pub const QQ_MAGIC: &[u8; 15] = b"SQLite header 3";
pub const QQ_PAGE: usize = 8192;
pub const WX_PAGE: usize = 4096;

pub fn digest(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}
pub fn hash_file(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    let mut file = File::open(path)?;
    let mut block = vec![0; 1024 * 1024];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        hash.update(&block[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
pub fn header(path: &Path, size: usize) -> Result<Vec<u8>> {
    let mut out = vec![0; size];
    let mut f = File::open(path)?;
    let n = f.read(&mut out)?;
    out.truncate(n);
    Ok(out)
}
pub fn plaintext(path: &Path) -> Result<bool> {
    Ok(header(path, 16)? == SQLITE)
}

// PCQQ 9.7.25.29417: whole-page little-endian Corrected Block TEA (XXTEA).
// Confirmed against KernelUtil.dll callback RVA 0x64107 and transform RVA 0x90f78.
fn mx(z: u32, y: u32, sum: u32, key: &[u32; 4], p: usize, e: usize) -> u32 {
    ((z >> 5 ^ y << 2).wrapping_add(y >> 3 ^ z << 4))
        ^ ((sum ^ y).wrapping_add(key[(p & 3) ^ e] ^ z))
}
pub fn qq_block(data: &mut [u8], key: &[u8], decrypt: bool) -> Result<()> {
    ensure!(
        key.len() == 16 && data.len() >= 8 && data.len() % 4 == 0,
        "INVALID_CODEC_INPUT: QQ 页或密钥长度不匹配"
    );
    let k: [u32; 4] =
        std::array::from_fn(|i| u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap()));
    let mut v: Vec<u32> = data
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let n = v.len();
    let delta = 0x9e3779b9u32;
    let rounds = 6 + 52 / n as u32;
    if decrypt {
        let mut sum = rounds.wrapping_mul(delta);
        let mut y = v[0];
        while sum != 0 {
            let e = ((sum >> 2) & 3) as usize;
            for p in (1..n).rev() {
                let z = v[p - 1];
                v[p] = v[p].wrapping_sub(mx(z, y, sum, &k, p, e));
                y = v[p];
            }
            let z = v[n - 1];
            v[0] = v[0].wrapping_sub(mx(z, y, sum, &k, 0, e));
            y = v[0];
            sum = sum.wrapping_sub(delta);
        }
    } else {
        let mut sum = 0u32;
        let mut z = v[n - 1];
        for _ in 0..rounds {
            sum = sum.wrapping_add(delta);
            let e = ((sum >> 2) & 3) as usize;
            for p in 0..n - 1 {
                let y = v[p + 1];
                v[p] = v[p].wrapping_add(mx(z, y, sum, &k, p, e));
                z = v[p];
            }
            let y = v[0];
            v[n - 1] = v[n - 1].wrapping_add(mx(z, y, sum, &k, n - 1, e));
            z = v[n - 1];
        }
    }
    for (chunk, value) in data.chunks_exact_mut(4).zip(v) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    Ok(())
}
#[cfg(test)]
pub fn decrypt_qq(input: &Path, output: &Path, key: &[u8]) -> Result<u64> {
    let size = fs::metadata(input)?.len();
    ensure!(
        size >= 1024 + QQ_PAGE as u64 && (size - 1024) % QQ_PAGE as u64 == 0,
        "DATABASE_TRUNCATED: QQ 数据库页不完整"
    );
    let mut source = File::open(input)?;
    let mut ext = vec![0; 1024];
    source.read_exact(&mut ext)?;
    ensure!(
        ext.starts_with(QQ_MAGIC),
        "DATABASE_FORMAT_UNSUPPORTED: QQ 扩展头不匹配"
    );
    let mut target = File::create(output)?;
    let pages = (size - 1024) / QQ_PAGE as u64;
    let mut block = vec![0; QQ_PAGE];
    for page in 1..=pages {
        source.read_exact(&mut block)?;
        qq_block(&mut block, key, true)?;
        if page == 1 {
            ensure!(
                block.starts_with(SQLITE),
                "KEY_MISMATCH: QQ 密钥与数据库不匹配"
            );
        }
        target.write_all(&block)?;
    }
    target.sync_all()?;
    Ok(pages)
}
pub fn decrypt_qq_delta(
    input: &Path,
    output: &Path,
    hashes: &Path,
    key: &[u8],
    previous: Option<(&Path, &Path)>,
) -> Result<u64> {
    use std::io::{Seek, SeekFrom};
    let size = fs::metadata(input)?.len();
    ensure!(
        size >= 1024 + QQ_PAGE as u64 && (size - 1024) % QQ_PAGE as u64 == 0,
        "DATABASE_TRUNCATED: QQ 数据库页不完整"
    );
    let pages = (size - 1024) / QQ_PAGE as u64;
    let mut source = File::open(input)?;
    let mut ext = [0; 1024];
    source.read_exact(&mut ext)?;
    ensure!(ext.starts_with(QQ_MAGIC), "DATABASE_FORMAT_UNSUPPORTED");
    let old_hashes = previous.and_then(|(_, hashes)| fs::read(hashes).ok());
    let reuse = previous.filter(|(cache, _)| cache.is_file() && old_hashes.is_some());
    if let Some((cache, _)) = reuse {
        fs::copy(cache, output)?;
    }
    let mut target = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(reuse.is_none())
        .open(output)?;
    target.set_len(pages * QQ_PAGE as u64)?;
    let mut out_hashes = File::create(hashes)?;
    let mut page = vec![0; QQ_PAGE];
    let mut changed = 0;
    for i in 0..pages as usize {
        source.read_exact(&mut page)?;
        let hash = Sha256::digest(&page);
        out_hashes.write_all(&hash)?;
        let same = reuse.is_some()
            && old_hashes.as_ref().is_some_and(|old| {
                old.get(i * 32..(i + 1) * 32)
                    .is_some_and(|h| h == hash.as_slice())
            });
        if same && i != 0 {
            continue;
        }
        qq_block(&mut page, key, true)?;
        if i == 0 {
            ensure!(
                page.starts_with(SQLITE),
                "KEY_MISMATCH: QQ 密钥与数据库不匹配"
            );
        }
        if !same {
            target.seek(SeekFrom::Start((i * QQ_PAGE) as u64))?;
            target.write_all(&page)?;
            changed += 1;
        }
    }
    out_hashes.sync_all()?;
    target.sync_all()?;
    Ok(changed)
}
pub fn wx_mac_key(key: &[u8], salt: &[u8]) -> Result<[u8; 32]> {
    ensure!(
        key.len() == 32 && salt.len() == 16,
        "KEY_MISMATCH: 微信密钥长度不匹配"
    );
    let salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3a).collect();
    let mut out = [0; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(key, &salt, 2, &mut out);
    Ok(out)
}
pub fn decrypt_wx_page(page: &[u8], number: u32, key: &[u8], mac_key: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        page.len() == WX_PAGE && number > 0,
        "DATABASE_TRUNCATED: 微信数据页不完整"
    );
    let start = if number == 1 { 16 } else { 0 };
    let mut mac =
        Hmac::<Sha512>::new_from_slice(mac_key).map_err(|_| anyhow::anyhow!("KEY_MISMATCH"))?;
    mac.update(&page[start..WX_PAGE - 64]);
    mac.update(&number.to_le_bytes());
    ensure!(
        mac.verify_slice(&page[WX_PAGE - 64..]).is_ok(),
        "PAGE_AUTH_FAILED: 微信数据页 {number} 校验失败"
    );
    let mut body = page[start..WX_PAGE - 80].to_vec();
    let decryptor =
        cbc::Decryptor::<aes::Aes256>::new_from_slices(key, &page[WX_PAGE - 80..WX_PAGE - 64])
            .map_err(|_| anyhow::anyhow!("KEY_MISMATCH"))?;
    decryptor
        .decrypt_padded_mut::<NoPadding>(&mut body)
        .map_err(|_| anyhow::anyhow!("DECRYPT_FAILED: 微信页长度错误"))?;
    let mut result = Vec::with_capacity(WX_PAGE);
    if number == 1 {
        result.extend_from_slice(SQLITE);
    }
    result.extend(body);
    result.resize(WX_PAGE, 0);
    Ok(result)
}
pub fn verify_wx_key(key: &[u8], page: &[u8]) -> bool {
    if page.len() != WX_PAGE {
        return false;
    }
    let Ok(mac) = wx_mac_key(key, &page[..16]) else {
        return false;
    };
    decrypt_wx_page(page, 1, key, &mac).is_ok()
}
pub fn decrypt_wechat(input: &Path, output: &Path, key: &[u8]) -> Result<u64> {
    let size = fs::metadata(input)?.len();
    ensure!(
        size >= WX_PAGE as u64 && size % WX_PAGE as u64 == 0,
        "DATABASE_TRUNCATED: 微信数据库页不完整"
    );
    let mut source = File::open(input)?;
    let mut page = vec![0; WX_PAGE];
    source.read_exact(&mut page)?;
    let mac = wx_mac_key(key, &page[..16])?;
    let first = decrypt_wx_page(&page, 1, key, &mac)?;
    let mut target = File::create(output)?;
    target.write_all(&first)?;
    let pages = size / WX_PAGE as u64;
    ensure!(pages <= u32::MAX as u64, "DATABASE_TOO_LARGE");
    for n in 2..=pages as u32 {
        source.read_exact(&mut page)?;
        target.write_all(&decrypt_wx_page(&page, n, key, &mac)?)?;
    }
    target.sync_all()?;
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qq_roundtrip_different_pages_and_keys() {
        for n in [8, 16, 512, 8192] {
            let mut page: Vec<u8> = (0..n).map(|i| (i * 37 + 19) as u8).collect();
            let plain = page.clone();
            qq_block(&mut page, &[0x42; 16], false).unwrap();
            assert_ne!(plain, page);
            qq_block(&mut page, &[0x42; 16], true).unwrap();
            assert_eq!(plain, page);
        }
    }
    #[test]
    fn invalid_key_and_size() {
        assert!(qq_block(&mut [0; 7], &[0; 16], true).is_err());
        assert!(!verify_wx_key(&[0; 32], &[0; 4096]));
    }
    #[test]
    fn qq_matches_pinned_dll_oracle() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/qq-page.json")).unwrap();
        let cipher = hex::decode(fixture["ciphertext"].as_str().unwrap()).unwrap();
        assert_eq!(digest(&cipher), fixture["cipherSha256"]);
        let plain: Vec<u8> = (0..8192).map(|i| (i * 37 + 19) as u8).collect();
        let mut encrypted = plain.clone();
        qq_block(&mut encrypted, &[0x42; 16], false).unwrap();
        assert_eq!(encrypted, cipher);
        let mut decoded = cipher;
        qq_block(&mut decoded, &[0x42; 16], true).unwrap();
        assert_eq!(decoded, plain);
    }
    #[test]
    fn wechat_matches_independent_pycryptodome_pages() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../test/fixtures/wechat.json")).unwrap();
        for file in fixture["files"].as_array().unwrap() {
            let encrypted = hex::decode(file["encrypted"].as_str().unwrap()).unwrap();
            let plain = hex::decode(file["plain"].as_str().unwrap()).unwrap();
            let key = [0x24; 32];
            let mac = wx_mac_key(&key, &encrypted[..16]).unwrap();
            for (i, page) in encrypted.chunks_exact(4096).enumerate() {
                let decoded = decrypt_wx_page(page, i as u32 + 1, &key, &mac).unwrap();
                assert_eq!(decoded, &plain[i * 4096..(i + 1) * 4096]);
                let mut bad = page.to_vec();
                bad[100] ^= 1;
                assert!(decrypt_wx_page(&bad, i as u32 + 1, &key, &mac).is_err());
            }
        }
    }
    #[test]
    fn qq_delta_matches_full_decode_and_preserves_prior_cache() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../test/fixtures/qq.json")).unwrap();
        let base =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fixture-tests");
        std::fs::create_dir_all(&base).unwrap();
        let dir = base.join(format!(
            "delta-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("input.db");
        let plain = hex::decode(fixture["plain"].as_str().unwrap()).unwrap();
        std::fs::write(
            &input,
            hex::decode(fixture["encrypted"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let full = dir.join("full.db");
        decrypt_qq(&input, &full, &[0x42; 16]).unwrap();
        assert_eq!(std::fs::read(&full).unwrap(), plain);
        let cached = dir.join("cached.db");
        let hashes = dir.join("hashes.bin");
        assert_eq!(
            decrypt_qq_delta(&input, &cached, &hashes, &[0x42; 16], None).unwrap(),
            (plain.len() / 8192) as u64
        );
        let second = dir.join("second.db");
        let second_hashes = dir.join("second-hashes.bin");
        assert_eq!(
            decrypt_qq_delta(
                &input,
                &second,
                &second_hashes,
                &[0x42; 16],
                Some((&cached, &hashes))
            )
            .unwrap(),
            0
        );
        let mut changed = std::fs::read(&input).unwrap();
        let offset = changed.len() - 8192;
        let mut last = changed[offset..].to_vec();
        qq_block(&mut last, &[0x42; 16], true).unwrap();
        last[100] ^= 1;
        let mut expected = plain.clone();
        let n = expected.len();
        expected[n - 8192 + 100] ^= 1;
        qq_block(&mut last, &[0x42; 16], false).unwrap();
        changed[offset..].copy_from_slice(&last);
        std::fs::write(&input, changed).unwrap();
        assert_eq!(
            decrypt_qq_delta(
                &input,
                &second,
                &second_hashes,
                &[0x42; 16],
                Some((&cached, &hashes))
            )
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&second).unwrap(), expected);
        assert_eq!(std::fs::read(&cached).unwrap(), plain);
        assert!(
            decrypt_qq_delta(
                &input,
                &dir.join("bad.db"),
                &dir.join("bad-hashes.bin"),
                &[0; 16],
                None
            )
            .is_err()
        );
        let resolved = std::fs::canonicalize(&dir).unwrap();
        assert!(resolved.starts_with(std::fs::canonicalize(&base).unwrap()));
        std::fs::remove_dir_all(resolved).unwrap();
    }
}
