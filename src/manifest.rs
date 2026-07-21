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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_manifest_and_quoted_comments() {
        let manifest = parse_manifest(
            r#"
            [package]
            name = "demo # still part of the name"
            version = "1.2.3"
            [build]
            entry = "src/main.px"
            out = "target/demo"
            [test]
            paths = ["tests", "specs/unit"] # ignored comment
            "#,
        )
        .unwrap();
        assert_eq!(
            manifest.package_name.as_deref(),
            Some("demo # still part of the name")
        );
        assert_eq!(manifest.package_version.as_deref(), Some("1.2.3"));
        assert_eq!(manifest.build_entry.as_deref(), Some("src/main.px"));
        assert_eq!(manifest.build_out.as_deref(), Some("target/demo"));
        assert_eq!(manifest.test_paths, vec!["tests", "specs/unit"]);
    }

    #[test]
    fn rejects_non_string_manifest_values() {
        let err = parse_manifest("[package]\nname = unquoted").unwrap_err();
        assert_eq!(err, "line 2: expected string literal");
    }

    #[test]
    fn finds_manifest_from_nested_directory() {
        let root = std::env::temp_dir().join(format!("plix-manifest-test-{}", std::process::id()));
        let nested = root.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("plix.toml"), "[package]\nname = \"demo\"").unwrap();
        assert_eq!(Manifest::find_from(&nested), Some(root.join("plix.toml")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
