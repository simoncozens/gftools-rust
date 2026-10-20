use serde_json::Value;
use skrifa::{FontRef, Tag, raw::TableProvider};
use write_fonts::{
    FontBuilder,
    from_obj::ToOwnedTable as _,
    tables::{
        head::{Flags, Head, MacStyle},
        hhea::Hhea,
        os2::{Os2, SelectionFlags},
        vhea::Vhea,
    },
    types::{FWord, Fixed, LongDateTime, UfWord},
};

use crate::GftoolsError;

fn parse_int(s: &str) -> std::result::Result<u64, std::num::ParseIntError> {
    if let Some(s) = s.strip_prefix("0x") {
        u64::from_str_radix(s, 16)
    } else if let Some(s) = s.strip_prefix("0o") {
        u64::from_str_radix(s, 8)
    } else if let Some(s) = s.strip_prefix("0b") {
        u64::from_str_radix(s, 2)
    } else {
        s.parse::<u64>()
    }
}

enum Effect {
    Or(u64),
    And(u64),
    SetValue(u64),
}

impl Effect {
    fn apply(&self, target: u64) -> u64 {
        match self {
            Effect::Or(mask) => target | mask,
            Effect::And(mask) => target & mask,
            Effect::SetValue(value) => *value,
        }
    }
}

fn parse_value(value: &str) -> Result<Effect, GftoolsError> {
    if let Some(v) = value.strip_prefix("|=") {
        let int_value = parse_int(v)
            .map_err(|_| GftoolsError::Fontsetter("Invalid integer literal".to_string()))?;
        return Ok(Effect::Or(int_value));
    }
    if let Some(v) = value.strip_prefix("&=") {
        let int_value = parse_int(v)
            .map_err(|_| GftoolsError::Fontsetter("Invalid integer literal".to_string()))?;
        return Ok(Effect::And(int_value));
    }
    Ok(Effect::SetValue(parse_int(value).map_err(|_| {
        GftoolsError::Fontsetter("Invalid integer literal".to_string())
    })?))
}

trait ScalarValue: Sized {
    fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError>;
}

macro_rules! impl_scalar_value {
    (unsigned: $($type:ty),+ $(,)?) => {
        $(
            impl ScalarValue for $type {
                fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError> {
                    value
                        .as_u64()
                        .and_then(|value| Self::try_from(value).ok())
                        .ok_or_else(|| {
                            GftoolsError::Fontsetter(format!("Invalid value for {field}"))
                        })
                }
            }
        )+
    };
    (signed: $($type:ty),+ $(,)?) => {
        $(
            impl ScalarValue for $type {
                fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError> {
                    value
                        .as_i64()
                        .and_then(|value| Self::try_from(value).ok())
                        .ok_or_else(|| {
                            GftoolsError::Fontsetter(format!("Invalid value for {field}"))
                        })
                }
            }
        )+
    };
}

impl_scalar_value!(unsigned: u8, u16, u32, u64);
impl_scalar_value!(signed: i8, i16, i32, i64);

impl ScalarValue for FWord {
    fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError> {
        Ok(Self::new(<i16 as ScalarValue>::parse(value, field)?))
    }
}

impl ScalarValue for UfWord {
    fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError> {
        Ok(Self::new(<u16 as ScalarValue>::parse(value, field)?))
    }
}

impl ScalarValue for LongDateTime {
    fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError> {
        Ok(Self::new(<i64 as ScalarValue>::parse(value, field)?))
    }
}

impl ScalarValue for Fixed {
    fn parse(value: &Value, field: &str) -> Result<Self, GftoolsError> {
        let value = value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| GftoolsError::Fontsetter(format!("Invalid value for {field}")))?;
        let bits = (value * 65_536.0).round();
        let bits = i32::try_from(bits as i64)
            .map_err(|_| GftoolsError::Fontsetter(format!("Invalid value for {field}")))?;
        Ok(Self::from_bits(bits))
    }
}

trait BitfieldValue: Sized {
    fn bits(self) -> u64;
    fn from_bits(bits: u64) -> Option<Self>;
}

macro_rules! impl_bitfield_value {
    ($($type:ty),+ $(,)?) => {
        $(
            impl BitfieldValue for $type {
                fn bits(self) -> u64 {
                    self.into()
                }

                fn from_bits(bits: u64) -> Option<Self> {
                    Self::try_from(bits).ok()
                }
            }
        )+
    };
}

impl_bitfield_value!(u8, u16, u32, u64);

impl BitfieldValue for SelectionFlags {
    fn bits(self) -> u64 {
        SelectionFlags::bits(&self).into()
    }

