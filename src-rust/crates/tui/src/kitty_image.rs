// kitty_image.rs — Inline image rendering via Kitty graphics protocol or Sixel.
//
// Strategy:
//   1. Detect which image protocol the terminal supports:
//      - Kitty: $TERM contains "kitty" OR $TERM_PROGRAM is "WezTerm"
//      - Sixel: $TERM contains xterm/screen/rxvt/mintty/iterm2
//      - Text: fallback to human-readable description
//   2. If base64 PNG/JPEG data is available:
//      - Kitty: emit APC escape sequence directly
//      - Sixel: decode base64 → PNG/JPEG → convert to Sixel → emit escape sequence
//      - Text: return a placeholder string
//   3. For URL sources: always fall back to text (no remote fetching)
//
// Kitty graphics protocol (APC sequence):
//   ESC _ G a=T,f=<fmt>,m=0,q=2,C=1 ; <base64-data> ESC \
//
// Sixel escape sequence:
//   ESC P q ... ESC \

use claurst_core::ImageSource;
use std::io::Write;

/// Maximum bytes per Kitty APC chunk.
const KITTY_CHUNK_SIZE: usize = 4096;

/// Maximum bytes per Sixel line (conservative limit for terminal compatibility).
const SIXEL_LINE_SIZE: usize = 1024;

// ---------------------------------------------------------------------------
// Image Protocol Detection
// ---------------------------------------------------------------------------

/// Supported image protocols in order of preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    Kitty,
    Sixel,
    Text,
}

/// Detect which image protocol the running terminal supports.
///
/// Returns:
/// - `ImageProtocol::Kitty` if $TERM contains "kitty" or $TERM_PROGRAM is "WezTerm"
/// - `ImageProtocol::Sixel` if $TERM contains xterm/screen/rxvt/mintty/iterm
/// - `ImageProtocol::Text` as fallback
pub fn detect_image_protocol() -> ImageProtocol {
    // Check for Kitty protocol (highest priority)
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("kitty") {
            return ImageProtocol::Kitty;
        }
    }

    if let Ok(prog) = std::env::var("TERM_PROGRAM") {
        if prog.eq_ignore_ascii_case("WezTerm") {
            return ImageProtocol::Kitty;
        }
    }

    // Check for Sixel protocol (medium priority)
    if let Ok(term) = std::env::var("TERM") {
        // xterm variants, screen/tmux, rxvt variants, mintty, iterm2
        if term.contains("xterm")
            || term.contains("screen")
            || term.contains("rxvt")
            || term.contains("mintty")
            || term.contains("iterm")
        {
            return ImageProtocol::Sixel;
        }
    }

    // Fallback to text
    ImageProtocol::Text
}

// Kept for backward compatibility
pub fn supports_kitty_graphics() -> bool {
    detect_image_protocol() == ImageProtocol::Kitty
}

// ---------------------------------------------------------------------------
// Core Rendering
// ---------------------------------------------------------------------------

/// Attempt to render `source` as an inline image.
///
/// * If an image protocol is available and the source carries base64 data,
///   the appropriate escape sequence is written to `stdout` and `None` is
///   returned (caller should skip adding a text line).
/// * Otherwise a human-readable fallback string is returned for display
///   as a normal text span.
///
/// The caller must flush stdout after this call when `None` is returned.
pub fn render_image(source: &ImageSource) -> Option<String> {
    // URL-type sources: never fetch remote URLs — fall back to text
    if source.source_type == "url" {
        let url = source.url.as_deref().unwrap_or("(no url)");
        return Some(format!("[Image: {}]", url));
    }

    // base64 data source
    if let Some(data) = &source.data {
        let protocol = detect_image_protocol();

        match protocol {
            ImageProtocol::Kitty => {
                emit_kitty_apc(data, source.media_type.as_deref());
                return None; // successfully emitted — caller skips text line
            }
            ImageProtocol::Sixel => {
                if emit_sixel(data, source.media_type.as_deref()) {
                    return None; // successfully emitted — caller skips text line
                }
                // Fall through to text if Sixel conversion fails
            }
            ImageProtocol::Text => {
                // Fall through to generate fallback text
            }
        }

        // Fallback: describe the type and rough size
        let media = source.media_type.as_deref().unwrap_or("image");
        let size_kb = (data.len() * 3 / 4) / 1024; // rough decoded byte count
        if size_kb > 0 {
            return Some(format!("[Image: {} ~{}KB]", media, size_kb));
        }
        return Some(format!("[Image: {}]", media));
    }

    // No data, no URL
    Some("[Image: embedded image]".to_string())
}

