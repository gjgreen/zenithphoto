slint::include_modules!(); // from build.rs compiled ui/main.slint and catalog_dialog.slint

use app_settings::AppSettings;
use catalog::{Catalog, NewImage};
use engine::ImageEngine;
use rfd::FileDialog;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::path::PathBuf;
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
    let mut settings = AppSettings::load().unwrap_or_default();
    let mut startup_error: Option<String> = None;
    let mut catalog_pair: Option<(Catalog, PathBuf)> = None;

    if let Some(path) = settings.get_last_catalog() {
        match Catalog::open(&path) {
            Ok(cat) => catalog_pair = Some((cat, path)),
            Err(err) => {
                eprintln!("Failed to open last catalog: {err}");
                startup_error = Some(format!("Failed to open last catalog: {err}"));
            }
        }
    }

    if catalog_pair.is_none() {
        catalog_pair = prompt_for_catalog_dialog(startup_error)?;
    }

    let Some((catalog, catalog_path)) = catalog_pair else {
        return Ok(()); // user quit
    };

    settings.set_last_catalog(catalog_path.clone());
    let _ = settings.save();
    let _ = Catalog::set_last_used(&catalog_path);

    let catalog = Rc::new(catalog);
    let ui = MainWindow::new()?;
    ui.set_current_path(catalog_path.to_string_lossy().to_string().into());

    let engine = Arc::new(ImageEngine::new());
    let ui_weak = ui.as_weak();
    let catalog_for_ui = catalog.clone();

    ui.on_open_image_request(move || {
        if ui_weak.upgrade().is_none() {
            return;
        }

        let path_opt = FileDialog::new()
            .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "gif"])
            .pick_file();

        if let Some(path) = path_opt {
            let _ = catalog_for_ui.insert_image(NewImage {
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

            let path_str = path.to_string_lossy().to_string();
            let engine = engine.clone();
            let ui_weak_inner = ui_weak.clone();

            std::thread::spawn(move || {
                match engine.open_preview(&path, 1024) {
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
                }
            });
        }
    });

    ui.run()
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
                match Catalog::open(&path) {
                    Ok(cat) => {
                        *selected.borrow_mut() = Some((cat, path.clone()));
                        slint::quit_event_loop().ok();
                    }
                    Err(err) => {
                        if let Some(dialog) = dialog_weak.upgrade() {
                            dialog
                                .set_error_text(format!("Unable to open catalog: {err}").into());
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
