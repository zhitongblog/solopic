use std::path::PathBuf;

use image::DynamicImage;
use serde::{Deserialize, Serialize};

use crate::io::{run_batch, OpResult, OutputMode};
use crate::report::Report;

/// 按边裁剪：从四条边各切掉指定像素，图片尺寸可以不一致。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CropSpec {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl CropSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.left == 0 && self.top == 0 && self.right == 0 && self.bottom == 0 {
            return Err(crate::locale::tr(
                "至少要指定一个方向的裁剪像素",
                "Specify at least one edge to crop",
            ));
        }
        Ok(())
    }
}

pub fn crop_image(img: &DynamicImage, spec: &CropSpec) -> Result<DynamicImage, String> {
    let (w, h) = (img.width(), img.height());
    if spec.left + spec.right >= w || spec.top + spec.bottom >= h {
        return Err(format!(
            "{} {w}x{h}",
            crate::locale::tr("裁剪量超过图片尺寸", "Crop amount exceeds image size")
        ));
    }
    Ok(img.crop_imm(spec.left, spec.top, w - spec.left - spec.right, h - spec.top - spec.bottom))
}

pub fn crop_files(files: &[PathBuf], spec: &CropSpec, out: &OutputMode) -> Report {
    if let Err(e) = spec.validate() {
        let mut r = Report::default();
        r.push_err("<参数>", e);
        return r;
    }
    run_batch(files, out, |img| {
        let result = crop_image(img, spec)?;
        let detail = format!("{}x{} -> {}x{}", img.width(), img.height(), result.width(), result.height());
        Ok(OpResult::Done(result, Some(detail)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn crop_edges() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 100, Rgb([10, 20, 30])));
        let spec = CropSpec { left: 100, bottom: 57, ..Default::default() };
        let out = crop_image(&img, &spec).unwrap();
        assert_eq!((out.width(), out.height()), (100, 43));
    }

    #[test]
    fn crop_too_much() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(50, 50));
        let spec = CropSpec { left: 30, right: 30, ..Default::default() };
        assert!(crop_image(&img, &spec).is_err());
    }

    #[test]
    fn crop_all_zero_rejected() {
        assert!(CropSpec::default().validate().is_err());
    }
}
