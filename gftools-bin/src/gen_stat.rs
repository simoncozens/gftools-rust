//! Generate a STAT table for each font in a variable font family.
//!
//! ```sh
//! # From the Google Fonts axis registry, in place
//! gftools-gen-stat 'Roboto[wdth,wght].ttf' --inplace
//!
//! # From a YAML config, into a directory
//! gftools-gen-stat font*.ttf --src my_stat.yaml --out ~/Desktop/out
//! ```
//!
//! Fonts are written to `<font>.fix` unless `--inplace` or `--out` is given.
//!
//! The `--src` file holds either a list of axis dictionaries for the whole
//! family, or a dictionary mapping font file names to such a list:
//!
//! ```yaml
//! Lora[wght].ttf:
//! - name: Weight
//!   tag: wght
//!   values:
//!   - name: Regular
//!     value: 400
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use gftools::{StatConfig, VarFont, gen_stat_tables, gen_stat_tables_from_config};

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Generate a STAT table for each font in a variable font family
struct Args {
    /// Variable TTF files which make up a family
    #[arg(required = true)]
    fonts: Vec<PathBuf>,
    /// Use a YAML file to build the STAT tables
    #[arg(long)]
    src: Option<PathBuf>,
    /// Output directory for the fonts
    #[arg(long, short = 'o')]
    out: Option<PathBuf>,
    /// Overwrite the input files
    #[arg(long)]
    inplace: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(&args)
}

fn run(args: &Args) -> Result<()> {
    println!("{:?}", args.fonts);

    let mut varfonts = Vec::new();
    for path in &args.fonts {
        let data = std::fs::read(path)
            .with_context(|| format!("Failed to read font file: {}", path.display()))?;
        varfonts.push(VarFont {
            filename: file_name(path),
            data,
        });
    }

    let fonts = if let Some(src) = &args.src {
        let config = std::fs::read_to_string(src)
            .with_context(|| format!("Failed to read config file: {}", src.display()))?;
        let config: StatConfig = serde_yaml_ng::from_str(&config)
            .with_context(|| format!("Failed to parse config file: {}", src.display()))?;
        gen_stat_tables_from_config(&config, &varfonts, None)?
    } else {
        gen_stat_tables(&varfonts)?
    };

    if let Some(out) = &args.out {
        std::fs::create_dir_all(out)
            .with_context(|| format!("Failed to create output directory: {}", out.display()))?;
    }
    for (path, data) in args.fonts.iter().zip(fonts) {
        let destination = output_path(path, args.out.as_deref(), args.inplace);
        if destination.is_file() {
            std::fs::remove_file(&destination)
                .with_context(|| format!("Failed to remove {}", destination.display()))?;
        }
        println!("Saving font to {}", destination.display());
        std::fs::write(&destination, data)
            .with_context(|| format!("Failed to write {}", destination.display()))?;
    }
    Ok(())
}

/// Where a font is written to: `--out`, `--inplace`, or `<font>.fix`.
fn output_path(font: &Path, out: Option<&Path>, inplace: bool) -> PathBuf {
    if let Some(out) = out {
        out.join(file_name(font))
    } else if inplace {
        font.to_path_buf()
    } else {
        let mut path = font.as_os_str().to_os_string();
        path.push(".fix");
        PathBuf::from(path)
    }
}

/// The file name of a font, which is also the key a per-file config uses.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::raw::TableProvider as _;
    use skrifa::raw::tables::stat::AxisValue;

    const ROBOTO: &str = "../gftools-lib/resources/test/Roboto[wdth,wght].ttf";
    const CONFIG: &str =
        "- name: Weight\n  tag: wght\n  values:\n  - name: Regular\n    value: 400\n";

    /// A copy of Roboto we can write over, in a scratch directory.
    fn scratch_font(directory: &Path, filename: &str) -> PathBuf {
        let destination = directory.join(filename);
        std::fs::copy(ROBOTO, &destination).unwrap();
        destination
    }

    fn stat_tags(font: &Path) -> Vec<String> {
        let data = std::fs::read(font).unwrap();
        let fontref = skrifa::FontRef::new(&data).unwrap();
        fontref
            .stat()
            .unwrap()
            .design_axes()
            .unwrap()
            .iter()
            .map(|axis| axis.axis_tag().to_string())
            .collect()
    }

    #[test]
    fn test_output_path() {
        let font = Path::new("/fonts/Roboto[wght].ttf");
        assert_eq!(
            output_path(font, None, false),
            Path::new("/fonts/Roboto[wght].ttf.fix")
        );
        assert_eq!(
            output_path(font, None, true),
            Path::new("/fonts/Roboto[wght].ttf")
        );
        assert_eq!(
            output_path(font, Some(Path::new("/out")), false),
            Path::new("/out/Roboto[wght].ttf")
        );
        // --out wins over --inplace, as the Python's if/elif does.
        assert_eq!(
            output_path(font, Some(Path::new("/out")), true),
            Path::new("/out/Roboto[wght].ttf")
        );
    }

    #[test]
    fn test_builds_from_a_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let font = scratch_font(tmp.path(), "Roboto[wght].ttf");
        let src = tmp.path().join("stat.yaml");
        std::fs::write(&src, CONFIG).unwrap();

        run(&Args {
            fonts: vec![font.clone()],
            src: Some(src),
            out: None,
            inplace: true,
        })
        .unwrap();

        // The configured STAT replaced Roboto's own.
        assert_eq!(stat_tags(&font), vec!["wght"]);
        let data = std::fs::read(&font).unwrap();
        let fontref = skrifa::FontRef::new(&data).unwrap();
        let values = fontref
            .stat()
            .unwrap()
            .offset_to_axis_values()
            .unwrap()
            .unwrap();
        let values = values.axis_values().iter().flatten().collect::<Vec<_>>();
        assert_eq!(values.len(), 1);
        let AxisValue::Format1(value) = &values[0] else {
            panic!("expected a format 1 axis value")
        };
        assert_eq!(value.value().to_f64(), 400.0);
    }

    #[test]
    fn test_builds_from_the_axis_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let font = scratch_font(tmp.path(), "Roboto[wght].ttf");
        let out = tmp.path().join("out");

        run(&Args {
            fonts: vec![font.clone()],
            src: None,
            out: Some(out.clone()),
            inplace: false,
        })
        .unwrap();

        // The registry's table is written to the output directory, and the
        // input is left alone.
        let tags = stat_tags(&out.join("Roboto[wght].ttf"));
        assert!(tags.contains(&"wght".to_string()), "{tags:?}");
        assert!(tags.contains(&"wdth".to_string()), "{tags:?}");
        assert!(!tmp.path().join("Roboto[wght].ttf.fix").exists());
        assert_eq!(stat_tags(&font), vec!["wdth", "wght", "ital"]);
    }

    #[test]
    fn test_writes_a_fix_file_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let font = scratch_font(tmp.path(), "Roboto[wght].ttf");
        let src = tmp.path().join("stat.yaml");
        std::fs::write(&src, CONFIG).unwrap();

        run(&Args {
            fonts: vec![font.clone()],
            src: Some(src),
            out: None,
            inplace: false,
        })
        .unwrap();

        let fix = tmp.path().join("Roboto[wght].ttf.fix");
        assert_eq!(stat_tags(&fix), vec!["wght"]);
        // The input font is untouched.
        assert_eq!(stat_tags(&font), vec!["wdth", "wght", "ital"]);
    }
}
