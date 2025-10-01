//! Tests for provider trait

use super::*;

#[test]
fn test_provider_capabilities() {
    let caps = ProviderCapabilities {
        supports_streaming: true,
        supports_tools: true,
        supports_vision: false,
    };

    assert!(caps.supports_streaming);
    assert!(caps.supports_tools);
    assert!(!caps.supports_vision);
}

#[test]
fn test_provider_capabilities_clone() {
    let caps1 = ProviderCapabilities {
        supports_streaming: true,
        supports_tools: false,
        supports_vision: true,
    };

    let caps2 = caps1.clone();

    assert_eq!(caps1.supports_streaming, caps2.supports_streaming);
    assert_eq!(caps1.supports_tools, caps2.supports_tools);
    assert_eq!(caps1.supports_vision, caps2.supports_vision);
}

#[test]
fn test_provider_capabilities_combinations() {
    // OpenAI-like capabilities
    let openai_caps = ProviderCapabilities {
        supports_streaming: true,
        supports_tools: true,
        supports_vision: true,
    };
    assert!(openai_caps.supports_streaming && openai_caps.supports_tools);

    // Anthropic-like capabilities
    let anthropic_caps = ProviderCapabilities {
        supports_streaming: true,
        supports_tools: true,
        supports_vision: true,
    };
    assert!(anthropic_caps.supports_streaming && anthropic_caps.supports_tools);

    // Basic provider
    let basic_caps = ProviderCapabilities {
        supports_streaming: false,
        supports_tools: false,
        supports_vision: false,
    };
    assert!(!basic_caps.supports_streaming);
}
