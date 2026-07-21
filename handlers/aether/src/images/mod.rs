use image::{DynamicImage, GenericImageView};
use std::error::Error;
use taffy::geometry::Size;

pub struct ImageRenderer {
    pub image: DynamicImage,
}

impl ImageRenderer {
    /// Fetch remote images using reqwest and decode using the image crate
    pub async fn fetch_and_decode(url: &str) -> Result<Self, Box<dyn Error>> {
        let bytes = reqwest::get(url).await?.bytes().await?;
        let image = image::load_from_memory(&bytes)?;
        Ok(Self { image })
    }

    /// Expose natural dimensions to taffy layout
    pub fn natural_dimensions(&self) -> Size<f32> {
        let (width, height) = self.image.dimensions();
        Size {
            width: width as f32,
            height: height as f32,
        }
    }
}
