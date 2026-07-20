// Plix build script.
//
// The runtime library (plixrt) is embedded into the `plix` binary as a
// static library: `libplixrt.a` on unix-gnu targets, `plixrt_embed.lib`
// on windows-msvc. When the user runs `plix build file.px -o app`, we
// generate an object file with Cranelift, extract this archive to a temp
// dir, and invoke the platform linker (cc / link.exe).
//
// Cargo normally produces target/<profile>/libplixrt.a (plixrt.lib on
// msvc) automatically because the plixrt crate declares
// crate-type = ["staticlib", "rlib"]. As a fallback (and whenever that
// artifact is stale relative to the rt sources — Cargo races rlib vs
// staticlib) we compile rt directly with $RUSTC.
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // per-target artifact names (staticlib naming differs on MSVC)
    let target = env::var("TARGET").unwrap_or_default();
    let msvc = target.contains("msvc");
    let (dest_name, cargo_candidate_name) = if msvc {
        ("plixrt_embed.lib", "plixrt.lib")
    } else {
        ("libplixrt.a", "libplixrt.a")
    };
    let dest = out_dir.join(dest_name);
    println!("cargo:rerun-if-changed=rt/src");
    println!("cargo:rerun-if-changed=rt/Cargo.toml");

    // remember the toolchain version for forge.rust_version() fallbacks
    let rv = env::var("RUSTC")
        .ok()
        .and_then(|r| Command::new(r).arg("--version").output().ok())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rustc (unknown)".to_string());
    println!("cargo:rustc-env=PLIX_RUSTC_VERSION={}", rv);

    // target/<profile> directory: .../build/<pkg>-<hash>/out -> up 3
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .map(|p| p.to_path_buf())
        .expect("cannot resolve target profile dir");
    let candidate = profile_dir.join(cargo_candidate_name);
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let freshest_rt = {
        let mut newest = std::fs::metadata(manifest_dir.join("rt/Cargo.toml"))
            .and_then(|m| m.modified())
            .ok();
        if let Ok(rd) = std::fs::read_dir(manifest_dir.join("rt/src")) {
            for ent in rd.flatten() {
                if ent.path().extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Ok(mt) = ent.metadata().and_then(|m| m.modified()) {
                        newest = Some(match newest {
                            Some(cur) if cur > mt => cur,
                            _ => mt,
                        });
                    }
                }
            }
        }
        newest
    };
    let candidate_fresh = candidate
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .zip(freshest_rt)
        .map(|(c, r)| c >= r)
        .unwrap_or(false);

    // Only reuse Cargo's artifact when it is provably newer than every rt
    // source: the *rlib* and *staticlib* artifacts race each other, and a
    // stale embedded archive silently ships old runtime semantics.
    if candidate.exists() && candidate_fresh {
        std::fs::copy(&candidate, &dest).expect("copy libplixrt.a");
        return;
    }

    // Fallback: direct rustc invocation on self-contained rt sources.
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let rt_lib = PathBuf::from(&manifest).join("rt/src/lib.rs");
    let opt = if env::var("PROFILE").as_deref() == Ok("release") {
        "-O"
    } else {
        "-Copt-level=2"
    };
    let status = Command::new(rustc)
        .args([
            "--edition=2021",
            "--crate-type=staticlib",
            "--crate-name=plixrt",
            opt,
            "-Cdebuginfo=0",
        ])
        .arg(&rt_lib)
        .arg("-o")
        .arg(&dest)
        .status()
        .expect("failed to invoke rustc for plixrt staticlib");
    if !status.success() || !dest.exists() {
        panic!("could not produce libplixrt.a");
    }
}
