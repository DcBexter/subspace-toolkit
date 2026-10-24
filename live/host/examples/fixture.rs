#[unsafe(no_mangle)]
pub extern "C" fn Add(value: i32) -> i32 {
    value + 1
}
