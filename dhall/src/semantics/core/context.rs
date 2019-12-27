use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::TypeError;
use crate::semantics::core::value::Value;
use crate::semantics::core::value::ValueKind;
use crate::semantics::core::var::{AlphaVar, Binder, Shift, Subst};
use crate::syntax::{Label, V};

#[derive(Debug, Clone)]
enum CtxItem {
    Kept(AlphaVar, Value),
    Replaced(Value),
}

#[derive(Debug, Clone)]
pub(crate) struct TyCtx {
    ctx: Vec<(Binder, CtxItem)>,
    /// Keeps track of the next free binder id to assign. Shared among all the contexts to ensure
    /// unicity across the expression.
    next_uid: Rc<RefCell<u64>>,
}

#[derive(Debug, Clone)]
pub(crate) struct VarCtx<'b> {
    ctx: Vec<&'b Binder>,
}

impl TyCtx {
    pub fn new() -> Self {
        TyCtx {
            ctx: Vec::new(),
            next_uid: Rc::new(RefCell::new(0)),
        }
    }
    fn with_vec(&self, vec: Vec<(Binder, CtxItem)>) -> Self {
        TyCtx {
            ctx: vec,
            next_uid: self.next_uid.clone(),
        }
    }
    pub fn insert_type(&self, x: &Binder, t: Value) -> Self {
        let mut vec = self.ctx.clone();
        vec.push((x.clone(), CtxItem::Kept(x.into(), t.under_binder(x))));
        self.with_vec(vec)
    }
    pub fn insert_value(
        &self,
        x: &Binder,
        e: Value,
    ) -> Result<Self, TypeError> {
        let mut vec = self.ctx.clone();
        vec.push((x.clone(), CtxItem::Replaced(e)));
        Ok(self.with_vec(vec))
    }
    pub fn lookup(&self, var: &V<Label>) -> Option<Value> {
        let mut var = var.clone();
        let mut shift_map: HashMap<Label, _> = HashMap::new();
        for (b, i) in self.ctx.iter().rev() {
            let l = b.to_label();
            match var.over_binder(&l) {
                None => {
                    let i = i.under_multiple_binders(&shift_map);
                    return Some(match i {
                        CtxItem::Kept(newvar, t) => {
                            Value::from_kind_and_type(ValueKind::Var(newvar), t)
                        }
                        CtxItem::Replaced(v) => v,
                    });
                }
                Some(newvar) => var = newvar,
            };
            if let CtxItem::Kept(_, _) = i {
                *shift_map.entry(l).or_insert(0) += 1;
            }
        }
        // Unbound variable
        None
    }
    pub fn new_binder(&self, l: &Label) -> Binder {
        let mut next_uid = self.next_uid.borrow_mut();
        let uid = *next_uid;
        *next_uid += 1;
        Binder::new(l.clone(), uid)
    }

    /// Given a var that makes sense in the current context, map the given function in such a way
    /// that the passed variable always makes sense in the context of the passed item.
    /// Once we pass the variable definition, the variable doesn't make sense anymore so we just
    /// copy the remaining items.
    fn do_with_var<E>(
        &self,
        var: &AlphaVar,
        mut f: impl FnMut(&AlphaVar, &CtxItem) -> Result<CtxItem, E>,
    ) -> Result<Self, E> {
        let mut vec = Vec::new();
        vec.reserve(self.ctx.len());
        let mut var = var.clone();
        let mut iter = self.ctx.iter().rev();
        for (l, i) in iter.by_ref() {
            vec.push((l.clone(), f(&var, i)?));
            if let CtxItem::Kept(_, _) = i {
                match var.over_binder(l) {
                    None => break,
                    Some(newvar) => var = newvar,
                };
            }
        }
        for (l, i) in iter {
            vec.push((l.clone(), (*i).clone()));
        }
        vec.reverse();
        Ok(self.with_vec(vec))
    }
    fn shift(&self, delta: isize, var: &AlphaVar) -> Option<Self> {
        if delta < 0 {
            Some(self.do_with_var(var, |var, i| Ok(i.shift(delta, &var)?))?)
        } else {
            Some(
                self.with_vec(
                    self.ctx
                        .iter()
                        .map(|(l, i)| Ok((l.clone(), i.shift(delta, &var)?)))
                        .collect::<Result<_, _>>()?,
                ),
            )
        }
    }
    fn subst_shift(&self, var: &AlphaVar, val: &Value) -> Self {
        self.do_with_var(var, |var, i| Ok::<_, !>(i.subst_shift(&var, val)))
            .unwrap()
    }
}

impl<'b> VarCtx<'b> {
    pub fn new() -> Self {
        VarCtx { ctx: Vec::new() }
    }
    pub fn insert(&self, binder: &'b Binder) -> Self {
        VarCtx {
            ctx: self.ctx.iter().copied().chain(Some(binder)).collect(),
        }
    }
    pub fn lookup(&self, binder: &Binder) -> Option<usize> {
        self.ctx
            .iter()
            .rev()
            .enumerate()
            .find(|(_, other)| binder.same_binder(other))
            .map(|(i, _)| i)
    }
    pub fn lookup_by_name(&self, binder: &Binder) -> Option<usize> {
        self.ctx
            .iter()
            .rev()
            .filter(|other| binder.name() == other.name())
            .enumerate()
            .find(|(_, other)| binder.same_binder(other))
            .map(|(i, _)| i)
    }
}

impl Shift for CtxItem {
    fn shift(&self, delta: isize, var: &AlphaVar) -> Option<Self> {
        Some(match self {
            CtxItem::Kept(v, t) => {
                CtxItem::Kept(v.shift(delta, var)?, t.shift(delta, var)?)
            }
            CtxItem::Replaced(e) => CtxItem::Replaced(e.shift(delta, var)?),
        })
    }
}

impl Shift for TyCtx {
    fn shift(&self, delta: isize, var: &AlphaVar) -> Option<Self> {
        self.shift(delta, var)
    }
}

impl Subst<Value> for CtxItem {
    fn subst_shift(&self, var: &AlphaVar, val: &Value) -> Self {
        match self {
            CtxItem::Replaced(e) => CtxItem::Replaced(e.subst_shift(var, val)),
            CtxItem::Kept(v, t) => match v.shift(-1, var) {
                None => CtxItem::Replaced(val.clone()),
                Some(newvar) => CtxItem::Kept(newvar, t.subst_shift(var, val)),
            },
        }
    }
}

impl Subst<Value> for TyCtx {
    fn subst_shift(&self, var: &AlphaVar, val: &Value) -> Self {
        self.subst_shift(var, val)
    }
}
