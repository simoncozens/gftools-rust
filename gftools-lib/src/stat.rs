//! Building the STAT table
//!
//! [`StatBuilder`] is the low-level builder, largely stolen from fea-rs, where
//! it is all private so we can't use it. On top of it, [`gen_stat_tables`] and
//! [`gen_stat_tables_from_config`] are the port of `gftools.stat`: the first
//! builds a family's STAT tables from the Google Fonts axis registry, the
//! second from a `buildStatTable`-style config.
//!
//! We're uninterested in anything other than 3/1/0x409, so we use Strings
//! instead of the more correct NameSpec.

use std::collections::HashMap;

use google_fonts_axisregistry::build_stat;
use serde::{de, Deserialize, Deserializer, Serialize};
use skrifa::{raw::TableProvider as _, FontRef};
use write_fonts::{
    from_obj::ToOwnedTable,
    tables::{
        name::{Name, NameRecord},
        stat as write_stat,
    },
    types::{Fixed, NameId, Tag},
    FontBuilder,
};

use crate::names::find_or_add_name;
use crate::utils::font_is_italic;
use crate::GftoolsError;

/// Builder for the STAT table, which contains design axis and axis value information.
#[derive(Clone, Debug)]
pub struct StatBuilder {
    // pub name: StatFallbackName,
    /// The design axes for the STAT table.
    pub records: Vec<AxisRecord>,
    /// The axis values for the STAT table.
    pub values: Vec<AxisValue>,
}

/// Represents a design axis in the STAT table.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisRecord {
    /// The four-character tag identifying the design axis.
    pub tag: Tag,
    /// The name of the design axis.
    pub name: String,
    /// The ordering of the design axis relative to other axes.
    pub ordering: u16,
}

/// Represents an axis value in the STAT table.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisValue {
    /// The flags associated with the axis value.
    pub flags: u16,
    /// The name of the axis value.
    pub name: String,
    /// The location of the axis value along the design axis.
    pub location: AxisLocation,
}

/// Represents the location of an axis value along a design axis.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum AxisLocation {
    /// Stat format 1 axis location, which specifies a single value along the design axis.
    One {
        /// The tag of the design axis.
        tag: Tag,
        /// The value of the axis along the design axis.
        value: Fixed,
    },
    /// Stat format 2 axis location, which specifies a range along the design axis.
    Two {
        /// The tag of the design axis.
        tag: Tag,
        /// The nominal value of the axis along the design axis.
        nominal: Fixed,
        /// The minimum value of the axis along the design axis.
        min: Fixed,
        /// The maximum value of the axis along the design axis.
        max: Fixed,
    },
    /// Stat format 3 axis location, which specifies a value and a linked value along the design axis.
    Three {
        /// The tag of the design axis.
        tag: Tag,
        /// The value of the axis along the design axis.
        value: Fixed,
        /// The linked value of the axis along the design axis.
        linked: Fixed,
    },
    /// Stat format 4 axis location, which specifies multiple values along the design axis.
    /// Each tuple contains the tag of the design axis and the corresponding value.
    Four(Vec<(Tag, Fixed)>),
}

