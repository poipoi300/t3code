use t3_contracts::project::ProjectIconColor;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
#[path = "icon_catalog.rs"]
mod catalog;
pub use catalog::LUCIDE_ICON_NAMES;

pub const POPULAR_PROJECT_ICONS: &[&str] = &[
    "folder-code",
    "code-2",
    "terminal",
    "globe-2",
    "server",
    "database",
    "bot",
    "sparkles",
    "smartphone",
    "monitor",
    "cloud-cog",
    "package",
    "book-open",
    "flask-conical",
    "shield-check",
    "rocket",
    "gamepad-2",
    "music",
    "image",
    "shopping-bag",
    "git-branch",
    "workflow",
    "wrench",
    "layers-3",
];
pub const PROJECT_EMOJIS: &[(&str, &str)] = &[
    ("💻", "Computer"),
    ("🛠️", "Tools"),
    ("🚀", "Rocket"),
    ("🤖", "Robot"),
    ("✨", "Sparkles"),
    ("⚡", "Lightning"),
    ("🌐", "Web"),
    ("📱", "Mobile"),
    ("🖥️", "Desktop"),
    ("⌨️", "Keyboard"),
    ("⚙️", "Gear"),
    ("🗄️", "Database"),
    ("☁️", "Cloud"),
    ("📦", "Package"),
    ("📚", "Books"),
    ("🧪", "Test tube"),
    ("🔒", "Lock"),
    ("🎮", "Game"),
    ("🎵", "Music"),
    ("🎬", "Movie"),
    ("🖼️", "Picture"),
    ("🛍️", "Shopping"),
    ("🔥", "Fire"),
    ("💡", "Idea"),
    ("🧩", "Puzzle"),
    ("📊", "Chart"),
    ("🧠", "Brain"),
    ("🦄", "Unicorn"),
    ("🐙", "Octopus"),
    ("🌱", "Seedling"),
];
pub struct ProjectIconColorOption {
    pub value: ProjectIconColor,
    pub label: &'static str,
    pub class_name: &'static str,
    pub swatch_class_name: &'static str,
}
macro_rules! colors { ($($value:ident: $name:literal, $label:literal),* $(,)?) => {
    pub const PROJECT_ICON_COLORS: &[ProjectIconColorOption] = &[$(ProjectIconColorOption { value: ProjectIconColor::$value, label: $label, class_name: concat!("text-", $name, "-600 dark:text-", $name, "-400"), swatch_class_name: concat!("bg-", $name, "-500") }),*];
}; }
colors! { Gray: "gray", "Gray", Red: "red", "Red", Orange: "orange", "Orange", Amber: "amber", "Amber", Yellow: "yellow", "Yellow", Lime: "lime", "Lime", Green: "green", "Green", Emerald: "emerald", "Emerald", Teal: "teal", "Teal", Cyan: "cyan", "Cyan", Sky: "sky", "Sky", Blue: "blue", "Blue", Indigo: "indigo", "Indigo", Violet: "violet", "Violet", Purple: "purple", "Purple", Fuchsia: "fuchsia", "Fuchsia", Pink: "pink", "Pink", Rose: "rose", "Rose" }
pub fn project_icon_color_class_name(color: &ProjectIconColor) -> &'static str {
    PROJECT_ICON_COLORS
        .iter()
        .find(|option| &option.value == color)
        .map_or("", |option| option.class_name)
}
pub fn filter_project_icon_names(query: &str) -> Vec<&'static str> {
    let normalized = query
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if normalized.is_empty() {
        return POPULAR_PROJECT_ICONS.to_vec();
    }
    LUCIDE_ICON_NAMES
        .iter()
        .copied()
        .filter(|name| name.contains(&normalized))
        .take(60)
        .collect()
}
pub fn is_lucide_icon_name(name: &str) -> bool {
    let pascal: String = name
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect();
    catalog::LUCIDE_EXPORT_NAMES.contains(&pascal.as_str())
}
pub fn first_emoji(value: &str) -> Option<&str> {
    let first = value.trim().graphemes(true).next()?;
    let glyphs: Vec<_> = first.chars().collect();
    let is_flag = glyphs.len() == 2
        && glyphs
            .iter()
            .all(|c| ('\u{1f1e6}'..='\u{1f1ff}').contains(c));
    let is_keycap = matches!(glyphs.as_slice(), [c, '\u{20e3}'] | [c, '\u{fe0f}', '\u{20e3}'] if c.is_ascii_digit() || matches!(c, '#' | '*'));
    let pictographic = glyphs.iter().any(|c| {
        catalog::EXTENDED_PICTOGRAPHIC
            .iter()
            .any(|(start, end)| (*start..=*end).contains(&(*c as u32)))
    });
    (is_flag || is_keycap || pictographic).then_some(first)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIdentity {
    pub monogram: String,
    pub color: ProjectIconColor,
}
fn is_number(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    )
}
fn is_word(c: char) -> bool {
    is_number(c)
        || matches!(
            get_general_category(c),
            GeneralCategory::UppercaseLetter
                | GeneralCategory::LowercaseLetter
                | GeneralCategory::TitlecaseLetter
                | GeneralCategory::ModifierLetter
                | GeneralCategory::OtherLetter
        )
}
pub fn derive_project_identity(name: &str) -> ProjectIdentity {
    let normalized: String = name.nfkc().collect();
    let normalized = normalized.trim();
    let words: Vec<_> = normalized
        .split(|c| !is_word(c))
        .filter(|s| !s.is_empty())
        .collect();
    let monogram = words.first().map_or_else(
        || "PR".into(),
        |word| {
            let first = word.chars().next().unwrap_or('P');
            let second = word
                .chars()
                .skip(1)
                .find(|c| is_number(*c))
                .or_else(|| {
                    if words.len() > 1 {
                        words.last().and_then(|w| w.chars().next())
                    } else {
                        word.chars().last()
                    }
                })
                .unwrap_or(first);
            [first, second]
                .into_iter()
                .flat_map(char::to_uppercase)
                .take(2)
                .collect()
        },
    );
    let seed = normalized.to_lowercase();
    let seed = if seed.is_empty() { "project" } else { &seed };
    let index = seed.chars().fold(0, |index, c| {
        (index * 31 + c as usize) % PROJECT_ICON_COLORS.len()
    });
    ProjectIdentity {
        monogram,
        color: PROJECT_ICON_COLORS[index].value.clone(),
    }
}
