//! Utility to dump variation font info.
//!
//! ```sh
//! gftools-varfont-info 'Roboto[wdth,wght].ttf'
//! ```
//!
//! Lists the axes and named instances declared in a variable font's `fvar`
//! table, the axis values in its `STAT` table, and the `name` table entries
//! which neither of those already accounts for.
//!
//! This follows Simon's `dump-names` script rather than the original Python
//! `varfont_info`, which it replaces: as well as the extra tables, a name ID
//! with no record is reported as `[?nameID=N?]` (and 0xFFFF as `[anonymous]`)
//! instead of being left blank, and a font without an `fvar` table is still
//! dumped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use skrifa::raw::TableProvider as _;
use skrifa::raw::tables::stat::AxisValue;
use skrifa::raw::types::{Fixed, NameId, Tag};
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider as _};
use tabled::Tabled;
use tabled::settings::{Panel, Style};

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
/// Utility to dump variation font info
struct Args {
    /// Fonts in OpenType (TTF/OTF) format
    #[arg(required = true)]
    fonts: Vec<PathBuf>,
}

#[derive(Tabled)]
struct NameRow {
    #[tabled(rename = "Name ID")]
    name_id: u16,
    #[tabled(rename = "Name")]
    name: String,
}

#[derive(Tabled)]
struct AxisRow {
    #[tabled(rename = "Tag")]
    tag: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Minimum")]
    minimum: String,
    #[tabled(rename = "Default")]
    default: String,
    #[tabled(rename = "Maximum")]
    maximum: String,
}

#[derive(Tabled)]
struct InstanceRow {
    #[tabled(rename = "Name ID")]
    name_id: u16,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Coordinates")]
    coordinates: String,
}

#[derive(Tabled)]
struct StatRow {
    #[tabled(rename = "Name ID")]
    name_id: u16,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Coordinate")]
    coordinate: String,
}

/// The `fvar` rows of a single font: its axes and its named instances.
struct FvarRows {
    axes: Vec<AxisRow>,
    instances: Vec<InstanceRow>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    for file in &args.fonts {
        let font_data = std::fs::read(file)
            .with_context(|| format!("Failed to read font file: {}", file.display()))?;
        let font = FontRef::new(&font_data)
            .with_context(|| format!("Failed to parse font file: {}", file.display()))?;
        println!("{}", file.display());

        // The name IDs which the other tables show names for are left out of
        // the `name` dump, so collect them before building it.
        let mut seen_ids = BTreeSet::new();
        let fvar = fvar_rows(&font, &mut seen_ids)?;
        let stat = stat_rows(&font, &mut seen_ids)?;
        if fvar.is_none() {
            println!("This font file lacks an 'fvar' table.");
        }

        print_table("name", &name_rows(&font, &seen_ids)?);
        if let Some(fvar) = &fvar {
            print_table("fvar axes", &fvar.axes);
            print_table("fvar instances", &fvar.instances);
        }
        if let Some(stat) = &stat {
            print_table("STAT", stat);
        }
    }
    Ok(())
}

fn print_table<T: Tabled>(title: &str, rows: &[T]) {
    let mut table = tabled::Table::new(rows);
    table.with(Style::sharp()).with(Panel::header(title));
    println!("{table}");
}

/// The name table, minus the name IDs `seen_ids` covers, keyed by name ID like
/// the Python (so the last record for a name ID wins) and in name ID order.
fn name_rows(font: &FontRef, seen_ids: &BTreeSet<u16>) -> Result<Vec<NameRow>> {
    let name_table = font.name()?;
    let string_data = name_table.string_data();
    let mut names = BTreeMap::new();
    for record in name_table.name_record() {
        let name_id = record.name_id().to_u16();
        if seen_ids.contains(&name_id) {
            continue;
        }
        // Records we can't decode are skipped rather than fatal, unlike the
        // Python's record.toUnicode().
        if let Ok(string) = record.string(string_data) {
            names.insert(name_id, string.to_string());
        }
    }
    Ok(names
        .into_iter()
        .map(|(name_id, name)| NameRow { name_id, name })
        .collect())
}