    fn from_bits(bits: u64) -> Option<Self> {
        u16::try_from(bits).ok().and_then(SelectionFlags::from_bits)
    }
}

macro_rules! impl_flag_bitfield_value {
    ($($type:ty),+ $(,)?) => {
        $(
            impl BitfieldValue for $type {
                fn bits(self) -> u64 {
                    <$type>::bits(&self).into()
                }

                fn from_bits(bits: u64) -> Option<Self> {
                    u16::try_from(bits).ok().and_then(<$type>::from_bits)
                }
            }
        )+
    };
}

impl_flag_bitfield_value!(Flags, MacStyle);

fn parse_bitfield<T>(value: &Value, target: T, field: &str) -> Result<T, GftoolsError>
where
    T: BitfieldValue,
{
    let value = value
        .as_str()
        .ok_or_else(|| GftoolsError::Fontsetter(format!("Invalid value for {field}")))?;
    let value = parse_value(value)?.apply(target.bits());
    T::from_bits(value)
        .ok_or_else(|| GftoolsError::Fontsetter(format!("Invalid value for {field}")))
}

fn parse_option_bitfield<T>(
    value: &Value,
    target: Option<T>,
    field: &str,
) -> Result<T, GftoolsError>
where
    T: BitfieldValue + Default,
{
    parse_bitfield(value, target.unwrap_or_default(), field)
}

macro_rules! set_scalar {
    ($target:expr, $value:expr, $field:literal) => {
        $target = ScalarValue::parse(&$value, $field)?
    };
}

macro_rules! set_option_scalar {
    ($target:expr, $value:expr, $field:literal) => {
        $target = Some(ScalarValue::parse(&$value, $field)?)
    };
}

macro_rules! set_bitfield {
    ($target:expr, $value:expr, $field:literal) => {
        $target = parse_bitfield(&$value, $target, $field)?
    };
}

macro_rules! set_option_bitfield {
    ($target:expr, $value:expr, $field:literal) => {
        $target = Some(parse_option_bitfield(&$value, $target, $field)?)
    };
}

macro_rules! assert_leaf {
    ($key:expr, $table:expr) => {
        // Key should just be a single field name
        if $key.len() != 1 {
            return Err(GftoolsError::Fontsetter(format!(
                "Invalid key for {} table",
                $table
            )));
        }
    };
}

trait FontSetter {
    fn set(&mut self, key: Vec<&str>, value: &Value) -> Result<(), GftoolsError>;
}

/// Sets a value in the specified font table.
///
/// # Arguments
///
/// * `font` - A mutable reference to the font.
/// * `key` - The key specifying the table and field to set, in the format `table->field`.
/// * `value` - The value to set. For bitfields, a string starting with `|=` OR-s the current contents of the field with the value; a
///   string starting with `&=` AND-s the current contents of the field with the value.
///
/// # Returns
///
/// A `Result` containing the updated font data as a `Vec<u8>` if successful, or a `GftoolsError` otherwise.
pub fn set_font_value(
    font: &FontRef<'_>,
    key: &str,
    value: &Value,
) -> Result<Vec<u8>, GftoolsError> {
    let mut key_parts = key.split("->").collect::<Vec<&str>>();
    key_parts.reverse(); // So we can pop them
    let table = key_parts
        .pop()
        .ok_or_else(|| GftoolsError::Fontsetter("Invalid key".to_string()))?;
    let mut builder = FontBuilder::new();
    match table {
        "OS/2" => {
            let mut os2: Os2 = font.os2()?.to_owned_table();
            os2.set(key_parts, value)?;
            builder.add_table(&os2)?;
        }
        "head" => {
            let mut head: Head = font.head()?.to_owned_table();
            head.set(key_parts, value)?;
            builder.add_table(&head)?;
        }
        "hhea" => {
            let mut hhea: Hhea = font.hhea()?.to_owned_table();
            hhea.set(key_parts, value)?;
            builder.add_table(&hhea)?;
        }
        "vhea" => {
            let mut vhea: Vhea = font.vhea()?.to_owned_table();
            vhea.set(key_parts, value)?;
            builder.add_table(&vhea)?;
        }
        _ => {
            return Err(GftoolsError::Fontsetter(format!(
                "Unknown table: {}",
                table
            )));
        }
    }
    Ok(builder.build())
}

