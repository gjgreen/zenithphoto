mod config;
mod import;

slint::include_modules!(); // from build.rs compiled ui/main.slint and catalog_dialog.slint

use anyhow::{anyhow, Context};
use catalog::db::CatalogDb;
use catalog::services::CatalogService;
use catalog::{Catalog, CatalogPath};
use config::ConfigStore;
use engine::ImageEngine;
use import::{
    import_images_with_callbacks, parse_keywords, scan_directory_with_options, CancellationFlag,
    DuplicateStrategy, ImportCallbacks, ImportMethod, ImportProgress, ImportStage, ScanOptions,
};
use rfd::{AsyncFileDialog, FileDialog};
use slint::{Model, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

fn preview_to_pixel_buffer(
    width: u32,
    height: u32,
    data: Vec<u8>,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let dest = buf.make_mut_bytes();
    dest.copy_from_slice(&data);
    buf
}

struct CatalogSession {
    service: CatalogService,
    path: PathBuf,
}

impl CatalogSession {
    fn new(service: CatalogService, path: PathBuf) -> Self {
        Self { service, path }
    }
}

type CatalogState = Rc<RefCell<Option<CatalogSession>>>;

fn main() -> Result<(), slint::PlatformError> {
    let config_store = ConfigStore::load().unwrap_or_else(|err| {
        eprintln!("Failed to load app configuration: {err}");
        ConfigStore::new_default()
    });

    let mut startup_error: Option<String> = None;
    let mut catalog_pair: Option<(CatalogService, PathBuf)> = None;

    if let Some(path) = config_store.last_catalog() {
        match open_catalog_service(&path) {
            Ok((cat, resolved)) => catalog_pair = Some((cat, resolved)),
            Err(err) => {
                eprintln!("Failed to open last catalog from config: {err}");
                startup_error = Some(format!("Failed to open last catalog: {err}"));
            }
        }
    }

    if catalog_pair.is_none() {
        if let Some(path) = Catalog::last_used() {
            match open_catalog_service(&path) {
                Ok((cat, resolved)) => catalog_pair = Some((cat, resolved)),
                Err(err) => {
                    eprintln!("Failed to open cached catalog: {err}");
                    if startup_error.is_none() {
                        startup_error = Some(format!("Failed to open cached catalog: {err}"));
                    }
                }
            }
        }
    }

    if catalog_pair.is_none() {
        catalog_pair = prompt_for_catalog_dialog(startup_error)?;
    }

    let Some((catalog_service, catalog_path)) = catalog_pair else {
        return Ok(());
    };

    if let Err(err) = Catalog::set_last_used(&catalog_path) {
        eprintln!("Failed to persist last catalog path: {err}");
    }

    let recent_snapshot = config_store
        .record_catalog(&catalog_path)
        .unwrap_or_else(|err| {
            eprintln!("Failed to update catalog history: {err}");
            config_store.snapshot()
        });

    let ui = MainWindow::new()?;
    ui.set_current_path(catalog_path.to_string_lossy().to_string().into());

    let ui_weak = ui.as_weak();
    let recent_model = Rc::new(VecModel::<SharedString>::default());
    refresh_recent_model(&recent_model, &recent_snapshot.recent_catalogs);
    ui.set_recent_catalogs(recent_model.clone().into());

    let catalog_state = Rc::new(RefCell::new(Some(CatalogSession::new(
        catalog_service,
        catalog_path.clone(),
    ))));
    let engine = Arc::new(ImageEngine::new());
    let active_import_ui: Rc<RefCell<Option<ImportPhotosScreen>>> = Rc::new(RefCell::new(None));

    {
        let catalog_for_ui = catalog_state.clone();
        let engine = engine.clone();
        let ui_weak = ui_weak.clone();
        ui.on_open_image_request(move || {
            if ui_weak.upgrade().is_none() {
                return;
            }

            let path_opt = FileDialog::new()
                .add_filter(
                    "Images",
                    &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "gif"],
                )
                .pick_file();

            if let Some(path) = path_opt {
                {
                    let catalog_guard = catalog_for_ui.borrow();
                    if let Some(session) = catalog_guard.as_ref() {
                        match session.service.import_image(&path) {
                            Ok(image) => {
                                if let Err(err) =
                                    session.service.generate_thumbnail(image.id, &path)
                                {
                                    eprintln!("Failed to generate thumbnail for preview: {err}");
                                }
                            }
                            Err(err) => {
                                eprintln!("Failed to add image to catalog: {err}");
                            }
                        }
                    } else {
                        return;
                    }
                }

                let path_str = path.to_string_lossy().to_string();
                let engine = engine.clone();
                let ui_weak_inner = ui_weak.clone();

                std::thread::spawn(move || match engine.open_preview(&path, 1024) {
                    Ok(preview) => {
                        let width = preview.width;
                        let height = preview.height;
                        let data = preview.data;
                        slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak_inner.upgrade() {
                                let buf = preview_to_pixel_buffer(width, height, data);
                                ui.set_preview_image(slint::Image::from_rgba8(buf));
                                ui.set_current_path(path_str.clone().into());
                            }
                        })
                        .ok();
                    }
                    Err(err) => {
                        eprintln!("Failed to open preview: {err}");
                    }
                });
            }
        });
    }

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        ui.on_open_catalog_requested(move || {
            spawn_open_catalog_dialog(&ui_weak, &catalog_state, &config_store, &recent_model);
        });
    }

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        ui.on_new_catalog_requested(move || {
            spawn_new_catalog_dialog(&ui_weak, &catalog_state, &config_store, &recent_model);
        });
    }

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        ui.on_open_recent_catalog_requested(move |path| {
            let path_buf = PathBuf::from(path.as_str());
            if let Err(err) = load_catalog_from_path(
                path_buf,
                &ui_weak,
                &catalog_state,
                &config_store,
                &recent_model,
            ) {
                eprintln!("{err}");
            }
        });
    }

    {
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        ui.on_clear_recent_catalogs_requested(move || match config_store.clear_recent_catalogs() {
            Ok(snapshot) => refresh_recent_model(&recent_model, &snapshot.recent_catalogs),
            Err(err) => eprintln!("Failed to clear recent catalogs: {err}"),
        });
    }

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let active_import_ui = active_import_ui.clone();
        ui.on_import_photos_requested(move || {
            launch_import_flow(&ui_weak, &catalog_state, &active_import_ui);
        });
    }

    ui.on_exit_requested(|| {
        slint::quit_event_loop().ok();
    });

    ui.run()
}

