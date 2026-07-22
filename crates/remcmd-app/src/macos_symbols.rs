use std::sync::{Arc, OnceLock};

use gpui::{Image, ImageFormat};
use objc2::AnyThread;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSColor, NSImage,
    NSImageSymbolConfiguration, NSImageSymbolScale,
};
use objc2_foundation::{NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString};

const RASTER_SIZE: f64 = 54.0;

pub(crate) fn close_circle(dark: bool) -> Option<Arc<Image>> {
    static LIGHT_SYMBOL: OnceLock<Option<Arc<Image>>> = OnceLock::new();
    static DARK_SYMBOL: OnceLock<Option<Arc<Image>>> = OnceLock::new();

    let cache = if dark { &DARK_SYMBOL } else { &LIGHT_SYMBOL };
    cache.get_or_init(|| render_close_circle(dark)).clone()
}

fn render_close_circle(dark: bool) -> Option<Arc<Image>> {
    let name = NSString::from_str("xmark.circle.fill");
    let description = NSString::from_str("Close");
    let symbol =
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&name, Some(&description))?;
    let scale = NSImageSymbolConfiguration::configurationWithScale(NSImageSymbolScale::Large);
    let mark = NSColor::colorWithWhite_alpha(if dark { 0.16 } else { 0.98 }, 1.0);
    let circle = NSColor::colorWithWhite_alpha(if dark { 0.82 } else { 0.32 }, 1.0);
    // SF Symbols exposes the cutout mark before the enclosing circle in this palette.
    let colors = NSArray::from_slice(&[&*mark, &*circle]);
    let palette = NSImageSymbolConfiguration::configurationWithPaletteColors(&colors);
    let configuration = scale.configurationByApplyingConfiguration(&palette);
    let symbol = symbol.imageWithSymbolConfiguration(&configuration)?;

    let raster_size = NSSize::new(RASTER_SIZE, RASTER_SIZE);
    let raster = NSImage::initWithSize(NSImage::alloc(), raster_size);
    #[allow(deprecated)]
    {
        raster.lockFocus();
        symbol.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), raster_size));
        raster.unlockFocus();
    }

    let tiff = raster.TIFFRepresentation()?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
    // SAFETY: An empty property dictionary is valid for PNG encoding.
    let data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }?;
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, data.to_vec())))
}