// ---------------------------------------------------------------------------
// Kitty Graphics Protocol (APC)
// ---------------------------------------------------------------------------

/// Returns the Kitty format parameter for a base64-encoded image payload.
///
/// Kitty `f=100` accepts a base64-encoded PNG or JPEG directly; the terminal
/// auto-detects the image type from the data header.  We always use `f=100`.
fn kitty_format(_media_type: Option<&str>) -> u8 {
    100
}

/// Emit the full Kitty graphics APC sequence for a base64-encoded image.
///
/// The base64 string is split into `KITTY_CHUNK_SIZE`-byte chunks and each
/// chunk is wrapped in the appropriate APC escape.  Everything is written
/// directly to `stdout`; the caller must flush.
fn emit_kitty_apc(base64_data: &str, media_type: Option<&str>) {
    let fmt = kitty_format(media_type);
    let mut stdout = std::io::stdout();

    // Strip any whitespace/newlines that may have been inserted into the
    // base64 string (the API sometimes line-wraps it).
    let clean: String = base64_data.chars().filter(|c| !c.is_whitespace()).collect();

    let chunks: Vec<&str> = clean
        .as_bytes()
        .chunks(KITTY_CHUNK_SIZE)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();

    if chunks.is_empty() {
        return;
    }

    let total = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        let first = i == 0;
        let last = i == total - 1;
        let more = if last { 0u8 } else { 1 };

        let params = if first {
            format!("a=T,f={},m={},q=2,C=1", fmt, more)
        } else {
            format!("a=T,m={},q=2", more)
        };

        // Write the APC sequence: ESC _ G <params> ; <base64-chunk> ESC \
        let _ = write!(stdout, "\x1b_G{};{}\x1b\\", params, chunk);
    }

    // Move to a new line so subsequent ratatui output begins cleanly.
    let _ = write!(stdout, "\r\n");
    let _ = stdout.flush();
}

// ---------------------------------------------------------------------------
// Sixel Protocol
// ---------------------------------------------------------------------------

/// Decode base64-encoded image data and convert to Sixel protocol.
///
/// Returns `true` if successful and the Sixel escape sequence was written to stdout.
/// Returns `false` if decoding or conversion fails.
fn emit_sixel(base64_data: &str, _media_type: Option<&str>) -> bool {
    // Decode base64
    let decoded = match decode_base64(base64_data) {
        Ok(bytes) => bytes,
        Err(_) => {
            return false;
        }
    };

    // Decode image data (PNG or JPEG)
    let img_data = match decode_image_data(&decoded) {
        Ok(data) => data,
        Err(_) => {
            return false;
        }
    };

    // Convert to Sixel using icy_sixel
    let sixel_bytes = match convert_to_sixel(&img_data) {
        Ok(data) => data,
        Err(_) => {
            return false;
        }
    };

    // Emit Sixel escape sequence
    emit_sixel_sequence(&sixel_bytes);
    true
}

/// Decode base64 string, stripping whitespace.
fn decode_base64(base64_data: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use base64::Engine;

    // Strip whitespace
    let clean: String = base64_data.chars().filter(|c| !c.is_whitespace()).collect();
    // Decode using base64 crate
    let decoded = base64::engine::general_purpose::STANDARD.decode(&clean)?;
    Ok(decoded)
}

/// Decoded image data with dimensions in RGBA format.
#[derive(Debug)]
struct ImageData {
    pixels: Vec<u8>, // RGBA format, 4 bytes per pixel
    width: u32,
    height: u32,
}

/// Decode PNG or JPEG image data into RGBA pixels.
fn decode_image_data(data: &[u8]) -> Result<ImageData, Box<dyn std::error::Error>> {
    // Try to detect PNG or JPEG by magic bytes and decode accordingly.

    // PNG magic: 89 50 4E 47
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return decode_png(data);
    }

    // JPEG magic: FF D8 FF
    if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return decode_jpeg(data);
    }

    // Try PNG first, then JPEG as fallbacks
    decode_png(data).or_else(|_| decode_jpeg(data))
}

