extern crate proc_macro;
use dhall_core::context::Context;
use dhall_core::*;
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::BTreeMap;

pub fn expr(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input_str = input.to_string();
    let expr: SubExpr<_, Import> = parse_expr(&input_str).unwrap().unnote();
    let no_import =
        |_: &Import| -> X { panic!("Don't use import in dhall::expr!()") };
    let expr = expr.map_embed(no_import);
    let output = quote_expr(&expr.unroll(), &Context::new());
    output.into()
}

pub fn subexpr(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input_str = input.to_string();
    let expr: SubExpr<_, Import> = parse_expr(&input_str).unwrap().unnote();
    let no_import =
        |_: &Import| -> X { panic!("Don't use import in dhall::subexpr!()") };
    let expr = expr.map_embed(no_import);
    let output = quote_subexpr(&expr, &Context::new());
    output.into()
}

// Returns an expression of type ExprF<T, _, _>, where T is the
// type of the subexpressions after interpolation.
pub fn quote_exprf<TS>(expr: ExprF<TS, Label, X, X>) -> TokenStream
where
    TS: quote::ToTokens + std::fmt::Debug,
{
    use dhall_core::ExprF::*;
    match expr {
        Var(_) => unreachable!(),
        Pi(x, t, b) => {
            let x = quote_label(&x);
            quote! { dhall_core::ExprF::Pi(#x, #t, #b) }
        }
        Lam(x, t, b) => {
            let x = quote_label(&x);
            quote! { dhall_core::ExprF::Lam(#x, #t, #b) }
        }
        App(f, a) => {
            let a = quote_vec(a);
            quote! { dhall_core::ExprF::App(#f, #a) }
        }
        Annot(x, t) => {
            quote! { dhall_core::ExprF::Annot(#x, #t) }
        }
        Const(c) => {
            let c = quote_const(c);
            quote! { dhall_core::ExprF::Const(#c) }
        }
        Builtin(b) => {
            let b = quote_builtin(b);
            quote! { dhall_core::ExprF::Builtin(#b) }
        }
        BinOp(o, a, b) => {
            let o = quote_binop(o);
            quote! { dhall_core::ExprF::BinOp(#o, #a, #b) }
        }
        NaturalLit(n) => {
            quote! { dhall_core::ExprF::NaturalLit(#n) }
        }
        BoolLit(b) => {
            quote! { dhall_core::ExprF::BoolLit(#b) }
        }
        EmptyOptionalLit(x) => {
            quote! { dhall_core::ExprF::EmptyOptionalLit(#x) }
        }
        NEOptionalLit(x) => {
            quote! { dhall_core::ExprF::NEOptionalLit(#x) }
        }
        EmptyListLit(t) => {
            quote! { dhall_core::ExprF::EmptyListLit(#t) }
        }
        NEListLit(es) => {
            let es = quote_vec(es);
            quote! { dhall_core::ExprF::NEListLit(#es) }
        }
        RecordType(m) => {
            let m = quote_map(m);
            quote! { dhall_core::ExprF::RecordType(#m) }
        }
        RecordLit(m) => {
            let m = quote_map(m);
            quote! { dhall_core::ExprF::RecordLit(#m) }
        }
        UnionType(m) => {
            let m = quote_opt_map(m);
            quote! { dhall_core::ExprF::UnionType(#m) }
        }
        e => unimplemented!("{:?}", e),
    }
}

// Returns an expression of type SubExpr<_, _>. Expects interpolated variables
// to be of type SubExpr<_, _>.
fn quote_subexpr(
    expr: &SubExpr<X, X>,
    ctx: &Context<Label, ()>,
) -> TokenStream {
    use dhall_core::ExprF::*;
    match expr.as_ref().map_ref_with_special_handling_of_binders(
        |e| quote_subexpr(e, ctx),
        |l, e| quote_subexpr(e, &ctx.insert(l.clone(), ())),
        |_| unreachable!(),
        |_| unreachable!(),
        Label::clone,
    ) {
        Var(V(ref s, n)) => {
            match ctx.lookup(s, n) {
                // Non-free variable; interpolates as itself
                Some(()) => {
                    let s: String = s.into();
                    let var = quote! { dhall_core::V(#s.into(), #n) };
                    rc(quote! { dhall_core::ExprF::Var(#var) })
                }
                // Free variable; interpolates as a rust variable
                None => {
                    let s: String = s.into();
                    // TODO: insert appropriate shifts ?
                    let v: TokenStream = s.parse().unwrap();
                    quote! { {
                        let x: dhall_core::SubExpr<_, _> = #v.clone();
                        x
                    } }
                }
            }
        }
        e => rc(quote_exprf(e)),
    }
}

// Returns an expression of type Expr<_, _>. Expects interpolated variables
// to be of type SubExpr<_, _>.
fn quote_expr(expr: &Expr<X, X>, ctx: &Context<Label, ()>) -> TokenStream {
    use dhall_core::ExprF::*;
    match expr.map_ref_with_special_handling_of_binders(
        |e| quote_subexpr(e, ctx),
        |l, e| quote_subexpr(e, &ctx.insert(l.clone(), ())),
        |_| unreachable!(),
        |_| unreachable!(),
        Label::clone,
    ) {
        Var(V(ref s, n)) => {
            match ctx.lookup(s, n) {
                // Non-free variable; interpolates as itself
                Some(()) => {
                    let s: String = s.into();
                    let var = quote! { dhall_core::V(#s.into(), #n) };
                    quote! { dhall_core::ExprF::Var(#var) }
                }
                // Free variable; interpolates as a rust variable
                None => {
                    let s: String = s.into();
                    // TODO: insert appropriate shifts ?
                    let v: TokenStream = s.parse().unwrap();
                    quote! { {
                        let x: dhall_core::SubExpr<_, _> = #v.clone();
                        x.unroll()
                    } }
                }
            }
        }
        e => quote_exprf(e),
    }
}

fn quote_builtin(b: Builtin) -> TokenStream {
    format!("dhall_core::Builtin::{:?}", b).parse().unwrap()
}

fn quote_const(c: Const) -> TokenStream {
    format!("dhall_core::Const::{:?}", c).parse().unwrap()
}

fn quote_binop(b: BinOp) -> TokenStream {
    format!("dhall_core::BinOp::{:?}", b).parse().unwrap()
}

fn quote_label(l: &Label) -> TokenStream {
    let l = String::from(l);
    quote! { dhall_core::Label::from(#l) }
}

fn rc(x: TokenStream) -> TokenStream {
    quote! { dhall_core::rc(#x) }
}

fn quote_opt<TS>(x: Option<TS>) -> TokenStream
where
    TS: quote::ToTokens + std::fmt::Debug,
{
    match x {
        Some(x) => quote!(Some(#x)),
        None => quote!(None),
    }
}

fn quote_vec<TS>(e: Vec<TS>) -> TokenStream
where
    TS: quote::ToTokens + std::fmt::Debug,
{
    quote! { vec![ #(#e),* ] }
}

fn quote_map<TS>(m: BTreeMap<Label, TS>) -> TokenStream
where
    TS: quote::ToTokens + std::fmt::Debug,
{
    let entries = m.into_iter().map(|(k, v)| {
        let k = quote_label(&k);
        quote!(m.insert(#k, #v);)
    });
    quote! { {
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        #( #entries )*
        m
    } }
}

fn quote_opt_map<TS>(m: BTreeMap<Label, Option<TS>>) -> TokenStream
where
    TS: quote::ToTokens + std::fmt::Debug,
{
    quote_map(m.into_iter().map(|(k, v)| (k, quote_opt(v))).collect())
}
