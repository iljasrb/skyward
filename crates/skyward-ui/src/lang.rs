//! The language the pages speak: English.

/// The language the pages speak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
}

/// A text the pages write.
pub type Text = &'static str;

impl Lang {
    /// The language as a locale, which [`Lang::for_locale`] reads back.
    pub fn tag(self) -> &'static str {
        match self {
            Lang::En => "en",
        }
    }

    /// The language for a locale. The pages speak English alone, so every
    /// valid locale is English; the signature stays fallible for the
    /// settings that read one.
    pub fn for_locale(tag: &str) -> anyhow::Result<Lang> {
        // ICU takes a hyphen alone between the parts of a locale.
        icu_locale_core::Locale::try_from_str(&tag.replace('_', "-"))
            .map_err(|e| anyhow::anyhow!("read the locale {tag:?}: {e}"))?;
        Ok(Lang::En)
    }
}

/// A text from the language files, with its placeholders filled.
#[macro_export]
macro_rules! t {
    ($($tt:tt)*) => {
        formatx::formatx!($($tt)*).unwrap_or_default()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::{TokenStream, TokenTree};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;
    use syn::visit::Visit;

    /// The texts a source file writes: its string literals, those among a
    /// macro's arguments too.
    #[derive(Default)]
    struct Texts(Vec<String>);

    impl Texts {
        fn tokens(&mut self, tokens: TokenStream) {
            for tree in tokens {
                match tree {
                    TokenTree::Literal(lit) => {
                        if let syn::Lit::Str(s) = syn::Lit::new(lit) {
                            self.0.push(s.value());
                        }
                    }
                    TokenTree::Group(group) => self.tokens(group.stream()),
                    TokenTree::Ident(_) | TokenTree::Punct(_) => {}
                }
            }
        }
    }

    impl<'ast> Visit<'ast> for Texts {
        fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
            self.0.push(lit.value());
        }

        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            self.tokens(mac.tokens.clone());
        }
    }

    #[test]
    fn the_typeface_draws_every_text_the_pages_write() {
        let f = crate::font::bundled();
        let mut texts = Texts::default();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            let file = syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
            texts.visit_file(&file);
        }
        let missing: BTreeSet<char> = texts.0.iter().flat_map(|t| t.chars()).filter(|&ch| !ch.is_control() && !f.body.has(ch)).collect();
        assert!(missing.is_empty(), "MiSans draws none of {missing:?}");
    }
    #[test]
    fn each_language_s_locale_reads_back_as_it() {
        assert_eq!(Lang::for_locale(Lang::En.tag()).unwrap(), Lang::En);
    }
}