/// Sets multiple values in the specified font.
///
/// # Arguments
///
/// * `font` - A byte slice representing the font data.
/// * `values` - A vector of key-value pairs specifying the table fields and values to set.
///
/// # Returns
///
/// A `Result` containing the updated font data as a `Vec<u8>` if successful, or a `GftoolsError` otherwise.
pub fn set_font_values(font: &[u8], values: &[(String, Value)]) -> Result<Vec<u8>, GftoolsError> {
    let mut result = font.to_vec();
    for (key, value) in values {
        result = set_font_value(&FontRef::new(&result)?, key, value)?;
    }
    Ok(result)
}

impl FontSetter for Os2 {
    fn set(&mut self, key: Vec<&str>, value: &Value) -> Result<(), GftoolsError> {
        assert_leaf!(key, "OS/2");

        let field = key[0];
        match field {
            "usWeightClass" => {
                set_scalar!(self.us_weight_class, value, "usWeightClass");
            }
            "usWidthClass" => {
                set_scalar!(self.us_width_class, value, "usWidthClass");
            }
            "fsType" => {
                set_bitfield!(self.fs_type, value, "fsType");
            }
            "ySubscriptXSize" => {
                set_scalar!(self.y_subscript_x_size, value, "ySubscriptXSize");
            }
            "ySubscriptYSize" => {
                set_scalar!(self.y_subscript_y_size, value, "ySubscriptYSize");
            }
            "ySubscriptXOffset" => {
                set_scalar!(self.y_subscript_x_offset, value, "ySubscriptXOffset");
            }
            "ySubscriptYOffset" => {
                set_scalar!(self.y_subscript_y_offset, value, "ySubscriptYOffset");
            }
            "ySuperscriptXSize" => {
                set_scalar!(self.y_superscript_x_size, value, "ySuperscriptXSize");
            }
            "ySuperscriptYSize" => {
                set_scalar!(self.y_superscript_y_size, value, "ySuperscriptYSize");
            }
            "ySuperscriptXOffset" => {
                set_scalar!(self.y_superscript_x_offset, value, "ySuperscriptXOffset");
            }
            "ySuperscriptYOffset" => {
                set_scalar!(self.y_superscript_y_offset, value, "ySuperscriptYOffset");
            }
            "yStrikeoutSize" => {
                set_scalar!(self.y_strikeout_size, value, "yStrikeoutSize");
            }
            "yStrikeoutPosition" => {
                set_scalar!(self.y_strikeout_position, value, "yStrikeoutPosition");
            }
            "sFamilyClass" => {
                set_scalar!(self.s_family_class, value, "sFamilyClass");
            }
            // No panose?
            "ulUnicodeRange1" => {
                set_bitfield!(self.ul_unicode_range_1, value, "ulUnicodeRange1");
            }
            "ulUnicodeRange2" => {
                set_bitfield!(self.ul_unicode_range_2, value, "ulUnicodeRange2");
            }
            "ulUnicodeRange3" => {
                set_bitfield!(self.ul_unicode_range_3, value, "ulUnicodeRange3");
            }
            "ulUnicodeRange4" => {
                set_bitfield!(self.ul_unicode_range_4, value, "ulUnicodeRange4");
            }
            "achVendID" => {
                self.ach_vend_id = parse_tag(value)?;
            }
            "fsSelection" => {
                set_bitfield!(self.fs_selection, value, "fsSelection");
            }
            "usFirstCharIndex" => {
                set_scalar!(self.us_first_char_index, value, "usFirstCharIndex");
            }
            "usLastCharIndex" => {
                set_scalar!(self.us_last_char_index, value, "usLastCharIndex");
            }
            "sTypoAscender" => {
                set_scalar!(self.s_typo_ascender, value, "sTypoAscender");
            }
            "sTypoDescender" => {
                set_scalar!(self.s_typo_descender, value, "sTypoDescender");
            }
            "sTypoLineGap" => {
                set_scalar!(self.s_typo_line_gap, value, "sTypoLineGap");
            }
            "usWinAscent" => {
                set_scalar!(self.us_win_ascent, value, "usWinAscent");
            }
            "usWinDescent" => {
                set_scalar!(self.us_win_descent, value, "usWinDescent");
            }
            "ulCodePageRange1" => {
                set_option_bitfield!(self.ul_code_page_range_1, value, "ulCodePageRange1");
            }
            "ulCodePageRange2" => {
                set_option_bitfield!(self.ul_code_page_range_2, value, "ulCodePageRange2");
            }
            "sxHeight" => {
                set_option_scalar!(self.sx_height, value, "sxHeight");
            }
            "sCapHeight" => {
                set_option_scalar!(self.s_cap_height, value, "sCapHeight");
            }
            "usDefaultChar" => {
                set_option_scalar!(self.us_default_char, value, "usDefaultChar");
            }
            "usBreakChar" => {
                set_option_scalar!(self.us_break_char, value, "usBreakChar");
            }
            "usMaxContext" => {
                set_option_scalar!(self.us_max_context, value, "usMaxContext");
            }
            "usLowerOpticalPointSize" => {
                set_option_scalar!(
                    self.us_lower_optical_point_size,
                    value,
                    "usLowerOpticalPointSize"
                );
            }
            "usUpperOpticalPointSize" => {
                set_option_scalar!(
                    self.us_upper_optical_point_size,
                    value,
                    "usUpperOpticalPointSize"
                );
            }

            _ => {
                return Err(GftoolsError::Fontsetter(format!(
                    "Unknown OS/2 table field: {}",
                    field
                )));
            }
        }
        Ok(())
    }
}