/// The `fvar` table's axes and named instances, or `None` if there isn't one.
///
/// Instance name IDs above 255 are added to `seen_ids`.
fn fvar_rows(font: &FontRef, seen_ids: &mut BTreeSet<u16>) -> Result<Option<FvarRows>> {
    let Ok(fvar) = font.fvar() else {
        return Ok(None);
    };
    let axes = fvar.axes()?;

    let axis_rows = axes
        .iter()
        .map(|axis| AxisRow {
            tag: axis.axis_tag().to_string(),
            name: resolve_name(font, axis.axis_name_id()),
            minimum: py_float(axis.min_value()),
            default: py_float(axis.default_value()),
            maximum: py_float(axis.max_value()),
        })
        .collect();

    let mut instance_rows = Vec::new();
    for instance in fvar.instances()?.iter().flatten() {
        let name_id = instance.subfamily_name_id.to_u16();
        if name_id > 255 {
            seen_ids.insert(name_id);
        }
        let coordinates = axes
            .iter()
            .zip(instance.coordinates.iter())
            .map(|(axis, coordinate)| {
                format!("{}: {}", axis.axis_tag(), py_float(coordinate.get()))
            })
            .collect::<Vec<_>>()
            .join(", ");
        instance_rows.push(InstanceRow {
            name_id,
            name: resolve_name(font, instance.subfamily_name_id),
            coordinates,
        });
    }

    Ok(Some(FvarRows {
        axes: axis_rows,
        instances: instance_rows,
    }))
}

