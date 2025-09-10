pub struct Foo<'a> {
    pub foo: &'a mut std::borrow::Cow<'a, str>
    //^~ ERROR: TODO
}

fn main() {}
