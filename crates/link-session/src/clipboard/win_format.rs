//! Windows clipboard formats to and from the protocol's types: `CF_HTML` ("HTML Format") and `CF_DIB`. Portable, so the
//! tests run on every OS.

/// The fragment of `CF_HTML` data, between its `StartFragment` and `EndFragment` byte offsets. An end past the data or
/// before the start stops at the data's end: some writers get the end offsets wrong.
pub fn html_of_cf_html(data: &[u8]) -> Option<String> {
    let header = &data[..data.len().min(1024)];
    let offset = |name: &str| -> Option<usize> {
        let header = String::from_utf8_lossy(header);
        let line = header.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?;
        line.trim().parse().ok()
    };
    // A clipboard block can be larger than the text it holds; UTF-8 HTML has no NUL of its own.
    let data = &data[..data.iter().position(|&byte| byte == 0).unwrap_or(data.len())];
    let start = offset("StartFragment").filter(|start| *start <= data.len())?;
    let end = offset("EndFragment").filter(|end| (start..=data.len()).contains(end)).unwrap_or(data.len());
    Some(String::from_utf8_lossy(&data[start..end]).into_owned())
}

/// `fragment` as `CF_HTML`: a version 0.9 header with byte offsets, the fragment wrapped in a document.
pub fn cf_html_of_html(fragment: &str) -> Vec<u8> {
    const HEADER: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\n\
                          StartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    const BEFORE: &str = "<html><body>\r\n<!--StartFragment-->";
    const AFTER: &str = "<!--EndFragment-->\r\n</body></html>";
    let start_html = HEADER.len();
    let start_fragment = start_html + BEFORE.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + AFTER.len();
    let header = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
         StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );
    [header.as_str(), BEFORE, fragment, AFTER].concat().into_bytes()
}

const INFO_HEADER: usize = 40;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// `BI_BITFIELDS` masks of the only 32-bit layout read: blue, green, red bytes, then an unused or alpha byte.
const BGR_MASKS: [u32; 3] = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF];

/// A 24- or 32-bit `CF_DIB` as PNG. 32-bit pixels whose alpha is zero everywhere count as opaque: most writers leave
/// that byte unset.
pub fn png_of_dib(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    let header = usize::try_from(u32_at(0)?).ok().filter(|size| *size >= INFO_HEADER)?;
    let width = u32_at(4)?.cast_signed();
    let height = u32_at(8)?.cast_signed();
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let mut pixels = header;
    match (bits, compression) {
        (24 | 32, BI_RGB) => {}
        (32, BI_BITFIELDS) => {
            if header == INFO_HEADER {
                pixels += 12;
            }
            if [u32_at(INFO_HEADER)?, u32_at(INFO_HEADER + 4)?, u32_at(INFO_HEADER + 8)?] != BGR_MASKS {
                return None;
            }
        }
        _ => return None,
    }
    let (width, rows) = (usize::try_from(width).ok().filter(|width| *width > 0)?, height.unsigned_abs() as usize);
    let bytes = usize::from(bits / 8);
    let stride = (width * bytes).div_ceil(4) * 4;
    let data = dib.get(pixels..pixels.checked_add(stride.checked_mul(rows)?)?)?;
    let mut rgba = Vec::with_capacity(width * rows * 4);
    for row in 0..rows {
        let source = if height > 0 { rows - 1 - row } else { row };
        for pixel in data[source * stride..][..width * bytes].chunks_exact(bytes) {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], if bytes == 4 { pixel[3] } else { 255 }]);
        }
    }
    if bytes == 4 && rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| pixel[3] = 255);
    }
    let image = image::RgbaImage::from_raw(u32::try_from(width).ok()?, u32::try_from(rows).ok()?, rgba)?;
    let mut png = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

