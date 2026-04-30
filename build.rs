fn main() {
    generate_splash();
    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

/// Convert assets/splash.png → OUT_DIR/splash.bin at compile time.
///
/// Output format: 1 bit per pixel, MSB = leftmost pixel, row-major, 128×64 = 1024 bytes.
/// This matches `embedded-graphics` `ImageRaw<BinaryColor>` with big-endian bit order.
fn generate_splash() {
    use image::{GrayImage, ImageReader, imageops};

    const W: u32 = 128;
    const H: u32 = 64;

    let src = std::path::Path::new("assets/splash.png");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dst = std::path::Path::new(&out_dir).join("splash.bin");

    println!("cargo:rerun-if-changed=assets/splash.png");

    let img = ImageReader::open(src)
        .expect("assets/splash.png not found")
        .decode()
        .expect("failed to decode assets/splash.png");

    // Image encodes drawing in the alpha channel (alpha=255 → drawn, alpha=0 → transparent).
    // Use alpha directly as luma: drawn pixels become white (lit), background becomes black.
    let rgba = img.to_rgba8();
    let alpha_as_luma: GrayImage = image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
        image::Luma([rgba.get_pixel(x, y)[3]])
    });
    // Resize to exactly 128×64.
    let img: GrayImage = imageops::resize(&alpha_as_luma, W, H, imageops::FilterType::Lanczos3);

    // Pack 8 pixels per byte, MSB first, threshold at 128
    let mut buf = vec![0u8; (W * H / 8) as usize];
    for y in 0..H {
        for x in 0..W {
            let pixel = img.get_pixel(x, y)[0];
            if pixel >= 128 {
                let bit_index = (y * W + x) as usize;
                buf[bit_index / 8] |= 0x80 >> (bit_index % 8);
            }
        }
    }

    std::fs::write(&dst, &buf).expect("failed to write splash.bin");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
