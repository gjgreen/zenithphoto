# Building ZenithPhoto on Windows

This document explains how to set up a Windows development environment and build a native `zenithphoto.exe` binary.

---

## 1. Install Rust (MSVC Toolchain)

Download Rustup for Windows:

[https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)

When prompted, choose the default:

* **Host triple:** `x86_64-pc-windows-msvc`
* **Toolchain:** stable

---

## 2. Install Microsoft Build Tools

ZenithPhoto requires the MSVC toolchain for linking.

Install **Visual Studio Build Tools**:
[https://visualstudio.microsoft.com/visual-cpp-build-tools/](https://visualstudio.microsoft.com/visual-cpp-build-tools/)

During installation, enable:

* **MSVC v143 or newer**
* **Windows 10 or 11 SDK**
* (Optional) CMake tools for Windows

---

## 3. Install CMake

Slint uses CMake as part of its build process.

Download:
[https://cmake.org/download/](https://cmake.org/download/)

Ensure `cmake.exe` is added to your PATH.

---

## 4. Install Ninja (Optional but Recommended)

Ninja improves build performance.

Download:
[https://github.com/ninja-build/ninja/releases](https://github.com/ninja-build/ninja/releases)

Place `ninja.exe` somewhere on your PATH.

---

## 5. (Optional) Install Slint Viewer

Useful for previewing UI files.

```bash
cargo install slint-viewer
```

---

## 6. Verify Your Environment

Open a new terminal and run:

```bash
rustc --version
cargo --version
cl
cmake --version
```

If `cl` is not found, launch:

* **Developer Command Prompt for VS**, or
* Add MSVC paths manually.

---

## 7. Build ZenithPhoto

With dependencies installed, build the app:

```bash
cargo build --release
```

The generated executable will be located at:

```
target/release/zenithphoto.exe
```

---

## 8. Notes on SQLite

ZenithPhoto uses SQLite via Rust.

If using the `bundled` feature:

```toml
rusqlite = { version = "0.30", features = ["bundled"] }
```

No system SQLite installation is necessary.

---

## 9. Notes on TLS/Network Dependencies

To avoid OpenSSL issues on Windows, use Rustls:

```toml
reqwest = { version = "0.12", features = ["json", "blocking", "rustls-tls"] }
```

---

## 10. Summary

To build ZenithPhoto on Windows, you need:

* Rust (MSVC)
* Visual Studio Build Tools (C++ workload + Windows SDK)
* CMake
* (Optional) Ninja

After installation, simply run:

```bash
cargo build --release
```

You're ready to develop and package ZenithPhoto for Windows!
