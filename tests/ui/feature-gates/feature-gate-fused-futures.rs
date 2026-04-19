//@ edition: 2024

async fn foo() -> u8 {
    123
} do fuse { //~ERROR: customizing polling/resuming behavior of completed futures/coroutines is experimental [E0658]
    unreachable!()
}

fn main() {}
