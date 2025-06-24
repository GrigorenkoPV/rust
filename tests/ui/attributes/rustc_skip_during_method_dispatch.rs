#![feature(rustc_attrs)]

#[rustc_skip_during_method_dispatch]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input [E0539]
trait NotAList {}

#[rustc_skip_during_method_dispatch = "array"]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input [E0539]
trait AlsoNotAList {}

#[rustc_skip_during_method_dispatch()]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
trait Argless {}

#[rustc_skip_during_method_dispatch(array = "2021", boxed_slice = "2024", array = "2018")]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
trait Duplicate {}

#[rustc_skip_during_method_dispatch(slice = "2024")]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
trait Unexpected {}

#[rustc_skip_during_method_dispatch(array = true)]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
trait KeyValue {}

#[rustc_skip_during_method_dispatch("array")]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
trait String {}

#[rustc_skip_during_method_dispatch(array = 2021, boxed_slice = "2007")]
//~^ ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
//~| ERROR: malformed `rustc_skip_during_method_dispatch` attribute input
trait BadEditions {}

#[rustc_skip_during_method_dispatch(array = "2021", boxed_slice = "2024")]
trait OK {}

#[rustc_skip_during_method_dispatch(array = "future")]
trait OKFuture {}

#[rustc_skip_during_method_dispatch(array = "2021")]
//~^ ERROR: attribute should be applied to a trait
impl OK for () {}

fn main() {}
