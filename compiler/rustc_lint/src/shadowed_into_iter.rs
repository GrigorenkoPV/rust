use rustc_hir::{self as hir, HirId, LangItem};
use rustc_middle::ty::{self, Ty};
use rustc_session::lint::FutureIncompatibilityReason;
use rustc_session::{declare_lint, impl_lint_pass};
use rustc_span::edition::Edition;

use crate::lints::{ArrayAsRefDiag, ShadowedIntoIterDiag, ShadowedIntoIterDiagSub};
use crate::{LateContext, LateLintPass, LintContext};

declare_lint! {
    /// The `array_into_iter` lint detects calling `into_iter` on arrays.
    ///
    /// ### Example
    ///
    /// ```rust,edition2018
    /// # #![allow(unused)]
    /// [1, 2, 3].into_iter().for_each(|n| { *n; });
    /// ```
    ///
    /// {{produces}}
    ///
    /// ### Explanation
    ///
    /// Since Rust 1.53, arrays implement `IntoIterator`. However, to avoid
    /// breakage, `array.into_iter()` in Rust 2015 and 2018 code will still
    /// behave as `(&array).into_iter()`, returning an iterator over
    /// references, just like in Rust 1.52 and earlier.
    /// This only applies to the method call syntax `array.into_iter()`, not to
    /// any other syntax such as `for _ in array` or `IntoIterator::into_iter(array)`.
    pub ARRAY_INTO_ITER,
    Warn,
    "detects calling `into_iter` on arrays in Rust 2015 and 2018",
    @future_incompatible = FutureIncompatibleInfo {
        reason: FutureIncompatibilityReason::EditionSemanticsChange(Edition::Edition2021),
        reference: "<https://doc.rust-lang.org/nightly/edition-guide/rust-2021/IntoIterator-for-arrays.html>",
    };
}

declare_lint! {
    /// The `boxed_slice_into_iter` lint detects calling `into_iter` on boxed slices.
    ///
    /// ### Example
    ///
    /// ```rust,edition2021
    /// # #![allow(unused)]
    /// vec![1, 2, 3].into_boxed_slice().into_iter().for_each(|n| { *n; });
    /// ```
    ///
    /// {{produces}}
    ///
    /// ### Explanation
    ///
    /// Since Rust 1.80.0, boxed slices implement `IntoIterator`. However, to avoid
    /// breakage, `boxed_slice.into_iter()` in Rust 2015, 2018, and 2021 code will still
    /// behave as `(&boxed_slice).into_iter()`, returning an iterator over
    /// references, just like in Rust 1.79.0 and earlier.
    /// This only applies to the method call syntax `boxed_slice.into_iter()`, not to
    /// any other syntax such as `for _ in boxed_slice` or `IntoIterator::into_iter(boxed_slice)`.
    pub BOXED_SLICE_INTO_ITER,
    Warn,
    "detects calling `into_iter` on boxed slices in Rust 2015, 2018, and 2021",
    @future_incompatible = FutureIncompatibleInfo {
        reason: FutureIncompatibilityReason::EditionSemanticsChange(Edition::Edition2024),
        reference: "<https://doc.rust-lang.org/nightly/edition-guide/rust-2024/intoiterator-box-slice.html>"
    };
}

declare_lint! {
    /// TODO
    pub ARRAY_AS_REF,
    Warn,
    "TODO"
}

#[derive(Copy, Clone)]
pub(crate) struct ShadowedIntoIter;

impl_lint_pass!(ShadowedIntoIter => [ARRAY_INTO_ITER, BOXED_SLICE_INTO_ITER, ARRAY_AS_REF]);

fn is_ref_to_array(ty: Ty<'_>) -> bool {
    if let ty::Ref(_, pointee_ty, _) = *ty.kind() { pointee_ty.is_array() } else { false }
}

