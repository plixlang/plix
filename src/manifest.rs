use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub build_entry: Option<String>,
    pub build_out: Option<String>,
    pub test_paths: Vec<String>,
    pub dependencies: HashMap<String, Dependency>,
}

#[derive(Debug, Clone)]
pub enum Dependency {
    Version(String),
    Git { url: String, tag: Option<String> },
    Path(String),
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
            ("dependencies", _) => {
                let dep = if val.starts_with('{') {
                    parse_dep_map(val, line_no)?
                } else {
                    Dependency::Version(parse_string(val, line_no)?)
                };
                m.dependencies.insert(key.to_string(), dep);
            }
            _ => {}
        }
    }
    Ok(m)
}

fn parse_dep_map(val: &str, line: usize) -> Result<Dependency, String> {
    let inner = val.trim_matches(|c| c == '{' || c == '}').trim();
    let mut map = HashMap::new();
    for pair in inner.split(',') {
        let parts: Vec<&str> = pair.split('=').collect();
        if parts.len() == 2 {
            let k = parts[0].trim();
            let v = parse_string(parts[1].trim(), line)?;
            map.insert(k.to_string(), v);
        }
    }
    if let Some(url) = map.remove("git") {
        Ok(Dependency::Git {
            url,
            tag: map.remove("tag"),
        })
    } else if let Some(path) = map.remove("path") {
        Ok(Dependency::Path(path))
    } else {
        Err(format!("line {}: invalid dependency map", line))
    }
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
        return Err(format!("line {}: expected string literal, got {}", line, v));
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
