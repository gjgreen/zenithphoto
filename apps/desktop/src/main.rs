mod config;
mod import;

slint::include_modules!(); // from build.rs compiled ui/main.slint and catalog_dialog.slint

use anyhow::{anyhow, Context};
use catalog::db::{CatalogDb, Folder, Image as CatalogImage, Thumbnail};
use catalog::services::{CatalogService, Edits};
use catalog::{Catalog, CatalogPath};
use config::ConfigStore;
use engine::ImageEngine;
use import::{
    import_images_with_callbacks, is_already_imported, parse_keywords, scan_directory_with_options,
    CancellationFlag, DuplicateStrategy, ImportCallbacks, ImportMethod, ImportProgress,
    ImportStage, ScanOptions,
};
use rfd::{AsyncFileDialog, FileDialog};
use slint::{Model, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
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

#[derive(Clone)]
struct FilterState {
    search: String,
    rating: i32,
    flag: String,
    color_label: String,
}

struct FolioState {
    folder_tree: Rc<VecModel<FolderNode>>,
    thumbnails: Rc<VecModel<ThumbnailItem>>,
    selection: Vec<i32>,
    selection_anchor: Option<usize>,
    filters: FilterState,
    current_folder: Option<String>,
}

impl FolioState {
    fn new() -> Self {
        Self {
            folder_tree: Rc::new(VecModel::default()),
            thumbnails: Rc::new(VecModel::default()),
            selection: Vec::new(),
            selection_anchor: None,
            filters: FilterState {
                search: String::new(),
                rating: 0,
                flag: String::new(),
                color_label: String::new(),
            },
            current_folder: None,
        }
    }

    fn reset_selection(&mut self) {
        self.selection.clear();
        self.selection_anchor = None;
    }
}

fn empty_metadata() -> ImageMetadata {
    ImageMetadata {
        file_path: "".into(),
        captured_at: "".into(),
        camera: "".into(),
        lens: "".into(),
        focal_length: "".into(),
        aperture: "".into(),
        shutter_speed: "".into(),
        iso: "".into(),
        gps_lat: "".into(),
        gps_lon: "".into(),
        rating: 0,
        flag: "".into(),
        color_label: "".into(),
        keywords: Rc::<VecModel<SharedString>>::default().into(),
    }
}

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
    ui.set_status_text("".into());

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
    let folio_state = Rc::new(RefCell::new(FolioState::new()));

    {
        let folio_guard = folio_state.borrow();
        ui.set_folder_tree(folio_guard.folder_tree.clone().into());
        ui.set_thumbnails(folio_guard.thumbnails.clone().into());
        ui.set_filter_search(folio_guard.filters.search.clone().into());
        ui.set_filter_rating(folio_guard.filters.rating);
        ui.set_filter_flag(folio_guard.filters.flag.clone().into());
        ui.set_filter_color_label(folio_guard.filters.color_label.clone().into());
    }
    ui.set_metadata(empty_metadata());
    ui.set_keywords_text("".into());
    ui.set_folio_size_summary("".into());
    ui.set_selected_image_id(-1);
    ui.set_folio_selected_count(0);
    ui.set_folio_total_count(0);
    ui.set_refine_preview(placeholder_image());
    ui.set_refine_path("".into());
    ui.set_refine_image_id(-1);
    ui.set_refine_exposure(0.0);
    ui.set_refine_contrast(0.0);
    ui.set_refine_highlights(0.0);
    ui.set_refine_shadows(0.0);
    ui.set_refine_whites(0.0);
    ui.set_refine_blacks(0.0);

    refresh_folio_tree(&ui_weak, &catalog_state, &folio_state);

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        ui.on_folder_selected(move |path| {
            let path_buf = PathBuf::from(path.as_str());
            {
                folio_state.borrow_mut().current_folder = Some(path.to_string());
            }
            load_folder_thumbnails(&catalog_state, &folio_state, &ui_weak, &path_buf);
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        let ui_weak = ui_weak.clone();
        ui.on_thumbnail_selected(move |image_id, range_select, toggle| {
            handle_thumbnail_selection(
                image_id,
                range_select,
                toggle,
                &catalog_state,
                &folio_state,
                &ui_weak,
            );
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let ui_weak = ui_weak.clone();
        let engine = engine.clone();
        ui.on_thumbnail_activated(move |image_id| {
            open_refine_screen(&catalog_state, &ui_weak, &engine, image_id);
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        let ui_weak = ui_weak.clone();
        ui.on_rating_changed(move |image_id, rating| {
            if let Err(err) =
                apply_rating_change(&catalog_state, &folio_state, &ui_weak, image_id, rating)
            {
                eprintln!("Failed to update rating: {err}");
            }
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        let ui_weak = ui_weak.clone();
        ui.on_flag_changed(move |image_id, flag| {
            if let Err(err) =
                apply_flag_change(&catalog_state, &folio_state, &ui_weak, image_id, flag.as_str())
            {
                eprintln!("Failed to update flag: {err}");
            }
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        let ui_weak = ui_weak.clone();
        ui.on_label_changed(move |image_id, label| {
            if let Err(err) = apply_color_label_change(
                &catalog_state,
                &folio_state,
                &ui_weak,
                image_id,
                label.as_str(),
            ) {
                eprintln!("Failed to update color label: {err}");
            }
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        let ui_weak = ui_weak.clone();
        ui.on_update_keywords(move |image_id, keywords| {
            if let Err(err) =
                update_keywords(&catalog_state, image_id, parse_keywords(keywords.as_str()))
            {
                eprintln!("Failed to update keywords: {err}");
            } else {
                if let Err(err) =
                    refresh_metadata_panel(&catalog_state, &ui_weak, image_id as i64)
                {
                    eprintln!("Failed to refresh metadata after keywords: {err}");
                }
                if let Some(ui) = ui_weak.upgrade() {
                    if let Some(id) = folio_state.borrow().selection.first() {
                        ui.set_selected_image_id(*id);
                    }
                }
            }
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let folio_state = folio_state.clone();
        let ui_weak = ui_weak.clone();
        ui.on_filters_changed(move |search, rating, flag, color_label| {
            folio_state.borrow_mut().filters = FilterState {
                search: search.to_string(),
                rating,
                flag: flag.to_string(),
                color_label: color_label.to_string(),
            };
            if let Some(folder) = folio_state.borrow().current_folder.clone() {
                load_folder_thumbnails(
                    &catalog_state,
                    &folio_state,
                    &ui_weak,
                    &PathBuf::from(folder),
                );
            }
        });
    }

    {
        let catalog_state = catalog_state.clone();
        let ui_weak = ui_weak.clone();
        let engine = engine.clone();
        ui.on_open_refine(move |image_id| {
            open_refine_screen(&catalog_state, &ui_weak, &engine, image_id);
        });
    }

    {
        let catalog_state = catalog_state.clone();
        ui.on_apply_edits(
            move |image_id, exposure, contrast, highlights, shadows, whites, blacks| {
                if let Err(err) = apply_refine_edits(
                    &catalog_state,
                    image_id,
                    exposure,
                    contrast,
                    highlights,
                    shadows,
                    whites,
                    blacks,
                ) {
                    eprintln!("Failed to save edits: {err}");
                }
            },
        );
    }

    ui.on_back_to_folio(move || {});

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        let folio_state = folio_state.clone();
        ui.on_open_catalog_requested(move || {
            spawn_open_catalog_dialog(
                &ui_weak,
                &catalog_state,
                &config_store,
                &recent_model,
                &folio_state,
            );
        });
    }

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        let folio_state = folio_state.clone();
        ui.on_new_catalog_requested(move || {
            spawn_new_catalog_dialog(
                &ui_weak,
                &catalog_state,
                &config_store,
                &recent_model,
                &folio_state,
            );
        });
    }

    {
        let ui_weak = ui_weak.clone();
        let catalog_state = catalog_state.clone();
        let config_store = config_store.clone();
        let recent_model = recent_model.clone();
        let folio_state = folio_state.clone();
        ui.on_open_recent_catalog_requested(move |path| {
            let path_buf = PathBuf::from(path.as_str());
            if let Err(err) = load_catalog_from_path(
                path_buf,
                &ui_weak,
                &catalog_state,
                &config_store,
                &recent_model,
                &folio_state,
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
    folio_state: &Rc<RefCell<FolioState>>,
) {
    let ui_weak = ui_weak.clone();
    let catalog_state = catalog_state.clone();
    let config_store = config_store.clone();
    let recent_model = recent_model.clone();
    let folio_state = folio_state.clone();

    let _ = slint::spawn_local(async move {
        if let Some(handle) = AsyncFileDialog::new()
            .set_title("Open Catalog")
            .add_filter("SQLite Catalog", &["sqlite", "zenithphotocatalog"])
            .pick_file()
            .await
        {
            let path = handle.path().to_path_buf();
            if let Err(err) = load_catalog_from_path(
                path,
                &ui_weak,
                &catalog_state,
                &config_store,
                &recent_model,
                &folio_state,
            ) {
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
    folio_state: &Rc<RefCell<FolioState>>,
) {
    let ui_weak = ui_weak.clone();
    let catalog_state = catalog_state.clone();
    let config_store = config_store.clone();
    let recent_model = recent_model.clone();
    let folio_state = folio_state.clone();

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
                        &folio_state,
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
    folio_state: &Rc<RefCell<FolioState>>,
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
        folio_state,
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
    folio_state: &Rc<RefCell<FolioState>>,
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

    refresh_folio_tree(ui_weak, catalog_state, folio_state);
}

type ImageId = i32;

/// Keeps UI selection (highlighted thumbnails) independent from checkbox state.
/// The selected_ids set mirrors the visible highlight, while anchor_id preserves
/// the last focused item for range selection.
#[derive(Default)]
struct SelectionState {
    selected_ids: HashSet<ImageId>,
    anchor_id: Option<ImageId>,
}

struct ImportViewState {
    thumbnails: Rc<VecModel<ImportThumbnail>>,
    /// Paths that are currently marked for import (checked=true).
    selected_paths: Rc<VecModel<SharedString>>,
    errors: Rc<VecModel<SharedString>>,
    directories: Rc<VecModel<SharedString>>,
    scan_cancel: CancellationFlag,
    import_cancel: CancellationFlag,
    /// Tracks highlighted selection separately from import checkboxes.
    selection: SelectionState,
    next_image_id: ImageId,
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
            selection: SelectionState::default(),
            next_image_id: 1,
        }
    }

    fn reset_for_scan(&mut self) {
        self.thumbnails.set_vec(Vec::new());
        self.selected_paths.set_vec(Vec::new());
        self.errors.set_vec(Vec::new());
        self.scan_cancel.cancel();
        self.scan_cancel = CancellationFlag::default();
        self.selection = SelectionState::default();
        self.next_image_id = 1;
    }

    fn is_selectable_id(&self, id: ImageId) -> bool {
        self.index_of(id)
            .and_then(|idx| self.thumbnails.row_data(idx))
            .map(|thumb| thumb.selectable)
            .unwrap_or(false)
    }

    fn index_of(&self, id: ImageId) -> Option<usize> {
        for idx in 0..self.thumbnails.row_count() {
            if let Some(thumb) = self.thumbnails.row_data(idx) {
                if thumb.id == id {
                    return Some(idx);
                }
            }
        }
        None
    }

    fn apply_selection_flags(&mut self) {
        // Keep selection in sync with the model and drop anything non-selectable.
        let mut invalid = Vec::new();
        for id in &self.selection.selected_ids {
            if !self.is_selectable_id(*id) {
                invalid.push(*id);
            }
        }
        for id in invalid {
            self.selection.selected_ids.remove(&id);
        }

        if let Some(anchor) = self.selection.anchor_id {
            if !self.is_selectable_id(anchor) {
                self.selection.anchor_id = self.selection.selected_ids.iter().next().copied();
            }
        }

        for idx in 0..self.thumbnails.row_count() {
            if let Some(mut thumb) = self.thumbnails.row_data(idx) {
                let is_selected = self.selection.selected_ids.contains(&thumb.id);
                if thumb.selected != is_selected {
                    thumb.selected = is_selected;
                    self.thumbnails.set_row_data(idx, thumb);
                }
            }
        }
    }

    fn rebuild_checked_paths(&self) {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        for idx in 0..self.thumbnails.row_count() {
            if let Some(thumb) = self.thumbnails.row_data(idx) {
                if thumb.checked && thumb.selectable && seen.insert(thumb.path.clone()) {
                    paths.push(thumb.path);
                }
            }
        }
        self.selected_paths.set_vec(paths);
    }
}

fn placeholder_image() -> slint::Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(1, 1);
    buf.make_mut_bytes().fill(0);
    slint::Image::from_rgba8(buf)
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

fn select_single(state: &Rc<RefCell<ImportViewState>>, id: ImageId) {
    let mut guard = state.borrow_mut();
    if !guard.is_selectable_id(id) {
        return;
    }
    guard.selection.selected_ids.clear();
    guard.selection.selected_ids.insert(id);
    guard.selection.anchor_id = Some(id);
    guard.apply_selection_flags();
}

fn select_range(state: &Rc<RefCell<ImportViewState>>, id: ImageId) {
    let mut guard = state.borrow_mut();
    if !guard.is_selectable_id(id) {
        return;
    }

    let Some(anchor) = guard.selection.anchor_id else {
        guard.selection.selected_ids.clear();
        guard.selection.selected_ids.insert(id);
        guard.selection.anchor_id = Some(id);
        guard.apply_selection_flags();
        return;
    };

    let Some(anchor_idx) = guard.index_of(anchor) else {
        guard.selection.selected_ids.clear();
        guard.selection.selected_ids.insert(id);
        guard.selection.anchor_id = Some(id);
        guard.apply_selection_flags();
        return;
    };

    let Some(target_idx) = guard.index_of(id) else {
        return;
    };

    let (start, end) = if anchor_idx <= target_idx {
        (anchor_idx, target_idx)
    } else {
        (target_idx, anchor_idx)
    };

    guard.selection.selected_ids.clear();
    for idx in start..=end {
        if let Some(thumb) = guard.thumbnails.row_data(idx) {
            if thumb.selectable {
                guard.selection.selected_ids.insert(thumb.id);
            }
        }
    }
    guard.selection.anchor_id = Some(id);
    guard.apply_selection_flags();
}

fn toggle_single(state: &Rc<RefCell<ImportViewState>>, id: ImageId) {
    let mut guard = state.borrow_mut();
    if !guard.is_selectable_id(id) {
        return;
    }

    if !guard.selection.selected_ids.remove(&id) {
        guard.selection.selected_ids.insert(id);
    }
    guard.apply_selection_flags();
}

fn select_all_allowed(state: &Rc<RefCell<ImportViewState>>) {
    let mut guard = state.borrow_mut();
    guard.selection.selected_ids.clear();
    let mut anchor: Option<ImageId> = None;
    for idx in 0..guard.thumbnails.row_count() {
        if let Some(thumb) = guard.thumbnails.row_data(idx) {
            if thumb.selectable {
                guard.selection.selected_ids.insert(thumb.id);
                if anchor.is_none() {
                    anchor = Some(thumb.id);
                }
            }
        }
    }
    guard.selection.anchor_id = anchor;
    guard.apply_selection_flags();
}

fn clear_selection(state: &Rc<RefCell<ImportViewState>>) {
    let mut guard = state.borrow_mut();
    guard.selection.selected_ids.clear();
    guard.apply_selection_flags();
}

fn set_all_checked(state: &Rc<RefCell<ImportViewState>>, checked: bool) {
    let guard = state.borrow_mut();
    for idx in 0..guard.thumbnails.row_count() {
        if let Some(mut thumb) = guard.thumbnails.row_data(idx) {
            if thumb.selectable {
                thumb.checked = checked;
                guard.thumbnails.set_row_data(idx, thumb);
            }
        }
    }
    guard.rebuild_checked_paths();
}

fn apply_checkbox_to_selection(state: &Rc<RefCell<ImportViewState>>, checked: bool) {
    let mut guard = state.borrow_mut();
    if guard.selection.selected_ids.is_empty() {
        if let Some(anchor) = guard.selection.anchor_id {
            if guard.is_selectable_id(anchor) {
                guard.selection.selected_ids.insert(anchor);
            }
        }
    }

    guard.apply_selection_flags();

    for idx in 0..guard.thumbnails.row_count() {
        if let Some(mut thumb) = guard.thumbnails.row_data(idx) {
            if guard.selection.selected_ids.contains(&thumb.id) && thumb.selectable {
                thumb.checked = checked;
                guard.thumbnails.set_row_data(idx, thumb);
            }
        }
    }
    guard.rebuild_checked_paths();
}

fn start_scan_for_directory(
    import_ui: &slint::Weak<ImportPhotosScreen>,
    state: &Rc<RefCell<ImportViewState>>,
    catalog_path: PathBuf,
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

    let db_path = catalog_path.to_string_lossy().to_string();
    let catalog_for_scan = CatalogDb::open(&db_path)
        .map(CatalogService::new)
        .map(Rc::new)
        .map_err(|err| {
            if let Some(ui) = import_ui.upgrade() {
                ui.set_status_text(format!("Unable to check duplicates: {err}").into());
            }
            err
        })
        .ok();

    let cancel_flag = { state.borrow().scan_cancel.clone() };
    let state_for_candidates = state.clone();
    let ui_weak = import_ui.clone();
    let seen = Rc::new(RefCell::new(HashSet::<PathBuf>::new()));
    let scan_opts = ScanOptions {
        on_candidate: Some(Arc::new({
            let seen = seen.clone();
            let catalog_for_scan = catalog_for_scan.clone();
            move |candidate| {
                if !seen.borrow_mut().insert(candidate.path.clone()) {
                    return;
                }

                let already_imported = catalog_for_scan
                    .as_ref()
                    .map(|svc| is_already_imported(svc, &candidate.path))
                    .unwrap_or(false);
                let selectable = !already_imported;
                let checked = selectable;
                let display_thumb = candidate.thumb.unwrap_or_else(placeholder_image);
                let path_text: SharedString = candidate.path.to_string_lossy().to_string().into();

                let mut guard = state_for_candidates.borrow_mut();
                let id = guard.next_image_id;
                guard.next_image_id += 1;

                if selectable && guard.selection.selected_ids.is_empty() {
                    guard.selection.selected_ids.insert(id);
                    guard.selection.anchor_id = Some(id);
                }

                let is_selected = guard.selection.selected_ids.contains(&id);

                guard.thumbnails.push(ImportThumbnail {
                    id,
                    path: path_text.clone(),
                    display_thumb,
                    selected: is_selected,
                    checked,
                    already_imported,
                    selectable,
                });
                guard.apply_selection_flags();
                guard.rebuild_checked_paths();
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_text(format!("Queued {}", candidate.path.display()).into());
                }
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
        let mut guard = state_done.borrow_mut();
        guard.apply_selection_flags();
        guard.rebuild_checked_paths();
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
                format!("{} ({}/{})", label, progress.completed, progress.total).into(),
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
        let catalog_path = catalog_path.clone();
        import_ui.on_browse_for_directory(move || {
            let import_ui_weak = import_ui_weak.clone();
            let view_state = view_state.clone();
            let catalog_path = catalog_path.clone();
            let _ = slint::spawn_local(async move {
                if let Some(handle) = AsyncFileDialog::new().pick_folder().await {
                    start_scan_for_directory(
                        &import_ui_weak,
                        &view_state,
                        catalog_path.clone(),
                        handle.path().to_path_buf(),
                    );
                }
            });
        });
    }

    {
        let import_ui_weak = import_ui_weak.clone();
        let view_state = view_state.clone();
        let catalog_path = catalog_path.clone();
        import_ui.on_directory_selected(move |path| {
            start_scan_for_directory(
                &import_ui_weak,
                &view_state,
                catalog_path.clone(),
                PathBuf::from(path.as_str()),
            );
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
            set_all_checked(&view_state, all_selected);
        });
    }

    {
        let view_state = view_state.clone();
        import_ui.on_thumbnail_clicked(move |image_id, shift, ctrl| {
            if shift {
                select_range(&view_state, image_id);
            } else if ctrl {
                toggle_single(&view_state, image_id);
            } else {
                select_single(&view_state, image_id);
            }
        });
    }

    {
        let view_state = view_state.clone();
        import_ui.on_checkbox_clicked(move |checked| {
            apply_checkbox_to_selection(&view_state, checked);
        });
    }

    {
        let view_state = view_state.clone();
        import_ui.on_keyboard_select_all(move || {
            select_all_allowed(&view_state);
        });
    }

    {
        let view_state = view_state.clone();
        import_ui.on_keyboard_clear_selection(move || {
            clear_selection(&view_state);
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
                let mut seen_files = HashSet::new();
                let mut files: Vec<PathBuf> = Vec::new();
                for p in paths.iter() {
                    let path_buf = PathBuf::from(p.as_str());
                    if seen_files.insert(path_buf.clone()) {
                        files.push(path_buf);
                    }
                }
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

fn refresh_folio_tree(
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
) {
    let folders = {
        let guard = catalog_state.borrow();
        let Some(session) = guard.as_ref() else {
            return;
        };
        match session.service.list_folders() {
            Ok(list) => list,
            Err(err) => {
                eprintln!("Failed to list folders: {err}");
                return;
            }
        }
    };

    let tree = build_folder_tree(&folders);
    {
        let guard = folio_state.borrow();
        guard.folder_tree.set_vec(tree);
    }

    if let Some(ui) = ui_weak.upgrade() {
        ui.set_folder_tree(folio_state.borrow().folder_tree.clone().into());
    }
}

fn build_folder_tree(folders: &[Folder]) -> Vec<FolderNode> {
    let mut paths: Vec<String> = folders.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    paths.dedup();

    let mut seen = HashSet::new();
    let mut nodes = Vec::new();

    for path in paths {
        let mut cumulative = if path.starts_with('/') {
            String::from("/")
        } else {
            String::new()
        };
        for (idx, part) in path.split('/').filter(|s| !s.is_empty()).enumerate() {
            if cumulative != "/" && !cumulative.is_empty() {
                cumulative.push('/');
            }
            cumulative.push_str(part);
            if seen.insert(cumulative.clone()) {
                nodes.push(FolderNode {
                    name: part.into(),
                    full_path: cumulative.clone().into(),
                    depth: idx as i32,
                });
            }
        }
    }

    nodes.sort_by(|a, b| a.full_path.cmp(&b.full_path));
    nodes
}

fn load_folder_thumbnails(
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
    ui_weak: &slint::Weak<MainWindow>,
    folder_path: &Path,
) {
    let (items, total_size) = {
        let guard = catalog_state.borrow();
        let Some(session) = guard.as_ref() else {
            return;
        };
        let filters = folio_state.borrow().filters.clone();
        let images = match session.service.list_images_in_folder(folder_path) {
            Ok(list) => list,
            Err(err) => {
                eprintln!("Failed to list images: {err}");
                return;
            }
        };
        let mut total_size = 0u64;
        let mut items = Vec::new();
        for img in images {
            if !passes_filters(&img, &filters) {
                continue;
            }

            if let Some(sz) = img.filesize {
                total_size += sz as u64;
            }

            let display_thumb = load_or_generate_thumbnail(&session.service, &img)
                .unwrap_or_else(placeholder_image);

            items.push(ThumbnailItem {
                id: img.id as i32,
                path: SharedString::from(img.original_path.clone()),
                display_thumb,
                selected: false,
                rating: img.rating.unwrap_or(0) as i32,
                flag: SharedString::from(img.flag.unwrap_or_default()),
                color_label: SharedString::from(img.color_label.unwrap_or_default()),
            });
        }
        (items, total_size)
    };

    let count = items.len();
    {
        let mut guard = folio_state.borrow_mut();
        guard.thumbnails.set_vec(items);
        guard.reset_selection();
    }

    if let Some(ui) = ui_weak.upgrade() {
        ui.set_thumbnails(folio_state.borrow().thumbnails.clone().into());
        ui.set_folio_total_count(count as i32);
        ui.set_folio_selected_count(0);
        ui.set_folio_size_summary(human_size(total_size).into());
        ui.set_metadata(empty_metadata());
        ui.set_keywords_text("".into());
        ui.set_selected_image_id(-1);
    }
}

fn passes_filters(image: &CatalogImage, filters: &FilterState) -> bool {
    if filters.rating > 0 && image.rating.unwrap_or(0) < filters.rating as i64 {
        return false;
    }

    if !filters.flag.is_empty()
        && image
            .flag
            .as_deref()
            .map(|f| f != filters.flag.as_str())
            .unwrap_or(true)
    {
        return false;
    }

    if !filters.color_label.is_empty()
        && image
            .color_label
            .as_deref()
            .map(|c| c != filters.color_label.as_str())
            .unwrap_or(true)
    {
        return false;
    }

    if !filters.search.is_empty() {
        let needle = filters.search.to_ascii_lowercase();
        let haystack = format!(
            "{} {}",
            image.filename.to_ascii_lowercase(),
            image.original_path.to_ascii_lowercase()
        );
        if !haystack.contains(&needle) {
            return false;
        }
    }

    true
}

fn load_or_generate_thumbnail(
    service: &CatalogService,
    image: &CatalogImage,
) -> Option<slint::Image> {
    if let Ok(Some(thumb)) = service.load_thumbnail(image.id) {
        if let Some(img) = thumbnail_to_image(&thumb) {
            return Some(img);
        }
    }

    let path = PathBuf::from(&image.original_path);
    if let Ok(Some(thumb)) = service.generate_thumbnail(image.id, &path) {
        return thumbnail_to_image(&thumb);
    }

    None
}

fn thumbnail_to_image(thumb: &Thumbnail) -> Option<slint::Image> {
    if let Some(bytes) = thumb
        .thumb_256
        .as_ref()
        .or_else(|| thumb.thumb_1024.as_ref())
    {
        return decode_thumbnail(bytes);
    }
    None
}

fn decode_thumbnail(bytes: &[u8]) -> Option<slint::Image> {
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = fit_into_square(&img, 256);
    let (width, height) = thumb.dimensions();
    let buffer =
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(thumb.as_raw(), width, height);
    Some(slint::Image::from_rgba8(buffer))
}

fn fit_into_square(img: &image::DynamicImage, max_dim: u32) -> image::RgbaImage {
    let resized = img
        .resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let (w, h) = resized.dimensions();
    if w == max_dim && h == max_dim {
        return resized;
    }

    let mut canvas =
        image::RgbaImage::from_pixel(max_dim, max_dim, image::Rgba([16, 16, 16, 255]));
    let offset_x = (max_dim - w) / 2;
    let offset_y = (max_dim - h) / 2;
    image::imageops::overlay(&mut canvas, &resized, offset_x.into(), offset_y.into());
    canvas
}

fn handle_thumbnail_selection(
    image_id: i32,
    range_select: bool,
    toggle: bool,
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
    ui_weak: &slint::Weak<MainWindow>,
) {
    let mut guard = folio_state.borrow_mut();
    let mut clicked_index: Option<usize> = None;
    for idx in 0..guard.thumbnails.row_count() {
        if let Some(thumb) = guard.thumbnails.row_data(idx) {
            if thumb.id == image_id {
                clicked_index = Some(idx);
                break;
            }
        }
    }

    let Some(idx) = clicked_index else {
        return;
    };

    if range_select {
        let anchor = guard.selection_anchor.unwrap_or(idx);
        let (start, end) = if anchor <= idx {
            (anchor, idx)
        } else {
            (idx, anchor)
        };

        guard.selection.clear();
        for i in 0..guard.thumbnails.row_count() {
            if let Some(mut thumb) = guard.thumbnails.row_data(i) {
                let selected = i >= start && i <= end;
                thumb.selected = selected;
                if selected {
                    guard.selection.push(thumb.id);
                }
                guard.thumbnails.set_row_data(i, thumb);
            }
        }
        guard.selection_anchor = Some(idx);
    } else if toggle {
        if let Some(mut thumb) = guard.thumbnails.row_data(idx) {
            thumb.selected = !thumb.selected;
            if thumb.selected {
                if !guard.selection.contains(&thumb.id) {
                    guard.selection.push(thumb.id);
                }
            } else {
                guard.selection.retain(|id| *id != thumb.id);
            }
            guard.thumbnails.set_row_data(idx, thumb);
        }
        guard.selection_anchor = Some(idx);
    } else {
        guard.selection.clear();
        for i in 0..guard.thumbnails.row_count() {
            if let Some(mut thumb) = guard.thumbnails.row_data(i) {
                let selected = i == idx;
                thumb.selected = selected;
                if selected {
                    guard.selection.push(thumb.id);
                }
                guard.thumbnails.set_row_data(i, thumb);
            }
        }
        guard.selection_anchor = Some(idx);
    }

    let selected_first = guard.selection.first().copied();
    let selected_len = guard.selection.len();
    drop(guard);

    if let Some(ui) = ui_weak.upgrade() {
        ui.set_folio_selected_count(selected_len as i32);
        if let Some(first) = selected_first {
            ui.set_selected_image_id(first as i32);
            if let Err(err) = refresh_metadata_panel(catalog_state, ui_weak, first as i64) {
                eprintln!("Failed to refresh metadata panel: {err}");
            }
        } else {
            ui.set_selected_image_id(-1);
            ui.set_metadata(empty_metadata());
            ui.set_keywords_text("".into());
        }
    }
}

fn refresh_thumbnail(
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
    image_id: i64,
) -> anyhow::Result<()> {
    let (image, display_thumb) = {
        let guard = catalog_state.borrow();
        let session = guard.as_ref().context("No catalog open")?;
        let meta = session
            .service
            .load_metadata(image_id)
            .with_context(|| format!("Failed to load metadata for image_id={image_id}"))?;
        let display_thumb = load_or_generate_thumbnail(&session.service, &meta.image)
            .unwrap_or_else(placeholder_image);
        (meta.image, display_thumb)
    };

    let guard = folio_state.borrow_mut();
    let mut target_idx: Option<usize> = None;
    for idx in 0..guard.thumbnails.row_count() {
        if let Some(thumb) = guard.thumbnails.row_data(idx) {
            if thumb.id == image_id as i32 {
                target_idx = Some(idx);
                break;
            }
        }
    }

    if let Some(idx) = target_idx {
        let is_selected = guard.selection.contains(&(image_id as i32));
        guard.thumbnails.set_row_data(
            idx,
            ThumbnailItem {
                id: image.id as i32,
                path: SharedString::from(image.original_path.clone()),
                display_thumb,
                rating: image.rating.unwrap_or(0) as i32,
                flag: SharedString::from(image.flag.unwrap_or_default()),
                color_label: SharedString::from(image.color_label.unwrap_or_default()),
                selected: is_selected,
            },
        );
    }

    Ok(())
}

fn refresh_metadata_panel(
    catalog_state: &CatalogState,
    ui_weak: &slint::Weak<MainWindow>,
    image_id: i64,
) -> anyhow::Result<()> {
    let meta = {
        let guard = catalog_state.borrow();
        let session = guard.as_ref().context("No catalog open")?;
        session
            .service
            .load_metadata(image_id)
            .with_context(|| format!("Failed to load metadata for image_id={image_id}"))?
    };

    let image = meta.image;
    let keywords_vec: Vec<SharedString> = meta
        .keywords
        .iter()
        .map(|k| SharedString::from(k.clone()))
        .collect();
    let keywords_model: Rc<VecModel<SharedString>> = Rc::new(VecModel::from(keywords_vec));

    let captured_at = image
        .captured_at
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    let camera = image.camera_model.or(image.camera_make).unwrap_or_default();
    let lens = image.lens_model.unwrap_or_default();
    let focal_length = image
        .focal_length
        .map(|f| format!("{f:.1}mm"))
        .unwrap_or_default();
    let aperture = image
        .aperture
        .map(|a| format!("f/{a:.1}"))
        .unwrap_or_default();
    let shutter_speed = image
        .shutter_speed
        .map(|s| {
            if s >= 1.0 {
                format!("{s:.1}s")
            } else {
                format!("1/{:.0}s", 1.0 / s.max(f64::EPSILON))
            }
        })
        .unwrap_or_default();
    let iso = image.iso.map(|i| i.to_string()).unwrap_or_default();
    let gps_lat = image
        .gps_latitude
        .map(|v| format!("{v:.5}"))
        .unwrap_or_default();
    let gps_lon = image
        .gps_longitude
        .map(|v| format!("{v:.5}"))
        .unwrap_or_default();

    let ui_metadata = ImageMetadata {
        file_path: image.original_path.clone().into(),
        captured_at: captured_at.into(),
        camera: camera.into(),
        lens: lens.into(),
        focal_length: focal_length.into(),
        aperture: aperture.into(),
        shutter_speed: shutter_speed.into(),
        iso: iso.into(),
        gps_lat: gps_lat.into(),
        gps_lon: gps_lon.into(),
        rating: image.rating.unwrap_or(0) as i32,
        flag: image.flag.clone().unwrap_or_default().into(),
        color_label: image.color_label.clone().unwrap_or_default().into(),
        keywords: keywords_model.clone().into(),
    };

    if let Some(ui) = ui_weak.upgrade() {
        ui.set_metadata(ui_metadata);
        ui.set_keywords_text(meta.keywords.join(", ").into());
        ui.set_selected_image_id(image_id as i32);
    }

    Ok(())
}

fn apply_rating_change(
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
    ui_weak: &slint::Weak<MainWindow>,
    image_id: i32,
    rating: i32,
) -> anyhow::Result<()> {
    {
        let mut guard = catalog_state.borrow_mut();
        let session = guard.as_mut().context("No catalog open")?;
        session.service.update_rating(image_id as i64, rating)?;
    }

    refresh_thumbnail(catalog_state, folio_state, image_id as i64)?;
    refresh_metadata_panel(catalog_state, ui_weak, image_id as i64)?;
    Ok(())
}

fn apply_flag_change(
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
    ui_weak: &slint::Weak<MainWindow>,
    image_id: i32,
    flag: &str,
) -> anyhow::Result<()> {
    {
        let mut guard = catalog_state.borrow_mut();
        let session = guard.as_mut().context("No catalog open")?;
        session.service.update_flag(image_id as i64, flag)?;
    }

    refresh_thumbnail(catalog_state, folio_state, image_id as i64)?;
    refresh_metadata_panel(catalog_state, ui_weak, image_id as i64)?;
    Ok(())
}

fn apply_color_label_change(
    catalog_state: &CatalogState,
    folio_state: &Rc<RefCell<FolioState>>,
    ui_weak: &slint::Weak<MainWindow>,
    image_id: i32,
    label: &str,
) -> anyhow::Result<()> {
    {
        let mut guard = catalog_state.borrow_mut();
        let session = guard.as_mut().context("No catalog open")?;
        session.service.update_color_label(image_id as i64, label)?;
    }

    refresh_thumbnail(catalog_state, folio_state, image_id as i64)?;
    refresh_metadata_panel(catalog_state, ui_weak, image_id as i64)?;
    Ok(())
}

fn update_keywords(
    catalog_state: &CatalogState,
    image_id: i32,
    keywords: Vec<String>,
) -> anyhow::Result<()> {
    let mut guard = catalog_state.borrow_mut();
    let session = guard.as_mut().context("No catalog open")?;
    session
        .service
        .update_keywords(image_id as i64, &keywords)?;
    Ok(())
}

fn open_refine_screen(
    catalog_state: &CatalogState,
    ui_weak: &slint::Weak<MainWindow>,
    engine: &Arc<ImageEngine>,
    image_id: i32,
) {
    let file_path = {
        let guard = catalog_state.borrow();
        let Some(session) = guard.as_ref() else {
            return;
        };
        match session.service.load_metadata(image_id as i64) {
            Ok(meta) => meta.image.original_path,
            Err(err) => {
                eprintln!("Failed to open refine metadata: {err}");
                return;
            }
        }
    };

    let path_buf = PathBuf::from(&file_path);
    if let Some(ui) = ui_weak.upgrade() {
        ui.set_refine_image_id(image_id);
        ui.set_refine_path(file_path.clone().into());
        ui.set_refine_exposure(0.0);
        ui.set_refine_contrast(0.0);
        ui.set_refine_highlights(0.0);
        ui.set_refine_shadows(0.0);
        ui.set_refine_whites(0.0);
        ui.set_refine_blacks(0.0);
        ui.set_current_tab(1);
    }

    let ui_for_preview = ui_weak.clone();
    let engine = engine.clone();
    std::thread::spawn(move || match engine.open_preview(&path_buf, 1600) {
        Ok(preview) => {
            let width = preview.width;
            let height = preview.height;
            let data = preview.data;
            slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_for_preview.upgrade() {
                    let buf = preview_to_pixel_buffer(width, height, data);
                    ui.set_refine_preview(slint::Image::from_rgba8(buf));
                }
            })
            .ok();
        }
        Err(err) => {
            eprintln!("Failed to render refine preview: {err}");
        }
    });
}

fn apply_refine_edits(
    catalog_state: &CatalogState,
    image_id: i32,
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
) -> anyhow::Result<()> {
    let mut guard = catalog_state.borrow_mut();
    let session = guard.as_mut().context("No catalog open")?;
    let edits_record = Edits {
        id: 0,
        image_id: image_id as i64,
        exposure: Some(exposure as f64),
        contrast: Some(contrast as f64),
        highlights: Some(highlights as f64),
        shadows: Some(shadows as f64),
        whites: Some(whites as f64),
        blacks: Some(blacks as f64),
        vibrance: None,
        saturation: None,
        temperature: None,
        tint: None,
        texture: None,
        clarity: None,
        dehaze: None,
        parametric_curve_json: None,
        color_grading_json: None,
        crop_json: None,
        masking_json: None,
        updated_at: None,
    };
    session.service.apply_edits(image_id as i64, edits_record)?;
    Ok(())
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0} {}", size, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}
