use t3_contracts::{
    device::DeviceToolVersions, model::PROVIDER_DISPLAY_NAMES,
    provider_instance::default_instance_id_for_driver, server::ServerProvider,
};
pub const DEVICE_TOOL_UPDATE_OWNERSHIP: &str = "This environment's T3 server chooses device tool versions for itself and its SSH hosts. Update that server to receive newer tool versions; updating only your browser or mobile app does not update a remote server.";

fn humanize_slug(slug: &str) -> String {
    let mut spaced = String::new();
    let mut lower = false;
    let mut separated = false;
    for character in slug.chars() {
        if character.is_ascii_uppercase() && lower {
            spaced.push(' ');
        }
        if matches!(character, '_' | '-') {
            if !separated {
                spaced.push(' ');
            }
            separated = true;
        } else {
            spaced.push(character);
            separated = false;
        }
        lower = character.is_ascii_lowercase();
    }
    let mut word_start = true;
    spaced
        .trim()
        .chars()
        .flat_map(|character| {
            let upper = word_start && (character.is_ascii_alphanumeric() || character == '_');
            word_start = !(character.is_ascii_alphanumeric() || character == '_');
            if upper {
                character.to_uppercase().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}
pub fn provider_instance_display_name(provider: &ServerProvider) -> String {
    let name = provider
        .display_name
        .as_ref()
        .and_then(Option::as_ref)
        .map(|name| name.0.trim())
        .filter(|name| !name.is_empty());
    let kind = PROVIDER_DISPLAY_NAMES
        .iter()
        .find(|(driver, _)| *driver == provider.driver.0)
        .map_or_else(
            || humanize_slug(&provider.driver.0),
            |(_, label)| label.to_string(),
        );
    if let Some(name) = name
        && name != kind
    {
        return name.into();
    }
    if provider.instance_id != default_instance_id_for_driver(&provider.driver) {
        let custom = humanize_slug(&provider.instance_id.0);
        if !custom.is_empty() {
            return custom;
        }
    }
    name.map(str::to_owned).unwrap_or(kind)
}
pub fn provider_instance_initials(label: &str) -> String {
    let label: String = label
        .chars()
        .map(|character| {
            use unicode_general_category::{GeneralCategory, get_general_category};
            if !matches!(character, '\'' | '’')
                && matches!(
                    get_general_category(character),
                    GeneralCategory::ConnectorPunctuation
                        | GeneralCategory::DashPunctuation
                        | GeneralCategory::OpenPunctuation
                        | GeneralCategory::ClosePunctuation
                        | GeneralCategory::InitialPunctuation
                        | GeneralCategory::FinalPunctuation
                        | GeneralCategory::OtherPunctuation
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect();
    let words: Vec<_> = label
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '’']))
        .filter(|word| !word.is_empty())
        .collect();
    match words.as_slice() {
        [] => String::new(),
        [word] => word.chars().take(2).collect::<String>().to_uppercase(),
        _ => words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .collect::<String>()
            .to_uppercase(),
    }
}
pub fn normalize_provider_accent(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    (value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit))
    .then_some(value)
}
pub fn provider_accent_foreground(accent: &str) -> &'static str {
    let channel = |offset| {
        accent
            .get(offset..offset + 2)
            .and_then(|value| u8::from_str_radix(value, 16).ok())
            .map_or(0.0, |value| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            })
    };
    let luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    if (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) {
        "#000000"
    } else {
        "#ffffff"
    }
}
pub struct InstanceBadge<'a> {
    pub driver: &'a str,
    pub accent: Option<&'a str>,
    pub acp_agent: Option<&'a str>,
    pub enabled: bool,
}
pub fn should_show_instance_badge(
    entry: &InstanceBadge<'_>,
    entries: &[InstanceBadge<'_>],
) -> bool {
    normalize_provider_accent(entry.accent).is_some()
        || entries
            .iter()
            .filter(|candidate| {
                candidate.driver == entry.driver
                    && (!entry.enabled || candidate.enabled)
                    && (entry.driver != "acpRegistry" || candidate.acp_agent == entry.acp_agent)
            })
            .take(2)
            .count()
            > 1
}
pub fn device_tool_version_labels(tools: Option<&DeviceToolVersions>) -> Vec<String> {
    let Some(tools) = tools else {
        return vec!["Device tool versions have not been checked.".into()];
    };
    [("Device hub", &tools.hub), ("Agent tools", &tools.agent)]
        .map(|(name, tool)| {
            let installed = if tool.installed_versions.is_empty() {
                "none".into()
            } else {
                tool.installed_versions.join(", ")
            };
            let running = tool
                .running_version
                .as_ref()
                .filter(|version| !version.is_empty())
                .map_or(String::new(), |version| format!("; running {version}"));
            format!(
                "{name}: installed {installed}; required {}{running}.",
                tool.required_version
            )
        })
        .into()
}
pub fn device_tool_update_policy(tools: Option<&DeviceToolVersions>) -> &'static str {
    let Some(tools) = tools else {
        return "Versions have not been checked. Reconnect the host and check versions.";
    };
    if [&tools.hub, &tools.agent].iter().any(|tool| {
        !tool.installed_versions.is_empty()
            && !tool.installed_versions.contains(&tool.required_version)
    }) {
        "Update pending. Required tools will install automatically when next used. The host needs network access; an older install is not used as a fallback."
    } else {
        "Required tools are installed automatically when needed. Checking versions does not install or start anything."
    }
}
