#![allow(non_snake_case)]
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use crate::normalize::normalize;
use dhall_core;
use dhall_core::context::Context;
use dhall_core::*;
use dhall_generator::dhall_expr;

use self::TypeMessage::*;

fn axiom<S>(c: Const) -> Result<Const, TypeError<S>> {
    use dhall_core::Const::*;
    use dhall_core::Expr::*;
    match c {
        Type => Ok(Kind),
        Kind => Err(TypeError::new(&Context::new(), rc(Const(Kind)), Untyped)),
    }
}

fn rule(a: Const, b: Const) -> Result<Const, ()> {
    use dhall_core::Const::*;
    match (a, b) {
        (Type, Kind) => Err(()),
        (Kind, Kind) => Ok(Kind),
        (Type, Type) | (Kind, Type) => Ok(Type),
    }
}

fn match_vars(vl: &V, vr: &V, ctx: &[(Label, Label)]) -> bool {
    let mut vl = vl.clone();
    let mut vr = vr.clone();
    let mut ctx = ctx.to_vec();
    ctx.reverse();
    while let Some((xL2, xR2)) = &ctx.pop() {
        match (&vl, &vr) {
            (V(xL, 0), V(xR, 0)) if xL == xL2 && xR == xR2 => return true,
            (V(xL, nL), V(xR, nR)) => {
                let nL2 = if xL == xL2 { nL - 1 } else { *nL };
                let nR2 = if xR == xR2 { nR - 1 } else { *nR };
                vl = V(xL.clone(), nL2);
                vr = V(xR.clone(), nR2);
            }
        }
    }
    vl == vr
}

// Takes normalized expressions as input
fn prop_equal<S, T>(eL0: &Expr<S, X>, eR0: &Expr<T, X>) -> bool
where
    S: ::std::fmt::Debug,
    T: ::std::fmt::Debug,
{
    use dhall_core::Expr::*;
    fn go<S, T>(
        ctx: &mut Vec<(Label, Label)>,
        el: &Expr<S, X>,
        er: &Expr<T, X>,
    ) -> bool
    where
        S: ::std::fmt::Debug,
        T: ::std::fmt::Debug,
    {
        match (el, er) {
            (&Const(a), &Const(b)) => a == b,
            (&Builtin(a), &Builtin(b)) => a == b,
            (&Var(ref vL), &Var(ref vR)) => match_vars(vL, vR, ctx),
            (&Pi(ref xL, ref tL, ref bL), &Pi(ref xR, ref tR, ref bR)) => {
                //ctx <- State.get
                let eq1 = go(ctx, tL.as_ref(), tR.as_ref());
                if eq1 {
                    //State.put ((xL, xR):ctx)
                    ctx.push((xL.clone(), xR.clone()));
                    let eq2 = go(ctx, bL.as_ref(), bR.as_ref());
                    //State.put ctx
                    let _ = ctx.pop();
                    eq2
                } else {
                    false
                }
            }
            (&App(ref fL, ref aL), &App(ref fR, ref aR)) => {
                go(ctx, fL.as_ref(), fR.as_ref())
                    && aL.len() == aR.len()
                    && aL
                        .iter()
                        .zip(aR.iter())
                        .all(|(aL, aR)| go(ctx, aL.as_ref(), aR.as_ref()))
            }
            (&RecordType(ref ktsL0), &RecordType(ref ktsR0)) => {
                ktsL0.len() == ktsR0.len()
                    && ktsL0.iter().zip(ktsR0.iter()).all(
                        |((kL, tL), (kR, tR))| {
                            kL == kR && go(ctx, tL.as_ref(), tR.as_ref())
                        },
                    )
            }
            (&UnionType(ref ktsL0), &UnionType(ref ktsR0)) => {
                ktsL0.len() == ktsR0.len()
                    && ktsL0.iter().zip(ktsR0.iter()).all(
                        |((kL, tL), (kR, tR))| {
                            kL == kR && go(ctx, tL.as_ref(), tR.as_ref())
                        },
                    )
            }
            (_, _) => false,
        }
    }
    let mut ctx = vec![];
    go::<S, T>(&mut ctx, eL0, eR0)
}