/// The `STAT` table's axis values, or `None` if there is no `STAT` table or it
/// has no axis value array.
///
/// Value name IDs above 255 are added to `seen_ids`.
fn stat_rows(font: &FontRef, seen_ids: &mut BTreeSet<u16>) -> Result<Option<Vec<StatRow>>> {
    let Ok(stat) = font.stat() else {
        return Ok(None);
    };
    // Axis values are referenced by index, so keep the design axis tags around.
    let axes = stat
        .design_axes()?
        .iter()
        .map(|axis| axis.axis_tag())
        .collect::<Vec<Tag>>();
    let Some(Ok(axis_values)) = stat.offset_to_axis_values() else {
        return Ok(None);
    };

    let mut rows = Vec::new();
    for axis_value in axis_values.axis_values().iter().flatten() {
        let (name_id, coordinate) = match &axis_value {
            AxisValue::Format1(value) => (
                value.value_name_id(),
                format!(
                    "{}: {}",
                    axis_tag(&axes, value.axis_index()),
                    py_float(value.value())
                ),
            ),
            AxisValue::Format2(value) => (
                value.value_name_id(),
                format!(
                    "{}: {}-{}-{}",
                    axis_tag(&axes, value.axis_index()),
                    py_float(value.range_min_value()),
                    py_float(value.nominal_value()),
                    py_float(value.range_max_value())
                ),
            ),
            AxisValue::Format3(value) => (
                value.value_name_id(),
                format!(
                    "{}: {} (linked to {})",
                    axis_tag(&axes, value.axis_index()),
                    py_float(value.value()),
                    py_float(value.linked_value())
                ),
            ),
            AxisValue::Format4(value) => (
                value.value_name_id(),
                value
                    .axis_values()
                    .iter()
                    .map(|record| {
                        format!(
                            "{}={}",
                            axis_tag(&axes, record.axis_index.get()),
                            py_float(record.value.get())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        };
        let id = name_id.to_u16();
        if id > 255 {
            seen_ids.insert(id);
        }
        rows.push(StatRow {
            name_id: id,
            name: resolve_name(font, name_id),
            coordinate,
        });
    }
    Ok(Some(rows))
}

/// The tag of the design axis at `index`, for reporting an axis value.
fn axis_tag(axes: &[Tag], index: u16) -> String {
    axes.get(index as usize)
        .map(|tag| tag.to_string())
        .unwrap_or_else(|| format!("[?axisIndex={index}?]"))
}

/// The name for a name ID, preferring the English record, with placeholders for
/// the name IDs the Python's `_ResolveName` also special-cased.
fn resolve_name(font: &FontRef, name_id: NameId) -> String {
    if name_id.to_u16() == 0xFFFF {
        return "[anonymous]".to_string();
    }
    font.localized_strings(StringId::new(name_id.to_u16()))
        .english_or_first()
        .map(|name| name.to_string())
        .unwrap_or_else(|| format!("[?nameID={}?]", name_id.to_u16()))
}

/// Format a fixed-point value the way Python does, so that our output lines up
/// with the script's: 100.0, not 100.
fn py_float(value: Fixed) -> String {
    format!("{:?}", value.to_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROBOTO: &str = "../gftools-lib/resources/test/Roboto[wdth,wght].ttf";
    const RADIO_CANADA: &str =
        "../gftools-builder/resources/radio-canada/variable/RadioCanadaDisplay[wght].ttf";

    fn open(path: &str) -> FontRef<'static> {
        // Leak the font data so the FontRef can be returned; the tests only
        // run a handful of times.
        let data: &'static [u8] = Box::leak(std::fs::read(path).unwrap().into_boxed_slice());
        FontRef::new(data).unwrap()
    }

    #[test]
    fn test_fvar_rows() {
        let font = open(ROBOTO);
        let mut seen_ids = BTreeSet::new();
        let fvar = fvar_rows(&font, &mut seen_ids).unwrap().unwrap();

        assert_eq!(
            fvar.axes
                .iter()
                .map(|axis| (
                    axis.tag.as_str(),
                    axis.name.as_str(),
                    axis.minimum.as_str(),
                    axis.default.as_str(),
                    axis.maximum.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("wght", "Weight", "100.0", "400.0", "900.0"),
                ("wdth", "Width", "75.0", "100.0", "100.0"),
            ]
        );

        assert_eq!(fvar.instances.len(), 18);
        assert_eq!(
            (fvar.instances[0].name_id, fvar.instances[0].name.as_str()),
            (259, "Thin")
        );
        assert_eq!(fvar.instances[0].coordinates, "wght: 100.0, wdth: 100.0");
        assert_eq!(
            (
                fvar.instances[17].name_id,
                fvar.instances[17].name.as_str(),
                fvar.instances[17].coordinates.as_str()
            ),
            (283, "Condensed Black", "wght: 900.0, wdth: 75.0")
        );
        // Instances with name IDs above 255 are left out of the name dump;
        // name ID 2 (the Regular instance) isn't.
        assert!(seen_ids.contains(&259));
        assert!(!seen_ids.contains(&2));
    }

    #[test]
    fn test_stat_rows() {
        let font = open(ROBOTO);
        let mut seen_ids = BTreeSet::new();
        let stat = stat_rows(&font, &mut seen_ids).unwrap().unwrap();

        assert_eq!(stat.len(), 12);
        assert_eq!(
            (
                stat[0].name_id,
                stat[0].name.as_str(),
                stat[0].coordinate.as_str()
            ),
            (294, "Condensed", "wdth: 75.0")
        );
        // Format 3 axis values name the value they are linked to...
        assert_eq!(
            (stat[5].name_id, stat[5].coordinate.as_str()),
            (300, "wght: 400.0 (linked to 700.0)")
        );
        // ...including for axes which aren't in fvar (Roboto's STAT has ital).
        assert_eq!(
            (
                stat[11].name_id,
                stat[11].name.as_str(),
                stat[11].coordinate.as_str()
            ),
            (296, "Weight", "ital: 0.0 (linked to 1.0)")
        );
        assert!(seen_ids.contains(&294));
    }

    #[test]
    fn test_name_rows() {
        let font = open(ROBOTO);
        let mut seen_ids = BTreeSet::new();
        fvar_rows(&font, &mut seen_ids).unwrap();
        stat_rows(&font, &mut seen_ids).unwrap();
        let names = name_rows(&font, &seen_ids).unwrap();

        // Sorted by name ID, and holding the axis names...
        assert!(
            names
                .windows(2)
                .all(|pair| pair[0].name_id < pair[1].name_id)
        );
        assert_eq!(
            names.iter().find(|row| row.name_id == 256).unwrap().name,
            "Weight"
        );
        // ...but not the names the fvar and STAT tables show themselves.
        assert!(
            names
                .iter()
                .all(|row| row.name_id != 259 && row.name_id != 294)
        );
        assert_eq!(
            names.iter().find(|row| row.name_id == 1).unwrap().name,
            "Roboto"
        );
    }

    #[test]
    fn test_font_without_stat() {
        let font = open(RADIO_CANADA);
        let mut seen_ids = BTreeSet::new();
        assert!(stat_rows(&font, &mut seen_ids).unwrap().is_none());
        let fvar = fvar_rows(&font, &mut seen_ids).unwrap().unwrap();
        assert_eq!(fvar.axes.len(), 1);
        assert_eq!(
            (fvar.axes[0].tag.as_str(), fvar.axes[0].name.as_str()),
            ("wght", "Weight")
        );
        assert_eq!(fvar.instances.len(), 4);
        assert_eq!(fvar.instances[0].coordinates, "wght: 400.0");
    }

    #[test]
    fn test_py_float() {
        assert_eq!(py_float(Fixed::from_f64(100.0)), "100.0");
        assert_eq!(py_float(Fixed::from_f64(62.5)), "62.5");
    }

    /// A `STAT` table with an empty axis value array gives us no rows to print.
    #[test]
    fn test_empty_table() {
        print_table::<StatRow>("STAT", &[]);
    }
}