/// A PNG or JPEG as a 32-bit bottom-up `CF_DIB`.
pub fn dib_of_image(encoded: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(encoded).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    let size = u32::try_from(image.as_raw().len()).ok()?;
    let mut dib = Vec::with_capacity(INFO_HEADER + image.as_raw().len());
    dib.extend_from_slice(&u32::try_from(INFO_HEADER).ok()?.to_le_bytes());
    dib.extend_from_slice(&width.cast_signed().to_le_bytes());
    dib.extend_from_slice(&height.cast_signed().to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&BI_RGB.to_le_bytes());
    dib.extend_from_slice(&size.to_le_bytes());
    dib.extend_from_slice(&[0; 16]);
    for row in image.rows().rev() {
        for pixel in row {
            let [red, green, blue, alpha] = pixel.0;
            dib.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    Some(dib)
}

/// `CF_UNICODETEXT` bytes up to the first NUL, with Windows line ends as `\n`.
pub fn text_of_utf16(data: &[u8]) -> String {
    let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit)).collect();
    let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end]).replace("\r\n", "\n")
}

/// `text` as NUL-terminated `CF_UNICODETEXT` bytes with `\r\n` line ends, which edit controls need.
pub fn utf16_of_text(text: &str) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// The length of `png` up to the end of its IEND chunk; a clipboard block can be larger than the image it holds.
pub fn png_len(png: &[u8]) -> Option<usize> {
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut at = 8_usize;
    loop {
        let len = usize::try_from(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?)).ok()?;
        let end = at.checked_add(12)?.checked_add(len)?;
        png.get(..end)?;
        if png.get(at + 4..at + 8)? == b"IEND" {
            return Some(end);
        }
        at = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_crosses_cf_html_both_ways_and_a_bad_end_offset_stops_at_the_data() {
        let fragment = "<b>Grüße</b> & <i>more</i>";
        assert_eq!(html_of_cf_html(&cf_html_of_html(fragment)).as_deref(), Some(fragment));
        let written = b"Version:1.0\r\nStartHTML:0000000105\r\nEndHTML:0000000999\r\nStartFragment:0000000105\r\n\
                        EndFragment:0000000999\r\n<p>cut</p>\0";
        assert_eq!(html_of_cf_html(written).as_deref(), Some("<p>cut</p>"));
        assert_eq!(html_of_cf_html(b"no header"), None);
    }

    /// A 3x2 24-bit bottom-up DIB: rows padded from 9 to 12 bytes, the last row in the data the top one.
    fn dib_24() -> Vec<u8> {
        let mut dib = vec![0; INFO_HEADER];
        dib[0] = 40;
        dib[4] = 3;
        dib[8] = 2;
        dib[12] = 1;
        dib[14] = 24;
        dib.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]);
        dib.extend_from_slice(&[0, 0, 0, 255, 255, 255, 1, 2, 3, 0, 0, 0]);
        dib
    }

    #[test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    fn a_dib_becomes_a_png_with_its_rows_in_order_and_back() {
        let png = png_of_dib(&dib_24()).expect("a PNG");
        let image = image::load_from_memory(&png).expect("decodes").into_rgba8();
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 255], "the top row is the last in the data");
        assert_eq!(image.get_pixel(2, 0).0, [3, 2, 1, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [0, 0, 255, 255], "blue, green, red in the data");
        assert_eq!(image.get_pixel(2, 1).0, [255, 0, 0, 255]);
        let again = png_of_dib(&dib_of_image(&png).expect("a DIB")).expect("a PNG again");
        assert_eq!(image::load_from_memory(&again).expect("decodes").into_rgba8(), image);
    }

    #[test]
    fn a_dib_too_short_for_its_size_or_of_another_depth_is_refused() {
        let dib = dib_24();
        assert_eq!(png_of_dib(&dib[..dib.len() - 1]), None);
        let mut paletted = dib;
        paletted[14] = 8;
        assert_eq!(png_of_dib(&paletted), None);
    }

    #[test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    fn padding_past_a_png_or_html_is_cut_and_text_crosses_with_windows_line_ends() {
        let png = png_of_dib(&dib_24()).expect("a PNG");
        let mut padded = png.clone();
        padded.extend_from_slice(&[0, 7, 7, 7]);
        assert_eq!(png_len(&padded), Some(png.len()));
        assert_eq!(png_len(&png[..png.len() - 1]), None, "a PNG cut before its IEND");
        assert_eq!(html_of_cf_html(b"StartFragment:0000000026\r\n<p>x</p>\0\0junk").as_deref(), Some("<p>x</p>"));
        let windows: Vec<u8> = "a\r\nb\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(utf16_of_text("a\nb"), windows);
        assert_eq!(text_of_utf16(&windows), "a\nb");
    }
}