fn spawn_open_catalog_dialog(
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &CatalogState,
    config_store: &ConfigStore,
    recent_model: &Rc<VecModel<SharedString>>,
) {
    let ui_weak = ui_weak.clone();
    let catalog_state = catalog_state.clone();
    let config_store = config_store.clone();
    let recent_model = recent_model.clone();

    let _ = slint::spawn_local(async move {
        if let Some(handle) = AsyncFileDialog::new()
            .set_title("Open Catalog")
            .add_filter("SQLite Catalog", &["sqlite", "zenithphotocatalog"])
            .pick_file()
            .await
        {
            let path = handle.path().to_path_buf();
            if let Err(err) =
                load_catalog_from_path(path, &ui_weak, &catalog_state, &config_store, &recent_model)
            {
                eprintln!("{err}");
            }
        }
    });
}

fn spawn_new_catalog_dialog(
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &CatalogState,
    config_store: &ConfigStore,
    recent_model: &Rc<VecModel<SharedString>>,
) {
    let ui_weak = ui_weak.clone();
    let catalog_state = catalog_state.clone();
    let config_store = config_store.clone();
    let recent_model = recent_model.clone();

    let _ = slint::spawn_local(async move {
        if let Some(handle) = AsyncFileDialog::new()
            .set_title("New Catalog")
            .set_file_name("Untitled.sqlite")
            .add_filter("SQLite Catalog", &["sqlite", "zenithphotocatalog"])
            .save_file()
            .await
        {
            let requested_path = handle.path().to_path_buf();
            match create_catalog_service(&requested_path) {
                Ok((catalog, resolved_path)) => {
                    apply_loaded_catalog(
                        catalog,
                        resolved_path,
                        &ui_weak,
                        &catalog_state,
                        &config_store,
                        &recent_model,
                    );
                }
                Err(err) => eprintln!("Failed to create catalog: {err}"),
            }
        }
    });
}

