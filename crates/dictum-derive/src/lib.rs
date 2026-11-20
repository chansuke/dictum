//! `#[derive(Dictum)]`. Use it through `dictum::Dictum`.
//!
//! From one field list it emits, inside an anonymous `const _: () = { .. }`:
//! - `static DESC: StructDesc` — field names (after renames) and `fn() -> Type` per
//!   field, in declaration order (skipped fields excluded);
//! - `impl Object` — `field(FieldId(i))` reads field `i` of that same list;
//! - `impl Field` — so the struct can be nested in another fact;
//! - `impl Dictum` — so it can be a schema root.
//!
//! Because type information and field access come from the same list in the same
//! expansion, they cannot disagree (the design doc's central derive invariant).
//!
//! Attributes:
//! - `#[dictum(rename_all = "PascalCase" | "camelCase")]` on the struct
//! - `#[dictum(crate = "path::to::dictum")]` on the struct (default `::dictum`)
//! - `#[dictum(rename = "Age")]`, `#[dictum(skip)]` on fields
//!
//! Rejected at compile time: enums, unions, tuple/unit structs, type or const
//! parameters (a `static` descriptor cannot depend on them), names that are not
//! dictum identifiers or are reserved words (`contains`, `matches`, ...), duplicate
//! names after renaming. Unsupported field types fail on the `Field` bound with a
//! message listing the supported ones.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::visit_mut::VisitMut;
use syn::{Data, DeriveInput, Error, Fields, GenericParam, LitStr, parse_macro_input};

/// Derive `dictum::Dictum` (and the hidden `Object` and `Field` impls).
#[proc_macro_derive(Dictum, attributes(dictum))]
pub fn derive_dictum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input).unwrap_or_else(Error::into_compile_error).into()
}

#[derive(Clone, Copy)]
enum RenameAll {
    Pascal,
    Camel,
}

struct StructAttrs {
    krate: syn::Path,
    rename_all: Option<RenameAll>,
}

struct FieldAttrs {
    rename: Option<LitStr>,
    skip: bool,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let attrs = struct_attrs(input)?;
    let krate = &attrs.krate;

    let mut lifetimes = Vec::new();
    for param in &input.generics.params {
        match param {
            GenericParam::Lifetime(l) => lifetimes.push(l.lifetime.ident.to_string()),
            other => {
                return Err(Error::new_spanned(
                    other,
                    "#[derive(Dictum)] supports lifetime parameters only: a struct's descriptor is a `static`, which cannot depend on type or const parameters",
                ));
            }
        }
    }

