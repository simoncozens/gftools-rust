//! Generate an OFL.txt license document from font copyright strings.
//!
//! ```sh
//! gftools-build-ofl 'Gulzar-Regular.ttf' out/
//! ```
//!
//! Like the Python, the license is built from the *first* font given, and
//! `OFL.txt` is written into the output directory (which has to exist).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use gftools::license::generate_ofl_license;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Generate an OFL.txt license document from font copyright strings
struct Args {
    /// Fonts in OpenType (TTF/OTF) format; the license comes from the first
    #[arg(required = true)]
    fonts: Vec<PathBuf>,
    /// Directory to write OFL.txt into
    out_dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(&args)
}

fn run(args: &Args) -> Result<()> {
    let Some(font_path) = args.fonts.first() else {
        anyhow::bail!("At least one font is needed to build a license");
    };
    let data = std::fs::read(font_path)
        .with_context(|| format!("Failed to read font file: {}", font_path.display()))?;
    let font = skrifa::FontRef::new(&data)
        .with_context(|| format!("Failed to parse font file: {}", font_path.display()))?;

    let license = generate_ofl_license(&font)?;
    let destination = args.out_dir.join("OFL.txt");
    std::fs::write(&destination, license)
        .with_context(|| format!("Failed to write {}", destination.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GULZAR: &str = "../gftools-lib/resources/test/gulzar/Gulzar-Regular.ttf";
    const GULZAR_BYLINE: &str =
        "Copyright 2021 The Gulzar Project Authors (http://github.com/googlefonts/gulzar/)";
    /// The Maven Pro notice asks for a Reserved Font Name.
    const MAVEN_PRO: &str =
        "../gftools-onboarder-tools/data/test/gf_fonts/ofl/mavenpro/MavenPro[wght].ttf";
    /// `gftools-lib/resources/OFL.txt`, which is the Python's `OFL_BODY_TEXT`.
    const OFL_BODY_LEN: usize = 4302;

    #[test]
    fn test_arguments() {
        // The Python takes `fonts... out_dir`: the last argument is the
        // directory, everything before it is a font.
        let args = Args::try_parse_from([
            "gftools-build-ofl",
            "Gulzar-Regular.ttf",
            "Gulzar-Bold.ttf",
            "/tmp/out",
        ])
        .unwrap();
        assert_eq!(
            args.fonts,
            vec![
                PathBuf::from("Gulzar-Regular.ttf"),
                PathBuf::from("Gulzar-Bold.ttf")
            ]
        );
        assert_eq!(args.out_dir, PathBuf::from("/tmp/out"));
        // A font without a directory to write to is an error, as it is for
        // argparse.
        assert!(Args::try_parse_from(["gftools-build-ofl", "Gulzar-Regular.ttf"]).is_err());
    }

    #[test]
    fn test_writes_a_license() {
        let tmp = tempfile::tempdir().unwrap();
        let out_dir = tmp.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();

        run(&Args {
            fonts: vec![PathBuf::from(GULZAR)],
            out_dir: out_dir.clone(),
        })
        .unwrap();

        let written = std::fs::read_to_string(out_dir.join("OFL.txt")).unwrap();
        assert!(written.starts_with(&format!("{GULZAR_BYLINE}\n\nThis Font Software")));
        // Byline, the newline the Python's f-string adds, then the OFL body.
        assert_eq!(written.len(), GULZAR_BYLINE.len() + 1 + OFL_BODY_LEN);
        assert!(written.ends_with("DEALINGS IN THE FONT SOFTWARE."));
    }

    #[test]
    fn test_out_dir_has_to_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let error = run(&Args {
            fonts: vec![PathBuf::from(GULZAR)],
            out_dir: tmp.path().join("nowhere"),
        })
        .unwrap_err();
        assert!(error.to_string().contains("Failed to write"));
    }

    #[test]
    fn test_reserved_font_name() {
        let tmp = tempfile::tempdir().unwrap();
        let error = run(&Args {
            fonts: vec![PathBuf::from(MAVEN_PRO)],
            out_dir: tmp.path().to_path_buf(),
        })
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Font copyright has Reserved Font Name"));
    }
}