fn load_catalog_from_path(
    selected_path: PathBuf,
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &CatalogState,
    config_store: &ConfigStore,
    recent_model: &Rc<VecModel<SharedString>>,
) -> Result<(), String> {
    let normalized_path = CatalogPath::new(&selected_path).into_path();
    if !normalized_path.exists() {
        return Err(format!(
            "Catalog '{}' does not exist",
            normalized_path.to_string_lossy()
        ));
    }

    let (catalog, resolved_path) = open_catalog_service(&normalized_path)
        .map_err(|err| format!("Unable to open catalog: {err}"))?;
    apply_loaded_catalog(
        catalog,
        resolved_path,
        ui_weak,
        catalog_state,
        config_store,
        recent_model,
    );
    Ok(())
}

fn apply_loaded_catalog(
    catalog: CatalogService,
    path: PathBuf,
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &CatalogState,
    config_store: &ConfigStore,
    recent_model: &Rc<VecModel<SharedString>>,
) {
    *catalog_state.borrow_mut() = Some(CatalogSession::new(catalog, path.clone()));

    if let Err(err) = Catalog::set_last_used(&path) {
        eprintln!("Failed to persist last catalog path: {err}");
    }

    match config_store.record_catalog(&path) {
        Ok(snapshot) => refresh_recent_model(recent_model, &snapshot.recent_catalogs),
        Err(err) => eprintln!("Failed to update catalog history: {err}"),
    }

    let display_path = path.to_string_lossy().to_string();
    slint::invoke_from_event_loop({
        let ui_weak = ui_weak.clone();
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_current_path(display_path.clone().into());
            }
        }
    })
    .ok();
}

struct ImportViewState {
    thumbnails: Rc<VecModel<ImageThumbnail>>,
    selected_paths: Rc<VecModel<SharedString>>,
    errors: Rc<VecModel<SharedString>>,
    directories: Rc<VecModel<SharedString>>,
    scan_cancel: CancellationFlag,
    import_cancel: CancellationFlag,
}

impl ImportViewState {
    fn new() -> Self {
        Self {
            thumbnails: Rc::new(VecModel::default()),
            selected_paths: Rc::new(VecModel::default()),
            errors: Rc::new(VecModel::default()),
            directories: Rc::new(VecModel::default()),
            scan_cancel: CancellationFlag::default(),
            import_cancel: CancellationFlag::default(),
        }
    }

    fn reset_for_scan(&mut self) {
        self.thumbnails.set_vec(Vec::new());
        self.selected_paths.set_vec(Vec::new());
        self.errors.set_vec(Vec::new());
        self.scan_cancel.cancel();
        self.scan_cancel = CancellationFlag::default();
    }
}

fn placeholder_image() -> slint::Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(1, 1);
    buf.make_mut_bytes().fill(0);
    slint::Image::from_rgba8(buf)
}

fn rebuild_selected_paths(
    thumbnails: &Rc<VecModel<ImageThumbnail>>,
    selected: &Rc<VecModel<SharedString>>,
) {
    let mut paths = Vec::new();
    for idx in 0..thumbnails.row_count() {
        if let Some(thumb) = thumbnails.row_data(idx) {
            if thumb.selected {
                paths.push(thumb.path.clone());
            }
        }
    }
    selected.set_vec(paths);
}

fn refresh_directory_model(model: &Rc<VecModel<SharedString>>, base: &Path) {
    let mut entries = vec![SharedString::from(base.to_string_lossy().to_string())];
    if let Ok(read_dir) = fs::read_dir(base) {
        for entry in read_dir.flatten() {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                entries.push(SharedString::from(
                    entry.path().to_string_lossy().to_string(),
                ));
            }
        }
    }
    model.set_vec(entries);
}

