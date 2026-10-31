//! Rendering rustdoc's type and signature structures back into Rust syntax.

use rustdoc_types::{
    Abi, Enum, FunctionSignature, GenericArg, GenericArgs, GenericBound, GenericParamDef,
    GenericParamDefKind, Generics, Item, ItemEnum, MacroKind, Path, PreciseCapturingArg, Struct,
    Term, TraitBoundModifier, Type, Union, WherePredicate,
};

/// Format a type as Rust source.
pub fn ty(t: &Type) -> String {
    match t {
        Type::ResolvedPath(p) => path(p),
        Type::Generic(g) => g.clone(),
        Type::Primitive(p) => p.clone(),
        Type::DynTrait(d) => {
            let mut parts: Vec<String> = d
                .traits
                .iter()
                .map(|pt| format!("{}{}", hrtb(&pt.generic_params), path(&pt.trait_)))
                .collect();
            if let Some(lt) = &d.lifetime {
                parts.push(lt.clone());
            }
            format!("dyn {}", parts.join(" + "))
        }
        Type::FunctionPointer(f) => format!(
            "{}{}fn{}{}",
            hrtb(&f.generic_params),
            header_prefix(f.header.is_unsafe, false, false, &f.header.abi),
            pointer_params(&f.sig),
            returns(&f.sig.output),
        ),
        Type::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(ty).collect();
            // A 1-tuple needs the trailing comma to stay a tuple.
            if inner.len() == 1 {
                format!("({},)", inner[0])
            } else {
                format!("({})", inner.join(", "))
            }
        }
        Type::Slice(t) => format!("[{}]", ty(t)),
        Type::Array { type_, len } => format!("[{}; {}]", ty(type_), len),
        Type::Pat { type_, .. } => {
            // Pattern types are unstable and have no stable surface syntax.
            format!("{} is _", ty(type_))
        }
        Type::ImplTrait(bounds) => format!("impl {}", bound_list(bounds)),
        Type::Infer => "_".to_string(),
        Type::RawPointer { is_mutable, type_ } => {
            format!(
                "*{} {}",
                if *is_mutable { "mut" } else { "const" },
                ty(type_)
            )
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => format!("{}{}", reference(lifetime, *is_mutable), ty(type_)),
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let base = match trait_ {
                Some(tr) if !tr.path.is_empty() => {
                    format!("<{} as {}>", ty(self_type), tr.path)
                }
                _ => ty(self_type),
            };
            let mut s = format!("{base}::{name}");
            if let Some(a) = args.as_ref() {
                s.push_str(&generic_args(a));
            }
            s
        }
    }
}

/// The `for<'a, 'b> ` prefix of a higher-ranked bound, or empty when there
/// is nothing to bind.
fn hrtb(params: &[GenericParamDef]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    format!("for<{}> ", names.join(", "))
}

/// A path with its generic arguments: `Iterator<Item = u8>`.
fn path(p: &Path) -> String {
    let args = p.args.as_deref().map(generic_args).unwrap_or_default();
    format!("{}{args}", p.path)
}

/// The `&'a mut ` that opens a reference, shared by reference types and
/// `&self` receivers.
fn reference(lifetime: &Option<String>, is_mutable: bool) -> String {
    let mut s = String::from("&");
    if let Some(lt) = lifetime {
        s.push_str(lt);
        s.push(' ');
    }
    if is_mutable {
        s.push_str("mut ");
    }
    s
}

/// The ` -> T` of a signature, or empty for one returning `()`.
fn returns(output: &Option<Type>) -> String {
    output
        .as_ref()
        .map(|t| format!(" -> {}", ty(t)))
        .unwrap_or_default()
}

fn header_prefix(is_unsafe: bool, is_const: bool, is_async: bool, abi: &Abi) -> String {
    let mut s = String::new();
    if is_const {
        s.push_str("const ");
    }
    if is_async {
        s.push_str("async ");
    }
    if is_unsafe {
        s.push_str("unsafe ");
    }
    match abi {
        Abi::Rust => {}
        Abi::C { .. } => s.push_str("extern \"C\" "),
        Abi::Cdecl { .. } => s.push_str("extern \"cdecl\" "),
        Abi::Stdcall { .. } => s.push_str("extern \"stdcall\" "),
        Abi::Fastcall { .. } => s.push_str("extern \"fastcall\" "),
        Abi::Aapcs { .. } => s.push_str("extern \"aapcs\" "),
        Abi::Win64 { .. } => s.push_str("extern \"win64\" "),
        Abi::SysV64 { .. } => s.push_str("extern \"sysv64\" "),
        Abi::System { .. } => s.push_str("extern \"system\" "),
        Abi::Other(o) => {
            s.push_str("extern \"");
            s.push_str(o);
            s.push_str("\" ");
        }
    }
    s
}

