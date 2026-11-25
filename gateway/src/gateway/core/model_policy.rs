//! Group publication policy. Vendor/price metadata is deliberately irrelevant.

/// Anchored, case-insensitive glob; only `*` has a special meaning.
pub fn matches(rule: &str, name: &str) -> bool {
    let rule = rule.to_lowercase();
    let name = name.to_lowercase();
    let parts: Vec<_> = rule.split('*').collect();
    if parts.len() == 1 {
        return rule == name;
    }
    let Some(mut rest) = name.strip_prefix(parts[0]) else {
        return false;
    };
    for part in &parts[1..parts.len() - 1] {
        let Some(at) = rest.find(part) else {
            return false;
        };
        rest = &rest[at + part.len()..];
    }
    rest.ends_with(parts[parts.len() - 1])
}

pub fn allows(rules: &[String], name: &str) -> bool {
    rules.is_empty() || rules.iter().any(|rule| matches(rule, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_models_and_literal_punctuation() {
        for (rule, name, expected) in [
            ("deepseek*", "deepseek-v5", true),
            ("qwen*", "Qwen4-Plus", true),
            ("*/qwen*", "Alibaba/Qwen4", true),
            ("deepseek*", "qwen4", false),
            ("deepseek-v4", "deepseek-v5", false),
            ("*a*b", "aab", true),
            ("ab*bc", "abc", false),
            ("glm.5+?", "glm.5+?", true),
            ("glm.5+?", "glmx555", false),
            ("*", "qwen4", true),
            ("qwen*plus", "qwen4-plus-extra", false),
        ] {
            assert_eq!(matches(rule, name), expected, "{rule} / {name}");
        }
        assert!(allows(&[], "unknown-new-model"));
    }
}
