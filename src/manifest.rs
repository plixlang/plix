use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub build_entry: Option<String>,
    pub build_out: Option<String>,
    pub test_paths: Vec<String>,
}

impl Manifest {
    pub fn load_from(path: &Path) -> Result<Manifest, String> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read manifest {}: {}", path.display(), e))?;
        parse_manifest(&src).map_err(|e| format!("{}: {}", path.display(), e))
    }

    pub fn find_from(start: &Path) -> Option<PathBuf> {
        let mut cur = if start.is_file() {
            start.parent()?.to_path_buf()
        } else {
            start.to_path_buf()
        };
        loop {
            let p = cur.join("plix.toml");
            if p.is_file() {
                return Some(p);
            }
            if !cur.pop() {
                return None;
            }
        }
    }
}

fn parse_manifest(src: &str) -> Result<Manifest, String> {
    let mut m = Manifest::default();
    let mut section = String::new();
    for (i, raw) in src.lines().enumerate() {
        let line_no = i + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some(eq) = line.find('=') else {
            return Err(format!("line {}: expected key = value", line_no));
        };
        let key = line[..eq].trim();
        let val = line[eq + 1..].trim();
        match (section.as_str(), key) {
            ("package", "name") => m.package_name = Some(parse_string(val, line_no)?),
            ("package", "version") => m.package_version = Some(parse_string(val, line_no)?),
            ("build", "entry") => m.build_entry = Some(parse_string(val, line_no)?),
            ("build", "out") => m.build_out = Some(parse_string(val, line_no)?),
            ("test", "paths") => m.test_paths = parse_string_array(val, line_no)?,
            _ => {}
        }
    }
    Ok(m)
}

fn strip_comment(s: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '#' {
            return &s[..i];
        }
    }
    s
}

fn parse_string(v: &str, line: usize) -> Result<String, String> {
    let v = v.trim();
    if !(v.starts_with('"') && v.ends_with('"') && v.len() >= 2) {
        return Err(format!("line {}: expected string literal", line));
    }
    let inner = &v[1..v.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let Some(e) = chars.next() else {
                return Err(format!("line {}: bad string escape", line));
            };
            out.push(match e {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '"' => '"',
                '\\' => '\\',
                x => x,
            });
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn parse_string_array(v: &str, line: usize) -> Result<Vec<String>, String> {
    let v = v.trim();
    if !(v.starts_with('[') && v.ends_with(']')) {
        return Err(format!("line {}: expected string array", line));
    }
    let inner = v[1..v.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in inner.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            cur.push(c);
        } else if c == ',' {
            out.push(parse_string(cur.trim(), line)?);
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(parse_string(cur.trim(), line)?);
    }
    Ok(out)
}
