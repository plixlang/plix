pub fn connect(_url: &str) -> String { "connected".into() }
pub fn query(sql: &str) -> String { format!("query:{}", sql) }\npub fn connect(url: &str) -> bool { url.len() > 0 }
