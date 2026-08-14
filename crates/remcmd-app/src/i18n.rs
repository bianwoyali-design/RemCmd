use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
use remcmd_core::LanguageMode;
use unic_langid::LanguageIdentifier;

const EN_US: &str = include_str!("../i18n/en-US/remcmd.ftl");
const ZH_CN: &str = include_str!("../i18n/zh-CN/remcmd.ftl");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLanguage {
    EnUs,
    ZhCn,
}

impl AppLanguage {
    pub fn resolve(mode: LanguageMode) -> Self {
        match mode {
            LanguageMode::EnUs => Self::EnUs,
            LanguageMode::ZhCn => Self::ZhCn,
            LanguageMode::System => sys_locale::get_locale()
                .filter(|locale| locale.to_ascii_lowercase().starts_with("zh"))
                .map_or(Self::EnUs, |_| Self::ZhCn),
        }
    }
}

pub struct Localizer {
    english: FluentBundle<FluentResource>,
    selected: FluentBundle<FluentResource>,
}

impl Localizer {
    pub fn new(mode: LanguageMode) -> Self {
        let language = AppLanguage::resolve(mode);
        Self {
            english: bundle("en-US", EN_US),
            selected: match language {
                AppLanguage::EnUs => bundle("en-US", EN_US),
                AppLanguage::ZhCn => bundle("zh-CN", ZH_CN),
            },
        }
    }

    pub fn text(&self, key: &str) -> String {
        self.text_with(key, None)
    }

    pub fn text_with(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        format_message(&self.selected, key, args)
            .or_else(|| format_message(&self.english, key, args))
            .unwrap_or_else(|| key.to_owned())
    }
}

fn bundle(locale: &str, source: &str) -> FluentBundle<FluentResource> {
    let locale: LanguageIdentifier = locale.parse().expect("bundled locale is valid");
    let resource = FluentResource::try_new(source.to_owned())
        .unwrap_or_else(|(_, errors)| panic!("invalid Fluent resource: {errors:?}"));
    let mut bundle = FluentBundle::new(vec![locale]);
    bundle.set_use_isolating(false);
    bundle
        .add_resource(resource)
        .expect("bundled Fluent resource has unique keys");
    bundle
}

fn format_message(
    bundle: &FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs<'_>>,
) -> Option<String> {
    let message = bundle.get_message(key)?;
    let pattern = message.value()?;
    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, args, &mut errors);
    if errors.is_empty() {
        Some(value.into_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn keys(resource: &str) -> BTreeSet<String> {
        resource
            .lines()
            .filter(|line| !line.starts_with(char::is_whitespace) && !line.starts_with('#'))
            .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim().to_owned()))
            .filter(|key| !key.is_empty())
            .collect()
    }

    #[test]
    fn fluent_catalogs_have_identical_key_sets() {
        assert_eq!(keys(EN_US), keys(ZH_CN));
    }

    #[test]
    fn fluent_variables_format_in_both_languages() {
        let mut args = FluentArgs::new();
        args.set("number", 2);
        assert_eq!(
            Localizer::new(LanguageMode::EnUs).text_with("terminal-number", Some(&args)),
            "Terminal 2"
        );
        assert_eq!(
            Localizer::new(LanguageMode::ZhCn).text_with("terminal-number", Some(&args)),
            "终端 2"
        );
    }

    #[test]
    fn missing_selected_message_falls_back_to_english() {
        let english = bundle("en-US", "only-english = English fallback\n");
        let selected = bundle("zh-CN", "another-key = 另一条消息\n");
        assert_eq!(
            format_message(&selected, "only-english", None).or_else(|| format_message(
                &english,
                "only-english",
                None
            )),
            Some("English fallback".into())
        );
    }
}
