use gftools_builder::{BuildConfig, build};
use serial_test::serial;
use std::{
    env,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Helper function to get the path to test resources
fn test_resources_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")
}

/// Helper function to copy a directory recursively
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            std::fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_radio_canada_build() {
    // Set up logging for the test
    let _ = env_logger::builder().is_test(true).try_init();

    // Create a temporary directory for the build
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let test_dir = temp_dir.path().join("radio-canada");

    // Copy the test resources to the temp directory
    let source_dir = test_resources_dir().join("radio-canada");
    copy_dir_recursive(&source_dir, &test_dir).expect("Failed to copy test resources");

    // Build the config path
    let config_path = test_dir.join("config.yaml");
    assert!(config_path.exists(), "Config file should exist");

    // Verify that the Glyphs files were copied
    let glyphs_files = [
        test_dir.join("RadioCanadaDisplay.glyphs"),
        test_dir.join("RadioCanadaDisplay-Italic.glyphs"),
    ];
    for file in &glyphs_files {
        assert!(
            file.exists(),
            "Glyphs file should be copied: {}",
            file.display()
        );
    }

    // Save the current directory so we can restore it
    let original_dir = std::env::current_dir().expect("Failed to get current directory");

    // Create build configuration
    let build_config = BuildConfig {
        config_path: config_path.to_string_lossy().to_string(),
        job_limit: 2, // Use fewer jobs for testing
        generate_only: false,
        #[cfg(feature = "graphviz")]
        draw_graph: false,
        ascii_graph: false,
        debug_intermediates: false,
        verbosity: log::Level::Error,
        progress: false,
    };

    // Run the build
    build(build_config).await.unwrap();

    let fonts_dir = test_dir.join("../fonts");

    // Check variable fonts
    let vf_dir = fonts_dir.join("variable");
    assert!(vf_dir.exists());
    let roman_vf = vf_dir.join("RadioCanadaDisplay[wght].ttf");
    assert!(roman_vf.exists(), "Roman variable font should be built");
    assert!(
        roman_vf.metadata().unwrap().len() > 1000,
        "Roman VF should have reasonable size"
    );

    let italic_vf = vf_dir.join("RadioCanadaDisplay-Italic[wght].ttf");
    assert!(italic_vf.exists(), "Italic variable font should be built");

    // Check webfonts (WOFF2)
    let webfonts_dir = fonts_dir.join("webfonts");
    assert!(webfonts_dir.exists());
    let roman_woff2 = webfonts_dir.join("RadioCanadaDisplay[wght].woff2");
    assert!(roman_woff2.exists(), "Roman WOFF2 webfont should be built");

    let italic_woff2 = webfonts_dir.join("RadioCanadaDisplay-Italic[wght].woff2");
    assert!(
        italic_woff2.exists(),
        "Italic WOFF2 webfont should be built"
    );

    // Verify WOFF2 files are smaller than TTF (compression working)
    let roman_ttf_size = roman_vf.metadata().unwrap().len();
    let roman_woff2_size = roman_woff2.metadata().unwrap().len();
    assert!(
        roman_woff2_size < roman_ttf_size,
        "WOFF2 should be smaller than TTF: {} < {}",
        roman_woff2_size,
        roman_ttf_size
    );

    println!("✓ Build succeeded!");
    println!("✓ Roman VF: {} bytes", roman_ttf_size);
    println!(
        "✓ Roman WOFF2: {} bytes ({:.1}% of TTF)",
        roman_woff2_size,
        (roman_woff2_size as f64 / roman_ttf_size as f64) * 100.0
    );
    let _ = std::env::set_current_dir(&original_dir);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_radio_canada_generate_recipe() {
    // Set up logging for the test
    let _ = env_logger::builder().is_test(true).try_init();

    // Create a temporary directory for the test
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let test_dir = temp_dir.path().join("radio-canada");

    // Copy the test resources to the temp directory
    let source_dir = test_resources_dir().join("radio-canada");
    copy_dir_recursive(&source_dir, &test_dir).expect("Failed to copy test resources");

    let config_path = test_dir.join("config.yaml");

    // Save the current directory so we can restore it
    let original_dir = std::env::current_dir().expect("Failed to get current directory");

    // Create build configuration for recipe generation
    let build_config = BuildConfig {
        config_path: config_path.to_string_lossy().to_string(),
        job_limit: 1,
        generate_only: true,
        #[cfg(feature = "graphviz")]
        draw_graph: false,
        ascii_graph: false,
        debug_intermediates: false,
        verbosity: log::Level::Error,
        progress: false,
    };

    // Run recipe generation (should just print, not build)
    let result = build(build_config).await;

    // Restore the original directory
    let _ = std::env::set_current_dir(&original_dir);

    assert!(
        result.is_ok(),
        "Recipe generation should succeed: {:?}",
        result.err()
    );

    // We can't easily capture stdout in this test, but at least we verify it doesn't crash
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_radio_canada_ascii_graph() {
    // Set up logging for the test
    let _ = env_logger::builder().is_test(true).try_init();

    // Create a temporary directory for the test
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let test_dir = temp_dir.path().join("radio-canada");

    // Copy the test resources to the temp directory
    let source_dir = test_resources_dir().join("radio-canada");
    copy_dir_recursive(&source_dir, &test_dir).expect("Failed to copy test resources");

    let config_path = test_dir.join("config.yaml");

    // Save the current directory so we can restore it
    let original_dir = std::env::current_dir().expect("Failed to get current directory");

    // Create build configuration for ASCII graph generation
    let build_config = BuildConfig {
        config_path: config_path.to_string_lossy().to_string(),
        job_limit: 1,
        generate_only: false,
        #[cfg(feature = "graphviz")]
        draw_graph: false,
        ascii_graph: true,
        debug_intermediates: false,
        verbosity: log::Level::Error,
        progress: false,
    };

    // Run ASCII graph generation
    let result = build(build_config).await;

    // Restore the original directory
    let _ = std::env::set_current_dir(&original_dir);

    assert!(
        result.is_ok(),
        "ASCII graph generation should succeed: {:?}",
        result.err()
    );
}