fn type_of_builtin<S>(b: Builtin) -> Rc<Expr<S, X>> {
    use dhall_core::Builtin::*;
    match b {
        Bool | Natural | Integer | Double | Text => dhall_expr!(Type),
        List | Optional => dhall_expr!(
            Type -> Type
        ),
        NaturalFold => dhall_expr!(
            Natural ->
            forall (natural: Type) ->
            forall (succ: natural -> natural) ->
            forall (zero: natural) ->
            natural
        ),
        NaturalBuild => dhall_expr!(
            (forall (natural: Type) ->
                forall (succ: natural -> natural) ->
                forall (zero: natural) ->
                natural) ->
            Natural
        ),
        NaturalIsZero | NaturalEven | NaturalOdd => dhall_expr!(
            Natural -> Bool
        ),
        ListBuild => dhall_expr!(
            forall (a: Type) ->
            (forall (list: Type) ->
                forall (cons: a -> list -> list) ->
                forall (nil: list) ->
                list) ->
            List a
        ),
        ListFold => dhall_expr!(
            forall (a: Type) ->
            List a ->
            forall (list: Type) ->
            forall (cons: a -> list -> list) ->
            forall (nil: list) ->
            list
        ),
        ListLength => dhall_expr!(forall (a: Type) -> List a -> Natural),
        ListHead | ListLast => {
            dhall_expr!(forall (a: Type) -> List a -> Optional a)
        }
        ListIndexed => dhall_expr!(
            forall (a: Type) ->
            List a ->
            List { index: Natural, value: a }
        ),
        ListReverse => dhall_expr!(
            forall (a: Type) -> List a -> List a
        ),
        OptionalFold => dhall_expr!(
            forall (a: Type) ->
            Optional a ->
            forall (optional: Type) ->
            forall (just: a -> optional) ->
            forall (nothing: optional) ->
            optional
        ),
        _ => panic!("Unimplemented typecheck case: {:?}", b),
    }
}

