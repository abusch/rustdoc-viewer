//! Rendering rustdoc's type and signature structures back into Rust syntax.

use rustdoc_types::{
    Abi, FunctionSignature, GenericArg, GenericArgs, GenericBound, GenericParamDef,
    GenericParamDefKind, Generics, Item, ItemEnum, PreciseCapturingArg, Term, TraitBoundModifier,
    Type, WherePredicate,
};

/// Format a type as Rust source.
pub fn ty(t: &Type) -> String {
    match t {
        Type::ResolvedPath(p) => {
            let mut s = p.path.clone();
            if let Some(args) = &p.args {
                s.push_str(&generic_args(args));
            }
            s
        }
        Type::Generic(g) => g.clone(),
        Type::Primitive(p) => p.clone(),
        Type::DynTrait(d) => {
            let mut parts: Vec<String> = d
                .traits
                .iter()
                .map(|pt| {
                    let mut s = String::new();
                    if !pt.generic_params.is_empty() {
                        s.push_str(&hrtb(&pt.generic_params));
                    }
                    s.push_str(&pt.trait_.path);
                    if let Some(a) = &pt.trait_.args {
                        s.push_str(&generic_args(a));
                    }
                    s
                })
                .collect();
            if let Some(lt) = &d.lifetime {
                parts.push(lt.clone());
            }
            format!("dyn {}", parts.join(" + "))
        }
        Type::FunctionPointer(f) => {
            let mut s = String::new();
            if !f.generic_params.is_empty() {
                s.push_str(&hrtb(&f.generic_params));
            }
            s.push_str(&header_prefix(
                f.header.is_unsafe,
                false,
                false,
                &f.header.abi,
            ));
            s.push_str("fn");
            s.push_str(&fn_params(&f.sig, false));
            if let Some(out) = &f.sig.output {
                s.push_str(" -> ");
                s.push_str(&ty(out));
            }
            s
        }
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
        } => {
            let mut s = String::from("&");
            if let Some(lt) = lifetime {
                s.push_str(lt);
                s.push(' ');
            }
            if *is_mutable {
                s.push_str("mut ");
            }
            s.push_str(&ty(type_));
            s
        }
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

/// `for<'a, 'b> ` prefix used by HRTBs.
fn hrtb(params: &[GenericParamDef]) -> String {
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    format!("for<{}> ", names.join(", "))
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
            let mut s = format!("({})", ins.join(", "));
            if let Some(o) = output {
                s.push_str(" -> ");
                s.push_str(&ty(o));
            }
            s
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
            let mut s = String::new();
            if !generic_params.is_empty() {
                s.push_str(&hrtb(generic_params));
            }
            match modifier {
                TraitBoundModifier::None => {}
                TraitBoundModifier::Maybe => s.push('?'),
                TraitBoundModifier::MaybeConst => s.push_str("~const "),
            }
            s.push_str(&trait_.path);
            if let Some(a) = &trait_.args {
                s.push_str(&generic_args(a));
            }
            s
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
            } => {
                let mut s = String::new();
                if !generic_params.is_empty() {
                    s.push_str(&hrtb(generic_params));
                }
                s.push_str(&ty(type_));
                s.push_str(": ");
                s.push_str(&bound_list(bounds));
                s
            }
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

/// Parameter list, rendering a leading receiver as `&self` / `self` etc.
fn fn_params(sig: &FunctionSignature, allow_self: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, (name, t)) in sig.inputs.iter().enumerate() {
        if allow_self && i == 0 && name == "self" {
            parts.push(self_param(t));
            continue;
        }
        if allow_self {
            parts.push(format!("{name}: {}", ty(t)));
        } else {
            // Function pointers name their params, but the type reads better bare.
            parts.push(ty(t));
        }
    }
    if sig.is_c_variadic {
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
            Type::Generic(g) if g == "Self" => {
                let mut s = String::from("&");
                if let Some(lt) = lifetime {
                    s.push_str(lt);
                    s.push(' ');
                }
                if *is_mutable {
                    s.push_str("mut ");
                }
                s.push_str("self");
                s
            }
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
            s.push_str(&fn_params(&f.sig, true));
            if let Some(out) = &f.sig.output {
                s.push_str(" -> ");
                s.push_str(&ty(out));
            }
            s.push_str(&where_clause(&f.generics));
            Some(s)
        }
        ItemEnum::Struct(st) => {
            let mut s = format!("struct {name}{}", generic_params(&st.generics));
            s.push_str(&where_clause(&st.generics));
            Some(s)
        }
        ItemEnum::Enum(e) => {
            let mut s = format!("enum {name}{}", generic_params(&e.generics));
            s.push_str(&where_clause(&e.generics));
            Some(s)
        }
        ItemEnum::Union(u) => Some(format!(
            "union {name}{}{}",
            generic_params(&u.generics),
            where_clause(&u.generics)
        )),
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
        _ => None,
    }
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
        s.push_str(&tr.path);
        if let Some(a) = &tr.args {
            s.push_str(&generic_args(a));
        }
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
        use rustdoc_types::{GenericParamDef, GenericParamDefKind};
        let _ = (GenericParamDef {
            name: "T".into(),
            kind: GenericParamDefKind::Lifetime { outlives: vec![] },
        },);
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
        assert_eq!(fn_params(&sig, true), "(&self)");
    }
}
