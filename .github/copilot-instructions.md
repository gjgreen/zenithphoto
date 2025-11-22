# ZenithPhoto AI Coding Agent Instructions

## Project Overview
ZenithPhoto is a multi-crate Rust project for photo management and editing. It uses a modular architecture with clear separation between core types, engine logic, and application frontends.

### Major Components
- **core-types/**: Shared data structures and types used across the project.
- **crates/engine/**: Core business logic and photo processing engine.
- **apps/desktop/**: Desktop application, built with Slint for UI. Contains build scripts and UI definitions in `ui/main.slint`.

### Data Flow & Integration
- Data types are defined in `core-types` and imported by both engine and apps.
- The engine crate provides reusable logic, called by the desktop app via Rust module imports.
- UI is defined in Slint (`main.slint`), with Rust glue code in `apps/desktop/src/main.rs`.

## Developer Workflows
- **Build**: Use `cargo build --workspace` from the repo root to build all crates and apps.
- **Run Desktop App**: `cargo run --package desktop` from the repo root.
- **Test Engine**: `cargo test -p engine`.
- **Debugging**: Use standard Rust debugging tools (e.g., `rust-gdb`, `lldb`).
- **UI Changes**: Edit `.slint` files in `apps/desktop/ui/`, then rebuild the desktop app.

## Project-Specific Conventions
- All shared types must be defined in `core-types` and imported via crate dependencies.
- UI logic is separated: Slint files for layout, Rust for event handling.
- Build scripts (`build.rs`) in app directories may perform custom asset or code generation.
- Use workspace-level `Cargo.toml` for dependency management and cross-crate references.

## External Dependencies
- **Slint**: For UI, see `apps/desktop/ui/main.slint` and related Rust bindings.
- **Rust crates**: Managed via Cargo; check each crate's `Cargo.toml` for specifics.

## Patterns & Examples
- To add a new feature, define types in `core-types`, implement logic in `engine`, and expose UI in `apps/desktop/ui/` and `src/main.rs`.
- Cross-crate communication uses Rust's module system and workspace dependencies.

## Key Files & Directories
- `core-types/src/lib.rs`: Shared types
- `crates/engine/src/lib.rs`: Engine logic
- `apps/desktop/src/main.rs`: Desktop app entry point
- `apps/desktop/ui/main.slint`: UI definition

---

**For questions or unclear conventions, ask for clarification or examples from maintainers.**
