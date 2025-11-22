mod config;

slint::include_modules!(); // from build.rs compiled ui/main.slint and catalog_dialog.slint

use catalog::{Catalog, CatalogError, CatalogPath, NewImage};
use config::ConfigStore;
use engine::ImageEngine;
use rfd::{AsyncFileDialog, FileDialog};
use slint::{Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
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

fn main() -> Result<(), slint::PlatformError> {
    let config_store = ConfigStore::load().unwrap_or_else(|err| {
        eprintln!("Failed to load app configuration: {err}");
        ConfigStore::new_default()
    });

    let mut startup_error: Option<String> = None;
    let mut catalog_pair: Option<(Catalog, PathBuf)> = None;

    if let Some(path) = config_store.last_catalog() {
        match open_catalog(&path) {
            Ok(cat) => catalog_pair = Some((cat, path)),
            Err(err) => {
                eprintln!("Failed to open last catalog from config: {err}");
                startup_error = Some(format!("Failed to open last catalog: {err}"));
            }
        }
    }

    if catalog_pair.is_none() {
        if let Some(path) = Catalog::last_used() {
            match open_catalog(&path) {
                Ok(cat) => catalog_pair = Some((cat, path)),
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

    let Some((catalog, catalog_path)) = catalog_pair else {
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

    let catalog_state = Rc::new(RefCell::new(Some(catalog)));
    let engine = Arc::new(ImageEngine::new());

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
                    if let Some(catalog) = catalog_guard.as_ref() {
                        let _ = catalog.insert_image(NewImage {
                            file_path: path.clone(),
                            rating: None,
                            flags: None,
                            capture_time_utc: None,
                            camera_make: None,
                            camera_model: None,
                            aperture: None,
                            shutter: None,
                            iso: None,
                            focal_length: None,
                        });
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

    ui.on_exit_requested(|| {
        slint::quit_event_loop().ok();
    });

    ui.run()
}

fn spawn_open_catalog_dialog(
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &Rc<RefCell<Option<Catalog>>>,
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
    catalog_state: &Rc<RefCell<Option<Catalog>>>,
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
            match Catalog::create(&requested_path) {
                Ok(catalog) => {
                    let resolved_path = catalog.path().to_path_buf();
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
    catalog_state: &Rc<RefCell<Option<Catalog>>>,
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

    let catalog =
        open_catalog(&normalized_path).map_err(|err| format!("Unable to open catalog: {err}"))?;
    apply_loaded_catalog(
        catalog,
        normalized_path,
        ui_weak,
        catalog_state,
        config_store,
        recent_model,
    );
    Ok(())
}

fn apply_loaded_catalog(
    catalog: Catalog,
    path: PathBuf,
    ui_weak: &slint::Weak<MainWindow>,
    catalog_state: &Rc<RefCell<Option<Catalog>>>,
    config_store: &ConfigStore,
    recent_model: &Rc<VecModel<SharedString>>,
) {
    *catalog_state.borrow_mut() = Some(catalog);

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

fn refresh_recent_model(model: &Rc<VecModel<SharedString>>, entries: &[PathBuf]) {
    let data: Vec<SharedString> = entries
        .iter()
        .map(|path| SharedString::from(path.to_string_lossy().to_string()))
        .collect();
    model.set_vec(data);
}

fn open_catalog(path: &Path) -> Result<Catalog, CatalogError> {
    Catalog::open(path)
}

fn prompt_for_catalog_dialog(
    initial_error: Option<String>,
) -> Result<Option<(Catalog, PathBuf)>, slint::PlatformError> {
    let dialog = CatalogDialog::new()?;
    if let Some(msg) = initial_error {
        dialog.set_error_text(msg.into());
    }

    let selected: Rc<RefCell<Option<(Catalog, PathBuf)>>> = Rc::new(RefCell::new(None));

    {
        let dialog_weak = dialog.as_weak();
        let selected = selected.clone();
        dialog.on_open_existing_catalog(move || {
            if let Some(path) = FileDialog::new()
                .add_filter("Zenith Catalog", &["zenithphotocatalog", "sqlite"])
                .pick_file()
            {
                match open_catalog(&path) {
                    Ok(cat) => {
                        *selected.borrow_mut() = Some((cat, path.clone()));
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
                match Catalog::create(&path) {
                    Ok(cat) => {
                        *selected.borrow_mut() = Some((cat, path.clone()));
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
