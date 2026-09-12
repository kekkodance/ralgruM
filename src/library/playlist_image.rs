use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageReader, imageops::FilterType};
use std::{io::Cursor, path::Path, sync::Arc};

use crate::search::Provider;

pub(crate) const COVER_SIZE: u32 = 512;
pub(crate) const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_IMAGE_DIMENSION: u32 = 10_000;
pub(crate) const MAX_IMAGE_PIXELS: u64 = 40_000_000;
pub(crate) const SUPPORTED_COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
pub(crate) const SOUNDCLOUD_COVER_EXTENSIONS: &[&str] =
    &["jpg", "jpeg", "png", "gif", "jfif", "jpe", "pjpeg", "pjp"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoverImageFormat {
    Gif,
    Jpeg,
    Png,
    Webp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CoverDisplayGeometry {
    pub width: f32,
    pub height: f32,
    pub left: f32,
    pub top: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CoverDraft {
    pub file_name: String,
    pub bytes: Arc<Vec<u8>>,
    pub format: CoverImageFormat,
    pub width: u32,
    pub height: u32,
    pub zoom: u16,
    pub pan_x: i32,
    pub pan_y: i32,
}

impl CoverDraft {
    pub(crate) fn from_path(path: &Path) -> Result<Self, String> {
        let file_size = std::fs::metadata(path)
            .map_err(|_| "The selected cover could not be read.")?
            .len();
        if file_size == 0 || file_size > MAX_IMAGE_BYTES as u64 {
            return Err("The selected cover must be between 1 byte and 10 MB.".into());
        }
        let bytes =
            Arc::new(std::fs::read(path).map_err(|_| "The selected cover could not be read.")?);
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return Err("The selected cover must be between 1 byte and 10 MB.".into());
        }
        let (format, width, height) = image_metadata(bytes.as_ref())?;
        Ok(Self {
            file_name: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("cover")
                .to_owned(),
            bytes,
            format,
            width,
            height,
            zoom: 100,
            pan_x: 0,
            pan_y: 0,
        })
    }

    pub(crate) fn export_jpeg(&self) -> Result<Vec<u8>, String> {
        let image = decode_image(self.bytes.as_ref())?;
        crop_square(image, self.zoom, self.pan_x, self.pan_y)
    }

    pub(crate) fn export_base64(&self) -> Result<String, String> {
        let jpeg = self.export_jpeg()?;
        if jpeg.len() > MAX_IMAGE_BYTES {
            return Err("The cropped cover exceeds the 10 MB upload limit.".into());
        }
        Ok(STANDARD.encode(jpeg))
    }

    pub(crate) fn set_zoom(&mut self, zoom: u16) {
        let previous = self.zoom.clamp(100, 300);
        let next = zoom.clamp(100, 300);
        if previous != next {
            let ratio = next as f32 / previous as f32;
            self.pan_x = (self.pan_x as f32 * ratio).round() as i32;
            self.pan_y = (self.pan_y as f32 * ratio).round() as i32;
            self.zoom = next;
            self.clamp_pan();
        }
    }

    pub(crate) fn pan_from_drag(
        &mut self,
        start_pan_x: i32,
        start_pan_y: i32,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
    ) {
        if preview_size <= 0. {
            return;
        }
        let crop_pixels_per_display_pixel = COVER_SIZE as f32 / preview_size;
        self.pan_x = start_pan_x
            .saturating_sub((pointer_delta_x * crop_pixels_per_display_pixel).round() as i32);
        self.pan_y = start_pan_y
            .saturating_sub((pointer_delta_y * crop_pixels_per_display_pixel).round() as i32);
        self.clamp_pan();
    }

    pub(crate) fn display_geometry(&self, preview_size: f32) -> CoverDisplayGeometry {
        let (scaled_width, scaled_height) = self.scaled_dimensions();
        let (crop_x, crop_y) = self.crop_origin(scaled_width, scaled_height);
        let display_scale = preview_size / COVER_SIZE as f32;
        CoverDisplayGeometry {
            width: scaled_width * display_scale,
            height: scaled_height * display_scale,
            left: -(crop_x as f32) * display_scale,
            top: -(crop_y as f32) * display_scale,
        }
    }

    fn scaled_dimensions(&self) -> (f32, f32) {
        let width = self.width.max(1) as f32;
        let height = self.height.max(1) as f32;
        let base = (COVER_SIZE as f32 / width).max(COVER_SIZE as f32 / height);
        let scale = base * (self.zoom.clamp(100, 300) as f32 / 100.0);
        (
            (width * scale).max(COVER_SIZE as f32),
            (height * scale).max(COVER_SIZE as f32),
        )
    }

    fn crop_origin(&self, scaled_width: f32, scaled_height: f32) -> (i32, i32) {
        let scaled_width = scaled_width.round() as i32;
        let scaled_height = scaled_height.round() as i32;
        let max_x = scaled_width.saturating_sub(COVER_SIZE as i32);
        let max_y = scaled_height.saturating_sub(COVER_SIZE as i32);
        let x = (max_x / 2).saturating_add(self.pan_x).clamp(0, max_x);
        let y = (max_y / 2).saturating_add(self.pan_y).clamp(0, max_y);
        (x, y)
    }

    fn clamp_pan(&mut self) {
        let (scaled_width, scaled_height) = self.scaled_dimensions();
        let max_x = (scaled_width.round() as i32).saturating_sub(COVER_SIZE as i32);
        let max_y = (scaled_height.round() as i32).saturating_sub(COVER_SIZE as i32);
        let center_x = max_x / 2;
        let center_y = max_y / 2;
        self.pan_x = self.pan_x.clamp(-center_x, max_x - center_x);
        self.pan_y = self.pan_y.clamp(-center_y, max_y - center_y);
    }
}

fn image_metadata(bytes: &[u8]) -> Result<(CoverImageFormat, u32, u32), String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "The selected cover format is invalid.")?;
    let format = match reader.format() {
        Some(image::ImageFormat::Jpeg) => CoverImageFormat::Jpeg,
        Some(image::ImageFormat::Png) => CoverImageFormat::Png,
        Some(image::ImageFormat::Gif) => CoverImageFormat::Gif,
        Some(image::ImageFormat::WebP) => CoverImageFormat::Webp,
        _ => return Err("The selected cover format is not supported.".into()),
    };
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| "The selected cover dimensions could not be read.")?;
    if width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err("The selected cover is too large. Images must be at most 10000x10000 and 40 megapixels.".into());
    }
    Ok((format, width, height))
}

