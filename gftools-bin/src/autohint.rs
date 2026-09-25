//! Automatically hint a TrueType font.
//!
//! ```sh
//! gftools-autohint --auto-script Gulzar-Regular.ttf
//! gftools-autohint --fail-ok --auto-script --discount-latin -o out.ttf in.ttf
//! ```
//!
//! The Python script is a wrapper around the external `ttfautohint` binary, and
//! works around that binary's inability to write to the file it is reading by
//! hinting into a temporary `<input>.autohinted` file and moving it back over
//! the input. We instead call the Rust autohinter (`tilvisan`) directly, which
//! hands back the hinted font as bytes, so the destination can be the input.
//!
//! The arguments follow `gftools/scripts/autohint.py`; the hinting itself
//! (`--fail-ok`, `--auto-script`, `--discount-latin`) follows the `Autohint`
//! operation in `gftools-builder`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use gftools::primary_script;
use skrifa::FontRef;
use tilvisan::{autohint, Args as AutohinterArgs, ScriptClassIndex};

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Automatically hint a TrueType font
struct Args {
    /// If the autohinting fails, copy the input file to the output
    #[arg(long)]
    fail_ok: bool,
    /// Automatically determine the script for key glyphs
    #[arg(long)]
    auto_script: bool,
    /// When determining the script, ignore Latin glyphs
    #[arg(long)]
    discount_latin: bool,
    /// Any additional arguments to pass to the autohinter
    #[arg(long, allow_hyphen_values = true)]
    args: Option<String>,
    /// File to save the autohinted font (may be same as input). Defaults to
    /// same as input
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Font to hint
    font: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(&args)
}

fn run(args: &Args) -> Result<()> {
    let destination = args.output.clone().unwrap_or_else(|| args.font.clone());
    let mut autohinter_args = autohinter_arguments(&args.font, &destination, args.args.as_deref())?;

    if args.auto_script {
        let data = std::fs::read(&args.font)
            .with_context(|| format!("Failed to read font file: {}", args.font.display()))?;
        let font = FontRef::new(&data)
            .with_context(|| format!("Failed to parse font file: {}", args.font.display()))?;
        // A font whose key glyphs are unscripted keeps the autohinter's default.
        if let Some(script) = primary_script(&font, args.discount_latin) {
            match ScriptClassIndex::from_tag(&script.to_ascii_lowercase()) {
                Ok(index) => autohinter_args.default_script = index,
                // The script of a font is not necessarily one the autohinter
                // knows about, and giving up before we have hinted anything
                // means we can still honour `--fail-ok`.
                Err(_) if args.fail_ok => {
                    eprintln!(
                        "Unknown script {script} for autohinting, but fail-ok is set, continuing."
                    );
                    return copy(&args.font, &destination);
                }
                Err(e) => bail!("Unknown script for autohinting: {e}"),
            }
        }
    }

    match autohint(&autohinter_args) {
        Ok(hinted) => std::fs::write(&destination, hinted)
            .with_context(|| format!("Failed to write {}", destination.display())),
        Err(e) if args.fail_ok => {
            eprintln!("ttfautohint failed, just copying file: {e}");
            copy(&args.font, &destination)
        }
        Err(e) => Err(anyhow::Error::new(e).context("Autohinting failed")),
    }
}

