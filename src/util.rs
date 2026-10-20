use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use rand::seq::SliceRandom;
use rand::RngCore;
use sha2::{Digest, Sha256};

const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Code d'accès lisible, ex. `k7m2-qx9p`.
pub fn gen_code() -> String {
    let mut rng = OsRng;
    let mut chars: Vec<char> = (0..8)
        .map(|_| *CODE_ALPHABET.choose(&mut rng).unwrap() as char)
        .collect();
    chars.insert(4, '-');
    chars.into_iter().collect()
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Comparaison en temps constant.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn hash_password(pw: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .expect("hachage Argon2")
        .to_string()
}

pub fn verify_password(hash: &str, pw: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(pw.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub async fn hash_password_async(pw: String) -> String {
    tokio::task::spawn_blocking(move || hash_password(&pw))
        .await
        .expect("tâche de hachage")
}

pub async fn verify_password_async(hash: String, pw: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&hash, &pw))
        .await
        .unwrap_or(false)
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        let m = match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c if c.is_ascii_alphanumeric() => c,
            _ => continue,
        };
        out.push(m);
    }
    out
}

/// Nettoie un champ texte : retire les espaces superflus et normalise les retours à la ligne.
pub fn clean(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

pub fn fr_date(s: &str) -> String {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() == 3
        && p[0].len() == 4
        && p[1].len() == 2
        && p[2].len() == 2
        && p.iter().all(|x| x.chars().all(|c| c.is_ascii_digit()))
    {
        format!("{}/{}/{}", p[2], p[1], p[0])
    } else {
        s.to_string()
    }
}

/// `2026-10-20T12:30:00Z` → `20/10/2026 à 12:30` (UTC).
pub fn fr_datetime(iso: &str) -> String {
    if iso.len() >= 16 && iso.as_bytes().get(10) == Some(&b'T') {
        format!("{} à {}", fr_date(&iso[..10]), &iso[11..16])
    } else {
        iso.to_string()
    }
}
