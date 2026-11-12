#![cfg(not(miri))]

use proc_macro2::{Span, TokenStream, TokenTree};
use std::{
    fs,
    path::{Path, PathBuf},
};
use syn::{
    parse::Parser,
    spanned::Spanned,
    visit::{self, Visit},
};

struct Audit<'a> {
    file: &'a Path,
    lines: Vec<&'a str>,
    allowed: bool,
    violations: Vec<String>,
}

impl Audit<'_> {
    fn check(&mut self, span: Span) {
        let line = span.start().line;
        if !self.allowed {
            self.violations.push(format!(
                "{}:{line}: unsafe location is outside B-2",
                self.file.display()
            ));
        }
        let previous = line.checked_sub(2).and_then(|index| self.lines.get(index));
        if !previous.is_some_and(|text| text.trim_start().starts_with("// SAFETY:")) {
            self.violations.push(format!(
                "{}:{line}: preceding SAFETY comment is required",
                self.file.display()
            ));
        }
    }
    fn macro_tokens(&mut self, tokens: TokenStream) {
        let tokens: Vec<_> = tokens.into_iter().collect();
        for (index, token) in tokens.iter().enumerate() {
            match token {
                TokenTree::Group(group) => self.macro_tokens(group.stream()),
                TokenTree::Ident(ident) if ident == "unsafe" => {
                    let suffix: TokenStream = tokens[index..].iter().cloned().collect();
                    let function_type = |input: syn::parse::ParseStream<'_>| {
                        input.parse::<syn::TypeBareFn>()?;
                        input.parse::<TokenStream>()
                    };
                    if function_type.parse2(suffix).is_err() {
                        self.check(ident.span());
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for Audit<'_> {
    fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
        self.check(node.unsafe_token.span);
        visit::visit_expr_unsafe(self, node);
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if let Some(token) = &node.sig.unsafety {
            self.check(token.span);
        }
        visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if let Some(token) = &node.sig.unsafety {
            self.check(token.span);
        }
        visit::visit_impl_item_fn(self, node);
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if let Some(token) = &node.unsafety {
            self.check(token.span);
        }
        visit::visit_item_impl(self, node);
    }
    fn visit_item_foreign_mod(&mut self, node: &'ast syn::ItemForeignMod) {
        if let Some(token) = &node.unsafety {
            self.check(token.span);
        }
        visit::visit_item_foreign_mod(self, node);
    }
    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        if node.path().is_ident("unsafe") {
            self.check(node.span());
        }
        visit::visit_attribute(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.macro_tokens(node.tokens.clone());
        visit::visit_macro(self, node);
    }
}

fn sources(path: &Path, output: &mut Vec<PathBuf>) {
    if !path.exists() {
        return;
    }
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, output);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            output.push(path);
        }
    }
}

#[test]
fn unsafe_is_localized_and_documents_its_contract() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let root = manifest.parent().unwrap().parent().unwrap();
    let mut files = Vec::new();
    for folder in ["crates", "plugins", "fixtures"] {
        sources(&root.join(folder), &mut files);
    }
    sources(&manifest.join("src"), &mut files);
    sources(&manifest.join("tests"), &mut files);
    files.sort();
    files.dedup();
    let mut violations = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        let parsed =
            syn::parse_file(&source).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        let relative = file.strip_prefix(root).unwrap().to_string_lossy();
        let allowed = file.starts_with(&manifest)
            || relative.starts_with("crates/cordis-host/")
            || relative.starts_with("crates/cordis-sdk-macros/");
        let mut audit = Audit {
            file: &file,
            lines: source.lines().collect(),
            allowed,
            violations: Vec::new(),
        };
        audit.visit_file(&parsed);
        violations.extend(audit.violations);
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