/// Turn our arguments into the autohinter's
fn autohinter_arguments(
    input: &Path,
    output: &Path,
    extra: Option<&str>,
) -> Result<AutohinterArgs> {
    let extra = extra
        .unwrap_or_default()
        .split_whitespace()
        .map(|arg| arg.to_owned());
    let argv = [
        "ttfautohint".to_owned(),
        input.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .chain(extra);
    AutohinterArgs::try_parse_from(argv).context("Failed to parse autohinter arguments")
}

/// The Python's `--fail-ok` behaviour: the input is left as the output.
/// Copying a file over itself is an error, so that case is a no-op.
fn copy(input: &Path, destination: &Path) -> Result<()> {
    if input == destination {
        return Ok(());
    }
    std::fs::copy(input, destination).with_context(|| {
        format!(
            "Failed to copy {} to {}",
            input.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use skrifa::Tag;

    use super::*;

    const BUHID: &str = "../gftools-lib/resources/test/notosansbuhid/NotoSansBuhid-Regular.ttf";

    fn args(font: &Path) -> Args {
        Args {
            fail_ok: false,
            auto_script: false,
            discount_latin: false,
            args: None,
            output: None,
            font: font.to_path_buf(),
        }
    }

    /// Copy a fixture into a temporary directory, so the tests can write over it.
    fn fixture(source: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join(Path::new(source).file_name().unwrap());
        std::fs::copy(source, &font).unwrap();
        (dir, font)
    }

    #[test]
    fn command_line_arguments() {
        let args = Args::try_parse_from(["gftools-autohint", "Font.ttf"]).unwrap();
        assert_eq!(args.font, PathBuf::from("Font.ttf"));
        assert_eq!(args.output, None);
        assert!(!args.fail_ok && !args.auto_script && !args.discount_latin);
        assert_eq!(args.args, None);

        let args = Args::try_parse_from([
            "gftools-autohint",
            "--fail-ok",
            "--auto-script",
            "--discount-latin",
            "--args",
            "-a qs -c",
            "-o",
            "Out.ttf",
            "Font.ttf",
        ])
        .unwrap();
        assert_eq!(args.font, PathBuf::from("Font.ttf"));
        assert_eq!(args.output, Some(PathBuf::from("Out.ttf")));
        assert!(args.fail_ok && args.auto_script && args.discount_latin);
        assert_eq!(args.args.as_deref(), Some("-a qs -c"));
    }

    #[test]
    fn hints_a_font() {
        let (_dir, font) = fixture(BUHID);
        let original = std::fs::read(&font).unwrap();
        let output = font.with_file_name("Hinted.ttf");

        let args = Args {
            auto_script: true,
            output: Some(output.clone()),
            ..args(&font)
        };
        run(&args).unwrap();

        let hinted = std::fs::read(&output).unwrap();
        assert_ne!(hinted, original, "the output should be hinted");
        assert_eq!(std::fs::read(&font).unwrap(), original, "the input is kept");

        // The autohinter adds its own instructions to the font.
        let hinted = FontRef::new(&hinted).unwrap();
        assert!(hinted.table_data(Tag::new(b"fpgm")).is_some());
        assert!(hinted.table_data(Tag::new(b"prep")).is_some());
    }

    #[test]
    fn output_defaults_to_the_input() {
        let (_dir, font) = fixture(BUHID);
        let original = std::fs::read(&font).unwrap();

        run(&args(&font)).unwrap();

        assert_ne!(std::fs::read(&font).unwrap(), original);
    }

    #[test]
    fn arguments_are_passed_to_the_autohinter() {
        let (_dir, font) = fixture(BUHID);
        let args = Args {
            args: Some("--not-an-option".to_owned()),
            ..args(&font)
        };
        assert!(run(&args).is_err());
    }

    #[test]
    fn fail_ok_copies_the_input_on_failure() {
        let (dir, font) = fixture(BUHID);
        // A file the autohinter cannot make any sense of.
        std::fs::write(&font, "not a font").unwrap();
        let output = dir.path().join("Out.ttf");

        let failing = Args {
            output: Some(output.clone()),
            ..args(&font)
        };
        assert!(run(&failing).is_err());
        assert!(!output.exists());

        let ok = Args {
            fail_ok: true,
            ..failing
        };
        run(&ok).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"not a font");
    }

    #[test]
    fn fail_ok_leaves_the_input_alone() {
        let (_dir, font) = fixture(BUHID);
        std::fs::write(&font, "not a font").unwrap();

        let ok = Args {
            fail_ok: true,
            ..args(&font)
        };
        run(&ok).unwrap();

        assert_eq!(std::fs::read(&font).unwrap(), b"not a font");
    }
}
