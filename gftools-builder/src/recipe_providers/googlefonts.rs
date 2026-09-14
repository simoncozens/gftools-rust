use crate::recipe_providers::includesubsets::IncludeSubsetsOptions;
use babelfont::{Font, Instance, UserCoord, UserLocation};
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use serde_json::Value;
use std::{collections::HashMap, path::Path};

use crate::{
    error::ApplicationError,
    operations::{
        ConfigOperationBuilder, addsubset::AddSubsetConfig, fix::FixConfig, fontc::FontcConfig,
    },
    recipe::{Provider, Recipe},
};

#[derive(PartialEq, Debug, Clone, Copy)]
pub(crate) enum Style {
    Roman,
    Italic,
}

#[allow(clippy::upper_case_acronyms, dead_code)]
#[derive(PartialEq, Debug, Clone, Copy)]
pub(crate) enum FontFormat {
    TTF,
    OTF,
    WOFF2,
}
impl FontFormat {
    fn extension(&self) -> &'static str {
        match self {
            FontFormat::TTF => "ttf",
            FontFormat::OTF => "otf",
            FontFormat::WOFF2 => "woff2",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ItalicDescriptor {
    axis_tag: String,
    // min_value: UserCoord,
    max_value: UserCoord,
}

#[serde_inline_default]
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GoogleFontsOptions {
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub outputs: HashMap<String, String>,
    #[serde(default)]
    pub extra: HashMap<String, Value>,

    // Path-related options
    #[serde_inline_default("../fonts/".to_string())]
    pub output_dir: String,

    #[serde_inline_default("$outputDir/variable".to_string())]
    pub vf_dir: String,

    #[serde_inline_default("$outputDir/ttf".to_string())]
    pub tt_dir: String,

    #[serde_inline_default("$outputDir/otf".to_string())]
    pub ot_dir: String,

    #[serde_inline_default("$outputDir/webfonts".to_string())]
    pub woff_dir: String,

    #[serde(default)]
    pub filename_suffix: Option<String>,

    // Options about what we build
    #[serde_inline_default(true)]
    #[serde(rename = "buildVariable")]
    pub build_variable: bool,

    #[serde_inline_default(true)]
    #[serde(rename = "buildStatic")]
    pub build_static: bool,

    // Oops, OTFs don't exist in the fontc universe
    #[serde_inline_default(true)]
    #[serde(rename = "buildTTF")]
    pub build_ttf: bool,

    #[serde_inline_default(true)]
    pub build_webfont: bool,

    #[serde_inline_default(true)]
    pub remove_overlaps: bool,

    // Fix arguments
    #[serde(flatten, default)]
    pub fix_config: FixConfig,

    // Fontc arguments
    #[serde(flatten, default)]
    pub fontc_config: FontcConfig,

    // Options for adding subsets
    #[serde(default)]
    pub include_subsets: Vec<IncludeSubsetsOptions>,
}

impl Default for GoogleFontsOptions {
    fn default() -> Self {
        // Just deserialize nothing and let serde fill in the defaults
        serde_json::from_str("{}").unwrap()
    }
}

impl GoogleFontsOptions {
    fn vf_dir(&self) -> String {
        self.vf_dir.replace("$outputDir", &self.output_dir)
    }
    fn tt_dir(&self) -> String {
        self.tt_dir.replace("$outputDir", &self.output_dir)
    }
    fn ot_dir(&self) -> String {
        self.ot_dir.replace("$outputDir", &self.output_dir)
    }
    fn woff_dir(&self) -> String {
        self.woff_dir.replace("$outputDir", &self.output_dir)
    }

    pub(crate) fn vf_filename(
        &self,
        source: &Font,
        suffix: Option<&str>,
        format: FontFormat,
        italic_ds: Option<&ItalicDescriptor>,
        roman: Style,
    ) -> Result<String, ApplicationError> {
        let suffix = suffix.unwrap_or("");
        let extension = format.extension();
        let mut sourcebase = source
            .source
            .as_ref()
            .and_then(|x| {
                Path::new(&x)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            })
            .ok_or(ApplicationError::InvalidRecipe(
                "Source font does not have a valid filename".to_string(),
            ))?;

        let mut tags = source
            .axes
            .iter()
            .map(|axis| axis.tag.to_string())
            .collect::<Vec<String>>();
        if let Some(axis_tag) = italic_ds.map(|x| &x.axis_tag) {
            if roman == Style::Italic {
                sourcebase.push_str("-Italic");
            }
            tags.retain(|tag| tag != axis_tag);
        }

        tags.sort();
        let axis_tags = tags.join(",");

        let mut directory = self.vf_dir();
        if extension == "woff2" {
            directory = self.woff_dir();
        }

        if !suffix.is_empty() {
            if sourcebase.contains("-Italic") {
                sourcebase = sourcebase.replace("-Italic", &format!("{}-Italic", suffix));
            } else {
                sourcebase.push_str(suffix);
            }
        }

        Ok(format!("{directory}/{sourcebase}[{axis_tags}].{extension}"))
    }

    #[allow(dead_code)]
    pub fn static_filename(
        &self,
        instancebase: &str,
        suffix: Option<&str>,
        extension: Option<&str>,
    ) -> String {
        let suffix = suffix.unwrap_or("");
        let extension = extension.unwrap_or("ttf");

        let outdir = match extension {
            "ttf" => self.tt_dir(),
            "otf" => self.ot_dir(),
            "woff2" => self.woff_dir(),
            _ => self.tt_dir(),
        };

        let mut instancebase = instancebase.to_string();

        if !suffix.is_empty() {
            if let Some((familyname, style)) = instancebase.rsplit_once('-') {
                instancebase = format!("{familyname}{suffix}-{style}");
            } else {
                instancebase.push_str(suffix);
            }
        }

        format!("{outdir}/{instancebase}.{extension}")
    }
}

pub struct GoogleFontsProvider {
    options: GoogleFontsOptions,
    sources: Vec<Font>,
    recipe: Recipe,
}

impl GoogleFontsProvider {
    pub fn new(options: GoogleFontsOptions) -> Self {
        GoogleFontsProvider {
            options,
            sources: vec![],
            recipe: Recipe::default(),
        }
    }
}

impl Provider for GoogleFontsProvider {
    fn generate_recipe(&self) -> Result<Recipe, ApplicationError> {
        let mut provider = Self::new(self.options.clone());
        provider.load_all_sources()?;
        provider.build_all_variables()?;
        provider.build_all_statics()?;

        // Implementation for rewriting the recipe for Google fonts
        Ok(provider.recipe)
    }
}

impl GoogleFontsProvider {
    fn style_for_instance(
        &self,
        source: &Font,
        instance: &Instance,
        italic_ds: &ItalicDescriptor,
    ) -> Style {
        let Some((_, designspace_value)) = instance
            .location
            .iter()
            .find(|(axis, _)| axis.to_string() == italic_ds.axis_tag)
        else {
            return Style::Roman;
        };

        source
            .axes
            .iter()
            .find(|axis| axis.tag.to_string() == italic_ds.axis_tag)
            .and_then(|axis| axis.designspace_to_userspace(*designspace_value).ok())
            .map(|value| {
                if value == italic_ds.max_value {
                    Style::Italic
                } else {
                    Style::Roman
                }
            })
            .unwrap_or(Style::Roman)
    }

    fn vf_source_for_instance(
        &self,
        source: &Font,
        instance: &Instance,
    ) -> Result<String, ApplicationError> {
        let italic_ds = self.has_slant_italic(source);
        let style = italic_ds
            .as_ref()
            .map(|italic_ds| self.style_for_instance(source, instance, italic_ds))
            .unwrap_or(Style::Roman);

        self.options.vf_filename(
            source,
            self.options.filename_suffix.as_deref(),
            FontFormat::TTF,
            italic_ds.as_ref(),
            style,
        )
    }

    fn load_all_sources(&mut self) -> Result<(), ApplicationError> {
        for source in &self.options.sources {
            let font = babelfont::load(source).map_err(|e| {
                ApplicationError::InvalidRecipe(format!("Failed to load source {source}: {e}"))
            })?;
            self.sources.push(font);
        }
        Ok(())
    }

    fn build_all_variables(&mut self) -> Result<(), ApplicationError> {
        if !self.options.build_variable {
            return Ok(());
        }
        let variable_targets = self
            .sources
            .iter()
            .filter(|source| source.masters.len() >= 2)
            .flat_map(|source| {
                if let Some(italic_ds) = self.has_slant_italic(source) {
                    vec![
                        (source, Some(italic_ds.clone()), Style::Italic),
                        (source, Some(italic_ds), Style::Roman),
                        // if we have a stat file, we need to rewrite it here, unfortunately
                    ]
                } else {
                    vec![(source, None, Style::Roman)]
                }
            })
            .collect::<Vec<_>>();
        let filenames = variable_targets
            .iter()
            .map(|(source, italic_ds, roman)| {
                self.options.vf_filename(
                    source,
                    self.options.filename_suffix.as_deref(),
                    FontFormat::TTF,
                    italic_ds.as_ref(),
                    *roman,
                )
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        let mut new_recipes = vec![];
        for (index, (source, italic_ds, style)) in variable_targets.into_iter().enumerate() {
            let siblings = if index == 0 {
                Some(filenames.iter().skip(1).cloned().collect::<Vec<String>>())
            } else {
                None
            };
            new_recipes.push(self.build_a_variable(source, italic_ds.as_ref(), style, siblings)?);
        }
        let mut flat_recipes = Recipe::new();
        for recipe in new_recipes {
            flat_recipes.extend(recipe.clone());
        }
        // Do avar2
        self.recipe.extend(flat_recipes);
        Ok(())
    }

    fn build_all_statics(&mut self) -> Result<(), ApplicationError> {
        if !self.options.build_static {
            return Ok(());
        }
        for source in self.sources.iter() {
            for instance in source.instances.iter() {
                let recipe = if source.masters.len() > 1 && self.options.build_variable {
                    self.instantiate_a_static(source, instance, FontFormat::TTF)?
                } else {
                    self.build_a_static(source, instance, FontFormat::TTF)?
                };
                self.recipe.extend(recipe);
            }
            // If there are no instances, build at default
            if source.instances.is_empty() {
                let default_instance = Instance {
                    // Try in order: Preferred Subfamily Name, Style Name, styleMapStyleName "Regular"
                    name: source
                        .names
                        .preferred_subfamily_name
                        .get_default()
                        .or_else(|| source.names.wws_subfamily_name.get_default())
                        .unwrap_or(&"Regular".to_string())
                        .into(),
                    ..Default::default()
                };
                println!("Font names: {:?}", source.names);
                let recipe = self.build_a_static(source, &default_instance, FontFormat::TTF)?;
                self.recipe.extend(recipe);
            }
        }
        Ok(())
    }

    fn instantiate_a_static(
        &self,
        source: &Font,
        instance: &Instance,
        format: FontFormat,
    ) -> Result<Recipe, ApplicationError> {
        log::debug!(
            "Considering how to build static font for {} instance {:?}",
            source
                .names
                .family_name
                .get_default()
                .unwrap_or(&"Unknown family".to_string()),
            instance.location
        );
        let instance_base = format!(
            "{}-{}",
            source
                .names
                .family_name
                .get_default()
                .unwrap_or(&"Unknown".to_string()),
            instance
                .name
                .get_default()
                .unwrap_or(&"Regular".to_string())
        )
        .replace(" ", "");
        let target = self.options.static_filename(
            &instance_base,
            self.options.filename_suffix.as_deref(),
            Some(format.extension()),
        );
        log::debug!("Static target filename: {}", target);
        let mut recipe = Recipe::new();
        let vf_filename = self.vf_source_for_instance(source, instance)?;
        let mut builder = ConfigOperationBuilder::new().source(vf_filename);
        if source.instances.len() > 1 {
            let user_loc = instance_user_location(source, instance)?;
            builder = builder.instance(&user_loc);
        }
        // Remove overlap
        builder = builder.remove_overlaps();
        // Autohint steps
        builder = builder.autohint(None);
        // VTT steps
        builder = builder.fix(&self.options.fix_config);

        if self.options.build_webfont && format == FontFormat::TTF {
            let webfont_target = self.options.static_filename(
                &instance_base,
                self.options.filename_suffix.as_deref(),
                Some("woff2"),
            );
            log::debug!(" Building webfont target: {}", webfont_target);
            let webfont_builder = builder.clone().compress();
            recipe.insert(webfont_target, webfont_builder.build());
        }

        recipe.insert(target, builder.build());
        Ok(recipe)
    }

    fn build_a_variable(
        &self,
        source: &Font,
        italic_ds: Option<&ItalicDescriptor>,
        roman: Style,
        siblings: Option<Vec<String>>,
    ) -> Result<Recipe, ApplicationError> {
        log::debug!(
            "Considering how to build variable font for {}",
            source
                .names
                .family_name
                .get_default()
                .unwrap_or(&"Unknown family".to_string())
        );
        let mut recipe = Recipe::new();
        let target = self.options.vf_filename(
            source,
            self.options.filename_suffix.as_deref(),
            FontFormat::TTF,
            italic_ds,
            roman,
        )?;
        log::debug!("VF target filename: {}", target);

        let mut builder = ConfigOperationBuilder::new();
        builder = builder.source(
            source
                .source
                .as_ref()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );
        builder = self.add_subset_steps(builder)?;
        builder = builder.compile(&self.options.fontc_config);
        // Any post-compile steps
        // Any VTT steps
        // If italic, subspace the axes according to style

        builder = builder.fix(&self.options.fix_config);
        if let Some(siblings) = siblings {
            builder = builder.buildstat(&siblings);
        }

        if self.options.build_webfont {
            let webfont_target = self.options.vf_filename(
                source,
                self.options.filename_suffix.as_deref(),
                FontFormat::WOFF2,
                italic_ds,
                roman,
            )?;
            log::debug!(" Building webfont target: {}", webfont_target);
            let webfont_builder = builder.clone().compress();
            recipe.insert(webfont_target, webfont_builder.build());
        }

        // Smallcaps go here

        recipe.insert(target, builder.build());
        Ok(recipe)
    }

    fn build_a_static(
        &self,
        source: &Font,
        instance: &Instance,
        _format: FontFormat,
    ) -> Result<Recipe, ApplicationError> {
        log::debug!(
            "Considering how to build static font from single master source {}",
            source
                .names
                .family_name
                .get_default()
                .unwrap_or(&"Unknown family".to_string())
        );
        let mut recipe = Recipe::new();
        let family_name = source
            .names
            .family_name
            .get_default()
            .unwrap_or(&"Unknown".to_string())
            .replace(" ", "");
        let style_name = instance
            .name
            .get_default()
            .unwrap_or(&"Regular".to_string())
            .replace(" ", "");
        let instance_base = format!("{}-{}", family_name, style_name);

        let target = self.options.static_filename(
            &instance_base,
            self.options.filename_suffix.as_deref(),
            None,
        );
        log::debug!("Static target filename: {}", target);

        let mut builder = ConfigOperationBuilder::new();
        builder = builder.source(
            source
                .source
                .as_ref()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );
        builder = self.add_subset_steps(builder)?;
        // If there are more than 1 masters, we need to slim down the font to
        // the master and instance location specified.
        if source.masters.len() > 1 {
            builder = builder.instantiate_source(&instance.location);
        }
        builder = builder.compile(&self.options.fontc_config);
        // Any post-compile steps
        // Any VTT steps
        // If italic, subspace the axes according to style

        builder = builder.fix(&self.options.fix_config);
        if self.options.build_webfont {
            let webfont_target = self.options.static_filename(
                &instance_base,
                self.options.filename_suffix.as_deref(),
                Some("woff2"),
            );
            log::debug!(" Building webfont target: {}", webfont_target);
            let webfont_builder = builder.clone().compress();
            recipe.insert(webfont_target, webfont_builder.build());
        }

        // Smallcaps go here

        recipe.insert(target, builder.build());
        Ok(recipe)
    }

    fn has_slant_italic(&self, source: &Font) -> Option<ItalicDescriptor> {
        for axis in &source.axes {
            if axis.tag == "ital"
                && let Some((_min, _, max)) = axis.bounds()
            {
                return Some(ItalicDescriptor {
                    axis_tag: axis.tag.to_string(),
                    // min_value: min,
                    max_value: max,
                });
            }
        }

        for axis in &source.axes {
            if axis.tag == "slnt"
                && let Some((min, _, _max)) = axis.bounds()
            {
                return Some(ItalicDescriptor {
                    axis_tag: axis.tag.to_string(),
                    // min_value: max,
                    max_value: min,
                });
            }
        }

        None
    }

    fn add_subset_steps(
        &self,
        mut builder: ConfigOperationBuilder,
    ) -> Result<ConfigOperationBuilder, ApplicationError> {
        for subset_options in &self.options.include_subsets {
            let donor_font = subset_options.obtain_donor_font()?;
            let codepoints = subset_options.subset.resolve()?;
            builder = builder.add_subset(
                &AddSubsetConfig {
                    include_glyphs: vec![],
                    exclude_glyphs: vec![],
                    include_codepoints: codepoints,
                    existing_glyph_handling: if subset_options.force {
                        fontmerge::ExistingGlyphHandling::Replace
                    } else {
                        fontmerge::ExistingGlyphHandling::Skip
                    },
                    layout_handling: subset_options.layout_handling,
                },
                &donor_font,
            )
        }
        Ok(builder)
    }
}

pub fn instance_user_location(
    source: &Font,
    instance: &Instance,
) -> Result<UserLocation, ApplicationError> {
    let design_loc = &instance.location;
    let mut user_loc: UserLocation = UserLocation::default();
    for axis in source.axes.iter() {
        let design_coord = design_loc.get(axis.tag);
        let Some(user_coord) = design_coord
            .map(|d| axis.designspace_to_userspace(d))
            .transpose()
            .map_err(|e| {
                ApplicationError::InvalidRecipe(format!(
                    "Failed to convert design coordinate {:?} for axis {} to user coordinate: {:?}",
                    design_coord, axis.tag, e
                ))
            })?
            .or(axis.default)
        else {
            return Err(ApplicationError::InvalidRecipe(format!(
                "Instance location is missing coordinate for axis {} and axis does not have a default",
                axis.tag
            )));
        };
        user_loc.insert(axis.tag, user_coord);
    }
    log::debug!(
        "Converted design location {:?} to user location {:?}",
        design_loc,
        user_loc
    );
    Ok(user_loc)
}
