use hypercolor_types::attachment::ComponentCategory;
use hypercolor_ui::api::TemplateSummary;
use hypercolor_ui::components::component_picker::{filter_components, selected_result_index};

fn template(id: &str, name: &str, vendor: &str, category: ComponentCategory) -> TemplateSummary {
    TemplateSummary {
        id: id.to_owned(),
        name: name.to_owned(),
        vendor: vendor.to_owned(),
        category,
        origin: None,
        led_count: 16,
        description: String::new(),
        tags: Vec::new(),
    }
}

#[test]
fn filter_components_sorts_matches_by_vendor_then_name() {
    let results = filter_components(
        &[
            template("b", "SL Infinity Fan", "Lian Li", ComponentCategory::Fan),
            template("a", "QL Fan", "Corsair", ComponentCategory::Fan),
            template("c", "AL Fan", "Lian Li", ComponentCategory::Fan),
        ],
        "fan",
    );

    let ids = results
        .into_iter()
        .map(|template| template.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["a", "c", "b"]);
}

#[test]
fn selected_result_index_finds_last_selected_template_in_filtered_results() {
    let results = filter_components(
        &[
            template("front", "Front Fan", "Lian Li", ComponentCategory::Fan),
            template("rear", "Rear Fan", "Lian Li", ComponentCategory::Fan),
            template("strip", "Case Strip", "Corsair", ComponentCategory::Strip),
        ],
        "fan",
    );

    assert_eq!(selected_result_index(&results, Some("rear")), Some(1));
    assert_eq!(selected_result_index(&results, Some("strip")), None);
    assert_eq!(selected_result_index(&results, None), None);
}
