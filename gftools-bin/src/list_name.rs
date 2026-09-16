use anyhow::Context;
use clap::Parser;
use serde::Serialize;
use skrifa::raw::TableProvider as _;
use std::path::PathBuf;
use tabled::Tabled;

#[derive(Parser)]
struct Args {
    /// Output in CSV format
    #[clap(long)]
    csv: bool,
    /// Font files
    files: Vec<PathBuf>,
}

#[derive(Tabled, Serialize)]
struct SimpleNameEntry {
    font: String,
    platform_id: u16,
    encoding_id: u16,
    language_id: u16,
    name_id: u16,
    name_string: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut name_records = vec![];
    for file in args.files {
        let font_data = std::fs::read(&file)
            .with_context(|| format!("Failed to read font file: {}", file.display()))?;
        let font_ref = skrifa::FontRef::new(&font_data)
            .with_context(|| format!("Failed to parse font file: {}", file.display()))?;
        let name_data = font_ref.name()?.string_data();
        for name in font_ref.name()?.name_record() {
            name_records.push(SimpleNameEntry {
                font: file.file_name().unwrap().to_string_lossy().to_string(),
                platform_id: name.platform_id(),
                encoding_id: name.encoding_id(),
                language_id: name.language_id(),
                name_id: name.name_id().to_u16(),
                name_string: name.string(name_data)?.to_string(),
            });
        }
    }
    if args.csv {
        let mut wtr = csv::Writer::from_writer(std::io::stdout());
        for record in &name_records {
            wtr.serialize(record)?;
        }
        wtr.flush()?;
    } else {
        let table = tabled::Table::new(&name_records);
        println!("{}", table);
    }
    Ok(())
}