/// Type-check an expression and return the expression'i type if type-checking
/// suceeds or an error if type-checking fails
///
/// `type_with` does not necessarily normalize the type since full normalization
/// is not necessary for just type-checking.  If you actually care about the
/// returned type then you may want to `normalize` it afterwards.
pub fn type_with<S>(
    ctx: &Context<Label, Rc<Expr<S, X>>>,
    e: Rc<Expr<S, X>>,
) -> Result<Rc<Expr<S, X>>, TypeError<S>>
where
    S: ::std::fmt::Debug,
{
    use dhall_core::BinOp::*;
    use dhall_core::Builtin::*;
    use dhall_core::Const::*;
    use dhall_core::Expr;
    use dhall_core::Expr::*;
    let mkerr = |msg: TypeMessage<_>| TypeError::new(ctx, e.clone(), msg);
    let ensure_const = |x: &SubExpr<_, _>, msg: TypeMessage<_>| match x.as_ref()
    {
        Const(k) => Ok(*k),
        _ => Err(mkerr(msg)),
    };
    let ensure_is_type =
        |x: SubExpr<_, _>, msg: TypeMessage<_>| match x.as_ref() {
            Const(Type) => Ok(()),
            _ => Err(mkerr(msg)),
        };

    match e.as_ref() {
        Const(c) => axiom(*c).map(Const),
        Var(V(x, n)) => {
            return ctx
                .lookup(x, *n)
                .cloned()
                .ok_or_else(|| mkerr(UnboundVariable))
        }
        Lam(x, tA, b) => {
            let ctx2 = ctx
                .insert(x.clone(), tA.clone())
                .map(|e| shift(1, &V(x.clone(), 0), e));
            let tB = type_with(&ctx2, b.clone())?;
            let p = rc(Pi(x.clone(), tA.clone(), tB));
            let _ = type_with(ctx, p.clone())?;
            return Ok(p);
        }
        Pi(x, tA, tB) => {
            let tA2 = normalized_type_with(ctx, tA.clone())?;
            let kA = ensure_const(&tA2, InvalidInputType(tA.clone()))?;

            let ctx2 = ctx
                .insert(x.clone(), tA.clone())
                .map(|e| shift(1, &V(x.clone(), 0), e));
            let tB = normalized_type_with(&ctx2, tB.clone())?;
            let kB = match tB.as_ref() {
                Const(k) => *k,
                _ => {
                    return Err(TypeError::new(
                        &ctx2,
                        e.clone(),
                        InvalidOutputType(tB),
                    ));
                }
            };

            match rule(kA, kB) {
                Err(()) => Err(mkerr(NoDependentTypes(tA.clone(), tB))),
                Ok(_) => Ok(Const(kB)),
            }
        }
        App(f, args) => {
            // Recurse on args
            let (a, tf) = match args.split_last() {
                None => return type_with(ctx, f.clone()),
                Some((a, args)) => (
                    a,
                    normalized_type_with(
                        ctx,
                        rc(App(f.clone(), args.to_vec())),
                    )?,
                ),
            };
            let (x, tA, tB) = match tf.as_ref() {
                Pi(x, tA, tB) => (x, tA, tB),
                _ => {
                    return Err(mkerr(NotAFunction(f.clone(), tf)));
                }
            };
            let tA = normalize(Rc::clone(tA));
            let tA2 = normalized_type_with(ctx, a.clone())?;
            if prop_equal(tA.as_ref(), tA2.as_ref()) {
                let vx0 = &V(x.clone(), 0);
                let a2 = shift(1, vx0, a);
                let tB2 = subst(vx0, &a2, &tB);
                let tB3 = shift(-1, vx0, &tB2);
                return Ok(tB3);
            } else {
                Err(mkerr(TypeMismatch(f.clone(), tA, a.clone(), tA2)))
            }
        }
        Let(f, mt, r, b) => {
            let r = if let Some(t) = mt {
                rc(Annot(Rc::clone(r), Rc::clone(t)))
            } else {
                Rc::clone(r)
            };

            let tR = type_with(ctx, r)?;
            let ttR = normalized_type_with(ctx, tR.clone())?;
            // Don't bother to provide a `let`-specific version of this error
            // message because this should never happen anyway
            let kR = ensure_const(&ttR, InvalidInputType(tR.clone()))?;

            let ctx2 = ctx.insert(f.clone(), tR.clone());
            let tB = type_with(&ctx2, b.clone())?;
            let ttB = normalized_type_with(ctx, tB.clone())?;
            // Don't bother to provide a `let`-specific version of this error
            // message because this should never happen anyway
            let kB = ensure_const(&ttB, InvalidOutputType(tB.clone()))?;

            if let Err(()) = rule(kR, kB) {
                return Err(mkerr(NoDependentLet(tR, tB)));
            }

            return Ok(tB);
        }
        Annot(x, t) => {
            // This is mainly just to check that `t` is not `Kind`
            let _ = type_with(ctx, t.clone())?;

            let t2 = normalized_type_with(ctx, x.clone())?;
            let t = normalize(t.clone());
            if prop_equal(t.as_ref(), t2.as_ref()) {
                return Ok(t.clone());
            } else {
                Err(mkerr(AnnotMismatch(x.clone(), t, t2)))
            }
        }
        BoolIf(x, y, z) => {
            let tx = normalized_type_with(ctx, x.clone())?;
            match tx.as_ref() {
                Builtin(Bool) => {}
                _ => {
                    return Err(mkerr(InvalidPredicate(x.clone(), tx)));
                }
            }
            let ty = normalized_type_with(ctx, y.clone())?;
            let tty = normalized_type_with(ctx, ty.clone())?;
            ensure_is_type(
                tty.clone(),
                IfBranchMustBeTerm(true, y.clone(), ty.clone(), tty.clone()),
            )?;

            let tz = normalized_type_with(ctx, z.clone())?;
            let ttz = normalized_type_with(ctx, tz.clone())?;
            ensure_is_type(
                ttz.clone(),
                IfBranchMustBeTerm(false, z.clone(), tz.clone(), ttz.clone()),
            )?;

            if !prop_equal(ty.as_ref(), tz.as_ref()) {
                return Err(mkerr(IfBranchMismatch(
                    y.clone(),
                    z.clone(),
                    ty,
                    tz,
                )));
            }
            return Ok(ty);
        }
        EmptyListLit(t) => {
            let s = normalized_type_with(ctx, t.clone())?;
            ensure_is_type(s, InvalidListType(t.clone()))?;
            let t = normalize(Rc::clone(t));
            return Ok(dhall_expr!(List t));
        }
        NEListLit(xs) => {
            let mut iter = xs.iter().enumerate();
            let (_, first_x) = iter.next().unwrap();
            let t = type_with(ctx, first_x.clone())?;

            let s = normalized_type_with(ctx, t.clone())?;
            ensure_is_type(s, InvalidListType(t.clone()))?;
            let t = normalize(t);
            for (i, x) in iter {
                let t2 = normalized_type_with(ctx, x.clone())?;
                if !prop_equal(t.as_ref(), t2.as_ref()) {
                    return Err(mkerr(InvalidListElement(i, t, x.clone(), t2)));
                }
            }
            return Ok(dhall_expr!(List t));
        }
        EmptyOptionalLit(t) => {
            let s = normalized_type_with(ctx, t.clone())?;
            ensure_is_type(s, InvalidOptionalType(t.clone()))?;
            let t = normalize(t.clone());
            return Ok(dhall_expr!(Optional t));
        }
        NEOptionalLit(x) => {
            let t: Rc<Expr<_, _>> = type_with(ctx, x.clone())?;
            let s = normalized_type_with(ctx, t.clone())?;
            ensure_is_type(s, InvalidOptionalType(t.clone()))?;
            let t = normalize(t);
            return Ok(dhall_expr!(Optional t));
        }
        RecordType(kts) => {
            for (k, t) in kts {
                let s = normalized_type_with(ctx, t.clone())?;
                ensure_is_type(s, InvalidFieldType(k.clone(), t.clone()))?;
            }
            Ok(Const(Type))
        }
        RecordLit(kvs) => {
            let kts = kvs
                .iter()
                .map(|(k, v)| {
                    let t = type_with(ctx, v.clone())?;
                    let s = normalized_type_with(ctx, t.clone())?;
                    ensure_is_type(s, InvalidField(k.clone(), v.clone()))?;
                    Ok((k.clone(), t))
                })
                .collect::<Result<_, _>>()?;
            Ok(RecordType(kts))
        }
        Field(r, x) => {
            let t = normalized_type_with(ctx, r.clone())?;
            match t.as_ref() {
                RecordType(kts) => {
                    return kts.get(x).cloned().ok_or_else(|| {
                        mkerr(MissingField(x.clone(), t.clone()))
                    })
                }
                _ => Err(mkerr(NotARecord(x.clone(), r.clone(), t.clone()))),
            }
        }
        Builtin(b) => return Ok(type_of_builtin(*b)),
        BoolLit(_) => Ok(Builtin(Bool)),
        NaturalLit(_) => Ok(Builtin(Natural)),
        IntegerLit(_) => Ok(Builtin(Integer)),
        DoubleLit(_) => Ok(Builtin(Double)),
        TextLit(_) => Ok(Builtin(Text)),
        BinOp(o, l, r) => {
            let t = match o {
                BoolAnd => Bool,
                BoolOr => Bool,
                BoolEQ => Bool,
                BoolNE => Bool,
                NaturalPlus => Natural,
                NaturalTimes => Natural,
                TextAppend => Text,
                _ => panic!("Unimplemented typecheck case: {:?}", e),
            };
            let tl = normalized_type_with(ctx, l.clone())?;
            match tl.as_ref() {
                Builtin(lt) if *lt == t => {}
                _ => return Err(mkerr(BinOpTypeMismatch(*o, l.clone(), tl))),
            }

            let tr = normalized_type_with(ctx, r.clone())?;
            match tr.as_ref() {
                Builtin(rt) if *rt == t => {}
                _ => return Err(mkerr(BinOpTypeMismatch(*o, r.clone(), tr))),
            }

            Ok(Builtin(t))
        }
        Embed(p) => match *p {},
        _ => panic!("Unimplemented typecheck case: {:?}", e),
    }
    .map(rc)
}

