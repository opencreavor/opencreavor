#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAI,
}

pub fn provider_for_path(path: &str) -> Option<Provider> {
    if path == "/v1/anthropic" || path.starts_with("/v1/anthropic/") {
        return Some(Provider::Anthropic);
    }

    if path == "/v1/openai" || path.starts_with("/v1/openai/") {
        return Some(Provider::OpenAI);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_anthropic_paths_to_anthropic() {
        assert_eq!(
            provider_for_path("/v1/anthropic/messages"),
            Some(Provider::Anthropic)
        );
    }

    #[test]
    fn routes_openai_paths_to_openai() {
        assert_eq!(
            provider_for_path("/v1/openai/responses"),
            Some(Provider::OpenAI)
        );
    }

    #[test]
    fn ignores_unknown_paths() {
        assert_eq!(provider_for_path("/v1/other/messages"), None);
    }
}
