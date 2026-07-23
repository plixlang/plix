use crate::heap::*;

pub fn async_run(f: &str) -> String {
    format!("async:{}", f)
}

pub fn async_await(v: V) -> V {
    // Demo implementation: just return the value
    v
}