fn toggle_thumbnail_selection(state: &Rc<RefCell<ImportViewState>>, path: &str, selected: bool) {
    let guard = state.borrow_mut();
    for idx in 0..guard.thumbnails.row_count() {
        if let Some(mut thumb) = guard.thumbnails.row_data(idx) {
            if thumb.path.as_str() == path {
                thumb.selected = selected;
                guard.thumbnails.set_row_data(idx, thumb);
                break;
            }
        }
    }
    rebuild_selected_paths(&guard.thumbnails, &guard.selected_paths);
}

fn set_all_selection(state: &Rc<RefCell<ImportViewState>>, selected: bool) {
    let guard = state.borrow_mut();
    for idx in 0..guard.thumbnails.row_count() {
        if let Some(mut thumb) = guard.thumbnails.row_data(idx) {
            thumb.selected = selected;
            guard.thumbnails.set_row_data(idx, thumb);
        }
    }
    rebuild_selected_paths(&guard.thumbnails, &guard.selected_paths);
}

fn start_scan_for_directory(
    import_ui: &slint::Weak<ImportPhotosScreen>,
    state: &Rc<RefCell<ImportViewState>>,
    path: PathBuf,
) {
    if !path.exists() {
        if let Some(ui) = import_ui.upgrade() {
            ui.set_status_text("Folder not found".into());
        }
        return;
    }

    {
        let mut guard = state.borrow_mut();
        guard.reset_for_scan();
        refresh_directory_model(&guard.directories, &path);
    }

    if let Some(ui) = import_ui.upgrade() {
        ui.set_selected_directory(path.to_string_lossy().to_string().into());
        ui.set_status_text("Scanning for images…".into());
        ui.set_importing(true);
        ui.set_progress(0.0);
    }

    let cancel_flag = { state.borrow().scan_cancel.clone() };
    let state_for_candidates = state.clone();
    let ui_weak = import_ui.clone();
    let scan_opts = ScanOptions {
        on_candidate: Some(Arc::new(move |candidate| {
            let guard = state_for_candidates.borrow_mut();
            let display_thumb = candidate.thumb.unwrap_or_else(placeholder_image);
            let path_text: SharedString = candidate.path.to_string_lossy().to_string().into();
            guard.thumbnails.push(ImageThumbnail {
                path: path_text.clone(),
                display_thumb,
                selected: true,
            });
            guard.selected_paths.push(path_text);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status_text(format!("Queued {}", candidate.path.display()).into());
            }
        })),
        cancel: cancel_flag.clone(),
    };
    let ui_done = import_ui.clone();
    let state_done = state.clone();
    let _ = slint::spawn_local(async move {
        let result = scan_directory_with_options(&path, scan_opts).await;
        if let Some(ui) = ui_done.upgrade() {
            ui.set_importing(false);
            match result {
                Ok(list) => ui.set_status_text(format!("Found {} photos", list.len()).into()),
                Err(err) => ui.set_status_text(format!("Scan failed: {err}").into()),
            }
        }
        rebuild_selected_paths(
            &state_done.borrow().thumbnails,
            &state_done.borrow().selected_paths,
        );
    });
}

fn stage_label(stage: &ImportStage) -> &'static str {
    match stage {
        ImportStage::Scanning => "Scanning",
        ImportStage::Copying => "Copying",
        ImportStage::Moving => "Moving",
        ImportStage::Cataloging => "Cataloging",
        ImportStage::Thumbnailing => "Thumbnails",
        ImportStage::Keywords => "Keywords",
    }
}

