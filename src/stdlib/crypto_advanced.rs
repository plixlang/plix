pub fn encrypt(data: &str, key: &str) -> String { format!("encrypted:{}:{}", data, key) }\npub fn decrypt(data: &str, key: &str) -> String { format!("decrypted:{}:{}", data, key) }
