use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

pub fn hash(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
}

pub fn verify(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Performs a throwaway Argon2 verification using the same parameters as
/// [`verify`]. Call this on the "user not found" login path so that the
/// response time is comparable to the "wrong password" path, preventing a
/// username-enumeration timing side-channel.
pub fn dummy_verify(password: &str) {
    // A fixed, well-formed PHC string so parsing succeeds and the full Argon2
    // computation runs. The result is intentionally discarded.
    const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$\
        J4moa2MFT8r0m3l0n8Q0m9Y0m9Y0m9Y0m9Y0m9Y0m9Y";
    let _ = verify(password, DUMMY_HASH);
}
