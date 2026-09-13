use super::protocol::Crop;
use image::RgbaImage;

/// Windows' top-down BGRA buffer. Keep the desktop in its native format so
/// releasing a small selection never converts millions of unrelated pixels.
pub struct DesktopFrame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl DesktopFrame {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Result<Self, String> {
        if width == 0
            || height == 0
            || u64::from(width) * u64::from(height) * 4 != bgra.len() as u64
        {
            return Err("Invalid screen capture.".into());
        }
        Ok(Self {
            width,
            height,
            bgra,
        })
    }

    pub fn crop(&self, crop: Crop) -> Result<RgbaImage, String> {
        crop.validate(self.width, self.height)?;
        let mut pixels = Vec::with_capacity(crop.width as usize * crop.height as usize * 4);
        for y in crop.y..crop.y + crop.height {
            let start = (y as usize * self.width as usize + crop.x as usize) * 4;
            pixels.extend_from_slice(&self.bgra[start..start + crop.width as usize * 4]);
        }
        Ok(rgba(crop.width, crop.height, pixels))
    }

    pub fn backdrop(&self) -> RgbaImage {
        // A 22px blur hides sampling detail; avoid a full desktop resampling pass.
        let scale = (800.0 / self.width as f64)
            .min(600.0 / self.height as f64)
            .min(1.0);
        let width = ((self.width as f64 * scale) as u32).max(1);
        let height = ((self.height as f64 * scale) as u32).max(1);
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            let source_y = u64::from(y) * u64::from(self.height) / u64::from(height);
            for x in 0..width {
                let source_x = u64::from(x) * u64::from(self.width) / u64::from(width);
                let start = (source_y * u64::from(self.width) + source_x) as usize * 4;
                pixels.extend_from_slice(&self.bgra[start..start + 4]);
            }
        }
        rgba(width, height, pixels)
    }
}

fn rgba(width: u32, height: u32, mut pixels: Vec<u8>) -> RgbaImage {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    RgbaImage::from_raw(width, height, pixels).expect("validated capture dimensions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_preserves_rows_offsets_colors_and_opaque_alpha() {
        let mut bytes = Vec::new();
        for y in 0..8u8 {
            for x in 0..9u8 {
                bytes.extend([x, y, 200, 0]);
            }
        }
        let frame = DesktopFrame::new(9, 8, bytes).unwrap();
        let crop = frame
            .crop(Crop {
                x: 3,
                y: 2,
                width: 4,
                height: 5,
            })
            .unwrap();
        assert_eq!(crop.dimensions(), (4, 5));
        assert_eq!(crop.get_pixel(0, 0).0, [200, 2, 3, 255]);
        assert_eq!(crop.get_pixel(3, 4).0, [200, 6, 6, 255]);
        assert!(frame
            .crop(Crop {
                x: 7,
                y: 0,
                width: 4,
                height: 4
            })
            .is_err());
    }

    #[test]
    fn backdrop_fits_landscape_and_portrait_without_enlarging() {
        for (width, height, expected) in [
            (1600, 900, (800, 450)),
            (900, 1600, (337, 600)),
            (4, 4, (4, 4)),
        ] {
            let frame = DesktopFrame::new(
                width,
                height,
                [10, 20, 30, 0].repeat((width * height) as usize),
            )
            .unwrap();
            let image = frame.backdrop();
            assert_eq!(image.dimensions(), expected);
            assert!(image.pixels().all(|pixel| pixel.0 == [30, 20, 10, 255]));
        }
        assert!(DesktopFrame::new(4, 4, vec![0; 4]).is_err());
    }
}
