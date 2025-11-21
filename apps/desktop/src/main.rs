slint::include_modules!(); // from build.rs compiled ui/main.slint

use engine::ImageEngine;
use slint::{SharedPixelBuffer, Rgba8Pixel};
use std::path::PathBuf;
use std::sync::Arc;

fn preview_to_pixel_buffer(
    width: u32,
    height: u32,
    data: Vec<u8>,
) -> SharedPixelBuffer<Rgba8Pixel> {
    // Slint wants a SharedPixelBuffer; we assume RGBA8
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    {
        let dest = buf.make_mut_bytes();
        dest.copy_from_slice(&data);
    }
    buf
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;

    let engine = Arc::new(ImageEngine::new());
    let ui_weak = ui.as_weak();

    ui.on_open_image_request(move || {
        let ui = match ui_weak.upgrade() {
            Some(ui) => ui,
            None => return,
        };

        // Simple native file dialog using rfd crate (add it if you like),
        // or just hard-code a path for now.
        #[cfg(feature = "use-rfd")]
        let path_opt = rfd::FileDialog::new().pick_file();

        #[cfg(not(feature = "use-rfd"))]
        let path_opt: Option<PathBuf> = {
            eprintln!("TODO: integrate a proper file dialog; using hard-coded path for now.");
            None
        };

        if let Some(path) = path_opt {
            let path_str = path.to_string_lossy().to_string();
            let engine = engine.clone();
            let ui_weak_inner = ui_weak.clone();

            // Run decoding off the UI thread
            std::thread::spawn(move || {
                match engine.open_preview(&path, 1024) {
                    Ok(preview) => {
                        let buf = preview_to_pixel_buffer(
                            preview.width,
                            preview.height,
                            preview.data,
                        );
                        if let Some(ui) = ui_weak_inner.upgrade() {
                            slint::invoke_from_event_loop(move || {
                                ui.set_preview_image(slint::Image::from_rgba8(buf));
                                ui.set_current_path(path_str.into());
                            })
                            .ok();
                        }
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
