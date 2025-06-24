use rustc_attr_data_structures::AttributeKind;
use rustc_feature::{AttributeTemplate, template};
use rustc_span::edition::Edition;
use rustc_span::{Symbol, sym};

use crate::attributes::{AttributeOrder, OnDuplicate, SingleAttributeParser};
use crate::context::{AcceptContext, Stage};
use crate::parser::ArgParser;

pub(crate) struct SkipDuringMethodDispatchParser;

impl<S: Stage> SingleAttributeParser<S> for SkipDuringMethodDispatchParser {
    const PATH: &[Symbol] = &[sym::rustc_skip_during_method_dispatch];
    const ATTRIBUTE_ORDER: AttributeOrder = AttributeOrder::KeepFirst;
    const ON_DUPLICATE: OnDuplicate<S> = OnDuplicate::Error;

    const TEMPLATE: AttributeTemplate = template!(List: "array, boxed_slice");

    fn convert(cx: &mut AcceptContext<'_, '_, S>, args: &ArgParser<'_>) -> Option<AttributeKind> {
        let mut array = None;
        let mut boxed_slice = None;
        let Some(args) = args.list() else {
            cx.expected_list(cx.attr_span);
            return None;
        };
        if args.is_empty() {
            cx.expected_at_least_one_argument(args.span);
            return None;
        }
        for arg in args.mixed() {
            let Some(arg) = arg.meta_item() else {
                cx.unexpected_literal(arg.span());
                continue;
            };
            let Some(edition_parser) = arg.args().name_value() else {
                cx.expected_name_value(arg.span(), None);
                continue;
            };
            let Some(edition) = edition_parser.value_as_str() else {
                cx.expected_string_literal(
                    edition_parser.value_span,
                    Some(edition_parser.value_as_lit()),
                );
                continue;
            };
            let edition: Edition = match edition.as_str().parse() {
                Ok(edition) => edition,
                Err(()) => {
                    cx.expected_specific_argument(
                        edition_parser.value_span,
                        // FIXME: find a better way to list all valid editions
                        vec!["2015", "2018", "2021", "2024", "future"],
                    );
                    continue;
                }
            };
            let path = arg.path();
            let (key, skip): (Symbol, &mut Option<Edition>) = match path.word_sym() {
                Some(key @ sym::array) => (key, &mut array),
                Some(key @ sym::boxed_slice) => (key, &mut boxed_slice),
                _ => {
                    cx.expected_specific_argument(path.span(), vec!["array", "boxed_slice"]);
                    continue;
                }
            };
            if skip.replace(edition).is_some() {
                cx.duplicate_key(arg.span(), key);
            }
        }
        Some(AttributeKind::SkipDuringMethodDispatch { array, boxed_slice, span: cx.attr_span })
    }
}