impl FontSetter for Head {
    fn set(&mut self, key: Vec<&str>, value: &Value) -> Result<(), GftoolsError> {
        assert_leaf!(key, "head");

        let field = key[0];
        match field {
            "fontRevision" => set_scalar!(self.font_revision, value, "fontRevision"),
            "checkSumAdjustment" => {
                set_scalar!(self.checksum_adjustment, value, "checkSumAdjustment")
            }
            "magicNumber" => set_scalar!(self.magic_number, value, "magicNumber"),
            "flags" => set_bitfield!(self.flags, value, "flags"),
            "unitsPerEm" => set_scalar!(self.units_per_em, value, "unitsPerEm"),
            "created" => set_scalar!(self.created, value, "created"),
            "modified" => set_scalar!(self.modified, value, "modified"),
            "xMin" => set_scalar!(self.x_min, value, "xMin"),
            "yMin" => set_scalar!(self.y_min, value, "yMin"),
            "xMax" => set_scalar!(self.x_max, value, "xMax"),
            "yMax" => set_scalar!(self.y_max, value, "yMax"),
            "macStyle" => set_bitfield!(self.mac_style, value, "macStyle"),
            "lowestRecPPEM" => set_scalar!(self.lowest_rec_ppem, value, "lowestRecPPEM"),
            "fontDirectionHint" => {
                set_scalar!(self.font_direction_hint, value, "fontDirectionHint")
            }
            "indexToLocFormat" => set_scalar!(self.index_to_loc_format, value, "indexToLocFormat"),
            _ => {
                return Err(GftoolsError::Fontsetter(format!(
                    "Unknown head table field: {}",
                    field
                )));
            }
        }
        Ok(())
    }
}

impl FontSetter for Hhea {
    fn set(&mut self, key: Vec<&str>, value: &Value) -> Result<(), GftoolsError> {
        assert_leaf!(key, "hhea");

        let field = key[0];
        match field {
            "ascent" => set_scalar!(self.ascender, value, "ascent"),
            "descent" => set_scalar!(self.descender, value, "descent"),
            "lineGap" => set_scalar!(self.line_gap, value, "lineGap"),
            "advanceWidthMax" => set_scalar!(self.advance_width_max, value, "advanceWidthMax"),
            "minLeftSideBearing" => {
                set_scalar!(self.min_left_side_bearing, value, "minLeftSideBearing")
            }
            "minRightSideBearing" => {
                set_scalar!(self.min_right_side_bearing, value, "minRightSideBearing")
            }
            "xMaxExtent" => set_scalar!(self.x_max_extent, value, "xMaxExtent"),
            "caretSlopeRise" => set_scalar!(self.caret_slope_rise, value, "caretSlopeRise"),
            "caretSlopeRun" => set_scalar!(self.caret_slope_run, value, "caretSlopeRun"),
            "caretOffset" => set_scalar!(self.caret_offset, value, "caretOffset"),
            "numberOfHMetrics" => {
                set_scalar!(self.number_of_h_metrics, value, "numberOfHMetrics")
            }
            _ => {
                return Err(GftoolsError::Fontsetter(format!(
                    "Unknown hhea table field: {}",
                    field
                )));
            }
        }
        Ok(())
    }
}