    let named = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(n) => &n.named,
            _ => return Err(Error::new_spanned(&input.ident, "#[derive(Dictum)] needs a struct with named fields")),
        },
        _ => return Err(Error::new_spanned(&input.ident, "#[derive(Dictum)] supports structs only")),
    };

    let mut names: Vec<String> = Vec::new();
    let mut descs = Vec::new();
    let mut arms = Vec::new();
    for field in named {
        let fa = field_attrs(field)?;
        if fa.skip {
            continue;
        }
        let Some(ident) = &field.ident else { continue };
        let rust_name = ident.to_string();
        let rust_name = rust_name.strip_prefix("r#").unwrap_or(&rust_name);
        let (name, name_span) = match &fa.rename {
            Some(lit) => (lit.value(), lit.span()),
            None => (apply_rename_all(rust_name, attrs.rename_all), ident.span()),
        };
        if !dictum_syntax::is_identifier(&name) {
            let why = if dictum_syntax::is_reserved(&name) {
                "is a reserved word in dictum"
            } else {
                "is not a dictum identifier"
            };
            return Err(Error::new(
                name_span,
                format!("`{name}` {why}; use #[dictum(rename = \"...\")] or #[dictum(skip)]"),
            ));
        }
        if names.contains(&name) {
            return Err(Error::new(name_span, format!("two fields are named `{name}` in rules")));
        }
        let Ok(index) = u16::try_from(descs.len()) else {
            return Err(Error::new_spanned(field, "too many fields (more than 65535)"));
        };
        let mut static_ty = field.ty.clone();
        StaticLifetimes { names: &lifetimes }.visit_type_mut(&mut static_ty);
        descs.push(quote! {
            #krate::__private::FieldDesc::new(#name, <#static_ty as #krate::__private::Field>::ty)
                .with_range(<#static_ty as #krate::__private::Field>::RANGE)
        });
        arms.push(quote! { #index => #krate::__private::Field::read(&self.#ident), });
        names.push(name);
    }

    let ident = &input.ident;
    let type_name = ident.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let statics = lifetimes.iter().map(|_| quote!('static));
    let static_self = if lifetimes.is_empty() { quote!(#ident) } else { quote!(#ident<#(#statics),*>) };

    Ok(quote! {
        const _: () = {
            static __DICTUM_DESC: #krate::__private::StructDesc =
                #krate::__private::StructDesc::new(#type_name, &[#(#descs),*]);

            impl #impl_generics #krate::__private::Object for #ident #ty_generics #where_clause {
                fn desc(&self) -> &'static #krate::__private::StructDesc {
                    &__DICTUM_DESC
                }
                fn field(&self, id: #krate::__private::FieldId) -> #krate::__private::Value<'_> {
                    match id.index() {
                        #(#arms)*
                        _ => #krate::__private::missing_field(),
                    }
                }
            }

            impl #impl_generics #krate::__private::Field for #ident #ty_generics #where_clause {
                fn ty() -> #krate::__private::Type {
                    #krate::__private::Type::Struct(&__DICTUM_DESC)
                }
                fn read(&self) -> #krate::__private::Value<'_> {
                    #krate::__private::Value::object(self)
                }
            }

            impl #impl_generics #krate::Dictum for #ident #ty_generics #where_clause {
                type Static = #static_self;
                fn __desc() -> &'static #krate::__private::StructDesc {
                    &__DICTUM_DESC
                }
            }
        };
    })
}

fn struct_attrs(input: &DeriveInput) -> syn::Result<StructAttrs> {
    let mut out = StructAttrs { krate: syn::parse_quote!(::dictum), rename_all: None };
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("dictum")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                let lit: LitStr = meta.value()?.parse()?;
                out.krate = lit.parse()?;
                Ok(())
            } else if meta.path.is_ident("rename_all") {
                let lit: LitStr = meta.value()?.parse()?;
                out.rename_all = Some(match lit.value().as_str() {
                    "PascalCase" => RenameAll::Pascal,
                    "camelCase" => RenameAll::Camel,
                    _ => return Err(Error::new(lit.span(), "expected \"PascalCase\" or \"camelCase\"")),
                });
                Ok(())
            } else {
                Err(meta.error("expected `rename_all` or `crate`"))
            }
        })?;
    }
    Ok(out)
}

fn field_attrs(field: &syn::Field) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs { rename: None, skip: false };
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("dictum")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                out.rename = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("skip") {
                out.skip = true;
                Ok(())
            } else {
                Err(meta.error("expected `rename` or `skip`"))
            }
        })?;
    }
    Ok(out)
}

/// `user_name` → `UserName` (Pascal) or `userName` (camel). Leading/trailing and
/// repeated underscores are dropped, as serde does.
fn apply_rename_all(name: &str, rule: Option<RenameAll>) -> String {
    let Some(rule) = rule else { return name.to_owned() };
    let mut out = String::with_capacity(name.len());
    let mut upper_next = matches!(rule, RenameAll::Pascal);
    for c in name.chars() {
        if c == '_' {
            upper_next = !out.is_empty() || matches!(rule, RenameAll::Pascal);
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Replace the struct's own lifetime parameters by `'static` in a field type, for
/// the `static` descriptor (which cannot name `'a`). Types are lifetime-independent
/// in dictum, so `<&'static User as Field>::ty` is `<&'a User as Field>::ty`.
struct StaticLifetimes<'n> {
    names: &'n [String],
}

impl VisitMut for StaticLifetimes<'_> {
    fn visit_lifetime_mut(&mut self, l: &mut syn::Lifetime) {
        if self.names.iter().any(|n| l.ident == n.as_str()) {
            *l = syn::Lifetime::new("'static", Span::call_site());
        }
    }
}