pub fn generic_args(args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            let mut parts: Vec<String> = args.iter().map(generic_arg).collect();
            for c in constraints {
                let val = match &c.binding {
                    rustdoc_types::AssocItemConstraintKind::Equality(t) => {
                        format!("= {}", term(t))
                    }
                    rustdoc_types::AssocItemConstraintKind::Constraint(b) => {
                        format!(": {}", bound_list(b))
                    }
                };
                parts.push(format!("{} {}", c.name, val));
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!("<{}>", parts.join(", "))
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let ins: Vec<String> = inputs.iter().map(ty).collect();
            format!("({}){}", ins.join(", "), returns(output))
        }
        GenericArgs::ReturnTypeNotation => "(..)".to_string(),
    }
}

fn generic_arg(a: &GenericArg) -> String {
    match a {
        GenericArg::Lifetime(lt) => lt.clone(),
        GenericArg::Type(t) => ty(t),
        GenericArg::Const(c) => c.expr.clone(),
        GenericArg::Infer => "_".to_string(),
    }
}

fn term(t: &Term) -> String {
    match t {
        Term::Type(t) => ty(t),
        Term::Constant(c) => c.expr.clone(),
    }
}

pub fn bound_list(bounds: &[GenericBound]) -> String {
    let parts: Vec<String> = bounds.iter().map(bound).collect();
    parts.join(" + ")
}

fn bound(b: &GenericBound) -> String {
    match b {
        GenericBound::TraitBound {
            trait_,
            generic_params,
            modifier,
        } => {
            let modifier = match modifier {
                TraitBoundModifier::None => "",
                TraitBoundModifier::Maybe => "?",
                TraitBoundModifier::MaybeConst => "~const ",
            };
            format!("{}{modifier}{}", hrtb(generic_params), path(trait_))
        }
        GenericBound::Outlives(lt) => lt.clone(),
        GenericBound::Use(args) => {
            let names: Vec<&str> = args
                .iter()
                .map(|a| match a {
                    PreciseCapturingArg::Lifetime(lt) => lt.as_str(),
                    PreciseCapturingArg::Param(p) => p.as_str(),
                })
                .collect();
            format!("use<{}>", names.join(", "))
        }
    }
}