impl StatBuilder {
    /// Builds the STAT table using the provided name records.
    ///
    /// Use this after creating a `StatBuilder` instance and filling it with
    /// the necessary axis records and axis values.
    pub fn build(&self, name_records: &mut Vec<NameRecord>) -> write_stat::Stat {
        // let elided_fallback_name_id = match &self.name {
        //     StatFallbackName::Id(id) => *id,
        //     StatFallbackName::Record(name) => find_or_add_name(name_records, name),
        // };

        //HACK: we jump through a bunch of hoops to ensure our output matches
        //feaLib's; in particular we want to add our name table entries grouped by
        //axis.
        let mut sorted_values = HashMap::<Tag, Vec<_>>::new();
        let mut sorted_records = self.records.iter().collect::<Vec<_>>();
        sorted_records.sort_by_key(|x| x.ordering);

        for axis_value in &self.values {
            match axis_value.location {
                AxisLocation::One { tag, .. }
                | AxisLocation::Two { tag, .. }
                | AxisLocation::Three { tag, .. } => {
                    sorted_values.entry(tag).or_default().push(axis_value)
                }
                AxisLocation::Four(_) => sorted_values
                    .entry(Tag::default())
                    .or_default()
                    .push(axis_value),
            }
        }

        let mut design_axes = Vec::with_capacity(self.records.len());
        let mut axis_values = Vec::with_capacity(self.values.len());

        for (i, record) in self.records.iter().enumerate() {
            let name_id = find_or_add_name(name_records, &record.name);
            let record = write_stat::AxisRecord {
                axis_tag: record.tag,
                axis_name_id: name_id,
                axis_ordering: record.ordering,
            };
            for axis_value in sorted_values
                .get(&record.axis_tag)
                .iter()
                .flat_map(|x| x.iter())
            {
                let flags = write_stat::AxisValueTableFlags::from_bits(axis_value.flags).unwrap();
                let name_id = find_or_add_name(name_records, &axis_value.name);

                let value = match &axis_value.location {
                    AxisLocation::One { value, .. } => write_stat::AxisValue::format_1(
                        //TODO: validate that all referenced tags refer to existing axes
                        i as u16, flags, name_id, *value,
                    ),
                    AxisLocation::Two {
                        nominal, min, max, ..
                    } => write_stat::AxisValue::format_2(
                        i as _, flags, name_id, *nominal, *min, *max,
                    ),
                    AxisLocation::Three { value, linked, .. } => {
                        write_stat::AxisValue::format_3(i as _, flags, name_id, *value, *linked)
                    }

                    AxisLocation::Four(_) => panic!("assigned to separate group"),
                };
                axis_values.push(value);
            }

            design_axes.push(record);
        }

        let format4 = sorted_values
            .remove(&Tag::default())
            .unwrap_or_default()
            .into_iter()
            .map(|format4| {
                let flags = write_stat::AxisValueTableFlags::from_bits(format4.flags).unwrap();
                let name_id = find_or_add_name(name_records, &format4.name);

                let AxisLocation::Four(values) = &format4.location else {
                    panic!("only format 4 in this group")
                };
                let mapping = values
                    .iter()
                    .map(|(tag, value)| {
                        let axis_index = design_axes
                            .iter()
                            .position(|rec| rec.axis_tag == *tag)
                            .expect("validated");
                        write_stat::AxisValueRecord::new(axis_index as _, *value)
                    })
                    .collect();
                write_stat::AxisValue::format_4(flags, name_id, mapping)
            });

        //feaLib puts format4 records first
        let axis_values = format4.chain(axis_values).collect();
        write_stat::Stat::new(design_axes, axis_values, NameId::from(2))
    }

    /// Reorder a fontTools-format configuration into a builder.
    ///
    /// `buildStatTable` takes this same list of axis dictionaries, so we map
    /// each axis to an [`AxisRecord`] and its `values` to [`AxisValue`]s,
    /// keeping the config's order: an axis without an explicit ordering is
    /// ordered where it appears, and an axis's values keep their order.
    pub fn from_config(axes: &[AxisConfig]) -> Result<Self, GftoolsError> {
        let mut records = Vec::with_capacity(axes.len());
        let mut values = Vec::new();
        for (index, axis) in axes.iter().enumerate() {
            let tag = Tag::new_checked(axis.tag.as_bytes()).map_err(|_| {
                GftoolsError::Misc(format!("Invalid axis tag in config: {:?}", axis.tag))
            })?;
            records.push(AxisRecord {
                tag,
                name: axis.name.to_axis_name()?,
                ordering: axis.ordering.unwrap_or(index as u16),
            });
            for value in &axis.values {
                values.push(value.to_axis_value(tag)?);
            }
        }
        Ok(StatBuilder { records, values })
    }
}

// Above we had a generic STAT table builder, which supported everything,
// including format 4 axis value records. But we also have to support the old
// config files which use a different format: records nested inside axes.
// So the rest of this file is about handling those.

/// A variable font, with the file name a per-file config is keyed by.
#[derive(Debug, Clone)]
pub struct VarFont {
    /// The file name of the font, as a per-file config spells it.
    pub filename: String,
    /// The font data.
    pub data: Vec<u8>,
}

