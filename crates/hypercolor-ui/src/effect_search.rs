use crate::api;

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedEffect {
    pub effect: api::EffectSummary,
    search_text: String,
}

impl IndexedEffect {
    pub fn new(effect: api::EffectSummary) -> Self {
        let search_text = [
            effect.name.to_lowercase(),
            effect.description.to_lowercase(),
            effect.author.to_lowercase(),
            effect.category.to_lowercase(),
            effect.tags.join(" ").to_lowercase(),
        ]
        .join(" ");

        Self {
            effect,
            search_text,
        }
    }

    pub fn matches_search(&self, term: &str) -> bool {
        term.is_empty() || self.search_text.contains(term)
    }
}