/// The `<T: Bound, 'a, const N: usize>` clause, or empty.
pub fn generic_params(g: &Generics) -> String {
    let parts: Vec<String> = g
        .params
        .iter()
        .filter_map(|p| {
            match &p.kind {
                // Synthetic params come from `impl Trait` in argument position
                // and are already rendered at the use site.
                GenericParamDefKind::Type {
                    is_synthetic: true, ..
                } => None,
                GenericParamDefKind::Lifetime { outlives } => Some(if outlives.is_empty() {
                    p.name.clone()
                } else {
                    format!("{}: {}", p.name, outlives.join(" + "))
                }),
                GenericParamDefKind::Type {
                    bounds, default, ..
                } => {
                    let mut s = p.name.clone();
                    if !bounds.is_empty() {
                        s.push_str(": ");
                        s.push_str(&bound_list(bounds));
                    }
                    if let Some(d) = default {
                        s.push_str(" = ");
                        s.push_str(&ty(d));
                    }
                    Some(s)
                }
                GenericParamDefKind::Const { type_, default } => {
                    let mut s = format!("const {}: {}", p.name, ty(type_));
                    if let Some(d) = default {
                        s.push_str(" = ");
                        s.push_str(d);
                    }
                    Some(s)
                }
            }
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("<{}>", parts.join(", "))
    }
}

/// The `where` clause, or empty.
pub fn where_clause(g: &Generics) -> String {
    let parts: Vec<String> = g
        .where_predicates
        .iter()
        // Rustdoc emits bound predicates with no bounds (an artifact of
        // const-trait desugaring); they would render as a bare `T:`.
        .filter(
            |w| !matches!(w, WherePredicate::BoundPredicate { bounds, .. } if bounds.is_empty()),
        )
        .map(|w| match w {
            WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => format!(
                "{}{}: {}",
                hrtb(generic_params),
                ty(type_),
                bound_list(bounds)
            ),
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                format!("{}: {}", lifetime, outlives.join(" + "))
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                format!("{} == {}", ty(lhs), term(rhs))
            }
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!(" where {}", parts.join(", "))
    }
}

/// A declared function's parameter list, rendering a leading receiver the way
/// it is written: `(&mut self, n: usize)`.
fn fn_params(sig: &FunctionSignature) -> String {
    let params = sig.inputs.iter().enumerate().map(|(i, (name, t))| {
        if i == 0 && name == "self" {
            self_param(t)
        } else {
            format!("{name}: {}", ty(t))
        }
    });
    param_list(params, sig.is_c_variadic)
}

/// A function pointer's parameter list. Pointers can name their parameters,
/// but the types read better bare: `fn(usize) -> bool`.
fn pointer_params(sig: &FunctionSignature) -> String {
    param_list(sig.inputs.iter().map(|(_, t)| ty(t)), sig.is_c_variadic)
}

fn param_list(params: impl Iterator<Item = String>, is_c_variadic: bool) -> String {
    let mut parts: Vec<String> = params.collect();
    if is_c_variadic {
        parts.push("...".to_string());
    }
    format!("({})", parts.join(", "))
}

/// Render the receiver the way rustdoc displays it.
fn self_param(t: &Type) -> String {
    match t {
        Type::Generic(g) if g == "Self" => "self".to_string(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => match type_.as_ref() {
            Type::Generic(g) if g == "Self" => format!("{}self", reference(lifetime, *is_mutable)),
            _ => format!("self: {}", ty(t)),
        },
        _ => format!("self: {}", ty(t)),
    }
}

/// The full one-line signature of an item, as shown in its declaration block.
pub fn signature(item: &Item) -> Option<String> {
    let name = item.name.as_deref().unwrap_or("_");
    match &item.inner {
        ItemEnum::Function(f) => {
            let mut s = header_prefix(
                f.header.is_unsafe,
                f.header.is_const,
                f.header.is_async,
                &f.header.abi,
            );
            s.push_str("fn ");
            s.push_str(name);
            s.push_str(&generic_params(&f.generics));
            s.push_str(&fn_params(&f.sig));
            s.push_str(&returns(&f.sig.output));
            s.push_str(&where_clause(&f.generics));
            Some(s)
        }
        ItemEnum::Struct(Struct { generics, .. }) => Some(type_decl("struct", name, generics)),
        ItemEnum::Enum(Enum { generics, .. }) => Some(type_decl("enum", name, generics)),
        ItemEnum::Union(Union { generics, .. }) => Some(type_decl("union", name, generics)),
        ItemEnum::Trait(t) => {
            let mut s = String::new();
            if t.is_unsafe {
                s.push_str("unsafe ");
            }
            s.push_str("trait ");
            s.push_str(name);
            s.push_str(&generic_params(&t.generics));
            if !t.bounds.is_empty() {
                s.push_str(": ");
                s.push_str(&bound_list(&t.bounds));
            }
            s.push_str(&where_clause(&t.generics));
            Some(s)
        }
        ItemEnum::TypeAlias(a) => Some(format!(
            "type {name}{} = {}",
            generic_params(&a.generics),
            ty(&a.type_)
        )),
        ItemEnum::Constant { type_, const_ } => {
            Some(format!("const {name}: {} = {}", ty(type_), const_.expr))
        }
        ItemEnum::Static(s) => Some(format!(
            "static {}{name}: {} = {}",
            if s.is_mutable { "mut " } else { "" },
            ty(&s.type_),
            s.expr
        )),
        ItemEnum::AssocConst { type_, .. } => Some(format!("const {name}: {}", ty(type_))),
        ItemEnum::AssocType { bounds, type_, .. } => {
            let mut s = format!("type {name}");
            if !bounds.is_empty() {
                s.push_str(": ");
                s.push_str(&bound_list(bounds));
            }
            if let Some(t) = type_ {
                s.push_str(" = ");
                s.push_str(&ty(t));
            }
            Some(s)
        }
        ItemEnum::StructField(t) => Some(format!("{name}: {}", ty(t))),
        ItemEnum::Primitive(_) => Some(format!("primitive {name}")),
        ItemEnum::Module(_) => Some(format!("mod {name}")),
        ItemEnum::Macro(def) => Some(def.clone()),
        // Built-in macros have no source to quote, so show how one is invoked.
        ItemEnum::ProcMacro(m) => Some(match m.kind {
            MacroKind::Bang => format!("{name}!() {{ /* proc-macro */ }}"),
            MacroKind::Attr => format!("#[{name}]"),
            MacroKind::Derive => format!("#[derive({name})]"),
        }),
        _ => None,
    }
}

/// A type's declaration line: `struct Vec<T, A: Allocator = Global>`. Its
/// fields or variants are listed beneath, not spelled out here.
fn type_decl(keyword: &str, name: &str, generics: &Generics) -> String {
    format!(
        "{keyword} {name}{}{}",
        generic_params(generics),
        where_clause(generics)
    )
}

/// The header line for an impl block, e.g. `impl<T> Display for Vec<T>`.
pub fn impl_header(im: &rustdoc_types::Impl) -> String {
    let mut s = String::from("impl");
    s.push_str(&generic_params(&im.generics));
    s.push(' ');
    if let Some(tr) = &im.trait_ {
        if im.is_negative {
            s.push('!');
        }
        s.push_str(&path(tr));
        s.push_str(" for ");
    }
    s.push_str(&ty(&im.for_));
    s.push_str(&where_clause(&im.generics));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustdoc_types::{Id, Path};

    fn path(name: &str, args: Option<GenericArgs>) -> Type {
        Type::ResolvedPath(Path {
            path: name.to_string(),
            id: Id(0),
            args: args.map(Box::new),
        })
    }

    #[test]
    fn formats_basic_types() {
        assert_eq!(ty(&Type::Primitive("u8".into())), "u8");
        assert_eq!(ty(&Type::Generic("T".into())), "T");
        assert_eq!(ty(&Type::Infer), "_");
        assert_eq!(
            ty(&Type::Slice(Box::new(Type::Primitive("u8".into())))),
            "[u8]"
        );
    }

    #[test]
    fn one_tuple_keeps_trailing_comma() {
        let t = Type::Tuple(vec![Type::Primitive("u8".into())]);
        assert_eq!(ty(&t), "(u8,)");
        let t = Type::Tuple(vec![]);
        assert_eq!(ty(&t), "()");
        let t = Type::Tuple(vec![
            Type::Primitive("u8".into()),
            Type::Primitive("i32".into()),
        ]);
        assert_eq!(ty(&t), "(u8, i32)");
    }

    #[test]
    fn formats_references() {
        let t = Type::BorrowedRef {
            lifetime: Some("'a".into()),
            is_mutable: true,
            type_: Box::new(Type::Primitive("str".into())),
        };
        assert_eq!(ty(&t), "&'a mut str");

        let t = Type::BorrowedRef {
            lifetime: None,
            is_mutable: false,
            type_: Box::new(Type::Primitive("str".into())),
        };
        assert_eq!(ty(&t), "&str");
    }

    #[test]
    fn formats_raw_pointers_and_arrays() {
        let t = Type::RawPointer {
            is_mutable: false,
            type_: Box::new(Type::Primitive("u8".into())),
        };
        assert_eq!(ty(&t), "*const u8");

        let t = Type::Array {
            type_: Box::new(Type::Primitive("u8".into())),
            len: "4".into(),
        };
        assert_eq!(ty(&t), "[u8; 4]");
    }

    #[test]
    fn formats_generic_paths() {
        let args = GenericArgs::AngleBracketed {
            args: vec![GenericArg::Type(Type::Primitive("u8".into()))],
            constraints: vec![],
        };
        assert_eq!(ty(&path("Vec", Some(args))), "Vec<u8>");
        // No args must not emit an empty `<>`.
        let empty = GenericArgs::AngleBracketed {
            args: vec![],
            constraints: vec![],
        };
        assert_eq!(ty(&path("Foo", Some(empty))), "Foo");
    }

    #[test]
    fn drops_empty_where_predicates() {
        // Rustdoc emits `T:` with no bounds for const-trait desugaring; that
        // must not surface as a dangling `where T:`.
        let g = Generics {
            params: vec![],
            where_predicates: vec![WherePredicate::BoundPredicate {
                type_: Type::Generic("T".into()),
                bounds: vec![],
                generic_params: vec![],
            }],
        };
        assert_eq!(where_clause(&g), "");
    }

    #[test]
    fn renders_self_receiver() {
        let sig = FunctionSignature {
            inputs: vec![(
                "self".into(),
                Type::BorrowedRef {
                    lifetime: None,
                    is_mutable: false,
                    type_: Box::new(Type::Generic("Self".into())),
                },
            )],
            output: None,
            is_c_variadic: false,
        };
        assert_eq!(fn_params(&sig), "(&self)");
    }
}
