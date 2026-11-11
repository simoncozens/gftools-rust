use babelfont::{DesignCoord, DesignLocation, Instance, Tag};
use gftools::KNOWN_WEIGHTS;
use google_fonts_axisregistry::AxisRegistry;

fn main() {
    let ufo_file: String = std::env::args()
        .nth(1)
        .expect("Please provide a UFO file path");
    let mut font = babelfont::load(&ufo_file).expect("Failed to load UFO file");
    font.instances.clear();
    let registry = AxisRegistry::new();
    let wght_vals = font
        .axes
        .iter()
        .find(|axis| axis.tag == "wght")
        .map(|ax| {
            let mut min = ax.min.map(|x| x.to_f64()).unwrap_or(100.0);
            let max = ax.max.map(|x| x.to_f64()).unwrap_or(900.0);
            let mut vals = vec![min];
            while min < max {
                min += 100.0;
                vals.push(min);
            }
            vals
        })
        .unwrap_or_default();
    let italic_axis = font
        .axes
        .iter()
        .find(|axis| axis.tag == "ital" || axis.tag == "slnt");
    let default_coords = font
        .axes
        .iter()
        .map(|ax| {
            let registry_axis = registry.get(&ax.tag.to_string());
            // If the axis is not in the registry, or the current default is within the
            // registry's axis min/max, take the current default, otherwise take the registry
            // default
            if let Some(registry_axis) = registry_axis {
                if let Some(default) = ax.default {
                    let default_f64 = default.to_f64();
                    if default_f64 >= registry_axis.min_value() as f64
                        && default_f64 <= registry_axis.max_value() as f64
                    {
                        return (ax.tag, DesignCoord::new(default_f64));
                    }
                }
                return (
                    ax.tag,
                    DesignCoord::new(registry_axis.default_value() as f64),
                );
            }
            (
                ax.tag,
                ax.default
                    .map(|x| DesignCoord::new(x.to_f64()))
                    .unwrap_or(DesignCoord::new(0.0)),
            )
        })
        .collect::<DesignLocation>();
    let weight_to_names = KNOWN_WEIGHTS
        .iter()
        .map(|(name, weight)| (*weight as u64, name.to_string()))
        .collect::<std::collections::HashMap<_, _>>();
    for wght in wght_vals {
        let mut location: DesignLocation = default_coords.clone();
        location.insert(Tag::new(b"wght"), DesignCoord::new(wght));
        let name = weight_to_names
            .get(&(wght as u64))
            .cloned()
            .unwrap_or_else(|| wght.to_string());

        let mut new_instance = Instance {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone().into(),
            location,
            ..Default::default()
        };
        if let Some(italic) = italic_axis {
            font.instances.push(new_instance.clone());
            new_instance.name = format!("{} Italic", name).into();
            if italic.tag == "ital" {
                new_instance
                    .location
                    .insert(Tag::new(b"ital"), DesignCoord::new(1.0));
            } else {
                // Min slant
                new_instance.location.insert(
                    Tag::new(b"slnt"),
                    // Realistically there is going to be an axis minimum, but babelfont doesn't
                    // expect one.
                    DesignCoord::new(italic.min.map(|x| x.to_f64()).unwrap_or(-12.0)),
                );
            }
        }
        font.instances.push(new_instance);
    }
    if let Err(e) = font.save(&ufo_file) {
        eprintln!("Failed to save UFO file {}: {}", ufo_file, e);
    }
}
