use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ImageId(pub i64);

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct ImageFlags: u8 {
        const FLAGGED   = 0b0000_0001;
        const REJECTED  = 0b0000_0010;
        const VIRTUAL   = 0b0000_0100;
    }
}

/// Simple preview image type for the UI layer.
/// Slint will convert this into a SharedPixelBuffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major.
    pub data: Vec<u8>,
}

pub mod raw_jpeg;
