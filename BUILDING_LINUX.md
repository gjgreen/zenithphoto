# Building ZenithPhoto on Linux

This document explains how to set up a Linux development environment and build a native ZenithPhoto executable.

ZenithPhoto is written in **Rust** with a **Slint UI**, uses **SQLite**, and compiles cleanly on all major Linux distributions.

---

# 1. Install Rust

Use **rustup**, the official Rust toolchain installer.

### Ubuntu / Debian

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Fedora / RHEL / CentOS Stream

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Arch Linux

```bash
pacman -S rustup
rustup default stable
```

Make sure Rust is working:

```bash
cargo --version
rustc --version
```

---

# 2. Install Required Build Tools and Dependencies

Slint uses CMake during builds, and some Linux distros require additional system libraries.

## Ubuntu / Debian

```bash
sudo apt update
sudo apt install -y build-essential cmake libxkbcommon-dev libfontconfig1-dev libgl1-mesa-dev libgl1-mesa-glx pkg-config
```

## Fedora

```bash
sudo dnf install -y gcc gcc-c++ cmake fontconfig-devel libxkbcommon-devel mesa-libGL-devel pkgconf-pkg-config
```

## Arch Linux

```bash
sudo pacman -S --needed base-devel cmake fontconfig libxkbcommon mesa
```

If you want to use the Slint viewer:

```bash
cargo install slint-viewer
```

---

# 3. SQLite Notes

ZenithPhoto uses SQLite. With the recommended configuration:

```toml
rusqlite = { version = "0.30", features = ["bundled"] }
```

SQLite will be compiled from source, and **no system SQLite is required**.

If you prefer to use system SQLite, install:

* Ubuntu/Debian: `libsqlite3-dev`
* Fedora: `sqlite-devel`
* Arch: `sqlite`

---

# 4. Build ZenithPhoto

Inside the ZenithPhoto repository root:

```bash
cargo build --release
```

The optimized Linux binary will appear at:

```
target/release/zenithphoto
```

---

# 5. Running ZenithPhoto

Simply run:

```bash
./target/release/zenithphoto
```

If your system uses Wayland and you encounter display issues, try forcing X11:

```bash
export WINIT_UNIX_BACKEND=x11
```

---

# 6. Optional — Install Ninja

Ninja can speed up CMake/Slint builds.

### Ubuntu

```bash
sudo apt install ninja-build
```

### Fedora

```bash
sudo dnf install ninja-build
```

### Arch

```bash
sudo pacman -S ninja
```

---

# 7. Summary

To build ZenithPhoto on Linux, install:

* Rust (via rustup)
* C/C++ toolchain (gcc + build tools)
* CMake
* Fontconfig, XKBcommon, GL libraries
* (Optional) Ninja

Then build:

```bash
cargo build --release
```

ZenithPhoto should now compile and run cleanly on all modern Linux distributions.