fn begin_import(
    session_path: PathBuf,
    import_ui: &slint::Weak<ImportPhotosScreen>,
    state: Rc<RefCell<ImportViewState>>,
    active_import: Rc<RefCell<Option<ImportPhotosScreen>>>,
    file_paths: Vec<PathBuf>,
    keywords: Vec<String>,
    method: ImportMethod,
    destination_directory: Option<PathBuf>,
    allow_duplicates: bool,
) {
    if file_paths.is_empty() {
        if let Some(ui) = import_ui.upgrade() {
            ui.set_status_text("Select at least one photo to import".into());
        }
        return;
    }

    if matches!(method, ImportMethod::Copy | ImportMethod::Move) && destination_directory.is_none()
    {
        if let Some(ui) = import_ui.upgrade() {
            ui.set_status_text("Choose a destination for Copy/Move imports".into());
        }
        return;
    }

    let cancel_flag = {
        let mut guard = state.borrow_mut();
        guard.import_cancel.cancel();
        guard.import_cancel = CancellationFlag::default();
        guard.import_cancel.clone()
    };

    if let Some(ui) = import_ui.upgrade() {
        ui.set_importing(true);
        ui.set_progress(0.0);
        ui.set_status_text("Importing…".into());
    }

    let duplicate_strategy = if allow_duplicates {
        DuplicateStrategy::ImportAnyway
    } else {
        DuplicateStrategy::Skip
    };

    let errors_model = state.borrow().errors.clone();
    let ui_for_progress = import_ui.clone();
    let state_for_progress = state.clone();
    let active_import = active_import.clone();

    let progress_cb = Arc::new(move |progress: ImportProgress| {
        if let Some(ui) = ui_for_progress.upgrade() {
            let total = progress.total.max(1);
            let pct = (progress.completed as f32) / (total as f32);
            let label = progress
                .message
                .as_deref()
                .unwrap_or(stage_label(&progress.stage));
            ui.set_progress(pct);
            ui.set_status_text(
                format!(
                    "{} ({}/{})",
                    label,
                    progress.completed,
                    progress.total
                )
                .into(),
            );
        }
    });

    let error_ui = import_ui.clone();
    let on_error_cb = Arc::new(move |path: PathBuf, msg: String| {
        let msg_clone = msg.clone();
        let error_text = SharedString::from(format!("{}: {}", path.to_string_lossy(), msg_clone));
        errors_model.push(error_text);
        if let Some(ui) = error_ui.upgrade() {
            ui.set_status_text(format!("Error: {msg_clone}").into());
        }
    });

    let callbacks = ImportCallbacks {
        progress: Some(progress_cb),
        on_error: Some(on_error_cb),
        duplicate_strategy,
        cancel: cancel_flag.clone(),
    };

    let ui_done = import_ui.clone();
    let _ = slint::spawn_local(async move {
        let db_path = session_path.to_string_lossy().to_string();
        let service = match CatalogDb::open(&db_path).map(CatalogService::new) {
            Ok(service) => service,
            Err(err) => {
                if let Some(ui) = ui_done.upgrade() {
                    ui.set_importing(false);
                    ui.set_status_text(format!("Unable to open catalog: {err}").into());
                }
                return;
            }
        };

        let result = import_images_with_callbacks(
            &service,
            &file_paths,
            &keywords,
            method,
            destination_directory.clone(),
            callbacks,
        )
        .await;

        if let Some(ui) = ui_done.upgrade() {
            ui.set_importing(false);
            match result {
                Ok(report) => {
                    let mut summary = format!("Imported {}", report.imported);
                    if !report.duplicates.is_empty() {
                        summary.push_str(&format!(", skipped {}", report.duplicates.len()));
                    }
                    if !report.failed.is_empty() {
                        summary.push_str(&format!(", {} failed", report.failed.len()));
                        for (path, msg) in report.failed {
                            state_for_progress
                                .borrow()
                                .errors
                                .push(format!("{}: {}", path.to_string_lossy(), msg).into());
                        }
                    }
                    if report.canceled {
                        summary.push_str(" (canceled)");
                    }
                    ui.set_status_text(summary.clone().into());
                    ui.set_progress(1.0);
                    ui.hide().ok();
                    active_import.borrow_mut().take();
                }
                Err(err) => {
                    ui.set_status_text(format!("Import failed: {err}").into());
                    state_for_progress
                        .borrow()
                        .errors
                        .push(format!("Import failed: {err}").into());
                }
            }
        }
    });
}