fn decode_image(bytes: &[u8]) -> Result<DynamicImage, String> {
    image_metadata(bytes)?;
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "The selected cover format is invalid.")?
        .decode()
        .map_err(|_| "The selected cover could not be decoded.".into())
}

fn crop_square(
    image: DynamicImage,
    zoom_percent: u16,
    pan_x: i32,
    pan_y: i32,
) -> Result<Vec<u8>, String> {
    let width = image.width() as f32;
    let height = image.height() as f32;
    let base = (COVER_SIZE as f32 / width).max(COVER_SIZE as f32 / height);
    let scale = base * (zoom_percent.clamp(100, 300) as f32 / 100.0);
    let scaled_width = (width * scale).round().max(COVER_SIZE as f32) as u32;
    let scaled_height = (height * scale).round().max(COVER_SIZE as f32) as u32;
    let scaled = image.resize_exact(scaled_width, scaled_height, FilterType::Lanczos3);
    let max_x = scaled_width.saturating_sub(COVER_SIZE) as i32;
    let max_y = scaled_height.saturating_sub(COVER_SIZE) as i32;
    let x = ((scaled_width as i32 - COVER_SIZE as i32) / 2)
        .saturating_add(pan_x)
        .clamp(0, max_x) as u32;
    let y = ((scaled_height as i32 - COVER_SIZE as i32) / 2)
        .saturating_add(pan_y)
        .clamp(0, max_y) as u32;
    let cropped = scaled.crop_imm(x, y, COVER_SIZE, COVER_SIZE).to_rgb8();
    let mut output = Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 90);
    encoder
        .encode_image(&image::DynamicImage::ImageRgb8(cropped))
        .map_err(|_| "The cropped cover could not be encoded as JPEG.")?;
    Ok(output.into_inner())
}

pub(crate) fn choose_cover_for(provider: Provider) -> Result<Option<CoverDraft>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(cover_filter_label(provider), cover_extensions(provider))
        .pick_file()
    else {
        return Ok(None);
    };
    if !is_supported_cover_path_for(&path, provider) {
        return Err(format!(
            "The selected cover must use one of these formats: {}.",
            cover_extensions_label(provider)
        ));
    }
    CoverDraft::from_path(&path).map(Some)
}

pub(crate) fn choose_cover_with_preview_for(
    provider: Provider,
) -> Result<Option<(CoverDraft, Vec<u8>)>, String> {
    Ok(choose_cover_for(provider)?.map(cover_with_preview))
}

pub(crate) fn cover_with_preview_from_path(path: &Path) -> Result<(CoverDraft, Vec<u8>), String> {
    CoverDraft::from_path(path).map(cover_with_preview)
}

#[cfg(test)]
pub(crate) fn is_supported_cover_path(path: &Path) -> bool {
    is_supported_cover_path_for(path, Provider::Deezer)
}

pub(crate) fn is_supported_cover_path_for(path: &Path, provider: Provider) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            cover_extensions(provider)
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

pub(crate) fn cover_extensions(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::Deezer => SUPPORTED_COVER_EXTENSIONS,
        Provider::SoundCloud => SOUNDCLOUD_COVER_EXTENSIONS,
    }
}

pub(crate) fn cover_filter_label(provider: Provider) -> &'static str {
    let _ = provider;
    "Images"
}

pub(crate) fn cover_extensions_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "JPG, JPEG, PNG, or WebP",
        Provider::SoundCloud => "JPG, JPEG, PNG, GIF, JFIF, JPE, PJPEG, or PJP",
    }
}