impl FontSetter for Vhea {
    fn set(&mut self, key: Vec<&str>, value: &Value) -> Result<(), GftoolsError> {
        assert_leaf!(key, "vhea");

        let field = key[0];
        match field {
            "ascent" => set_scalar!(self.ascender, value, "ascent"),
            "descent" => set_scalar!(self.descender, value, "descent"),
            "lineGap" => set_scalar!(self.line_gap, value, "lineGap"),
            "advanceHeightMax" => {
                set_scalar!(self.advance_height_max, value, "advanceHeightMax")
            }
            "minTopSideBearing" => {
                set_scalar!(self.min_top_side_bearing, value, "minTopSideBearing")
            }
            "minBottomSideBearing" => {
                set_scalar!(self.min_bottom_side_bearing, value, "minBottomSideBearing")
            }
            "yMaxExtent" => set_scalar!(self.y_max_extent, value, "yMaxExtent"),
            "caretSlopeRise" => set_scalar!(self.caret_slope_rise, value, "caretSlopeRise"),
            "caretSlopeRun" => set_scalar!(self.caret_slope_run, value, "caretSlopeRun"),
            "caretOffset" => set_scalar!(self.caret_offset, value, "caretOffset"),
            "numberOfLongVerMetrics" => set_scalar!(
                self.number_of_long_ver_metrics,
                value,
                "numberOfLongVerMetrics"
            ),
            _ => {
                return Err(GftoolsError::Fontsetter(format!(
                    "Unknown vhea table field: {}",
                    field
                )));
            }
        }
        Ok(())
    }
}

fn parse_tag(value: &Value) -> Result<Tag, GftoolsError> {
    Tag::new_checked(
        value
            .as_str()
            .ok_or_else(|| {
                GftoolsError::Fontsetter(format!("Invalid value for achVendID: {}", value))
            })?
            .as_bytes(),
    )
    .map_err(|e| GftoolsError::Fontsetter(format!("Invalid tag for achVendID: {}", e)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Effect, FontSetter, Head, Hhea, ScalarValue, Vhea, parse_bitfield, parse_option_bitfield,
        parse_value,
    };

    #[test]
    fn parses_scalar_values_with_their_destination_width_and_signedness() {
        assert_eq!(
            <u16 as ScalarValue>::parse(&json!(100), "field").unwrap(),
            100
        );
        assert_eq!(
            <i16 as ScalarValue>::parse(&json!(-100), "field").unwrap(),
            -100
        );
        assert!(<u16 as ScalarValue>::parse(&json!(65_536), "field").is_err());
        assert!(<i16 as ScalarValue>::parse(&json!(32_768), "field").is_err());
        assert!(<u16 as ScalarValue>::parse(&json!(-1), "field").is_err());
    }

    #[test]
    fn parses_bitfield_assignments() {
        assert_eq!(parse_value("12").unwrap().apply(3), 12);
        assert_eq!(parse_value("|=0x04").unwrap().apply(3), 7);
        assert_eq!(parse_value("&=0x06").unwrap().apply(7), 6);
        assert!(matches!(parse_value("|=0x10000"), Ok(Effect::Or(65_536))));
    }

    #[test]
    fn bitfield_updates_preserve_the_destination_width() {
        assert_eq!(parse_bitfield(&json!("|=0x04"), 3u16, "fsType").unwrap(), 7);
        assert!(parse_bitfield(&json!("0x10000"), 0u16, "fsType").is_err());
    }

    #[test]
    fn optional_bitfields_use_zero_when_the_existing_value_is_absent() {
        assert_eq!(
            parse_option_bitfield(&json!("|=0x04"), None::<u32>, "ulCodePageRange1").unwrap(),
            4
        );
        assert_eq!(
            parse_option_bitfield(&json!("&=0x0f"), Some(0xffu32), "ulCodePageRange1").unwrap(),
            15
        );
    }

    #[test]
    fn sets_head_scalars_and_bitfields() {
        let mut head = Head::default();
        head.set(vec!["unitsPerEm"], &json!(1000)).unwrap();
        head.set(vec!["fontRevision"], &json!(1.5)).unwrap();
        head.set(vec!["flags"], &json!("|=0x01")).unwrap();

        assert_eq!(head.units_per_em, 1000);
        assert_eq!(head.font_revision.to_bits(), 98_304);
        assert_eq!(head.flags.bits(), 1);
    }

    #[test]
    fn sets_horizontal_and_vertical_header_fields() {
        let mut hhea = Hhea::default();
        hhea.set(vec!["ascent"], &json!(750)).unwrap();
        hhea.set(vec!["minLeftSideBearing"], &json!(-20)).unwrap();

        let mut vhea = Vhea::default();
        vhea.set(vec!["advanceHeightMax"], &json!(1200)).unwrap();
        vhea.set(vec!["minBottomSideBearing"], &json!(-30)).unwrap();

        assert_eq!(hhea.ascender.to_i16(), 750);
        assert_eq!(hhea.min_left_side_bearing.to_i16(), -20);
        assert_eq!(vhea.advance_height_max.to_u16(), 1200);
        assert_eq!(vhea.min_bottom_side_bearing.to_i16(), -30);
    }
}