fn launch_import_flow(
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &CatalogState,
    active_import: &Rc<RefCell<Option<ImportPhotosScreen>>>,
) {
    let catalog_path = {
        let guard = catalog_state.borrow();
        let Some(session) = guard.as_ref() else {
            eprintln!("No catalog open; cannot import photos");
            return;
        };
        session.path.clone()
    };

    let import_ui = match ImportPhotosScreen::new() {
        Ok(ui) => ui,
        Err(err) => {
            eprintln!("Failed to open import window: {err}");
            return;
        }
    };

    import_ui.set_importing(false);
    import_ui.set_progress(0.0);
    import_ui.set_status_text("Select a folder to begin".into());
    import_ui.set_keywords("".into());
    import_ui.set_destination_directory("".into());
    import_ui.set_allow_duplicates(false);

    let view_state = Rc::new(RefCell::new(ImportViewState::new()));
    {
        let guard = view_state.borrow();
        import_ui.set_thumbnails(guard.thumbnails.clone().into());
        import_ui.set_selected_paths(guard.selected_paths.clone().into());
        import_ui.set_error_messages(guard.errors.clone().into());
        import_ui.set_directories(guard.directories.clone().into());
    }

    let import_ui_weak = import_ui.as_weak();

    // Prefill with the user's home directory for convenience.
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        refresh_directory_model(&view_state.borrow().directories, &home);
        import_ui.set_selected_directory(home.to_string_lossy().to_string().into());
    }

    {
        let import_ui_weak = import_ui_weak.clone();
        let view_state = view_state.clone();
        import_ui.on_browse_for_directory(move || {
            let import_ui_weak = import_ui_weak.clone();
            let view_state = view_state.clone();
            let _ = slint::spawn_local(async move {
                if let Some(handle) = AsyncFileDialog::new().pick_folder().await {
                    start_scan_for_directory(
                        &import_ui_weak,
                        &view_state,
                        handle.path().to_path_buf(),
                    );
                }
            });
        });
    }

    {
        let import_ui_weak = import_ui_weak.clone();
        let view_state = view_state.clone();
        import_ui.on_directory_selected(move |path| {
            start_scan_for_directory(&import_ui_weak, &view_state, PathBuf::from(path.as_str()));
        });
    }

    {
        let import_ui_weak = import_ui_weak.clone();
        import_ui.on_browse_for_destination(move || {
            let import_ui_weak = import_ui_weak.clone();
            let _ = slint::spawn_local(async move {
                if let Some(handle) = AsyncFileDialog::new().pick_folder().await {
                    if let Some(ui) = import_ui_weak.upgrade() {
                        ui.set_destination_directory(
                            handle.path().to_string_lossy().to_string().into(),
                        );
                    }
                }
            });
        });
    }

    {
        let view_state = view_state.clone();
        import_ui.on_select_all_requested(move |all_selected| {
            set_all_selection(&view_state, all_selected);
        });
    }

    {
        let view_state = view_state.clone();
        import_ui.on_thumbnail_toggled(move |path, selected| {
            toggle_thumbnail_selection(&view_state, &path, selected);
        });
    }

    {
        let import_ui_weak = import_ui_weak.clone();
        let view_state = view_state.clone();
        let active_import = active_import.clone();
        import_ui.on_cancel_import(move || {
            {
                let guard = view_state.borrow();
                guard.scan_cancel.cancel();
                guard.import_cancel.cancel();
            }
            if let Some(ui) = import_ui_weak.upgrade() {
                ui.hide().ok();
            }
            active_import.borrow_mut().take();
        });
    }

    {
        let import_ui_weak = import_ui_weak.clone();
        let view_state = view_state.clone();
        let active_import = active_import.clone();
        let catalog_path = catalog_path.clone();
        import_ui.on_perform_import(
            move |_source_dir, destination_dir, paths, keyword_text, method_str| {
                let allow_duplicates = import_ui_weak
                    .upgrade()
                    .map(|ui| ui.get_allow_duplicates())
                    .unwrap_or(false);
                let files: Vec<PathBuf> = paths.iter().map(|p| PathBuf::from(p.as_str())).collect();
                let keywords = parse_keywords(keyword_text.as_str());
                let method = match method_str.as_str() {
                    "Copy" => ImportMethod::Copy,
                    "Move" => ImportMethod::Move,
                    _ => ImportMethod::Add,
                };
                let destination = match method {
                    ImportMethod::Copy | ImportMethod::Move => {
                        if destination_dir.is_empty() {
                            None
                        } else {
                            Some(PathBuf::from(destination_dir.as_str()))
                        }
                    }
                    ImportMethod::Add => None,
                };
                begin_import(
                    catalog_path.clone(),
                    &import_ui_weak,
                    view_state.clone(),
                    active_import.clone(),
                    files,
                    keywords,
                    method,
                    destination,
                    allow_duplicates,
                );
            },
        );
    }

    if let Some(ui) = ui_weak.upgrade() {
        ui.set_status_text("Import window opened".into());
    }

    import_ui.show().ok();
    *active_import.borrow_mut() = Some(import_ui);
}

