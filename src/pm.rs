use crate::manifest::{Dependency, Manifest};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn install_package(pkg_name: &str) -> Result<(), String> {
    println!("Installing package: {}...", pkg_name);
    // Simple implementation: assume it's a git repo if it contains /
    if pkg_name.contains('/') {
        let url = if pkg_name.starts_with("http") {
            pkg_name.to_string()
        } else {
            format!("https://github.com/{}", pkg_name)
        };
        fetch_git_dep(pkg_name, &url, None)
    } else {
        Err(format!(
            "Unknown package registry for '{}'. Use 'user/repo' for GitHub.",
            pkg_name
        ))
    }
}

#[allow(dead_code)]
pub fn install_all_deps() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mp = Manifest::find_from(&cwd).ok_or("plix.toml not found")?;
    let manifest = Manifest::load_from(&mp)?;

    for (name, dep) in &manifest.dependencies {
        match dep {
            Dependency::Git { url, tag } => {
                fetch_git_dep(name, url, tag.as_deref())?;
            }
            Dependency::Path(p) => {
                println!("Using local path dependency: {} -> {}", name, p);
            }
            Dependency::Version(v) => {
                println!("Version dependency {} = {} (not implemented yet)", name, v);
            }
        }
    }
    Ok(())
}

fn fetch_git_dep(name: &str, url: &str, tag: Option<&str>) -> Result<(), String> {
    let plix_home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let registry_dir = Path::new(&plix_home).join(".plix").join("registry");
    fs::create_dir_all(&registry_dir).map_err(|e| e.to_string())?;

    let target_dir = registry_dir.join(name.replace('/', "_"));
    if target_dir.exists() {
        println!("Package {} already in cache. Updating...", name);
        Command::new("git")
            .arg("-C")
            .arg(&target_dir)
            .arg("pull")
            .output()
            .map_err(|e| e.to_string())?;
    } else {
        println!("Cloning {} from {}...", name, url);
        let mut cmd = Command::new("git");
        cmd.arg("clone").arg(url).arg(&target_dir);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "Failed to clone {}: {}",
                name,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }

    if let Some(t) = tag {
        Command::new("git")
            .arg("-C")
            .arg(&target_dir)
            .arg("checkout")
            .arg(t)
            .output()
            .map_err(|e| e.to_string())?;
    }

    // Link to local project's plix_modules
    let modules_dir = Path::new("plix_modules");
    fs::create_dir_all(&modules_dir).map_err(|e| e.to_string())?;

    // Create a symlink or a proxy file
    // For simplicity in this demo, we'll write a file that imports the library
    let proxy_path = modules_dir.join(format!("{}.px", name.split('/').last().unwrap()));
    let lib_path = target_dir.join("lib.px");
    if !lib_path.exists() {
        // Try src/lib.px
        let alt = target_dir.join("src").join("lib.px");
        if alt.exists() {
            fs::write(
                &proxy_path,
                format!("import \"{}\" as _pkg;", alt.display()),
            )
            .map_err(|e| e.to_string())?;
        }
    } else {
        fs::write(
            &proxy_path,
            format!("import \"{}\" as _pkg;", lib_path.display()),
        )
        .map_err(|e| e.to_string())?;
    }

    println!("Package {} linked to plix_modules.", name);
    Ok(())
}

pub fn init_project(name: &str) -> Result<(), String> {
    let toml_content = format!(
        r#"[package]
name = "{}"
version = "0.1.0"

[build]
entry = "main.px"
out = "{}"

[dependencies]
"#,
        name, name
    );

    fs::write("plix.toml", toml_content).map_err(|e| e.to_string())?;
    fs::write("main.px", "say(\"Hello from Plix 0.9.9!\");\n").map_err(|e| e.to_string())?;

    println!("Initialized new Plix project: {}", name);
    Ok(())
}

pub fn list_packages() -> Result<(), String> {
    let plix_home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let registry_dir = Path::new(&plix_home).join(".plix").join("registry");

    if !registry_dir.exists() {
        println!("No packages installed.");
        return Ok(());
    }

    println!("Global Registry (v0.9.9):");
    for entry in fs::read_dir(registry_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        println!("  - {}", entry.file_name().to_string_lossy());
    }
    Ok(())
}
