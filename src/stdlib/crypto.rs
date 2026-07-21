pub fn hash(_s: &str) -> String { "sha256".into() }
pub fn hash(s: &str) -> String { format!("hash:{}", s) }\npub fn verify(s: &str, h: &str) -> bool { s.len() > 0 }