fn refresh_recent_model(model: &Rc<VecModel<SharedString>>, entries: &[PathBuf]) {
    let data: Vec<SharedString> = entries
        .iter()
        .map(|path| SharedString::from(path.to_string_lossy().to_string()))
        .collect();
    model.set_vec(data);
}

fn open_catalog_service(path: &Path) -> anyhow::Result<(CatalogService, PathBuf)> {
    let normalized_path = CatalogPath::new(path).into_path();
    let db_path = normalized_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid catalog path"))?;
    let db = CatalogDb::open(db_path)?;
    Ok((CatalogService::new(db), normalized_path))
}

fn create_catalog_service(path: &Path) -> anyhow::Result<(CatalogService, PathBuf)> {
    let normalized_path = CatalogPath::new(path).into_path();
    if let Some(parent) = normalized_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create catalog directory {}", parent.display()))?;
    }

    open_catalog_service(&normalized_path)
}

fn prompt_for_catalog_dialog(
    initial_error: Option<String>,
) -> Result<Option<(CatalogService, PathBuf)>, slint::PlatformError> {
    let dialog = CatalogDialog::new()?;
    if let Some(msg) = initial_error {
        dialog.set_error_text(msg.into());
    }

    let selected: Rc<RefCell<Option<(CatalogService, PathBuf)>>> = Rc::new(RefCell::new(None));

    {
        let dialog_weak = dialog.as_weak();
        let selected = selected.clone();
        dialog.on_open_existing_catalog(move || {
            if let Some(path) = FileDialog::new()
                .add_filter("Zenith Catalog", &["zenithphotocatalog", "sqlite"])
                .pick_file()
            {
                let normalized = CatalogPath::new(&path).into_path();
                match open_catalog_service(&normalized) {
                    Ok((cat, resolved)) => {
                        *selected.borrow_mut() = Some((cat, resolved));
                        slint::quit_event_loop().ok();
                    }
                    Err(err) => {
                        if let Some(dialog) = dialog_weak.upgrade() {
                            dialog.set_error_text(format!("Unable to open catalog: {err}").into());
                        }
                    }
                }
            }
        });
    }

    {
        let dialog_weak = dialog.as_weak();
        let selected = selected.clone();
        dialog.on_create_new_catalog(move || {
            if let Some(path) = FileDialog::new()
                .set_file_name("Untitled.zenithphotocatalog")
                .add_filter("Zenith Catalog", &["zenithphotocatalog", "sqlite"])
                .save_file()
            {
                match create_catalog_service(&path) {
                    Ok((cat, resolved)) => {
                        *selected.borrow_mut() = Some((cat, resolved));
                        slint::quit_event_loop().ok();
                    }
                    Err(err) => {
                        if let Some(dialog) = dialog_weak.upgrade() {
                            dialog
                                .set_error_text(format!("Unable to create catalog: {err}").into());
                        }
                    }
                }
            }
        });
    }

    dialog.on_quit(|| {
        std::process::exit(0);
    });

    dialog.run()?;

    let result = selected.borrow_mut().take();
    Ok(result)
}