/// The configuration [`gen_stat_tables_from_config`] takes: the same shape as
/// the fontTools `buildStatTable` arguments which gftools passes it.
///
/// Either one list of axes for the whole family, or a dictionary mapping font
/// file names to such a list:
///
/// ```yaml
/// Lora[wght].ttf:
/// - name: Weight
///   tag: wght
///   values:
///   - name: Regular
///     value: 400
/// ```
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum StatConfig {
    /// One configuration for every font in the family.
    Family(Vec<AxisConfig>),
    /// A configuration per font file name.
    PerFile(HashMap<String, Vec<AxisConfig>>),
}

impl StatConfig {
    /// The axes configured for the font called `filename`.
    pub fn axes_for(&self, filename: &str) -> Result<&[AxisConfig], GftoolsError> {
        match self {
            StatConfig::Family(axes) => Ok(axes),
            StatConfig::PerFile(configs) => {
                configs.get(filename).map(Vec::as_slice).ok_or_else(|| {
                    GftoolsError::Misc(format!("Filename {filename} not found in stat dictionary"))
                })
            }
        }
    }
}

// A hand-written `Deserialize` rather than an untagged enum, so that a mistake
// inside an axis is reported as a mistake inside an axis.
impl<'de> Deserialize<'de> for StatConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StatConfigVisitor;

        impl<'de> de::Visitor<'de> for StatConfigVisitor {
            type Value = StatConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter
                    .write_str("a list of axes, or a mapping of font file name to a list of axes")
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                Vec::<AxisConfig>::deserialize(de::value::SeqAccessDeserializer::new(seq))
                    .map(StatConfig::Family)
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                HashMap::<String, Vec<AxisConfig>>::deserialize(
                    de::value::MapAccessDeserializer::new(map),
                )
                .map(StatConfig::PerFile)
            }
        }

        deserializer.deserialize_any(StatConfigVisitor)
    }
}

/// A design axis in a [`StatConfig`], as fontTools' `buildStatTable` takes it.
///
/// This should not be used for new code; it's specifically for parsing YAML
/// configurations.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AxisConfig {
    /// The four-character axis tag, e.g. `wght`.
    pub tag: String,
    /// The axis's name, e.g. `Weight`.
    pub name: NameSpec,
    /// Where the axis sorts; the axis's position in the list when not given.
    #[serde(default)]
    pub ordering: Option<u16>,
    /// The axis values belonging to this axis.
    #[serde(default)]
    pub values: Vec<AxisValueConfig>,
}

/// An axis value in a [`StatConfig`].
///
/// Its STAT format follows from which fields are present:
/// `value` (format 1, or format 3 with a `linkedValue`), or `nominalValue`
/// (format 2, with an optional range).
///
/// This should not be used for new code; it's specifically for parsing YAML
/// configurations.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AxisValueConfig {
    /// The value's name, e.g. `Regular`.
    pub name: NameSpec,
    /// The value's flags; zero when not given.
    #[serde(default)]
    pub flags: u16,
    /// The value along the axis.
    #[serde(default)]
    pub value: Option<f64>,
    /// The value this one is linked to, which makes it a format 3 record.
    #[serde(default)]
    pub linked_value: Option<f64>,
    /// The nominal value, which makes this a format 2 record.
    #[serde(default)]
    pub nominal_value: Option<f64>,
    /// The low end of a format 2 record's range.
    #[serde(default)]
    pub range_min_value: Option<f64>,
    /// The high end of a format 2 record's range.
    #[serde(default)]
    pub range_max_value: Option<f64>,
}

impl AxisValueConfig {
    /// Reorder this value into an [`AxisValue`], picking its format the way the
    /// Python does: a `value` (with a `linkedValue` if there is one), else a
    /// `nominalValue`.
    fn to_axis_value(&self, tag: Tag) -> Result<AxisValue, GftoolsError> {
        let location = if let Some(value) = self.value {
            let value = Fixed::from_f64(value);
            match self.linked_value {
                Some(linked) => AxisLocation::Three {
                    tag,
                    value,
                    linked: Fixed::from_f64(linked),
                },
                None => AxisLocation::One { tag, value },
            }
        } else if let Some(nominal) = self.nominal_value {
            AxisLocation::Two {
                tag,
                nominal: Fixed::from_f64(nominal),
                min: self
                    .range_min_value
                    .map_or_else(negative_infinity, Fixed::from_f64),
                max: self
                    .range_max_value
                    .map_or_else(positive_infinity, Fixed::from_f64),
            }
        } else {
            return Err(GftoolsError::Misc(
                "Can't determine format for AxisValue".to_string(),
            ));
        };
        Ok(AxisValue {
            flags: self.flags,
            name: self.name.to_name(),
            location,
        })
    }
}

