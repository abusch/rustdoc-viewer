//! What the app makes of each [`ItemKind`]: how it is named, and what it can hold.
//!
//! Every screen that names or groups items reads it from here, so a kind is
//! spelled the same way on the page header, the module listing, and the search
//! results. A kind comes either from rustdoc's `paths` ([`Universe::kind_of`])
//! or from an item in hand ([`ItemEnum::item_kind`]); the two agree, but only
//! the second covers items missing from `paths`.
//!
//! [`Universe::kind_of`]: crate::docs::Universe::kind_of
//! [`ItemEnum::item_kind`]: rustdoc_types::ItemEnum::item_kind

use rustdoc_types::ItemKind;

/// The kind as a page names it: "Struct", "Type Alias".
pub fn label(k: ItemKind) -> &'static str {
    match k {
        ItemKind::Module => "Module",
        ItemKind::Struct => "Struct",
        ItemKind::StructField => "Field",
        ItemKind::Union => "Union",
        ItemKind::Enum => "Enum",
        ItemKind::Variant => "Variant",
        ItemKind::Function => "Function",
        ItemKind::Trait => "Trait",
        ItemKind::TraitAlias => "Trait Alias",
        ItemKind::Impl => "Impl",
        ItemKind::TypeAlias => "Type Alias",
        ItemKind::Constant => "Constant",
        ItemKind::Static => "Static",
        ItemKind::Macro => "Macro",
        ItemKind::ProcAttribute => "Attribute Macro",
        ItemKind::ProcDerive => "Derive Macro",
        ItemKind::AssocConst => "Assoc Const",
        ItemKind::AssocType => "Assoc Type",
        ItemKind::Primitive => "Primitive",
        ItemKind::Keyword => "Keyword",
        ItemKind::ExternCrate => "Extern Crate",
        ItemKind::Use => "Use",
        ItemKind::ExternType => "Extern Type",
        ItemKind::Attribute => "Attribute",
    }
}

/// The kind as a search result tags it: the keyword that declares it, where
/// there is one, since that is what a reader scans a column of results for.
pub fn badge(k: ItemKind) -> &'static str {
    match k {
        ItemKind::Module => "mod",
        ItemKind::Struct => "struct",
        ItemKind::StructField => "field",
        ItemKind::Union => "union",
        ItemKind::Enum => "enum",
        ItemKind::Variant => "variant",
        ItemKind::Function => "fn",
        ItemKind::Trait | ItemKind::TraitAlias => "trait",
        ItemKind::Impl => "impl",
        ItemKind::TypeAlias | ItemKind::AssocType | ItemKind::ExternType => "type",
        ItemKind::Constant | ItemKind::AssocConst => "const",
        ItemKind::Static => "static",
        ItemKind::Macro => "macro",
        ItemKind::ProcAttribute | ItemKind::Attribute => "attr",
        ItemKind::ProcDerive => "derive",
        ItemKind::Primitive => "prim",
        ItemKind::Keyword => "keyword",
        ItemKind::ExternCrate => "crate",
        ItemKind::Use => "use",
    }
}

/// Whether items of this kind carry methods and other associated items.
///
/// These are the kinds a member is shown on rather than given a page of its
/// own; a module is not among them, which is what keeps `mem::swap` a page
/// while `String::push_str` is a row on `String`'s.
pub fn has_members(k: ItemKind) -> bool {
    matches!(
        k,
        ItemKind::Struct | ItemKind::Enum | ItemKind::Union | ItemKind::Trait | ItemKind::Primitive
    )
}

/// Whether a kind can hold other items, and so can serve as a parent.
pub fn is_container(k: ItemKind) -> bool {
    k == ItemKind::Module || has_members(k)
}