/// Decode PNG image data into RGBA pixels.
///
/// Uses the `image` crate to decode PNG data and convert to RGBA8 format.
/// Returns an error if decoding fails.
fn decode_png(data: &[u8]) -> Result<ImageData, Box<dyn std::error::Error>> {
    use image::ImageReader;
    use std::io::Cursor;

    // Decode the PNG using the image crate with explicit format hint
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    let image = reader
        .decode()
        .map_err(|e| format!("Failed to decode PNG: {}", e))?;

    // Convert to RGBA8 format
    let rgba_image = image.to_rgba8();
    let (width, height) = rgba_image.dimensions();
    let pixels = rgba_image.into_raw();

    Ok(ImageData {
        pixels,
        width,
        height,
    })
}

/// Decode JPEG image data into RGBA pixels.
///
/// Uses the `image` crate to decode JPEG data and convert to RGBA8 format.
/// Returns an error if decoding fails.
fn decode_jpeg(data: &[u8]) -> Result<ImageData, Box<dyn std::error::Error>> {
    use image::ImageReader;
    use std::io::Cursor;

    // Decode the JPEG using the image crate with explicit format hint
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    let image = reader
        .decode()
        .map_err(|e| format!("Failed to decode JPEG: {}", e))?;

    // Convert to RGBA8 format
    let rgba_image = image.to_rgba8();
    let (width, height) = rgba_image.dimensions();
    let pixels = rgba_image.into_raw();

    Ok(ImageData {
        pixels,
        width,
        height,
    })
}

/// Convert RGBA image data to Sixel format using the icy_sixel library.
///
/// The icy_sixel crate provides high-quality color quantization and dithering
/// to convert true-color images down to the 256-color palette supported by Sixel.
fn convert_to_sixel(img_data: &ImageData) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use icy_sixel::encoder::EncodeOptions;

    // Use the simpler sixel_encode function with default options
    let sixel_string = icy_sixel::encoder::sixel_encode(
        &img_data.pixels,
        img_data.width as usize,
        img_data.height as usize,
        &EncodeOptions::default(),
    )?;

    // Convert the Sixel string to bytes for output
    Ok(sixel_string.into_bytes())
}

/// Emit Sixel escape sequence to stdout.
///
/// The sixel_output from icy_sixel is the raw Sixel data (without delimiters).
/// We wrap it with the proper escape sequence: ESC P ... ESC \
fn emit_sixel_sequence(sixel_data: &[u8]) {
    let mut stdout = std::io::stdout();

    // Write the escape sequence introduction: ESC P q
    let _ = write!(stdout, "\x1bPq");

    // Write sixel data in chunks to respect terminal line limits
    for chunk in sixel_data.chunks(SIXEL_LINE_SIZE) {
        if let Ok(s) = std::str::from_utf8(chunk) {
            let _ = write!(stdout, "{}", s);
        }
    }

    // End sequence: ESC \
    let _ = write!(stdout, "\x1b\\");

    // Move to a new line so subsequent output begins cleanly
    let _ = write!(stdout, "\r\n");
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that we can decode a minimal valid PNG image.
    /// This is a 1x1 transparent PNG (smallest possible valid PNG).
    #[test]
    fn test_decode_minimal_png() {
        // Minimal 1x1 transparent PNG created with:
        // echo -ne '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82' > test.png
        let png_data = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        let result = decode_png(&png_data);
        assert!(result.is_ok(), "PNG decoding should succeed");

        let img = result.unwrap();
        assert_eq!(img.width, 1, "PNG width should be 1");
        assert_eq!(img.height, 1, "PNG height should be 1");
        assert_eq!(img.pixels.len(), 4, "RGBA8 1x1 image should have 4 bytes");
    }

    /// Test that decode_image_data correctly identifies and decodes PNG by magic bytes.
    #[test]
    fn test_decode_image_data_detects_png() {
        let png_data = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        let result = decode_image_data(&png_data);
        assert!(
            result.is_ok(),
            "decode_image_data should detect and decode PNG"
        );

        let img = result.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 1);
    }

    /// Test that invalid image data produces an error.
    #[test]
    fn test_decode_invalid_image() {
        let invalid_data = vec![0x00, 0x00, 0x00, 0x00];
        let result = decode_image_data(&invalid_data);
        assert!(
            result.is_err(),
            "Invalid image data should produce an error"
        );
    }

    /// Test that decode_image_data rejects empty data.
    #[test]
    fn test_decode_empty_data() {
        let empty_data = vec![];
        let result = decode_image_data(&empty_data);
        assert!(result.is_err(), "Empty data should produce an error");
    }
}