impl NameSpec {
    /// The string which goes into the name table.
    ///
    /// Numbers are stringified, as the Python's `str()` does: some configs
    /// (Climate Crisis) name axis values with years.
    pub fn to_name(&self) -> String {
        match self {
            NameSpec::Name(name) => name.clone(),
            NameSpec::Number(number) => number.to_string(),
        }
    }

    /// The name table entry for an axis name.
    ///
    /// The Python accepts a name ID here and uses it as-is; we work in strings
    /// (see the module comment), so a numeric axis name is an error rather than
    /// a new name record which happens to be called "300".
    fn to_axis_name(&self) -> Result<String, GftoolsError> {
        match self {
            NameSpec::Name(name) => Ok(name.clone()),
            NameSpec::Number(number) => Err(GftoolsError::Misc(format!(
                "Axis name {number} looks like a name ID; the Rust STAT builder only supports string axis names"
            ))),
        }
    }
}

/// A name in a [`StatConfig`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum NameSpec {
    /// A name to find in (or add to) the name table.
    Name(String),
    /// A number, which is stringified.
    Number(i64),
}

/// `AXIS_VALUE_NEGATIVE_INFINITY`.
fn negative_infinity() -> Fixed {
    Fixed::from_bits(i32::MIN)
}

/// `AXIS_VALUE_POSITIVE_INFINITY`.
fn positive_infinity() -> Fixed {
    Fixed::from_bits(i32::MAX)
}

/// Port of `gftools.stat.gen_stat_tables`: give every font a STAT table built
/// from the Google Fonts axis registry, using the family's other fonts as
/// siblings.
///
/// Returns the new font data, in the same order as the input.
pub fn gen_stat_tables(varfonts: &[VarFont]) -> Result<Vec<Vec<u8>>, GftoolsError> {
    varfonts
        .iter()
        .enumerate()
        .map(|(index, font)| {
            let siblings = varfonts
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, sibling)| sibling.data.as_slice())
                .collect::<Vec<_>>();
            build_stat(&font.data, &siblings).map_err(|e| GftoolsError::Misc(e.to_string()))
        })
        .collect()
}

/// Port of `gftools.stat.gen_stat_tables_from_config`: give every font a STAT
/// table built from a `buildStatTable`-style `config`.
///
/// The Python's `locations` argument (format 4 cross-axis values) is not
/// supported, so the elided fallback name is always name ID 2, as the Python
/// defaults it.
///
/// When the config is the family-wide list rather than a per-file dictionary
/// and `has_italic` (default: whether any font's style name says "Italic") is
/// true, an `ital` axis is appended to the config for each font. A per-file
/// config has to spell its own `ital` axis out.
///
/// Returns the new font data, in the same order as the input.
pub fn gen_stat_tables_from_config(
    config: &StatConfig,
    varfonts: &[VarFont],
    has_italic: Option<bool>,
) -> Result<Vec<Vec<u8>>, GftoolsError> {
    // The Python asserts every font has an fvar table.
    for font in varfonts {
        let fontref = FontRef::new(&font.data)?;
        if fontref.fvar().is_err() {
            return Err(GftoolsError::Misc(format!(
                "{} does not have an fvar table",
                font.filename
            )));
        }
    }

    let has_italic = match has_italic {
        Some(has_italic) => has_italic,
        None => varfonts
            .iter()
            .any(|font| FontRef::new(&font.data).is_ok_and(|fontref| font_is_italic(&fontref))),
    };

    // The Python rejects an `ital` axis in a family-wide config when the family
    // has an italic, because it generates that axis itself.
    let generated_italic_axis = match config {
        StatConfig::Family(axes) if has_italic => {
            if axes.iter().any(|axis| axis.tag == "ital") {
                return Err(GftoolsError::Misc(
                    "ital axis should not appear in stat config".to_string(),
                ));
            }
            true
        }
        _ => false,
    };

    varfonts
        .iter()
        .map(|font| {
            let fontref = FontRef::new(&font.data)?;
            let mut axes = config.axes_for(&font.filename)?.to_vec();
            if generated_italic_axis {
                axes.push(italic_axis(font_is_italic(&fontref)));
            }
            let mut name_table: Name = fontref.name()?.to_owned_table();
            let stat = StatBuilder::from_config(&axes)?.build(&mut name_table.name_record);
            // Name records need to be sorted, else write_fonts panics.
            name_table.name_record.sort();
            let mut builder = FontBuilder::new();
            builder.add_table(&name_table)?;
            builder.add_table(&stat)?;
            builder.copy_missing_tables(fontref);
            Ok(builder.build())
        })
        .collect()
}

