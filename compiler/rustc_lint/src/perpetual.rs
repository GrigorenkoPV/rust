use rustc_ast::Mutability;
use rustc_hir::{self as hir, AmbigArg, MutTy, TyKind};
use rustc_middle::ty::layout::HasTyCtxt;
use rustc_session::{declare_lint, declare_lint_pass};

use crate::{LateContext, LateLintPass, LintContext};

declare_lint! {
    /// TODO
    pub PERPETUAL_INVARIANT_MUT_BORROW,
    Deny,
    "TODO",
}

declare_lint_pass!(PerpetualInvariantMutBorrow => [PERPETUAL_INVARIANT_MUT_BORROW]);

impl<'tcx> LateLintPass<'tcx> for PerpetualInvariantMutBorrow {
    fn check_ty(&mut self, cx: &LateContext<'tcx>, ty: &'tcx hir::Ty<'tcx, AmbigArg>) {
        if let hir::Ty { span, kind: TyKind::Ref(lt, MutTy { ty, mutbl: Mutability::Mut }), .. } =
            ty
            && let Some(typeck_results) = cx.maybe_typeck_results()
            && let Some(def_id) = dbg!(typeck_results.type_dependent_def_id(ty.hir_id))
            && let variances = cx.tcx().variances_of(def_id)
        {
            dbg!(variances);
        }
    }
}
