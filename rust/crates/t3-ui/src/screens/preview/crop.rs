//! Crop the viewer's JPEG frame to the marked region and encode the crop as PNG.

use t3_client::preview::annotation::AnnotationRegion;

/// `None` when the bytes are not a decodable JPEG, or the region leaves the decoded frame.
pub(super) fn crop_png(jpeg: &[u8], region: AnnotationRegion) -> Option<Vec<u8>> {
    let mut decoder =
        zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(jpeg));
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    let channels = pixels.len().checked_div(width.checked_mul(height)?)?;
    let color = match channels {
        1 => png::ColorType::Grayscale,
        3 => png::ColorType::Rgb,
        _ => return None,
    };
    let (x, y) = (region.x as usize, region.y as usize);
    let (w, h) = (region.width as usize, region.height as usize);
    if w == 0 || h == 0 || x.checked_add(w)? > width || y.checked_add(h)? > height {
        return None;
    }
    let mut crop = Vec::with_capacity(w * h * channels);
    for row in y..y + h {
        let start = (row * width + x) * channels;
        crop.extend_from_slice(pixels.get(start..start + w * channels)?);
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, region.width, region.height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().ok()?.write_image_data(&crop).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use t3_client::preview::annotation::png_size;

    use super::*;

    const FRAME: &[u8] = include_bytes!("../../../tests/ui/prv_2.jpg");

    fn frame_size() -> (u32, u32) {
        let mut decoder =
            zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(FRAME));
        decoder.decode().expect("fixture decodes");
        let info = decoder.info().expect("fixture info");
        (u32::from(info.width), u32::from(info.height))
    }

    #[test]
    fn the_crop_has_the_regions_size_and_the_frames_pixels() {
        let (width, height) = frame_size();
        let region = AnnotationRegion {
            x: width / 4,
            y: height / 4,
            width: width / 2,
            height: height / 2,
        };
        let png = crop_png(FRAME, region).expect("crop");
        assert_eq!(png_size(&png), Some((region.width, region.height)));
        let mut reader = png::Decoder::new(png.as_slice())
            .read_info()
            .expect("png decodes");
        let mut cropped = vec![0; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut cropped).expect("png frame");
        let mut full =
            zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(FRAME));
        let pixels = full.decode().expect("frame");
        let channels = frame.line_size / region.width as usize;
        let at = |x: u32, y: u32| ((y * width + x) as usize) * channels;
        let first = at(region.x, region.y);
        assert_eq!(cropped[..channels], pixels[first..first + channels]);
        let last_in_crop =
            (region.height as usize - 1) * frame.line_size + (region.width as usize - 1) * channels;
        let last = at(region.x + region.width - 1, region.y + region.height - 1);
        assert_eq!(
            cropped[last_in_crop..last_in_crop + channels],
            pixels[last..last + channels]
        );
    }

    #[test]
    fn a_region_outside_the_frame_or_bytes_that_are_not_a_jpeg_crop_nothing() {
        let (width, height) = frame_size();
        let outside = AnnotationRegion {
            x: width - 1,
            y: 0,
            width: 2,
            height: 1,
        };
        assert_eq!(crop_png(FRAME, outside), None);
        let empty = AnnotationRegion {
            x: 0,
            y: 0,
            width: 0,
            height: 1,
        };
        assert_eq!(crop_png(FRAME, empty), None);
        let any = AnnotationRegion {
            x: 0,
            y: height - 1,
            width: 1,
            height: 1,
        };
        assert_eq!(crop_png(b"not a jpeg", any), None);
    }
}
