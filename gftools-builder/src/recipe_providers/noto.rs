use std::collections::HashMap;

use babelfont::{Font, Instance, UserLocation};

use crate::{
    error::ApplicationError,
    operations::{ConfigOperationBuilder, OpStep, addsubset::AddSubsetConfig, fix::FixConfig},
    recipe::{Provider, Recipe, Step},
    recipe_providers::googlefonts::{GoogleFontsOptions, instance_user_location},
};

pub type NotoOptions = GoogleFontsOptions; // They're the same these days

pub struct NotoProvider {
    options: NotoOptions,
    sources: Vec<Font>,
    recipe: Recipe,
    resolved_subset_steps: Vec<ResolvedSubsetStep>,
}

struct ResolvedSubsetStep {
    donor_font: String,
    config: AddSubsetConfig,
}

impl NotoProvider {
    pub fn new(options: NotoOptions) -> Self {
        NotoProvider {
            options,
            sources: vec![],
            recipe: Recipe::default(),
            resolved_subset_steps: vec![],
        }
    }

    fn load_all_sources(&mut self) -> Result<(), ApplicationError> {
        for source in &self.options.sources {
            log::debug!("Loading source font: {}", source);
            let font = babelfont::load(source).map_err(|e| {
                ApplicationError::InvalidRecipe(format!("Failed to load source {source}: {e}"))
            })?;
            self.sources.push(font);
        }
        Ok(())
    }

    fn familyname_path(source: &Font) -> Result<String, ApplicationError> {
        Ok(source
            .names
            .family_name
            .get_default()
            .ok_or_else(|| {
                ApplicationError::InvalidRecipe("Source font is missing a family name".to_string())
            })?
            .replace(" ", ""))
    }

    fn sourcebase(source: &Font) -> Result<String, ApplicationError> {
        Ok(source
            .source
            .as_ref()
            .ok_or_else(|| {
                ApplicationError::InvalidRecipe("Source font is missing a base".to_string())
            })?
            .file_stem()
            .ok_or_else(|| {
                ApplicationError::InvalidRecipe(
                    "Source font's base is not a valid filename".to_string(),
                )
            })?
            .to_string_lossy()
            .to_string())
    }

    fn axis_tags(source: &Font) -> Vec<String> {
        let mut tags = source
            .axes
            .iter()
            .map(|axis| axis.tag.to_string())
            .collect::<Vec<_>>();
        tags.sort();
        tags
    }

    fn variable_target(family: &str, bucket: &str, sourcebase: &str, axis_tags: &str) -> String {
        format!("../fonts/{family}/{bucket}/variable-ttf/{sourcebase}[{axis_tags}].ttf")
    }

    fn static_target(family: &str, bucket: &str, instancebase: &str) -> String {
        format!("../fonts/{family}/{bucket}/ttf/{instancebase}.ttf")
    }

    fn build_all_variables(&mut self) -> Result<(), ApplicationError> {
        if !self.options.build_variable {
            return Ok(());
        }

        for source in self
            .sources
            .iter()
            .filter(|source| source.masters.len() >= 2)
        {
            self.recipe.extend(self.build_a_variable(source)?);
        }
        Ok(())
    }

    fn build_a_variable(&self, source: &Font) -> Result<Recipe, ApplicationError> {
        let mut recipe = Recipe::new();

        let familyname_path = Self::familyname_path(source)?;
        let sourcebase = Self::sourcebase(source)?;
        let tags = Self::axis_tags(source);
        let axis_tags = tags.join(",");

        let source_path = source
            .source
            .as_ref()
            .ok_or_else(|| {
                ApplicationError::InvalidRecipe("Source font is missing a base".to_string())
            })?
            .to_string_lossy()
            .to_string();

        // Unhinted variable: compile + fix
        let unhinted_target =
            Self::variable_target(&familyname_path, "unhinted", &sourcebase, &axis_tags);
        let mut builder = ConfigOperationBuilder::new().source(source_path.clone());
        builder = builder.compile(&self.options.fontc_config);
        builder = builder.fix(&FixConfig::default());
        let unhinted_steps = builder.build();
        recipe.insert(unhinted_target.clone(), unhinted_steps.clone());
        add_slim(
            &mut recipe,
            &tags,
            &axis_tags,
            unhinted_target,
            unhinted_steps.clone(),
        );

        // Full + Googlefonts variables
        if !self.options.include_subsets.is_empty() {
            // Full variable: addSubset + compile
            let full_target =
                Self::variable_target(&familyname_path, "full", &sourcebase, &axis_tags);
            let mut full_builder = ConfigOperationBuilder::new().source(source_path.clone());
            full_builder = self.add_subset_steps(full_builder)?;
            full_builder = full_builder.compile(&self.options.fontc_config);
            let full_steps = full_builder.build();
            recipe.insert(full_target.clone(), full_steps.clone());
            add_slim(&mut recipe, &tags, &axis_tags, full_target, full_steps);

            // Googlefonts variable: addSubset + compile + fix
            let googlefonts_target =
                Self::variable_target(&familyname_path, "googlefonts", &sourcebase, &axis_tags);
            let mut gf_builder = ConfigOperationBuilder::new().source(source_path);
            gf_builder = self.add_subset_steps(gf_builder)?;
            gf_builder = gf_builder.compile(&self.options.fontc_config);
            gf_builder = gf_builder.fix(&self.options.fix_config);
            recipe.insert(googlefonts_target, gf_builder.build());
        } else {
            // Googlefonts variable without subset: compile + fix
            let googlefonts_target =
                Self::variable_target(&familyname_path, "googlefonts", &sourcebase, &axis_tags);
            let mut gf_builder = ConfigOperationBuilder::new().source(source_path);
            gf_builder = gf_builder.compile(&self.options.fontc_config);
            gf_builder = gf_builder.fix(&self.options.fix_config);
            recipe.insert(googlefonts_target, gf_builder.build());
        }

        Ok(recipe)
    }