/// The `ital` axis which the Python generates for families with an italic.
fn italic_axis(italic: bool) -> AxisConfig {
    let values = if italic {
        vec![AxisValueConfig {
            name: NameSpec::Name("Italic".to_string()),
            flags: 0,
            value: Some(1.0),
            linked_value: None,
            nominal_value: None,
            range_min_value: None,
            range_max_value: None,
        }]
    } else {
        vec![AxisValueConfig {
            name: NameSpec::Name("Roman".to_string()),
            flags: 0x2,
            value: Some(0.0),
            linked_value: Some(1.0),
            nominal_value: None,
            range_min_value: None,
            range_max_value: None,
        }]
    };
    AxisConfig {
        tag: "ital".to_string(),
        name: NameSpec::Name("Italic".to_string()),
        ordering: None,
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use skrifa::raw::tables::stat::AxisValue as ReadAxisValue;
    use skrifa::MetadataProvider as _;

    const ROBOTO: &str = "resources/test/Roboto[wdth,wght].ttf";

    fn roboto() -> VarFont {
        VarFont {
            filename: "Roboto[wdth,wght].ttf".to_string(),
            data: std::fs::read(ROBOTO).unwrap(),
        }
    }

    fn parse_config(yaml: &str) -> StatConfig {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    fn family_config(config: &StatConfig) -> &[AxisConfig] {
        match config {
            StatConfig::Family(axes) => axes,
            StatConfig::PerFile(_) => panic!("expected a family-wide config"),
        }
    }

    fn fmt(value: Fixed) -> String {
        format!("{:?}", value.to_f64())
    }

    fn localized_name(font: &FontRef, name_id: NameId) -> String {
        font.localized_strings(name_id)
            .english_or_first()
            .map(|name| name.to_string())
            .unwrap_or_default()
    }

    fn axis_tags(font: &FontRef) -> Vec<String> {
        font.stat()
            .unwrap()
            .design_axes()
            .unwrap()
            .iter()
            .map(|axis| axis.axis_tag().to_string())
            .collect()
    }

    /// The STAT's axis values as (axis tag, value name, description).
    fn axis_rows(font: &FontRef) -> Vec<(String, String, String)> {
        let stat = font.stat().unwrap();
        let axes = stat
            .design_axes()
            .unwrap()
            .iter()
            .map(|axis| axis.axis_tag().to_string())
            .collect::<Vec<_>>();
        let Some(Ok(values)) = stat.offset_to_axis_values() else {
            return Vec::new();
        };
        values
            .axis_values()
            .iter()
            .flatten()
            .map(|value| {
                let axis = axes[value.axis_index().unwrap_or_default() as usize].clone();
                let name = localized_name(font, value.value_name_id());
                let description = match &value {
                    ReadAxisValue::Format1(value) => format!("= {}", fmt(value.value())),
                    ReadAxisValue::Format2(value) => format!(
                        "= {} in {}-{}",
                        fmt(value.nominal_value()),
                        fmt(value.range_min_value()),
                        fmt(value.range_max_value())
                    ),
                    ReadAxisValue::Format3(value) => format!(
                        "= {} linked to {}",
                        fmt(value.value()),
                        fmt(value.linked_value())
                    ),
                    ReadAxisValue::Format4(_) => "format 4".to_string(),
                };
                (axis, name, description)
            })
            .collect()
    }

    fn axis_value_flags(font: &FontRef) -> Vec<u16> {
        let stat = font.stat().unwrap();
        let Some(Ok(values)) = stat.offset_to_axis_values() else {
            return Vec::new();
        };
        values
            .axis_values()
            .iter()
            .flatten()
            .map(|value| match &value {
                ReadAxisValue::Format1(value) => value.flags().bits(),
                ReadAxisValue::Format2(value) => value.flags().bits(),
                ReadAxisValue::Format3(value) => value.flags().bits(),
                ReadAxisValue::Format4(value) => value.flags().bits(),
            })
            .collect()
    }

    #[test]
    fn test_config_reordering() {
        let config = parse_config(
            r#"
- name: Weight
  tag: wght
  values:
  - name: Regular
    value: 400
    linkedValue: 700
    flags: 2
  - name: Bold
    value: 700
- name: Year
  tag: YEAR
  ordering: 3
  values:
  - name: 2020
    value: 2020
"#,
        );
        let builder = StatBuilder::from_config(family_config(&config)).unwrap();
        // The axes keep the config's order, and an axis without an ordering is
        // ordered where it appears.
        assert_eq!(
            builder.records,
            vec![
                AxisRecord {
                    tag: Tag::new(b"wght"),
                    name: "Weight".to_string(),
                    ordering: 0,
                },
                AxisRecord {
                    tag: Tag::new(b"YEAR"),
                    name: "Year".to_string(),
                    ordering: 3,
                },
            ]
        );
        // A `value` gives format 1, or format 3 with a `linkedValue`; the
        // value name is allowed to be a number.
        assert_eq!(
            builder.values,
            vec![
                AxisValue {
                    flags: 2,
                    name: "Regular".to_string(),
                    location: AxisLocation::Three {
                        tag: Tag::new(b"wght"),
                        value: Fixed::from_f64(400.0),
                        linked: Fixed::from_f64(700.0),
                    },
                },
                AxisValue {
                    flags: 0,
                    name: "Bold".to_string(),
                    location: AxisLocation::One {
                        tag: Tag::new(b"wght"),
                        value: Fixed::from_f64(700.0),
                    },
                },
                AxisValue {
                    flags: 0,
                    name: "2020".to_string(),
                    location: AxisLocation::One {
                        tag: Tag::new(b"YEAR"),
                        value: Fixed::from_f64(2020.0),
                    },
                },
            ]
        );
    }

    #[test]
    fn test_config_ranges() {
        let config = parse_config(
            r#"
- name: Weight
  tag: wght
  values:
  - name: Nominal
    nominalValue: 100
  - name: Ranged
    nominalValue: 200
    rangeMinValue: 150
    rangeMaxValue: 250
"#,
        );
        let builder = StatBuilder::from_config(family_config(&config)).unwrap();
        // A nominal value gives format 2, defaulting its range to the Python's
        // infinities.
        assert_eq!(
            builder.values[0].location,
            AxisLocation::Two {
                tag: Tag::new(b"wght"),
                nominal: Fixed::from_f64(100.0),
                min: Fixed::from_bits(i32::MIN),
                max: Fixed::from_bits(i32::MAX),
            }
        );
        assert_eq!(
            builder.values[1].location,
            AxisLocation::Two {
                tag: Tag::new(b"wght"),
                nominal: Fixed::from_f64(200.0),
                min: Fixed::from_f64(150.0),
                max: Fixed::from_f64(250.0),
            }
        );
    }

    #[test]
    fn test_config_without_a_value() {
        let config = parse_config("- name: Weight\n  tag: wght\n  values:\n  - name: Nonsense\n");
        let error = StatBuilder::from_config(family_config(&config)).unwrap_err();
        assert!(error
            .to_string()
            .contains("Can't determine format for AxisValue"));
    }

    #[test]
    fn test_per_file_config() {
        let config = parse_config(
            r#"
Font[wght].ttf:
- name: Weight
  tag: wght
  values:
  - name: Regular
    value: 400
Font-Italic[wght].ttf:
- name: Weight
  tag: wght
  values:
  - name: Italic
    value: 400
"#,
        );
        let StatConfig::PerFile(configs) = &config else {
            panic!("expected a per-file config")
        };
        assert_eq!(configs.len(), 2);
        assert_eq!(config.axes_for("Font[wght].ttf").unwrap()[0].tag, "wght");
        assert_eq!(
            config.axes_for("Font-Italic[wght].ttf").unwrap()[0].values[0]
                .name
                .to_name(),
            "Italic"
        );
        let error = config.axes_for("Nope.ttf").unwrap_err();
        assert!(error
            .to_string()
            .contains("Filename Nope.ttf not found in stat dictionary"));
    }

    #[test]
    fn test_gen_stat_tables_from_config() {
        let config = parse_config(
            r#"
- name: Weight
  tag: wght
  values:
  - name: Regular
    value: 400
    linkedValue: 700
    flags: 2
  - name: Bold
    value: 700
"#,
        );
        let fonts = vec![roboto()];
        let out = gen_stat_tables_from_config(&config, &fonts, None).unwrap();
        let font = FontRef::new(&out[0]).unwrap();
        // Roboto's own STAT table is replaced by the configured one.
        assert_eq!(axis_tags(&font), vec!["wght".to_string()]);
        assert_eq!(
            axis_rows(&font),
            vec![
                (
                    "wght".to_string(),
                    "Regular".to_string(),
                    "= 400.0 linked to 700.0".to_string()
                ),
                (
                    "wght".to_string(),
                    "Bold".to_string(),
                    "= 700.0".to_string()
                ),
            ]
        );
        assert_eq!(axis_value_flags(&font), vec![2, 0]);
        // The axis name went into the name table too.
        let stat = font.stat().unwrap();
        assert_eq!(
            localized_name(&font, stat.design_axes().unwrap()[0].axis_name_id()),
            "Weight"
        );
        // The rest of the font is copied over untouched.
        assert_eq!(
            font.axes()
                .iter()
                .map(|axis| axis.tag().to_string())
                .collect::<Vec<_>>(),
            vec!["wght", "wdth"]
        );
    }

    #[test]
    fn test_generated_italic_axis() {
        let config = parse_config(
            "- name: Weight\n  tag: wght\n  values:\n  - name: Regular\n    value: 400\n",
        );
        // Without italics, the config is used as it is.
        let out = gen_stat_tables_from_config(&config, &[roboto()], Some(false)).unwrap();
        assert_eq!(axis_tags(&FontRef::new(&out[0]).unwrap()), vec!["wght"]);
        // With italics, an ital axis is appended, holding the Roman value for
        // a font which is not itself an italic.
        let out = gen_stat_tables_from_config(&config, &[roboto()], Some(true)).unwrap();
        let font = FontRef::new(&out[0]).unwrap();
        assert_eq!(axis_tags(&font), vec!["wght", "ital"]);
        assert_eq!(
            axis_rows(&font).last().unwrap(),
            &(
                "ital".to_string(),
                "Roman".to_string(),
                "= 0.0 linked to 1.0".to_string()
            )
        );
        assert_eq!(axis_value_flags(&font), vec![0, 2]);
        // ...but an ital axis in a family-wide config is rejected, as the
        // Python rejects it.
        let with_ital =
            parse_config("- name: Italic\n  tag: ital\n  values:\n  - name: Roman\n    value: 0\n");
        let error = gen_stat_tables_from_config(&with_ital, &[roboto()], Some(true)).unwrap_err();
        assert!(error
            .to_string()
            .contains("ital axis should not appear in stat config"));
    }

    #[test]
    fn test_gen_stat_tables() {
        let fonts = vec![roboto()];
        let out = gen_stat_tables(&fonts).unwrap();
        let font = FontRef::new(&out[0]).unwrap();
        // The registry builds the STAT table from the font's own axes.
        let tags = axis_tags(&font);
        assert!(tags.contains(&"wght".to_string()), "{tags:?}");
        assert!(tags.contains(&"wdth".to_string()), "{tags:?}");
        assert!(!axis_rows(&font).is_empty());
    }
}