fn cover_with_preview(cover: CoverDraft) -> (CoverDraft, Vec<u8>) {
    let preview = cover.bytes.as_ref().clone();
    (cover, preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgb};

    fn draft() -> CoverDraft {
        let image =
            ImageBuffer::from_fn(1000, 500, |x, y| Rgb([(x % 255) as u8, (y % 255) as u8, 1]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        CoverDraft {
            file_name: "x.png".into(),
            bytes: Arc::new(bytes),
            format: CoverImageFormat::Png,
            width: 1000,
            height: 500,
            zoom: 100,
            pan_x: 0,
            pan_y: 0,
        }
    }

    #[test]
    fn crop_is_square_and_pan_is_clamped() {
        let mut cover = draft();
        cover.pan_x = i32::MAX;
        cover.pan_y = i32::MIN;
        let jpeg = cover.export_jpeg().unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 512));
    }

    #[test]
    fn base64_is_raw_and_zoom_is_bounded() {
        let mut cover = draft();
        cover.zoom = 999;
        let encoded = cover.export_base64().unwrap();
        assert!(!encoded.starts_with("data:"));
        assert!(!encoded.is_empty());
    }

    #[test]
    fn oversized_decoded_dimensions_are_rejected_before_decode() {
        let image = ImageBuffer::<Rgb<u8>, _>::new(10_001, 1);
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        assert!(decode_image(&bytes).unwrap_err().contains("too large"));
    }

    #[test]
    fn picker_extensions_match_enabled_decoders() {
        assert_eq!(SUPPORTED_COVER_EXTENSIONS, &["jpg", "jpeg", "png", "webp"]);
        assert_eq!(
            SOUNDCLOUD_COVER_EXTENSIONS,
            &["jpg", "jpeg", "png", "gif", "jfif", "jpe", "pjpeg", "pjp"]
        );
        assert_eq!(
            cover_extensions_label(Provider::Deezer),
            "JPG, JPEG, PNG, or WebP"
        );
        assert_eq!(
            cover_extensions_label(Provider::SoundCloud),
            "JPG, JPEG, PNG, GIF, JFIF, JPE, PJPEG, or PJP"
        );
        assert_eq!(cover_filter_label(Provider::Deezer), "Images");
        assert_eq!(cover_filter_label(Provider::SoundCloud), "Images");
    }

    #[test]
    fn dropped_cover_extensions_are_case_insensitive_and_reject_other_files() {
        assert!(is_supported_cover_path(Path::new("cover.JPG")));
        assert!(is_supported_cover_path(Path::new("cover.webp")));
        assert!(!is_supported_cover_path(Path::new("cover.gif")));
        assert!(!is_supported_cover_path(Path::new("cover")));
    }

    #[test]
    fn soundcloud_cover_extensions_are_case_insensitive_and_provider_specific() {
        for extension in SOUNDCLOUD_COVER_EXTENSIONS {
            assert!(is_supported_cover_path_for(
                Path::new(&format!("cover.{extension}")),
                Provider::SoundCloud
            ));
            assert!(is_supported_cover_path_for(
                Path::new(&format!("cover.{}", extension.to_uppercase())),
                Provider::SoundCloud
            ));
        }
        assert!(!is_supported_cover_path_for(
            Path::new("cover.webp"),
            Provider::SoundCloud
        ));
        assert!(!is_supported_cover_path_for(
            Path::new("cover.gif"),
            Provider::Deezer
        ));
    }

    #[test]
    fn gif_bytes_are_decoded_and_cropped_to_jpeg() {
        let image = ImageBuffer::from_fn(2, 2, |x, y| {
            image::Rgba([(x * 100) as u8, (y * 100) as u8, 1, 255])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Gif)
            .unwrap();
        assert_eq!(image_metadata(&bytes).unwrap().0, CoverImageFormat::Gif);
        let jpeg = crop_square(decode_image(&bytes).unwrap(), 100, 0, 0).unwrap();
        assert_eq!(
            image::load_from_memory(&jpeg).unwrap().dimensions(),
            (512, 512)
        );
    }

    #[test]
    fn drag_moves_the_image_with_the_pointer_and_clamps_to_the_crop() {
        let mut cover = draft();
        let before = cover.display_geometry(200.);
        cover.pan_from_drag(0, 0, 40., 0., 200.);
        let geometry = cover.display_geometry(200.);
        assert_eq!(cover.pan_x, -102);
        assert!(geometry.left > before.left);

        cover.pan_from_drag(0, 0, -10_000., 0., 200.);
        let geometry = cover.display_geometry(200.);
        assert_eq!(geometry.left + geometry.width, 200.);
    }

    #[test]
    fn zoom_keeps_the_same_crop_focus_and_geometry_covers_the_square() {
        let mut cover = draft();
        cover.pan_x = 80;
        cover.set_zoom(200);
        assert_eq!(cover.pan_x, 160);
        let geometry = cover.display_geometry(180.);
        assert!(geometry.width >= 180.);
        assert!(geometry.height >= 180.);
        assert!(geometry.left <= 0.);
        assert!(geometry.top <= 0.);
        assert!(geometry.left + geometry.width >= 180.);
        assert!(geometry.top + geometry.height >= 180.);
    }
}