fn find_array_index(adjusted_receiver_tys: &[Ty<'_>]) -> Option<usize> {
    if is_ref_to_array(*adjusted_receiver_tys.last().unwrap()) {
        adjusted_receiver_tys
            .iter()
            .copied()
            .take_while(|ty| !is_ref_to_array(*ty))
            .position(|ty| ty.is_array())
    } else {
        None
    }
}

fn is_ref_to_boxed_slice(ty: Ty<'_>) -> bool {
    if let ty::Ref(_, pointee_ty, _) = *ty.kind() {
        pointee_ty.boxed_ty().is_some_and(Ty::is_slice)
    } else {
        false
    }
}

fn find_boxed_slice_index(adjusted_receiver_tys: &[Ty<'_>]) -> Option<usize> {
    if is_ref_to_boxed_slice(*adjusted_receiver_tys.last().unwrap()) {
        adjusted_receiver_tys
            .iter()
            .copied()
            .take_while(|ty| !is_ref_to_boxed_slice(*ty))
            .position(|ty| ty.boxed_ty().is_some_and(Ty::is_slice))
    } else {
        None
    }
}

#[derive(Clone, Copy)]
enum Method {
    IntoIter,
    AsRef { mutable: bool },
}

impl Method {
    fn find_possibly_shadowed(cx: &LateContext<'_>, method: HirId) -> Option<Self> {
        match cx.tcx.as_lang_item(cx.typeck_results().type_dependent_def_id(method)?)? {
            LangItem::IntoIterIntoIter => Some(Self::IntoIter),
            LangItem::AsRefAsRef => Some(Self::AsRef { mutable: false }),
            LangItem::AsMutAsMut => Some(Self::AsRef { mutable: true }),
            _ => None,
        }
    }

    fn check_receiver(self, adjusted_receiver_tys: &[Ty<'_>]) -> Option<(Shadowed, bool)> {
        match self {
            Self::IntoIter => find_array_index(adjusted_receiver_tys)
                .map(|i| (i, IntoIterReceiver::Array))
                .or_else(|| {
                    find_boxed_slice_index(adjusted_receiver_tys)
                        .map(|i| (i, IntoIterReceiver::BoxedSlice))
                })
                .map(|(i, receiver)| (Shadowed::IntoIter { receiver }, i == 0)),
            Self::AsRef { mutable } => find_array_index(adjusted_receiver_tys)
                .map(|i| (Shadowed::ArrayAsRef { mutable }, i == 0)),
        }
    }
}

#[derive(Clone, Copy)]
enum IntoIterReceiver {
    Array,
    BoxedSlice,
}

impl IntoIterReceiver {
    fn as_str(self) -> &'static str {
        match self {
            Self::Array => "[T; N]",
            Self::BoxedSlice => "Box<[T]>",
        }
    }
}

#[derive(Clone, Copy)]
enum Shadowed {
    IntoIter { receiver: IntoIterReceiver },
    ArrayAsRef { mutable: bool },
}

impl Shadowed {
    fn edition(self) -> Edition {
        match self {
            Self::IntoIter { receiver: IntoIterReceiver::Array } => Edition::Edition2021,
            Self::IntoIter { receiver: IntoIterReceiver::BoxedSlice } => Edition::Edition2024,
            Self::ArrayAsRef { .. } => Edition::EditionFuture,
        }
    }
}

impl<'tcx> LateLintPass<'tcx> for ShadowedIntoIter {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx hir::Expr<'tcx>) {
        let hir::ExprKind::MethodCall(call, receiver_arg, ..) = &expr.kind else {
            return;
        };
        let Some(possibly_shadowed) = Method::find_possibly_shadowed(cx, expr.hir_id) else {
            return;
        };
        let receiver_ty = cx.typeck_results().expr_ty(receiver_arg);
        let adjustments = cx.typeck_results().expr_adjustments(receiver_arg);
        let adjusted_receiver_tys: Vec<_> =
            [receiver_ty].into_iter().chain(adjustments.iter().map(|adj| adj.target)).collect();
        let Some((shadowed, can_suggest_ufcs)) =
            possibly_shadowed.check_receiver(&adjusted_receiver_tys)
        else {
            return;
        };

        match shadowed {
            Shadowed::IntoIter { receiver } => {
                // If this expression comes from the `IntoIter::into_iter` inside of a for loop,
                // we should just suggest removing the `.into_iter()` or changing it to `.iter()`
                // to disambiguate if we want to iterate by-value or by-ref.
                let sub = if let Some((_, hir::Node::Expr(parent_expr))) =
                    cx.tcx.hir_parent_iter(expr.hir_id).nth(1)
                    && let hir::ExprKind::Match(arg, [_], hir::MatchSource::ForLoopDesugar) =
                        &parent_expr.kind
                    && let hir::ExprKind::Call(path, [_]) = &arg.kind
                    && let hir::ExprKind::Path(hir::QPath::LangItem(
                        hir::LangItem::IntoIterIntoIter,
                        ..,
                    )) = &path.kind
                {
                    Some(ShadowedIntoIterDiagSub::RemoveIntoIter {
                        span: receiver_arg.span.shrink_to_hi().to(expr.span.shrink_to_hi()),
                    })
                } else if can_suggest_ufcs {
                    Some(ShadowedIntoIterDiagSub::UseExplicitIntoIter {
                        start_span: expr.span.shrink_to_lo(),
                        end_span: receiver_arg.span.shrink_to_hi().to(expr.span.shrink_to_hi()),
                    })
                } else {
                    None
                };

                cx.emit_span_lint(
                    match receiver {
                        IntoIterReceiver::Array => ARRAY_INTO_ITER,
                        IntoIterReceiver::BoxedSlice => BOXED_SLICE_INTO_ITER,
                    },
                    call.ident.span,
                    ShadowedIntoIterDiag {
                        target: receiver.as_str(),
                        edition: shadowed.edition(),
                        suggestion: call.ident.span,
                        sub,
                    },
                )
            }
            Shadowed::ArrayAsRef { mutable } => cx.emit_span_lint(
                ARRAY_AS_REF,
                call.ident.span,
                ArrayAsRefDiag {
                    suggestion: call.ident.span,
                    replacement: if mutable { "as_mut_slice" } else { "as_slice" },
                },
            ),
        }
    }
}