    fn build_all_statics(&mut self, have_variables: bool) -> Result<(), ApplicationError> {
        if !self.options.build_static {
            return Ok(());
        }
        for source in self.sources.iter() {
            for instance in source.instances.iter() {
                self.recipe
                    .extend(self.build_a_static(source, instance, have_variables)?);
            }
        }
        Ok(())
    }

    fn build_a_static(
        &self,
        source: &Font,
        instance: &Instance,
        have_variables: bool,
    ) -> Result<Recipe, ApplicationError> {
        let mut recipe = Recipe::new();

        let familyname_path = Self::familyname_path(source)?;
        let source_path = source
            .source
            .as_ref()
            .ok_or_else(|| {
                ApplicationError::InvalidRecipe("Source font is missing a base".to_string())
            })?
            .to_string_lossy()
            .to_string();

        let instancebase = format!(
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

        let mut base_builder = ConfigOperationBuilder::new().source(source_path.clone());
        base_builder = base_builder.compile(&self.options.fontc_config);

        if source.instances.len() > 1 {
            let loc: UserLocation = instance_user_location(source, instance)?;
            base_builder = base_builder.instance(&loc);
        }

        // Remove overlaps
        base_builder = base_builder.remove_overlaps();

        // Unhinted static
        let unhinted_target = Self::static_target(&familyname_path, "unhinted", &instancebase);
        recipe.insert(unhinted_target, base_builder.clone().build());

        // Hinted static
        let hinted_target = Self::static_target(&familyname_path, "hinted", &instancebase);
        recipe.insert(
            hinted_target,
            base_builder
                .clone()
                .autohint(Some("--fail-ok".to_string()))
                .build(),
        );

        if !self.options.include_subsets.is_empty() {
            let mut full_builder = ConfigOperationBuilder::new().source(source_path);
            full_builder = self.add_subset_steps(full_builder)?;
            full_builder = full_builder.compile(&self.options.fontc_config);

            if source.instances.len() > 1 {
                let user_loc = instance_user_location(source, instance)?;
                full_builder = full_builder.instance(&user_loc);
            }

            // Full static: addSubset + compile + instance + autohint
            let full_target = Self::static_target(&familyname_path, "full", &instancebase);
            recipe.insert(
                full_target,
                full_builder
                    .clone()
                    .autohint(Some("--fail-ok".to_string()))
                    .build(),
            );

            // Googlefonts static: addSubset + compile + instance + autohint + fix
            if !have_variables {
                // Only build statics for GF if we don't have a variable
                let googlefonts_target =
                    Self::static_target(&familyname_path, "googlefonts", &instancebase);
                let mut gf_builder = full_builder.autohint(Some("--fail-ok".to_string()));
                gf_builder = gf_builder.fix(&self.options.fix_config);
                recipe.insert(googlefonts_target, gf_builder.build());
            }
        } else if !have_variables {
            // Googlefonts static without subset: compile + instance + autohint + fix
            let googlefonts_target =
                Self::static_target(&familyname_path, "googlefonts", &instancebase);
            let mut gf_builder = base_builder.autohint(Some("--fail-ok".to_string()));
            gf_builder = gf_builder.fix(&self.options.fix_config);
            recipe.insert(googlefonts_target, gf_builder.build());
        }

        Ok(recipe)
    }

    fn resolve_subset_steps(&mut self) -> Result<(), ApplicationError> {
        self.resolved_subset_steps.clear();
        for subset_options in &self.options.include_subsets {
            let donor_font = subset_options.obtain_donor_font()?;
            let codepoints = subset_options.subset.resolve()?;
            self.resolved_subset_steps.push(ResolvedSubsetStep {
                donor_font,
                config: AddSubsetConfig {
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
            });
        }
        Ok(())
    }

    // Copied from googlefonts.rs. We should find a better way to share this logic.
    fn add_subset_steps(
        &self,
        mut builder: ConfigOperationBuilder,
    ) -> Result<ConfigOperationBuilder, ApplicationError> {
        for step in &self.resolved_subset_steps {
            builder = builder.add_subset(&step.config, &step.donor_font);
        }
        Ok(builder)
    }
}

fn add_slim(
    recipe: &mut Recipe,
    tags: &[String],
    axis_tags: &str,
    target: String,
    mut steps: crate::recipe::ConfigOperation,
) {
    let slim_target = target
        .replace("variable-ttf", "slim-variable-ttf")
        .replace(&format!("[{axis_tags}]"), "[wght]");
    let mut slim_space = "wght=400:700".to_string();
    if tags.contains(&"wdth".to_string()) {
        slim_space += ",wdth=drop";
    }
    steps.0.push(Step::OperationStep {
        operation: OpStep::Subspace,
        args: Some(slim_space),
        input_file: None,
        extra: HashMap::new(),
        needs: vec![],
    });
    recipe.insert(slim_target, steps);
}

impl Provider for NotoProvider {
    fn generate_recipe(&self) -> Result<Recipe, ApplicationError> {
        let mut provider = Self::new(self.options.clone());
        provider.load_all_sources()?;
        provider.resolve_subset_steps()?;
        provider.build_all_variables()?;
        let have_variables = !provider.recipe.is_empty();
        provider.build_all_statics(have_variables)?;
        Ok(provider.recipe)
    }
}
