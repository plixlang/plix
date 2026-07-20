# Installing Plix

Plix is a **single self-contained executable** — the `plix` binary embeds
the entire runtime, so installing it never drags a runtime library onto the
target machine.

## Prebuilt binaries (recommended)

Grab the archive for your platform from the project's Releases page:

| file | platform |
|---|---|
| `plix-<ver>-x86_64-unknown-linux-gnu.tar.gz` | Linux x86-64 (glibc, e.g. Ubuntu 18.04+ / Debian 10+) |
| `plix-<ver>-x86_64-pc-windows-msvc.zip` | Windows 10/11 x64 |
| `plix-<ver>-x86_64-apple-darwin.tar.gz` | macOS (Intel) 11+ |
| `plix-<ver>-aarch64-apple-darwin.tar.gz` | macOS (Apple Silicon M1–M4) |

Linux/macOS: unpack and run the installer:

```sh
tar xzf plix-*.tar.gz
cd plix-*
./install.sh            # installs to /usr/local/bin (PREFIX=/opt ./install.sh to change)
plix --version
```

Windows: unpack the zip and add the folder to your `PATH` (or call
`plix.exe` in place).

### What `plix run` needs at runtime

Nothing beyond the OS C runtime — which is always present. Compiled
programs are the same: truly standalone.

### What `plix build` (native compilation) needs

A C linker on the host:

- **Linux**: `gcc` or `clang` (`sudo apt install build-essential`).
- **macOS**: Xcode Command Line Tools (`xcode-select --install`).
- **Windows**: Visual Studio *Build Tools* (MSVC — run `plix build` from an
  "x64 Native Tools" developer prompt, or with a proper dev environment),
  or MinGW-w64 with `gcc` on `PATH`.

If you only run programs (`plix run`, `plix exec`... — well, `exec` also
compiles), no C toolchain is involved.

### Python integration (optional)

`import py "numpy"`/`import ai` load **libpython at runtime** by dlopen —
no Python at build time, no ABI coupling. Any CPython 3.8–3.13 install is
found automatically (distro paths on Linux, Framework/homebrew paths on
macOS, `python3.dll`/`python3xx.dll` + official-installer locations on
Windows). Override the discovery with:

```sh
export PLIX_PYTHON_LIB=/path/to/libpython3.13.so   # .dylib / .dll too
```

Programs **without** `py`/`ai` imports never touch Python.

## Building from source

Needs [Rust](https://rustup.rs) (stable, 1.75+ — requires the 2021 edition)
and the C linker shown above. Then:

```bash
git clone <repo-url> plix && cd plix
cargo build --release          # ~2-3 minutes
./target/release/plix --version
bash tests/run_all.sh ./target/release/plix    # full dual-mode battery
```

That is the whole dependency list: no C libraries, no Python, no system
packages beyond the compiler toolchain. The same commands work on
Windows (PowerShell, MSVC) and macOS (arm64 and x86_64): both backends of
Plix (interpreter and Cranelift code generator) are portable by
construction — the native code generator emits the host's object format
(ELF / COFF / Mach-O) automatically.

### Installing from source

```bash
install -Dm755 target/release/plix ~/.local/bin/plix     # or /usr/local/bin
```

## Verifying an install

```bash
plix --version                       # plix 0.3.0
plix run examples/typed.px           # interpreter
plix build examples/typed.px -o /tmp/typed && /tmp/typed   # native path
```