pub fn normalized_type_with<S>(
    ctx: &Context<Label, Rc<Expr<S, X>>>,
    e: Rc<Expr<S, X>>,
) -> Result<Rc<Expr<S, X>>, TypeError<S>>
where
    S: ::std::fmt::Debug,
{
    Ok(normalize(type_with(ctx, e)?))
}

/// `typeOf` is the same as `type_with` with an empty context, meaning that the
/// expression must be closed (i.e. no free variables), otherwise type-checking
/// will fail.
pub fn type_of<S: ::std::fmt::Debug>(
    e: Rc<Expr<S, X>>,
) -> Result<Rc<Expr<S, X>>, TypeError<S>> {
    let ctx = Context::new();
    type_with(&ctx, e) //.map(|e| e.into_owned())
}

/// The specific type error
#[derive(Debug)]
pub enum TypeMessage<S> {
    UnboundVariable,
    InvalidInputType(Rc<Expr<S, X>>),
    InvalidOutputType(Rc<Expr<S, X>>),
    NotAFunction(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    TypeMismatch(
        Rc<Expr<S, X>>,
        Rc<Expr<S, X>>,
        Rc<Expr<S, X>>,
        Rc<Expr<S, X>>,
    ),
    AnnotMismatch(Rc<Expr<S, X>>, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    Untyped,
    InvalidListElement(usize, Rc<Expr<S, X>>, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    InvalidListType(Rc<Expr<S, X>>),
    InvalidOptionalElement(Rc<Expr<S, X>>, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    InvalidOptionalLiteral(usize),
    InvalidOptionalType(Rc<Expr<S, X>>),
    InvalidPredicate(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    IfBranchMismatch(
        Rc<Expr<S, X>>,
        Rc<Expr<S, X>>,
        Rc<Expr<S, X>>,
        Rc<Expr<S, X>>,
    ),
    IfBranchMustBeTerm(bool, Rc<Expr<S, X>>, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    InvalidField(Label, Rc<Expr<S, X>>),
    InvalidFieldType(Label, Rc<Expr<S, X>>),
    InvalidAlternative(Label, Rc<Expr<S, X>>),
    InvalidAlternativeType(Label, Rc<Expr<S, X>>),
    DuplicateAlternative(Label),
    MustCombineARecord(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    FieldCollision(Label),
    MustMergeARecord(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    MustMergeUnion(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    UnusedHandler(HashSet<Label>),
    MissingHandler(HashSet<Label>),
    HandlerInputTypeMismatch(Label, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    HandlerOutputTypeMismatch(Label, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    HandlerNotAFunction(Label, Rc<Expr<S, X>>),
    NotARecord(Label, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    MissingField(Label, Rc<Expr<S, X>>),
    BinOpTypeMismatch(BinOp, Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    NoDependentLet(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
    NoDependentTypes(Rc<Expr<S, X>>, Rc<Expr<S, X>>),
}

/// A structured type error that includes context
#[derive(Debug)]
pub struct TypeError<S> {
    pub context: Context<Label, Rc<Expr<S, X>>>,
    pub current: Rc<Expr<S, X>>,
    pub type_message: TypeMessage<S>,
}

impl<S> TypeError<S> {
    pub fn new(
        context: &Context<Label, Rc<Expr<S, X>>>,
        current: Rc<Expr<S, X>>,
        type_message: TypeMessage<S>,
    ) -> Self {
        TypeError {
            context: context.clone(),
            current: current,
            type_message,
        }
    }
}

impl<S: fmt::Debug> ::std::error::Error for TypeMessage<S> {
    fn description(&self) -> &str {
        match *self {
            UnboundVariable => "Unbound variable",
            InvalidInputType(_) => "Invalid function input",
            InvalidOutputType(_) => "Invalid function output",
            NotAFunction(_, _) => "Not a function",
            TypeMismatch(_, _, _, _) => "Wrong type of function argument",
            _ => "Unhandled error",
        }
    }
}

impl<S> fmt::Display for TypeMessage<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            UnboundVariable => {
                f.write_str(include_str!("errors/UnboundVariable.txt"))
            }
            TypeMismatch(e0, e1, e2, e3) => {
                let template = include_str!("errors/TypeMismatch.txt");
                let s = template
                    .replace("$txt0", &format!("{}", e0))
                    .replace("$txt1", &format!("{}", e1))
                    .replace("$txt2", &format!("{}", e2))
                    .replace("$txt3", &format!("{}", e3));
                f.write_str(&s)
            }
            _ => f.write_str("Unhandled error message"),
        }
    }
}
